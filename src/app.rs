//! Top-level coordinator wiring the Slint window to the plugin runtime.
//!
//! The plugins live inside a dedicated tokio current-thread runtime so the
//! rquickjs `AsyncRuntime` futures (`!Send` across `async_with`) can be polled
//! without crossing thread boundaries. Yields cross back to the Slint event
//! loop via `slint::invoke_from_event_loop`. Stale yields from slow plugins
//! are filtered by a monotonic `query_id`.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::QueryWindow;
use crate::confirm::{ConfirmationSummary, PendingConfirmation};
use crate::frecency::{self, FrecencyDb};
use crate::plugins;
use crate::plugins::actions;
use crate::plugins::dispatch::{self, StreamedResult};
use crate::plugins::loader::{self, LoaderOptions};
use crate::plugins::registry::PluginRegistry;
use crate::plugins::result::RankedResult;
use crate::plugins::runtime::LoadedPlugin;
use crate::query_history::{InputAction, QueryHistoryDb, QueryHistoryState};
use crate::settings::Settings;
use crate::settings_ui::SettingsController;
use crate::ui::{ConfirmCapRow, ResultRow};
use crate::window;

/// Handle to the running plugin host. Drop to shut it down.
pub struct PluginHost {
    query_tx: mpsc::UnboundedSender<HostMessage>,
}

