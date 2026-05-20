//! Top-level coordinator wiring the Slint window to the plugin runtime.
//!
//! Responsibilities:
//!   * Owns a dedicated background thread running a Tokio current-thread
//!     runtime; the loaded plugins live inside that thread so the rquickjs
//!     `AsyncRuntime` futures (`!Send` across `async_with`) can be polled
//!     without crossing thread boundaries.
//!   * Receives `query(input)` messages from the main thread (Slint event
//!     loop), dispatches them to every plugin in parallel using the
//!     streaming dispatcher, and routes each yielded row back to the Slint
//!     thread via `slint::invoke_from_event_loop`.
//!   * On every new keystroke, cancels the in-flight dispatch (`CancellationToken`)
//!     and starts a fresh one. Stale yields are dropped via a monotonic
//!     `query_id` check the receiver task performs before invoking the UI.
//!   * Holds the latest result snapshot in an `Arc<Mutex<_>>` so the
//!     `invoke-selected` callback can look up the highlighted row's action.

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
use crate::plugins::actions;
use crate::plugins::dispatch::{self, StreamedResult};
use crate::plugins::loader::{self, LoaderOptions};
use crate::plugins::result::RankedResult;
use crate::plugins::runtime::LoadedPlugin;
use crate::ui::ResultRow;
use crate::window;

/// Handle to the running plugin host. Drop to shut it down.
pub struct PluginHost {
    query_tx: mpsc::UnboundedSender<HostMessage>,
}

enum HostMessage {
    Query { id: u64, input: String },
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
) -> Result<PluginHost, Box<dyn Error>> {
    let (tx, rx) = mpsc::unbounded_channel::<HostMessage>();

    // Latest results: shared between the main thread (Enter looks up actions
    // here) and the renderer closures that update it after each stream
    // event. `Arc<Mutex>` is fine because the mutex is uncontended (every
    // touch happens on the main thread).
    let latest: Arc<Mutex<Vec<RankedResult>>> = Arc::new(Mutex::new(Vec::new()));

    // Monotonic query id used to drop stale results from slow plugins. Bumped
    // on every keystroke; the renderer task carries the id with each yield
    // and ignores anything where the id is older than the current value.
    let latest_id = Arc::new(AtomicU64::new(0));

    // Frecency database — opened best-effort. A failure here logs and
    // returns `None`; the daemon stays usable, just without re-ranking.
    let frecency_db = open_frecency_db();

    spawn_runtime_thread(
        rx,
        plugins_override,
        window.as_weak(),
        Arc::clone(&latest),
        Arc::clone(&latest_id),
        frecency_db.clone(),
    )?;

    wire_window_callbacks(window, tx.clone(), latest, &latest_id, frecency_db);

    Ok(PluginHost { query_tx: tx })
}

