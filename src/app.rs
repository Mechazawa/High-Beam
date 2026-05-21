//! Top-level coordinator wiring the Slint window to the plugin runtime.
//!
//! The plugins live inside a dedicated tokio current-thread runtime so the
//! rquickjs `AsyncRuntime` futures (`!Send` across `async_with`) can be polled
//! without crossing thread boundaries. Yields cross back to the Slint event
//! loop via `slint::invoke_from_event_loop`. Stale yields from slow plugins
//! are filtered by a monotonic `query_id`.

use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::QueryWindow;
use crate::frecency::{self, FrecencyDb};
use crate::plugins;
use crate::plugins::actions;
use crate::plugins::dispatch::{self, StreamedResult};
use crate::plugins::loader::{self, LoaderOptions};
use crate::plugins::registry::PluginRegistry;
use crate::plugins::result::RankedResult;
use crate::plugins::runtime::LoadedPlugin;
use crate::settings::Settings;
use crate::settings_ui::SettingsController;
use crate::ui::ResultRow;
use crate::window;

/// Handle to the running plugin host. Drop to shut it down.
pub struct PluginHost {
    query_tx: mpsc::UnboundedSender<HostMessage>,
}

enum HostMessage {
    Query { id: u64, input: String },
    Task(actions::HostTask),
    Shutdown,
}

/// Spin up the plugin runtime, wire callbacks to the given window, and
/// return the host handle. Caller must keep the returned `PluginHost`
/// alive for the lifetime of the daemon.
///
/// # Errors
///
/// Returns an error if the background thread can't be spawned.
pub fn start(
    window: &QueryWindow,
    plugins_override: Option<PathBuf>,
    settings: SettingsController,
) -> Result<PluginHost, Box<dyn Error>> {
    let (tx, rx) = mpsc::unbounded_channel::<HostMessage>();

    // Touched only on the main thread, so the mutex is uncontended.
    let latest: Arc<Mutex<Vec<RankedResult>>> = Arc::new(Mutex::new(Vec::new()));

    let latest_id = Arc::new(AtomicU64::new(0));

    let frecency_db = open_frecency_db();

    spawn_runtime_thread(
        rx,
        plugins_override,
        window.as_weak(),
        Arc::clone(&latest),
        Arc::clone(&latest_id),
        frecency_db.clone(),
    )?;

    wire_window_callbacks(
        window,
        tx.clone(),
        latest,
        &latest_id,
        frecency_db,
        settings,
    );

    Ok(PluginHost { query_tx: tx })
}

fn spawn_runtime_thread(
    mut rx: mpsc::UnboundedReceiver<HostMessage>,
    plugins_override: Option<PathBuf>,
    weak: slint::Weak<QueryWindow>,
    latest: Arc<Mutex<Vec<RankedResult>>>,
    latest_id: Arc<AtomicU64>,
    frecency_db: Option<FrecencyDb>,
) -> Result<(), Box<dyn Error>> {
    thread::Builder::new()
        .name("highbeam-plugin-runtime".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    tracing::error!(%err, "plugins: failed to start tokio runtime");
                    return;
                }
            };

            runtime.block_on(async move {
                let opts = LoaderOptions::resolve(plugins_override);
                let settings = Settings::load_or_default();
                let plugins = loader::load_all(&opts, &settings).await;
                if plugins.is_empty() {
                    tracing::warn!(
                        plugins_dir = %opts.plugins_dir.display(),
                        "plugins: no plugins loaded",
                    );
                }
                let registry = PluginRegistry::new(opts, plugins);
                let mut current_cancel: Option<CancellationToken> = None;

                while let Some(msg) = rx.recv().await {
                    match msg {
                        HostMessage::Query { id, input } => {
                            if id < latest_id.load(Ordering::Relaxed) {
                                continue;
                            }
                            if let Some(prev) = current_cancel.take() {
                                prev.cancel();
                            }
                            let snapshot = frecency_db.as_ref().map(FrecencyDb::snapshot);
                            let plugins = registry.snapshot().await;
                            let cancel = handle_query(
                                id,
                                &input,
                                &plugins,
                                weak.clone(),
                                Arc::clone(&latest),
                                Arc::clone(&latest_id),
                                snapshot,
                            );
                            current_cancel = Some(cancel);
                        }
                        HostMessage::Task(task) => {
                            if let Some(prev) = current_cancel.take() {
                                prev.cancel();
                            }
                            // Bump the query id so any stale yield from the
                            // last keystroke can't paint over the progress
                            // rows the task is about to push.
                            let task_id = latest_id.fetch_add(1, Ordering::Relaxed) + 1;
                            let progress = ProgressEmitter::new(
                                task_id,
                                weak.clone(),
                                Arc::clone(&latest),
                                Arc::clone(&latest_id),
                            );
                            handle_host_task(task, &registry, progress).await;
                        }
                        HostMessage::Shutdown => {
                            if let Some(prev) = current_cancel.take() {
                                prev.cancel();
                            }
                            break;
                        }
                    }
                }
            });
        })?;
    Ok(())
}