/// Shared state for the install-confirmation gate. Held behind a `Mutex` so
/// the Slint-thread callbacks and the runtime-thread install task can
/// co-ordinate without data races.
type ConfirmState = Arc<Mutex<Option<PendingConfirmation>>>;

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

    let confirm_state: ConfirmState = Arc::new(Mutex::new(None));

    let settings_snapshot = Settings::load_or_default();
    let history_db = open_query_history_db();
    let initial_entries = history_db
        .as_ref()
        .map(|db| db.load_recent(settings_snapshot.query_history_max_entries()))
        .unwrap_or_default();
    let history_state = Arc::new(Mutex::new(QueryHistoryState::new(initial_entries)));

    spawn_runtime_thread(
        rx,
        plugins_override,
        window.as_weak(),
        Arc::clone(&latest),
        Arc::clone(&latest_id),
        frecency_db.clone(),
        Arc::clone(&confirm_state),
    )?;

    wire_window_callbacks(
        window,
        &tx,
        &latest_id,
        WindowCallbackCtx {
            latest,
            frecency_db,
            settings,
            confirm_state,
            history_db,
            history_state,
        },
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
    confirm_state: ConfirmState,
) -> Result<(), Box<dyn Error>> {
    thread::Builder::new()
        .name("highbeam-plugin-runtime".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
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
                            handle_host_task(task, &registry, progress, Arc::clone(&confirm_state), weak.clone()).await;
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

/// Aggregate of plugin-result + confirm + history state threaded into
/// `wire_window_callbacks`. Groups logically-related handles so the function
/// stays within clippy's argument-count limit.
struct WindowCallbackCtx {
    latest: Arc<Mutex<Vec<RankedResult>>>,
    frecency_db: Option<FrecencyDb>,
    settings: SettingsController,
    confirm_state: ConfirmState,
    history_db: Option<QueryHistoryDb>,
    history_state: Arc<Mutex<QueryHistoryState>>,
}

fn wire_window_callbacks(
    window: &QueryWindow,
    tx: &mpsc::UnboundedSender<HostMessage>,
    latest_id: &Arc<AtomicU64>,
    ctx: WindowCallbackCtx,
) {
    let WindowCallbackCtx {
        latest,
        frecency_db,
        settings,
        confirm_state,
        history_db,
        history_state,
    } = ctx;

    let latest_id_for_main = Arc::clone(latest_id);
    let tx_for_edit = tx.clone();
    // Any edit while previewing commits: drop the muted-render flag and
    // exit the cycle on the Rust side. The check on `is-history-preview`
    // is a cheap Slint property read; the lock + state mutation only run
    // when we were actually in a preview, keeping the per-keystroke hot
    // path lock-free in the common case.
    let history_state_for_edit = Arc::clone(&history_state);
    let weak_for_edit = window.as_weak();
    window.on_query_edited(move |text| {
        if let Some(w) = weak_for_edit.upgrade()
            && w.get_is_history_preview()
        {
            if let Ok(mut hs) = history_state_for_edit.lock() {
                hs.mark_edited();
            }
            w.set_is_history_preview(false);
        }
        let id = latest_id_for_main.fetch_add(1, Ordering::Relaxed) + 1;
        if tx_for_edit.send(HostMessage::Query { id, input: text.into() }).is_err() {
            tracing::error!("plugins: runtime thread exited; query dropped");
        }
    });

    let weak_for_invoke = window.as_weak();
    let tx_for_invoke = tx.clone();
    let history_db_for_invoke = history_db.clone();
    let history_state_for_invoke = Arc::clone(&history_state);
    let settings_for_invoke = settings.clone();
    window.on_invoke_selected(move |meta, control, shift, alt| {
        let mods = (u8::from(meta) * crate::settings_ui::MOD_META)
            | (u8::from(control) * crate::settings_ui::MOD_CONTROL)
            | (u8::from(shift) * crate::settings_ui::MOD_SHIFT)
            | (u8::from(alt) * crate::settings_ui::MOD_ALT);
        let alt_held = settings_for_invoke.alt_modifier_held(mods);
        invoke_selected(
            &weak_for_invoke,
            &latest,
            frecency_db.as_ref(),
            &settings_for_invoke,
            &tx_for_invoke,
            history_db_for_invoke.as_ref(),
            &history_state_for_invoke,
            alt_held,
        );
    });

    wire_history_callbacks(window, &history_state, history_db.as_ref(), &settings);

    // Install — confirmed.
    let confirm_state_install = Arc::clone(&confirm_state);
    let weak_confirm_install = window.as_weak();
    window.on_confirm_install(move || {
        send_confirm_decision(&confirm_state_install, true, &weak_confirm_install);
    });

    // Install — cancelled.
    let confirm_state_cancel = confirm_state;
    let weak_confirm_cancel = window.as_weak();
    window.on_confirm_cancel(move || {
        send_confirm_decision(&confirm_state_cancel, false, &weak_confirm_cancel);
    });
}

/// Wire the four query-history Slint callbacks: Up + Down cycle, and the
/// dismiss callback that persists the live input to history when the
/// launcher hides (Esc / blur / action-induced hide).
fn wire_history_callbacks(
    window: &QueryWindow,
    history_state: &Arc<Mutex<QueryHistoryState>>,
    history_db: Option<&QueryHistoryDb>,
    settings: &SettingsController,
) {
    let weak_for_up = window.as_weak();
    let history_state_for_up = Arc::clone(history_state);
    window.on_history_up(move || {
        let Some(w) = weak_for_up.upgrade() else {
            return;
        };
        let current = w.get_query_text();
        if let Ok(mut hs) = history_state_for_up.lock()
            && let InputAction::SetTo(text) = hs.history_up(&current)
        {
            apply_history_text(&w, &text);
            w.set_is_history_preview(hs.is_preview());
        }
    });

    let weak_for_down = window.as_weak();
    let history_state_for_down = Arc::clone(history_state);
    window.on_history_down(move || {
        let Some(w) = weak_for_down.upgrade() else {
            return;
        };
        if let Ok(mut hs) = history_state_for_down.lock()
            && let InputAction::SetTo(text) = hs.history_down()
        {
            apply_history_text(&w, &text);
            w.set_is_history_preview(hs.is_preview());
        }
    });

    // Persist on dismiss (Esc / blur / action-induced hide). The empty-
    // string check on `SharedString` is allocation-free; only commit the
    // payload to a `String` if we're actually going to push. Previews are
    // skipped — the text is already in the DB. `invoke_selected` already
    // pushed any submitted query, and the DB dedups against the last
    // entry, so the action-then-hide path can't double-write.
    let history_db_for_dismiss = history_db.cloned();
    let history_state_for_dismiss = Arc::clone(history_state);
    let settings_for_dismiss = settings.clone();
    window.on_persist_dismiss(move |text| {
        if text.is_empty() {
            return;
        }
        if let Ok(hs) = history_state_for_dismiss.lock()
            && hs.is_preview()
        {
            return;
        }
        push_history(
            history_db_for_dismiss.as_ref(),
            &history_state_for_dismiss,
            text.as_str(),
            settings_for_dismiss.query_history_max_entries(),
        );
    });
}

/// Write `text` into the window's query input without firing `query_edited`.
///
/// `set-input-text` writes both `input.text` and `root.query-text` itself,
/// so a separate `set_query_text` would just be a redundant property
/// write. The whole point is to skip `edited` so the cycle cursor
/// survives — Enter or an edit commits and re-enters the regular pipeline.
fn apply_history_text(window: &QueryWindow, text: &str) {
    window.invoke_set_input_text(text.into());
}

/// Pull the pending oneshot sender out of `confirm_state` and send `decision`.
/// Then restore the launcher view so the UI isn't left on the confirm screen.
fn send_confirm_decision(state: &ConfirmState, decision: bool, weak: &slint::Weak<QueryWindow>) {
    let maybe_tx = match state.lock() {
        Ok(mut guard) => guard.take().map(|p| p.tx),
        Err(err) => {
            tracing::error!(%err, "confirm: state lock poisoned");
            return;
        }
    };
    if let Some(tx) = maybe_tx {
        let _ = tx.send(decision);
    }
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = weak.upgrade() {
            w.invoke_show_query();
        }
    });
}

