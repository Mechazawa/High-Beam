//! Top-level coordinator wiring the Slint window to the plugin runtime.
//!
//! Responsibilities:
//!   * Owns a dedicated background thread running a Tokio current-thread
//!     runtime; the loaded plugins live inside that thread so the rquickjs
//!     `AsyncRuntime` futures (`!Send` across `async_with`) can be polled
//!     without crossing thread boundaries.
//!   * Receives `query(input)` messages from the main thread (Slint event
//!     loop), dispatches them across plugins, and routes the resulting
//!     `Vec<RankedResult>` back to the Slint thread via
//!     `slint::invoke_from_event_loop`.
//!   * Holds the latest result snapshot in an `Arc<Mutex<_>>` so the
//!     `invoke-selected` callback can look up the highlighted row's action.
//!
//! Stage 4 will replace the "collect to Vec then render" model with
//! streaming, `AbortSignal` cancellation, and per-plugin debounce.

use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::sync::mpsc;

use crate::QueryWindow;
use crate::plugins::actions;
use crate::plugins::dispatch;
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
pub fn start(window: &QueryWindow) -> Result<PluginHost, Box<dyn Error>> {
    let (tx, mut rx) = mpsc::unbounded_channel::<HostMessage>();

    // Latest results: shared between the main thread (Enter looks up actions
    // here) and the `invoke_from_event_loop` closure on the main thread that
    // refreshes it after each dispatch. `Arc<Mutex>` rather than `Rc<RefCell>`
    // so we can move a clone into the worker-spawned closure that runs on
    // the main thread (which the borrow checker treats as a separate task).
    let latest: Arc<Mutex<Vec<RankedResult>>> = Arc::new(Mutex::new(Vec::new()));

    // Monotonic query id used to drop stale results from slow plugins. The
    // `AtomicU64` is bumped on every keystroke (main thread) and read by the
    // worker so it can skip work entirely when a newer keystroke arrives
    // before we finished the previous one.
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
                let plugins = loader::load_all(&LoaderOptions::dev_default()).await;
                if plugins.is_empty() {
                    eprintln!("plugins: no plugins loaded (drop one in ./plugins/)");
                }
                let plugins: Vec<LoadedPlugin> = plugins;
                while let Some(msg) = rx.recv().await {
                    match msg {
                        HostMessage::Query { id, input } => {
                            if id < latest_id_for_worker.load(Ordering::Relaxed) {
                                continue;
                            }
                            let results = dispatch::dispatch(&plugins, &input).await;
                            if id < latest_id_for_worker.load(Ordering::Relaxed) {
                                continue;
                            }
                            let weak = weak_for_worker.clone();
                            let latest = Arc::clone(&latest_for_worker);
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = weak.upgrade() {
                                    render_results(&w, &results);
                                    if let Ok(mut slot) = latest.lock() {
                                        *slot = results;
                                    }
                                }
                            });
                        }
                        HostMessage::Shutdown => break,
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
        // Mirror v2's "run and close" behaviour. `window::hide` clears the
        // input and result list via the slint-side `clear-input()`.
        window::hide(&w);
    });

    Ok(PluginHost { query_tx: tx })
}

/// Push the dispatch results into the window's row model.
///
/// Resets `selected-index` to 0 so the first row is highlighted whenever a
/// new result set lands.
fn render_results(window: &QueryWindow, results: &[RankedResult]) {
    let rows: Vec<ResultRow> = results
        .iter()
        .map(|r| ResultRow {
            key: r.composite_key().into(),
            title: r.result.title.clone().into(),
            subtitle: r.result.subtitle.clone().unwrap_or_default().into(),
            has_subtitle: r.result.subtitle.is_some(),
        })
        .collect();
    window.set_results(ModelRc::new(VecModel::from(rows)));
    window.set_selected_index(0);
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        let _ = self.query_tx.send(HostMessage::Shutdown);
    }
}
