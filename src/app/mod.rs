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
    /// bridge — Stage 4a still logs at DEBUG; Stage 4c paints to Slint.
    ViewInit {
        plugin: String,
        handle: u64,
        props: serde_json::Value,
    },
    /// Tear down a view frame on the JS side. Sent by
    /// `callbacks::pop_view_frame` and (later) by the
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

                                if let Err(err) = p.view_init(handle, &props, bridge).await {
                                    tracing::error!(%plugin, handle, %err, "views: init failed");
                                }
                            } else {
                                tracing::warn!(%plugin, handle, "views: init for unknown plugin");
                            }
                        }
                        HostMessage::ViewClose { plugin, handle } => {
                            let plugins = registry.snapshot().await;

                            if let Some(p) = plugins.iter().find(|p| p.manifest.name == plugin) {
                                if let Err(err) = p.view_close(handle).await {
                                    tracing::error!(%plugin, handle, %err, "views: close failed");
                                }
                            } else {
                                tracing::warn!(%plugin, handle, "views: close for unknown plugin");
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