/// Resolve the highlighted row, execute its action, bump frecency and push the
/// query to history on success.
#[allow(clippy::too_many_arguments)]
fn invoke_selected(
    weak: &slint::Weak<QueryWindow>,
    latest: &Arc<Mutex<Vec<RankedResult>>>,
    frecency_db: Option<&FrecencyDb>,
    settings: &SettingsController,
    host_tx: &mpsc::UnboundedSender<HostMessage>,
    history_db: Option<&QueryHistoryDb>,
    history_state: &Arc<Mutex<QueryHistoryState>>,
    alt_held: bool,
) {
    let Some(w) = weak.upgrade() else { return };

    let idx = usize::try_from(w.get_selected_index().max(0)).unwrap_or(0);
    let query_text = w.get_query_text().to_string();
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

    // Alt held + the result opts in via altAction ⇒ run the alternate.
    // No altAction set ⇒ fall back to the primary so the modifier is a
    // no-op for plugins that don't bother with secondary verbs.
    let action = if alt_held {
        picked
            .result
            .alt_action
            .clone()
            .unwrap_or_else(|| picked.result.action.clone())
    } else {
        picked.result.action.clone()
    };
    let plugin_name = picked.plugin_name.clone();
    let result_key = picked.result.key.clone();
    drop(snapshot);

    // Push the query to history before the action runs — if the action hides
    // the window we still want the entry recorded.
    push_history(
        history_db,
        history_state,
        &query_text,
        settings.query_history_max_entries(),
    );

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

/// Append `query` to the persistent history and update the in-memory state
/// machine. Runs on the UI thread — DB write is fast enough for a one-off
/// per Enter / dismiss. Both layers dedup against the last entry and trim
/// to `max_entries`, so the in-memory mirror stays in sync without a
/// follow-up `load_recent`.
fn push_history(
    history_db: Option<&QueryHistoryDb>,
    history_state: &Arc<Mutex<QueryHistoryState>>,
    query: &str,
    max_entries: usize,
) {
    if query.is_empty() {
        return;
    }
    if let Some(db) = history_db
        && let Err(err) = db.push(query, max_entries)
    {
        tracing::warn!(%err, "query_history: push failed");
    }
    if let Ok(mut hs) = history_state.lock() {
        hs.on_submit(query, max_entries);
    }
}

/// Open the query-history DB at the platform default path. Returns `None` on
/// failure so the daemon stays functional without history.
fn open_query_history_db() -> Option<QueryHistoryDb> {
    let Some(path) = crate::query_history::default_db_path() else {
        tracing::warn!("query_history: could not resolve data dir; running without history");
        return None;
    };
    match QueryHistoryDb::open(&path) {
        Ok(db) => Some(db),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                %err,
                "query_history: failed to open; continuing without history",
            );
            None
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
    let plugin_name_for_log = plugin_name.clone();
    let result_key_for_log = result_key.clone();
    if let Err(err) = thread::Builder::new()
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
    {
        tracing::warn!(
            plugin = %plugin_name_for_log,
            result_key = %result_key_for_log,
            %err,
            "frecency: bump thread spawn failed; pick lost",
        );
    }
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
    // `invoke_from_event_loop` only returns Err once the event loop has
    // exited — i.e. the daemon is shutting down. At that point dropping the
    // UI update is exactly what we want, so the Err arm is silently
    // discarded here and at every other call site in this module.
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
                        if let Ok(mut slot) = latest.lock() {
                            slot.clone_from(&live);
                        }
                        let snapshot = live.clone();
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
                alt_title: r.result.alt_title.clone().unwrap_or_default().into(),
                has_alt_title: r.result.alt_title.is_some(),
                alt_subtitle: r.result.alt_subtitle.clone().unwrap_or_default().into(),
                has_alt_subtitle: r.result.alt_subtitle.is_some(),
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
    fn emit(&self, key: &str, title: String, subtitle: Option<String>, action: plugins::result::Action) {
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
                alt_action: None,
                alt_title: None,
                alt_subtitle: None,
            },
            order: 0,
        };
        let snapshot: Vec<RankedResult> = match self.latest.lock() {
            Ok(mut slot) => {
                if let Some(existing) = slot.iter_mut().find(|r| r.result.key == next.result.key) {
                    existing.result = next.result.clone();
                } else {
                    let order = slot.len();
                    slot.push(RankedResult { order, ..next.clone() });
                }
                slot.clone()
            }
            Err(_) => return,
        };
        let weak = self.weak.clone();
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
    confirm_state: ConfirmState,
    weak: slint::Weak<QueryWindow>,
) {
    match task {
        actions::HostTask::Reload { name } => {
            run_reload(name, registry, &progress).await;
        }
        actions::HostTask::Install { url } => {
            run_install(&url, registry, &progress, &confirm_state, &weak).await;
        }
        actions::HostTask::UpdateAll => {
            run_update_all(registry, &progress, &confirm_state, &weak).await;
        }
    }
}