fn wire_window_callbacks(
    window: &QueryWindow,
    tx: mpsc::UnboundedSender<HostMessage>,
    latest: Arc<Mutex<Vec<RankedResult>>>,
    latest_id: &Arc<AtomicU64>,
    frecency_db: Option<FrecencyDb>,
    settings: SettingsController,
) {
    let latest_id_for_main = Arc::clone(latest_id);
    let tx_for_edit = tx.clone();
    window.on_query_edited(move |text| {
        let id = latest_id_for_main.fetch_add(1, Ordering::Relaxed) + 1;
        if tx_for_edit
            .send(HostMessage::Query {
                id,
                input: text.into(),
            })
            .is_err()
        {
            tracing::error!("plugins: runtime thread exited; query dropped");
        }
    });

    let weak_for_invoke = window.as_weak();
    let tx_for_invoke = tx;
    window.on_invoke_selected(move || {
        invoke_selected(
            &weak_for_invoke,
            &latest,
            frecency_db.as_ref(),
            &settings,
            &tx_for_invoke,
        );
    });
}

/// Resolve the highlighted row, execute its action, and bump frecency on
/// success.
fn invoke_selected(
    weak: &slint::Weak<QueryWindow>,
    latest: &Arc<Mutex<Vec<RankedResult>>>,
    frecency_db: Option<&FrecencyDb>,
    settings: &SettingsController,
    host_tx: &mpsc::UnboundedSender<HostMessage>,
) {
    let Some(w) = weak.upgrade() else { return };
    let idx = usize::try_from(w.get_selected_index().max(0)).unwrap_or(0);
    let snapshot = match latest.lock() {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(%err, "plugins: latest results lock poisoned");
            return;
        }
    };
    let Some(picked) = snapshot.get(idx) else {
        return;
    };
    let action = picked.result.action.clone();
    let plugin_name = picked.plugin_name.clone();
    let result_key = picked.result.key.clone();
    drop(snapshot);
    match actions::execute(&action) {
        Ok(outcome) => {
            if let Some(db) = frecency_db {
                spawn_pick_bump(db, plugin_name, result_key);
            }
            match outcome {
                actions::ActionOutcome::HideWindow => {
                    // Persisting on every hide keeps drag-then-pick paths
                    // covered too — a user can drag, then run a result
                    // without the position ever being lost.
                    window::hide_and_persist_position(&w, settings);
                }
                actions::ActionOutcome::OpenSettingsView => {
                    // Clear the input so the window doesn't briefly re-show
                    // a stale `settings` query the next time the launcher
                    // view comes up.
                    w.invoke_clear_input();
                    w.invoke_show_settings();
                }
                actions::ActionOutcome::KeepOpen => {}
                actions::ActionOutcome::HostTask(task) => {
                    // Clearing the input + results lets the streaming
                    // progress rows the runtime thread will push become the
                    // sole content of the launcher view.
                    w.invoke_clear_input();
                    w.set_results(ModelRc::new(VecModel::from(Vec::<ResultRow>::new())));
                    w.set_selected_index(0);
                    if host_tx.send(HostMessage::Task(task)).is_err() {
                        tracing::error!("plugins: runtime thread exited; host task dropped",);
                    }
                }
            }
        }
        Err(err) => {
            tracing::error!(plugin = %plugin_name, %err, "plugins: action failed");
            window::hide_and_persist_position(&w, settings);
        }
    }
}

