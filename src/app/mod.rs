//! Top-level coordinator wiring the Slint window to the plugin runtime.
//!
//! The plugins live inside a dedicated tokio current-thread runtime so the
//! rquickjs `AsyncRuntime` futures (`!Send` across `async_with`) can be polled
//! without crossing thread boundaries. Yields cross back to the Slint event
//! loop via `slint::invoke_from_event_loop`. Stale yields from slow plugins
//! are filtered by a monotonic `query_id`.
//!
//! Submodules:
//! - `callbacks` — Slint window-callback wiring.
//! - `query` — per-keystroke streaming dispatch + result rendering.
//! - `install_flow` — install / update / reload host-task pipeline.

mod callbacks;
mod install_flow;
mod query;

use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use slint::ComponentHandle;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::QueryWindow;
use crate::confirm::PendingConfirmation;
use crate::frecency::{self, FrecencyDb};
use crate::logging::LogErr;
use crate::plugins::actions;
use crate::plugins::loader::{self, LoaderOptions};
use crate::plugins::registry::PluginRegistry;
use crate::plugins::result::RankedResult;
use crate::plugins::runtime::LoadedPlugin;
use crate::query_history::{QueryHistoryDb, QueryHistoryState};
use crate::settings_ui::SettingsController;
use crate::views::ViewStack;

use callbacks::WindowCallbackCtx;

/// Handle to the running plugin host. Drop to shut it down.
pub struct PluginHost {
    query_tx: mpsc::UnboundedSender<HostMessage>,
}

/// Shared state for the install-confirmation gate. Held behind a `Mutex` so
/// the Slint-thread callbacks and the runtime-thread install task can
/// co-ordinate without data races.
pub(super) type ConfirmState = Arc<Mutex<Option<PendingConfirmation>>>;

pub(super) enum HostMessage {
    Query {
        id: u64,
        input: String,
    },
    Task(actions::HostTask),
    Shutdown,
    /// Drive a freshly-pushed view frame through its `setup → first render
    /// → mounted` sequence. Sent by `callbacks::push_view_frame`. The
    /// rendered tree comes back via the JS runtime's `__highbeam_paint_tree`
    /// bridge — painted into the launcher window from there.
    ViewInit {
        plugin: String,
        handle: u64,
        props: serde_json::Value,
    },
    /// User interaction with a rendered block fired a callback. Routed
    /// to the per-view spawn task (via `view_event_senders`) which calls
    /// `invoke_event` inside its `async_with!`.
    ViewEvent {
        plugin: String,
        handle: u64,
        callback_id: u64,
        value: serde_json::Value,
    },
    /// Tear down a view frame on the JS side. Sent by
    /// `callbacks::pop_view_frame` and by the
    /// `__highbeam_close_view_request` bridge when `render → null`.
    ViewClose {
        plugin: String,
        handle: u64,
    },
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

    let history_db = open_query_history_db();
    let initial_entries = history_db
        .as_ref()
        .map(|db| db.load_recent(settings.query_history_max_entries()))
        .unwrap_or_default();
    let history_state = Arc::new(Mutex::new(QueryHistoryState::new(initial_entries)));

    let view_stack = Arc::new(Mutex::new(ViewStack::new()));

    spawn_runtime_thread(
        rx,
        plugins_override,
        window.as_weak(),
        Arc::clone(&latest),
        Arc::clone(&latest_id),
        frecency_db.clone(),
        Arc::clone(&confirm_state),
        settings.clone(),
        Arc::clone(&view_stack),
        tx.clone(),
    )?;

    callbacks::wire_window_callbacks(
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
            view_stack,
        },
    );

    Ok(PluginHost { query_tx: tx })
}