/// Drive the install pipeline for a single manifest URL, streaming progress
/// rows under the stable key `install`. Returns the loaded plugin's name on
/// success — `run_update_all` re-uses this so each per-plugin update lands
/// progress under a stable per-plugin key.
async fn run_install(
    url: &str,
    registry: &PluginRegistry,
    progress: &ProgressEmitter,
    confirm_state: &ConfirmState,
    weak: &slint::Weak<QueryWindow>,
) -> Option<String> {
    progress.emit(
        "install",
        format!("Installing {url}…"),
        Some("fetching manifest".to_owned()),
        plugins::result::Action::Noop,
    );
    install_pipeline(url, "install", None, registry, progress, confirm_state, weak).await
}

/// Inner install pipeline, parameterised on the progress-row key so the
/// caller can decide between a single `install` row and per-plugin
/// `update-<name>` rows.
///
/// `installed_caps` is `Some` during an update and drives the capability diff
/// shown in the confirmation view.
#[allow(clippy::too_many_arguments)]
async fn install_pipeline(
    url: &str,
    progress_key: &str,
    installed_caps: Option<&[String]>,
    registry: &PluginRegistry,
    progress: &ProgressEmitter,
    confirm_state: &ConfirmState,
    weak: &slint::Weak<QueryWindow>,
) -> Option<String> {
    let manifest = match plugins::install::fetch_and_validate_manifest(url).await {
        Ok(m) => m,
        Err(err) => {
            progress.emit(
                progress_key,
                "Install failed".to_owned(),
                Some(err.to_string()),
                plugins::result::Action::Noop,
            );
            return None;
        }
    };

    // Gate on user confirmation before downloading anything.
    let confirmed = request_confirmation(&manifest, url, installed_caps, confirm_state, weak).await;
    if !confirmed {
        progress.emit(
            progress_key,
            format!("Install {} cancelled", manifest.name),
            None,
            plugins::result::Action::Noop,
        );
        return None;
    }

    let plugin_name = manifest.name.clone();
    let plugin_version = manifest.version.clone().unwrap_or_default();
    let staging = stage_payload(&manifest, url, progress_key, progress).await?;
    finalize_install(FinalizeCtx {
        plugin_name: &plugin_name,
        plugin_version: &plugin_version,
        staging_payload_root: &staging,
        progress_key,
        registry,
        progress,
    })
    .await
}

/// Show the confirmation view and await the user's decision.
/// Returns `true` when the user pressed Install, `false` for Cancel.
async fn request_confirmation(
    manifest: &plugins::manifest::Manifest,
    manifest_url: &str,
    installed_caps: Option<&[String]>,
    confirm_state: &ConfirmState,
    weak: &slint::Weak<QueryWindow>,
) -> bool {
    let summary = crate::confirm::ConfirmationSummary::from_manifest(manifest, manifest_url, installed_caps);

    let (tx, rx) = oneshot::channel::<bool>();

    // Stash the sender so the Slint callbacks can resolve it.
    match confirm_state.lock() {
        Ok(mut guard) => {
            *guard = Some(PendingConfirmation {
                tx,
                summary: summary.clone(),
            });
        }
        Err(err) => {
            tracing::error!(%err, "confirm: state lock poisoned; aborting install");
            return false;
        }
    }

    // Push the summary into Slint properties and flip to the confirm view.
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(w) = weak.upgrade() else { return };
        populate_confirm_view(&w, &summary);
        w.invoke_show_confirm();
    });

    rx.await.unwrap_or(false)
}