/// Open the frecency DB at the platform default path. Returns `None` on
/// failure so the daemon stays functional with default ranking.
fn open_frecency_db() -> Option<FrecencyDb> {
    let Some(path) = frecency::default_db_path() else {
        tracing::warn!("frecency: could not resolve data dir; running without frecency");
        return None;
    };
    match FrecencyDb::open(&path) {
        Ok(db) => Some(db),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                %err,
                "frecency: failed to open; continuing without frecency",
            );
            None
        }
    }
}

/// Run the pick bump off the UI thread. A plain OS thread (not a tokio
/// task) — the callsite is on the Slint event-loop thread where no tokio
/// runtime is registered.
fn spawn_pick_bump(db: &FrecencyDb, plugin_name: String, result_key: String) {
    let db = db.clone();
    thread::Builder::new()
        .name("highbeam-frecency-bump".into())
        .spawn(move || {
            if let Err(err) = db.bump(&plugin_name, &result_key) {
                tracing::warn!(
                    plugin = %plugin_name,
                    result_key = %result_key,
                    %err,
                    "frecency: bump failed",
                );
            }
        })
        .ok();
}

/// Start a fresh query: clear the row list, kick off the streaming dispatch,
/// and spawn a receiver task that merges yields into the UI as they arrive.
fn handle_query(
    id: u64,
    input: &str,
    plugins: &[Arc<LoadedPlugin>],
    weak: slint::Weak<QueryWindow>,
    latest: Arc<Mutex<Vec<RankedResult>>>,
    latest_id: Arc<AtomicU64>,
    frecency_snapshot: Option<frecency::Snapshot>,
) -> CancellationToken {
    let cancel = CancellationToken::new();

    let weak_reset = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = weak_reset.upgrade() {
            w.set_results(ModelRc::new(VecModel::from(Vec::<ResultRow>::new())));
            w.set_selected_index(0);
        }
    });

    let (yield_tx, mut yield_rx) = mpsc::unbounded_channel::<StreamedResult>();
    dispatch::dispatch_streaming(plugins, input, &cancel, &yield_tx);

    let cancel_for_recv = cancel.clone();
    tokio::spawn(async move {
        let mut live: Vec<RankedResult> = Vec::new();
        let mut order: usize = 0;
        loop {
            tokio::select! {
                () = cancel_for_recv.cancelled() => break,
                next = yield_rx.recv() => match next {
                    Some(streamed) => {
                        if id < latest_id.load(Ordering::Relaxed) {
                            continue;
                        }
                        dispatch::merge_with_snapshot(
                            &mut live,
                            &mut order,
                            streamed,
                            frecency_snapshot.as_ref(),
                        );
                        let snapshot = live.clone();
                        if let Ok(mut slot) = latest.lock() {
                            slot.clone_from(&snapshot);
                        }
                        let weak = weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = weak.upgrade() {
                                render_results(&w, &snapshot);
                            }
                        });
                    }
                    None => break,
                }
            }
        }
    });

    cancel
}

/// Push the dispatch results into the window's row model.
///
/// Preserves `selected-index` when the previously-selected row still exists;
/// otherwise resets to 0. Icons are decoded on this thread because
/// `slint::Image` isn't trivially constructed off-thread; the per-yield cost
/// is bounded by `max_rows` (9).
fn render_results(window: &QueryWindow, results: &[RankedResult]) {
    let previously_selected = window.get_selected_index();
    let rows: Vec<ResultRow> = results
        .iter()
        .map(|r| {
            let icon_spec = r.result.icon.as_deref();
            let has_icon = is_renderable_icon(icon_spec);
            ResultRow {
                key: r.composite_key().into(),
                title: r.result.title.clone().into(),
                subtitle: r.result.subtitle.clone().unwrap_or_default().into(),
                has_subtitle: r.result.subtitle.is_some(),
                icon: window::decode_icon(icon_spec),
                has_icon,
            }
        })
        .collect();
    let row_count = i32::try_from(rows.len()).unwrap_or(i32::MAX);
    window.set_results(ModelRc::new(VecModel::from(rows)));
    if previously_selected >= row_count || previously_selected < 0 {
        window.set_selected_index(0);
    }
}

/// Anything that isn't a base64 data URI is treated as "no icon" so the row
/// renders the muted placeholder rather than appearing to load nothing.
fn is_renderable_icon(spec: Option<&str>) -> bool {
    spec.is_some_and(|s| s.starts_with("data:") && s.contains(";base64,"))
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        let _ = self.query_tx.send(HostMessage::Shutdown);
    }
}