// The runtime thread loop is a single coherent state machine — splitting
// it into per-variant helpers would only add artificial seams.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn spawn_runtime_thread(
    mut rx: mpsc::UnboundedReceiver<HostMessage>,
    plugins_override: Option<PathBuf>,
    weak: slint::Weak<QueryWindow>,
    latest: Arc<Mutex<Vec<RankedResult>>>,
    latest_id: Arc<AtomicU64>,
    frecency_db: Option<FrecencyDb>,
    confirm_state: ConfirmState,
    settings: SettingsController,
    view_stack: Arc<Mutex<ViewStack>>,
    host_tx: mpsc::UnboundedSender<HostMessage>,
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
                let outcomes = loader::load_all(&opts, &settings.snapshot()).await;

                if outcomes.is_empty() {
                    tracing::warn!(
                        plugins_dir = %opts.plugins_dir.display(),
                        "plugins: no plugins loaded",
                    );
                }
                // Fire onEnable for plugins the loader marked as Install /
                // Update (a sideloaded plugin with no recorded version, or a
                // version bump applied via disk while the daemon was off).
                // No-op when the plugin doesn't export the hook.
                install_flow::fire_enable_hooks(&outcomes, &settings);
                let plugins: Vec<Arc<LoadedPlugin>> = outcomes.into_iter().map(|o| o.plugin).collect();
                let registry = PluginRegistry::new(opts, plugins);
                let mut current_cancel: Option<CancellationToken> = None;
                // Per-view close signals — keyed on (plugin, handle) so a
                // ViewClose message can wake the matching `spawn_view`
                // task without us having to keep a handle on each task.
                let mut view_close_signals: std::collections::HashMap<
                    (String, u64),
                    tokio_util::sync::CancellationToken,
                > = std::collections::HashMap::new();
                // Per-view event senders. The receiver lives in the
                // spawn_view task's select! loop so events arrive
                // serialised through the same async_with! that owns the
                // plugin's QuickJS context.
                let mut view_event_senders: std::collections::HashMap<
                    (String, u64),
                    mpsc::UnboundedSender<crate::plugins::runtime::ViewEventEnvelope>,
                > = std::collections::HashMap::new();

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
                            let cancel = query::handle_query(
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
                            // Any host task (reload / install / update)
                            // can swap the QuickJS context a view is
                            // attached to. Tear down every open frame
                            // first so plugins get a clean `unmounted`
                            // call before their old context dies.
                            for ((plugin, handle), signal) in view_close_signals.drain() {
                                tracing::info!(
                                    %plugin,
                                    handle,
                                    reason = "host task",
                                    "views: tearing down",
                                );
                                signal.cancel();
                            }
                            view_event_senders.clear();

                            if !view_stack.lock().map_or(true, |s| s.depth() == 0) {
                                let view_stack_for_clear = Arc::clone(&view_stack);
                                let weak_for_clear = weak.clone();
                                slint::invoke_from_event_loop(move || {
                                    if let Ok(mut stack) = view_stack_for_clear.lock() {
                                        stack.clear();
                                    }

                                    if let Some(w) = weak_for_clear.upgrade() {
                                        w.invoke_show_query();
                                    }
                                })
                                .log_debug("views: host-task stack clear");
                            }
                            // Bump the query id so any stale yield from the
                            // last keystroke can't paint over the progress
                            // rows the task is about to push.
                            let task_id = latest_id.fetch_add(1, Ordering::Relaxed) + 1;
                            let progress = install_flow::ProgressEmitter::new(
                                task_id,
                                weak.clone(),
                                Arc::clone(&latest),
                                Arc::clone(&latest_id),
                            );
                            install_flow::handle_host_task(
                                task,
                                &registry,
                                progress,
                                Arc::clone(&confirm_state),
                                weak.clone(),
                                &settings,
                            )
                            .await;
                        }
                        HostMessage::ViewInit { plugin, handle, props } => {
                            let plugins = registry.snapshot().await;

                            if let Some(p) = plugins.iter().find(|p| p.manifest.name == plugin) {
                                let bridge = callbacks::build_view_bridge(
                                    &plugin,
                                    Arc::clone(&view_stack),
                                    host_tx.clone(),
                                    &weak,
                                );
                                let (event_tx, event_rx) = mpsc::unbounded_channel();
                                view_close_signals.insert((plugin.clone(), handle), bridge.close_signal.clone());
                                view_event_senders.insert((plugin.clone(), handle), event_tx);
                                p.spawn_view(handle, &props, bridge, event_rx);
                            } else {
                                tracing::warn!(%plugin, handle, "views: init for unknown plugin");
                            }
                        }
                        HostMessage::ViewEvent {
                            plugin,
                            handle,
                            callback_id,
                            value,
                        } => {
                            if let Some(tx) = view_event_senders.get(&(plugin.clone(), handle)) {
                                if tx
                                    .send(crate::plugins::runtime::ViewEventEnvelope { callback_id, value })
                                    .is_err()
                                {
                                    tracing::debug!(%plugin, handle, callback_id, "views: event channel closed");
                                }
                            } else {
                                tracing::debug!(%plugin, handle, callback_id, "views: event for unknown view");
                            }
                        }
                        HostMessage::ViewClose { plugin, handle } => {
                            // Fire the bridge's close_signal — the per-view
                            // tokio task awaits it, runs `unmounted` inside
                            // its own async_with!, and exits. Drop the
                            // event sender so future events for this
                            // handle log + bail instead of queueing into a
                            // closed task.
                            view_event_senders.remove(&(plugin.clone(), handle));

                            if let Some(signal) = view_close_signals.remove(&(plugin.clone(), handle)) {
                                signal.cancel();
                            } else {
                                tracing::debug!(%plugin, handle, "views: close for unknown handle");
                            }
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

impl Drop for PluginHost {
    fn drop(&mut self) {
        self.query_tx
            .send(HostMessage::Shutdown)
            .log_debug("PluginHost::drop: worker already gone");
    }
}