/// Populate the `confirm-*` Slint properties from a [`ConfirmationSummary`].
fn populate_confirm_view(window: &QueryWindow, summary: &ConfirmationSummary) {
    window.set_confirm_plugin_name(SharedString::from(summary.plugin_name.as_str()));
    window.set_confirm_display_name(SharedString::from(summary.display_name.as_str()));
    window.set_confirm_version(SharedString::from(summary.version.as_str()));
    window.set_confirm_description(SharedString::from(summary.description.as_str()));
    window.set_confirm_manifest_url(SharedString::from(summary.manifest_url.as_str()));
    let rows: Vec<ConfirmCapRow> = summary
        .capabilities
        .iter()
        .map(|c| ConfirmCapRow {
            cap: SharedString::from(c.cap.as_str()),
            explanation: SharedString::from(c.explanation.as_str()),
            is_new: c.is_new,
        })
        .collect();
    window.set_confirm_capabilities(ModelRc::new(VecModel::from(rows)));
}

/// Download + (extract or write) into a fresh staging dir. Returns the
/// path of the staged payload root. Branches on whichever distribution
/// shape the manifest declared — `archiveUrl` (multi-file archive) or
/// `entryUrl` (single JS file).
async fn stage_payload(
    manifest: &plugins::manifest::Manifest,
    url: &str,
    progress_key: &str,
    progress: &ProgressEmitter,
) -> Option<PathBuf> {
    let plugin_name = manifest.name.clone();
    let plugin_version = manifest.version.clone().unwrap_or_default();

    let staging = match temp_dir_for_install(&plugin_name) {
        Ok(dir) => dir,
        Err(err) => {
            emit_install_failure(
                progress,
                progress_key,
                &plugin_name,
                &format!("create staging dir: {err}"),
            );
            return None;
        }
    };

    let payload_root = match (manifest.archive_url.as_deref(), manifest.entry_url.as_deref()) {
        (Some(archive_url), None) => {
            stage_from_archive(
                manifest,
                url,
                archive_url,
                &plugin_name,
                &plugin_version,
                &staging,
                progress_key,
                progress,
            )
            .await?
        }
        (None, Some(entry_url)) => {
            stage_from_entry(
                manifest,
                entry_url,
                &plugin_name,
                &plugin_version,
                &staging,
                progress_key,
                progress,
            )
            .await?
        }
        (Some(_), Some(_)) => {
            cleanup_staging(&staging);
            emit_install_failure(
                progress,
                progress_key,
                &plugin_name,
                "manifest declares both archiveUrl and entryUrl — pick one",
            );
            return None;
        }
        (None, None) => {
            cleanup_staging(&staging);
            emit_install_failure(
                progress,
                progress_key,
                &plugin_name,
                "manifest missing archiveUrl or entryUrl",
            );
            return None;
        }
    };

    let writeable = plugins::install::manifest_for_write(manifest, url);
    let payload_root_for_write = payload_root.clone();
    let write_result =
        tokio::task::spawn_blocking(move || plugins::install::write_manifest_json(&payload_root_for_write, &writeable))
            .await;
    if let Err(err) = unwrap_spawn_blocking(write_result, "write_manifest_json") {
        cleanup_staging(&staging);
        emit_install_failure(progress, progress_key, &plugin_name, &err);
        return None;
    }
    Some(payload_root)
}