/// Stable-key stream of progress rows shared with the launcher's result
/// list. Re-emitting a key replaces the previous row in place, so the
/// install / update / reload tasks can drive a row through "installing →
/// installed" without inventing new rows.
///
/// The emitter holds a fixed `query_id` — re-using `latest_id` keeps these
/// rows from being clobbered by a stale yield from a previously-cancelled
/// JS query.
struct ProgressEmitter {
    query_id: u64,
    weak: slint::Weak<QueryWindow>,
    latest: Arc<Mutex<Vec<RankedResult>>>,
    latest_id: Arc<AtomicU64>,
}

impl ProgressEmitter {
    fn new(
        query_id: u64,
        weak: slint::Weak<QueryWindow>,
        latest: Arc<Mutex<Vec<RankedResult>>>,
        latest_id: Arc<AtomicU64>,
    ) -> Self {
        Self {
            query_id,
            weak,
            latest,
            latest_id,
        }
    }

    /// Push (or replace by `key`) one progress row and re-render.
    fn emit(
        &self,
        key: &str,
        title: String,
        subtitle: Option<String>,
        action: plugins::result::Action,
    ) {
        // Skip the paint if another query/task has already taken over —
        // late-arriving progress for an abandoned task shouldn't repaint.
        if self.query_id < self.latest_id.load(Ordering::Relaxed) {
            return;
        }
        let next = RankedResult {
            plugin_name: crate::plugins::builtin::core::NAME.to_owned(),
            result: plugins::result::PluginResult {
                key: key.to_owned(),
                title,
                subtitle,
                icon: None,
                weight: 100.0,
                pinned: true,
                action,
            },
            order: 0,
        };
        if let Ok(mut slot) = self.latest.lock() {
            if let Some(existing) = slot.iter_mut().find(|r| r.result.key == next.result.key) {
                existing.result = next.result.clone();
            } else {
                let order = slot.len();
                slot.push(RankedResult {
                    order,
                    ..next.clone()
                });
            }
        }
        let weak = self.weak.clone();
        let snapshot: Vec<RankedResult> = self.latest.lock().map(|s| s.clone()).unwrap_or_default();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = weak.upgrade() {
                render_results(&w, &snapshot);
            }
        });
    }
}

async fn handle_host_task(
    task: actions::HostTask,
    registry: &PluginRegistry,
    progress: ProgressEmitter,
) {
    match task {
        actions::HostTask::Reload { name } => {
            run_reload(name, registry, &progress).await;
        }
        actions::HostTask::Install { .. } | actions::HostTask::UpdateAll => {
            // Wired up in follow-up commits; for now show a placeholder so
            // the user knows the action was received.
            progress.emit(
                "host-task-not-yet",
                "not yet implemented".to_owned(),
                Some("install / update wiring lands in a follow-up commit".to_owned()),
                plugins::result::Action::Noop,
            );
        }
    }
}

async fn run_reload(name: Option<String>, registry: &PluginRegistry, progress: &ProgressEmitter) {
    let settings = Settings::load_or_default();
    match name {
        None => {
            progress.emit(
                "reload-all",
                "Reloading all plugins…".to_owned(),
                None,
                plugins::result::Action::Noop,
            );
            let names = registry.reload_all(&settings).await;
            progress.emit(
                "reload-all",
                format!("Reloaded {} plugin(s)", names.len()),
                Some(if names.is_empty() {
                    "no plugins on disk".to_owned()
                } else {
                    names.join(", ")
                }),
                plugins::result::Action::Noop,
            );
        }
        Some(target) => {
            let key = format!("reload-{target}");
            progress.emit(
                &key,
                format!("Reloading {target}…"),
                None,
                plugins::result::Action::Noop,
            );
            match registry.reload_one(&target, &settings).await {
                Ok(plugin) => {
                    let version = plugin.manifest.version.clone();
                    progress.emit(
                        &key,
                        format!("Reloaded {target}"),
                        version.map(|v| format!("v{v}")),
                        plugins::result::Action::Noop,
                    );
                }
                Err(err) => {
                    progress.emit(
                        &key,
                        format!("Failed to reload {target}"),
                        Some(err.to_string()),
                        plugins::result::Action::Noop,
                    );
                }
            }
        }
    }
}