/// Spawn the plugin-runtime background thread + tokio runtime that owns the
/// loaded plugins and dispatches queries.
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
                    eprintln!("plugins: failed to start tokio runtime: {err}");
                    return;
                }
            };

            runtime.block_on(async move {
                let opts = LoaderOptions::resolve(plugins_override);
                let plugins = loader::load_all(&opts).await;
                if plugins.is_empty() {
                    eprintln!(
                        "plugins: no plugins loaded (looked in {})",
                        opts.plugins_dir.display()
                    );
                }
                let plugins: Vec<Arc<LoadedPlugin>> = plugins;
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
                            // Eager per-query snapshot: one source of truth for
                            // this query's ranking, fast enough at our row scale.
                            let snapshot = frecency_db.as_ref().map(FrecencyDb::snapshot);
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

/// Wire the per-keystroke `on_query_edited` and Enter-key `on_invoke_selected`
/// callbacks on the Slint window to the runtime channel and action executor.
fn wire_window_callbacks(
    window: &QueryWindow,
    tx: mpsc::UnboundedSender<HostMessage>,
    latest: Arc<Mutex<Vec<RankedResult>>>,
    latest_id: &Arc<AtomicU64>,
    frecency_db: Option<FrecencyDb>,
) {
    let latest_id_for_main = Arc::clone(latest_id);
    window.on_query_edited(move |text| {
        let id = latest_id_for_main.fetch_add(1, Ordering::Relaxed) + 1;
        if tx
            .send(HostMessage::Query {
                id,
                input: text.into(),
            })
            .is_err()
        {
            eprintln!("plugins: runtime thread exited; query dropped");
        }
    });

    let weak_for_invoke = window.as_weak();
    window.on_invoke_selected(move || {
        invoke_selected(&weak_for_invoke, &latest, frecency_db.as_ref());
    });
}

/// Resolve the highlighted row, execute its action, and bump frecency on
/// success. Pulled out of [`wire_window_callbacks`] so the line budget is
/// readable and we can describe the contract in one place.
fn invoke_selected(
    weak: &slint::Weak<QueryWindow>,
    latest: &Arc<Mutex<Vec<RankedResult>>>,
    frecency_db: Option<&FrecencyDb>,
) {
    let Some(w) = weak.upgrade() else { return };
    let idx = usize::try_from(w.get_selected_index().max(0)).unwrap_or(0);
    let snapshot = match latest.lock() {
        Ok(s) => s,
        Err(err) => {
            eprintln!("plugins: latest results lock poisoned: {err}");
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
        Ok(()) => {
            if let Some(db) = frecency_db {
                spawn_pick_bump(db, plugin_name, result_key);
            }
        }
        Err(err) => {
            eprintln!("plugins: {plugin_name}: action failed: {err}");
        }
    }
    window::hide(&w);
}

/// Open the frecency DB at the platform default path.
///
/// Failures (no `ProjectDirs`, can't open file, schema init error) are
/// logged and we return `None` so the daemon stays functional with default
/// (Stage 4) ranking. The user sees a single warning at startup, not a
/// crash.
fn open_frecency_db() -> Option<FrecencyDb> {
    let Some(path) = frecency::default_db_path() else {
        eprintln!("frecency: could not resolve data dir; running without frecency");
        return None;
    };
    match FrecencyDb::open(&path) {
        Ok(db) => Some(db),
        Err(err) => {
            eprintln!(
                "frecency: failed to open {} ({err}); continuing without frecency",
                path.display()
            );
            None
        }
    }
}

/// Run the pick bump off the UI thread. We use a plain OS thread (not a
/// tokio task) because the bump callsite is on the Slint event-loop thread
/// where no tokio runtime is registered.
fn spawn_pick_bump(db: &FrecencyDb, plugin_name: String, result_key: String) {
    let db = db.clone();
    thread::Builder::new()
        .name("highbeam-frecency-bump".into())
        .spawn(move || {
            if let Err(err) = db.bump(&plugin_name, &result_key) {
                eprintln!("frecency: bump {plugin_name}:{result_key} failed: {err}");
            }
        })
        .ok();
}

/// Start a fresh query: clear the row list, kick off the streaming dispatch,
/// and spawn a receiver task that merges yields into the UI as they arrive.
/// Returns the dispatch's cancellation token so the next keystroke can fire
/// it.
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

    // Reset the live row list on the main thread.
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
/// Preserves `selected-index` if the previously-selected row still exists;
/// otherwise resets to 0. (Stage 3 always reset; Stage 4's streaming model
/// would make that thrash the user's selection.)
///
/// Icons are decoded here, on the Slint thread, because each `ResultRow`
/// carries an `image` value and `slint::Image` isn't trivially constructed
/// off-thread without dragging in the renderer's pixel-buffer types upstream.
/// Doing it per-yield keeps the cost bounded — `max_rows` is 9 — and means a
/// malformed data URI surfaces as a blank slot, not a crash.
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

/// `has-icon` drives the row's placeholder vs icon styling. We treat anything
/// that doesn't look like a base64 data URI as "no icon" so the row gets the
/// muted outline rather than appearing to load something we never resolved.
fn is_renderable_icon(spec: Option<&str>) -> bool {
    spec.is_some_and(|s| s.starts_with("data:") && s.contains(";base64,"))
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        let _ = self.query_tx.send(HostMessage::Shutdown);
    }
}