#[allow(clippy::too_many_arguments)]
async fn stage_from_archive(
    manifest: &plugins::manifest::Manifest,
    install_url: &str,
    archive_url: &str,
    plugin_name: &str,
    plugin_version: &str,
    staging: &Path,
    progress_key: &str,
    progress: &ProgressEmitter,
) -> Option<PathBuf> {
    progress.emit(
        progress_key,
        format!("Installing {plugin_name} v{plugin_version}…"),
        Some(format!("downloading {archive_url}")),
        plugins::result::Action::Noop,
    );
    let (bytes, format) = match plugins::install::download_archive(archive_url).await {
        Ok(pair) => pair,
        Err(err) => {
            cleanup_staging(staging);
            emit_install_failure(progress, progress_key, plugin_name, &err.to_string());
            return None;
        }
    };
    let staging_for_extract = staging.to_path_buf();
    let extract_result =
        tokio::task::spawn_blocking(move || plugins::install::extract_archive(&bytes, format, &staging_for_extract))
            .await;
    if let Err(err) = unwrap_spawn_blocking(extract_result, "extract") {
        cleanup_staging(staging);
        emit_install_failure(progress, progress_key, plugin_name, &err);
        return None;
    }
    let payload_root = plugins::install::find_payload_root(staging);
    if let Err(err) = plugins::install::cross_check_embedded(&payload_root, manifest, install_url) {
        cleanup_staging(staging);
        emit_install_failure(progress, progress_key, plugin_name, &err.to_string());
        return None;
    }
    Some(payload_root)
}

#[allow(clippy::too_many_arguments)]
async fn stage_from_entry(
    manifest: &plugins::manifest::Manifest,
    entry_url: &str,
    plugin_name: &str,
    plugin_version: &str,
    staging: &Path,
    progress_key: &str,
    progress: &ProgressEmitter,
) -> Option<PathBuf> {
    progress.emit(
        progress_key,
        format!("Installing {plugin_name} v{plugin_version}…"),
        Some(format!("downloading {entry_url}")),
        plugins::result::Action::Noop,
    );
    let bytes = match plugins::install::download_entry(entry_url).await {
        Ok(b) => b,
        Err(err) => {
            cleanup_staging(staging);
            emit_install_failure(progress, progress_key, plugin_name, &err.to_string());
            return None;
        }
    };
    // The single-file shape mirrors what an archive would have produced:
    // `<staging>/<plugin>/<entry>` so `finalize_install`'s rename-into-place
    // doesn't need to know which path got us here.
    let payload_root = staging.join(plugin_name);
    let entry_filename = manifest.entry.clone();
    let payload_root_for_write = payload_root.clone();
    let write_result = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        std::fs::create_dir_all(&payload_root_for_write)?;
        std::fs::write(payload_root_for_write.join(&entry_filename), &bytes)
    })
    .await;
    if let Err(err) = unwrap_spawn_blocking(write_result, "write_entry") {
        cleanup_staging(staging);
        emit_install_failure(progress, progress_key, plugin_name, &err);
        return None;
    }
    Some(payload_root)
}

/// Flatten a `JoinResult<Result<T, E>>` from `spawn_blocking` into one
/// stringly-typed error so callers can pipe the message straight into
/// `emit_install_failure`. `op` names the operation for panicked-task logs.
fn unwrap_spawn_blocking<T, E: std::fmt::Display>(
    result: Result<Result<T, E>, tokio::task::JoinError>,
    op: &str,
) -> Result<T, String> {
    match result {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(err)) => Err(err.to_string()),
        Err(join_err) => Err(format!("{op} task panicked: {join_err}")),
    }
}

/// Bundle of references `finalize_install` consumes — purely a packaging
/// concession to clippy's arg-count lint; every field is borrowed.
struct FinalizeCtx<'a> {
    plugin_name: &'a str,
    plugin_version: &'a str,
    staging_payload_root: &'a Path,
    progress_key: &'a str,
    registry: &'a PluginRegistry,
    progress: &'a ProgressEmitter,
}

