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
    let (tx, mut rx) = mpsc::unbounded_channel::<HostMessage>();

    // Latest results: shared between the main thread (Enter looks up actions
    // here) and the renderer closures that update it after each stream
    // event. `Arc<Mutex>` is fine because the mutex is uncontended (every
    // touch happens on the main thread).
    let latest: Arc<Mutex<Vec<RankedResult>>> = Arc::new(Mutex::new(Vec::new()));

    // Monotonic query id used to drop stale results from slow plugins. Bumped
    // on every keystroke; the renderer task carries the id with each yield
    // and ignores anything where the id is older than the current value.
    let latest_id = Arc::new(AtomicU64::new(0));

    let weak_for_worker = window.as_weak();
    let latest_for_worker = Arc::clone(&latest);
    let latest_id_for_worker = Arc::clone(&latest_id);

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
                            if id < latest_id_for_worker.load(Ordering::Relaxed) {
                                continue;
                            }
                            if let Some(prev) = current_cancel.take() {
                                prev.cancel();
                            }
                            let cancel = handle_query(
                                id,
                                &input,
                                &plugins,
                                weak_for_worker.clone(),
                                Arc::clone(&latest_for_worker),
                                Arc::clone(&latest_id_for_worker),
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

    let tx_for_query = tx.clone();
    let latest_id_for_main = Arc::clone(&latest_id);
    window.on_query_edited(move |text| {
        let id = latest_id_for_main.fetch_add(1, Ordering::Relaxed) + 1;
        if tx_for_query
            .send(HostMessage::Query {
                id,
                input: text.into(),
            })
            .is_err()
        {
            eprintln!("plugins: runtime thread exited; query dropped");
        }
    });

    let latest_for_invoke = Arc::clone(&latest);
    let weak_for_invoke = window.as_weak();
    window.on_invoke_selected(move || {
        let Some(w) = weak_for_invoke.upgrade() else {
            return;
        };
        let idx = usize::try_from(w.get_selected_index().max(0)).unwrap_or(0);
        let snapshot = match latest_for_invoke.lock() {
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
        drop(snapshot);
        if let Err(err) = actions::execute(&action) {
            eprintln!("plugins: {plugin_name}: action failed: {err}");
        }
        window::hide(&w);
    });

    Ok(PluginHost { query_tx: tx })
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
                        dispatch::merge_into_live(&mut live, &mut order, streamed);
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
fn render_results(window: &QueryWindow, results: &[RankedResult]) {
    let previously_selected = window.get_selected_index();
    let rows: Vec<ResultRow> = results
        .iter()
        .map(|r| ResultRow {
            key: r.composite_key().into(),
            title: r.result.title.clone().into(),
            subtitle: r.result.subtitle.clone().unwrap_or_default().into(),
            has_subtitle: r.result.subtitle.is_some(),
        })
        .collect();
    let row_count = i32::try_from(rows.len()).unwrap_or(i32::MAX);
    window.set_results(ModelRc::new(VecModel::from(rows)));
    if previously_selected >= row_count || previously_selected < 0 {
        window.set_selected_index(0);
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        let _ = self.query_tx.send(HostMessage::Shutdown);
    }
}