/// Atomically swap the staged payload into the user's plugin dir, hot-reload
/// it, and report the result. `staging_payload_root` is the path returned by
/// `stage_payload` — its parent (the tmpdir) is cleaned up before returning.
async fn finalize_install(ctx: FinalizeCtx<'_>) -> Option<String> {
    let FinalizeCtx {
        plugin_name,
        plugin_version,
        staging_payload_root,
        progress_key,
        registry,
        progress,
    } = ctx;
    let plugins_dir = registry.options().plugins_dir.clone();
    let staging_path = staging_payload_root.to_path_buf();
    let plugin_name_owned = plugin_name.to_owned();
    let move_result = tokio::task::spawn_blocking(move || {
        plugins::install::move_into_plugins_dir(&staging_path, &plugins_dir, &plugin_name_owned)
    })
    .await;
    let installed_path = match unwrap_spawn_blocking(move_result, "move_into_plugins_dir") {
        Ok(p) => p,
        Err(err) => {
            cleanup_staging(staging_payload_root);
            emit_install_failure(progress, progress_key, plugin_name, &err);
            return None;
        }
    };

    // The payload root's parent is the staging tmpdir — best-effort wipe.
    if let Some(staging_parent) = staging_payload_root.parent() {
        cleanup_staging(staging_parent);
    }

    progress.emit(
        progress_key,
        format!("Loading {plugin_name} v{plugin_version}…"),
        Some(installed_path.display().to_string()),
        plugins::result::Action::Noop,
    );

    let settings = Settings::load_or_default();

    match loader::load_one_for_reload(&installed_path, &settings).await {
        Ok(loaded) => {
            registry.install(loaded).await;
            progress.emit(
                progress_key,
                format!("Installed {plugin_name} v{plugin_version}"),
                Some("ready to use".to_owned()),
                plugins::result::Action::Noop,
            );
            Some(plugin_name.to_owned())
        }
        Err(err) => {
            progress.emit(
                progress_key,
                format!("Installed {plugin_name} but load failed"),
                Some(err),
                plugins::result::Action::Noop,
            );
            None
        }
    }
}

fn emit_install_failure(progress: &ProgressEmitter, progress_key: &str, plugin_name: &str, detail: &str) {
    progress.emit(
        progress_key,
        format!("Install {plugin_name} failed"),
        Some(detail.to_owned()),
        plugins::result::Action::Noop,
    );
}

async fn run_update_all(
    registry: &PluginRegistry,
    progress: &ProgressEmitter,
    confirm_state: &ConfirmState,
    weak: &slint::Weak<QueryWindow>,
) {
    let plugins = registry.snapshot().await;
    if plugins.is_empty() {
        progress.emit(
            "update-summary",
            "No plugins loaded".to_owned(),
            Some("nothing to update".to_owned()),
            plugins::result::Action::Noop,
        );
        return;
    }
    let mut updated = 0usize;
    let mut up_to_date = 0usize;
    let mut failed = 0usize;

    for plugin in plugins {
        let local_version = plugin.manifest.version.clone().unwrap_or_default();
        let installed_caps = plugin.manifest.capabilities.clone();
        let Some(manifest_url) = plugin.manifest.manifest_url.clone() else {
            let key = format!("update-{}", plugin.manifest.name);
            progress.emit(
                &key,
                format!("Skipped {}", plugin.manifest.name),
                Some("no manifestUrl — plugin opts out of updates".to_owned()),
                plugins::result::Action::Noop,
            );
            continue;
        };

        let name = plugin.manifest.name.clone();
        let key = format!("update-{name}");
        progress.emit(
            &key,
            format!("Checking {name}…"),
            Some(manifest_url.clone()),
            plugins::result::Action::Noop,
        );

        let remote = match plugins::install::fetch_and_validate_manifest(&manifest_url).await {
            Ok(m) => m,
            Err(err) => {
                failed += 1;
                progress.emit(
                    &key,
                    format!("Update check failed: {name}"),
                    Some(err.to_string()),
                    plugins::result::Action::Noop,
                );
                continue;
            }
        };

        let remote_version = remote.version.clone().unwrap_or_default();

        if !plugins::manifest::is_newer_version(&remote_version, &local_version) {
            up_to_date += 1;
            progress.emit(
                &key,
                format!("Up to date: {name} v{local_version}"),
                None,
                plugins::result::Action::Noop,
            );
            continue;
        }

        progress.emit(
            &key,
            format!("Updating {name} v{local_version} → v{remote_version}…"),
            None,
            plugins::result::Action::Noop,
        );

        // Only prompt if the update introduces new capabilities.
        let needs_prompt = crate::confirm::update_needs_prompt(&remote.capabilities, &installed_caps);
        let caps_arg: Option<&[String]> = if needs_prompt { Some(&installed_caps) } else { None };

        if install_pipeline(&manifest_url, &key, caps_arg, registry, progress, confirm_state, weak)
            .await
            .is_some()
        {
            updated += 1;
        } else {
            failed += 1;
        }
    }
    progress.emit(
        "update-summary",
        format!("Update complete — {updated} updated, {up_to_date} up to date, {failed} failed"),
        None,
        plugins::result::Action::Noop,
    );
}

fn temp_dir_for_install(name: &str) -> std::io::Result<PathBuf> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("high-beam-install-{name}-{now}"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn cleanup_staging(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
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
