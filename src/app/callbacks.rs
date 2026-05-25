//! Slint window-callback wiring: query edits, action invocation, history
//! cycling, install-confirmation routing. The runtime thread reaches in via
//! a `mpsc` channel; this module owns the UI-thread side of the bridge.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::sync::mpsc;

use crate::QueryWindow;
use crate::frecency::FrecencyDb;
use crate::logging::LogErr;
use crate::plugins::actions;
use crate::plugins::result::{Action, RankedResult};
use crate::query_history::{InputAction, QueryHistoryDb, QueryHistoryState};
use crate::sdk::view::RuntimeBridge;
use crate::settings_ui::SettingsController;
use crate::ui::ResultRow;
use crate::views::{PushError, ViewFrame, ViewStack};
use crate::window;

use super::{ConfirmState, HostMessage};

/// Aggregate of plugin-result + confirm + history state threaded into
/// `wire_window_callbacks`. Groups logically-related handles so the function
/// stays within clippy's argument-count limit.
pub(super) struct WindowCallbackCtx {
    pub latest: Arc<Mutex<Vec<RankedResult>>>,
    pub frecency_db: Option<FrecencyDb>,
    pub settings: SettingsController,
    pub confirm_state: ConfirmState,
    pub history_db: Option<QueryHistoryDb>,
    pub history_state: Arc<Mutex<QueryHistoryState>>,
    pub view_stack: Arc<Mutex<ViewStack>>,
}

pub(super) fn wire_window_callbacks(
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
        view_stack,
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
    let view_stack_for_invoke = Arc::clone(&view_stack);

    window.on_invoke_selected(move |meta, control, shift, alt| {
        let mods = (u8::from(meta) * crate::hotkey::MOD_META)
            | (u8::from(control) * crate::hotkey::MOD_CONTROL)
            | (u8::from(shift) * crate::hotkey::MOD_SHIFT)
            | (u8::from(alt) * crate::hotkey::MOD_ALT);
        let alt_held = settings_for_invoke.alt_modifier_held(mods);

        invoke_selected(
            &weak_for_invoke,
            &latest,
            frecency_db.as_ref(),
            &settings_for_invoke,
            &tx_for_invoke,
            history_db_for_invoke.as_ref(),
            &history_state_for_invoke,
            &view_stack_for_invoke,
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
        tx.send(decision)
            .log_debug("confirm: pending receiver gone before decision");
    }
    let weak = weak.clone();
    slint::invoke_from_event_loop(move || {
        if let Some(w) = weak.upgrade() {
            w.invoke_show_query();
        }
    })
    .log_debug("confirm: post show_query to event loop");
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
    view_stack: &Arc<Mutex<ViewStack>>,
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
                spawn_pick_bump(db, plugin_name.clone(), result_key);
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
                actions::ActionOutcome::ShowView { handle, props, reset } => {
                    push_view_frame(view_stack, host_tx, &plugin_name, handle, props, reset);
                }
                actions::ActionOutcome::CloseView => {
                    pop_view_frame(view_stack, host_tx);
                }
            }
        }
        Err(err) => {
            tracing::error!(plugin = %plugin_name, %err, "plugins: action failed");
            window::hide_and_persist_position(&w, settings);
        }
    }
}

/// Construct the [`RuntimeBridge`] the JS view runtime calls back into.
/// Each plugin context gets its own — the closures capture the plugin
/// name plus enough Slint-thread state (the view stack, the host-message
/// channel) to route `dispatch` and `close_view_request` actions back
/// through `slint::invoke_from_event_loop`.
///
/// Called from the runtime thread when a `HostMessage::ViewInit` arrives;
/// the bridge then lives inside the plugin's `QuickJS` context until the
/// frame closes.
#[must_use]
pub(super) fn build_view_bridge(
    plugin_name: &str,
    view_stack: Arc<Mutex<ViewStack>>,
    host_tx: mpsc::UnboundedSender<HostMessage>,
) -> Arc<RuntimeBridge> {
    let dispatch = {
        let plugin = plugin_name.to_owned();
        let view_stack = Arc::clone(&view_stack);
        let host_tx = host_tx.clone();
        Box::new(move |action_json: String| {
            let plugin = plugin.clone();
            let view_stack = Arc::clone(&view_stack);
            let host_tx = host_tx.clone();
            slint::invoke_from_event_loop(move || {
                handle_view_dispatch(&plugin, &action_json, &view_stack, &host_tx);
            })
            .log_debug("views: dispatch invoke_from_event_loop");
        })
    };
    let close_request = {
        let plugin = plugin_name.to_owned();
        Box::new(move |handle: u64| {
            let plugin = plugin.clone();
            let view_stack = Arc::clone(&view_stack);
            let host_tx = host_tx.clone();
            slint::invoke_from_event_loop(move || {
                handle_view_close_request(&plugin, handle, &view_stack, &host_tx);
            })
            .log_debug("views: close_view_request invoke_from_event_loop");
        })
    };
    Arc::new(RuntimeBridge {
        plugin_name: plugin_name.to_owned(),
        dispatch,
        close_request,
    })
}

/// Slint-thread handler for `__highbeam_dispatch(action_json)`. Parses
/// the action, runs `actions::execute`, and routes the resulting
/// outcome — minus the `HideWindow` effect, which would tear down the
/// view the dispatch was *fired from*. View-only outcomes (`ShowView` /
/// `CloseView`) push or pop the stack; simple side-effecting actions
/// already ran inside `execute` before we got the outcome.
fn handle_view_dispatch(
    plugin: &str,
    action_json: &str,
    view_stack: &Arc<Mutex<ViewStack>>,
    host_tx: &mpsc::UnboundedSender<HostMessage>,
) {
    let action: Action = match serde_json::from_str(action_json) {
        Ok(a) => a,
        Err(err) => {
            tracing::error!(%plugin, %err, %action_json, "views: dispatch parse failed");
            return;
        }
    };
    let outcome = match actions::execute(&action) {
        Ok(o) => o,
        Err(err) => {
            tracing::error!(%plugin, %err, "views: dispatch execute failed");
            return;
        }
    };

    match outcome {
        // Dispatched-from-view: HideWindow's "hide" half is a no-op. The
        // action's side effect (URL opened, clipboard written, subprocess
        // spawned, etc.) already ran inside execute() above.
        actions::ActionOutcome::HideWindow | actions::ActionOutcome::KeepOpen => {}
        actions::ActionOutcome::ShowView { handle, props, reset } => {
            push_view_frame(view_stack, host_tx, plugin, handle, props, reset);
        }
        actions::ActionOutcome::CloseView => {
            pop_view_frame(view_stack, host_tx);
        }
        actions::ActionOutcome::OpenSettingsView => {
            tracing::warn!(%plugin, "views: dispatch of openSettings ignored — would tear down the view");
        }
        actions::ActionOutcome::HostTask(_) => {
            tracing::warn!(%plugin, "views: dispatch of host task ignored");
        }
    }
}

/// Slint-thread handler for `__highbeam_close_view_request(handle)`. The
/// JS runtime has already torn down its instance for `handle`; we just
/// need to pop the matching frame and notify the runtime thread (which
/// then calls `view_close`, a safe no-op on an already-closed JS
/// instance). Verifies the top frame matches `(plugin, handle)` — a
/// mismatch means a paused deeper frame somehow triggered the close,
/// which would be a bug we should observe.
fn handle_view_close_request(
    plugin: &str,
    handle: u64,
    view_stack: &Arc<Mutex<ViewStack>>,
    host_tx: &mpsc::UnboundedSender<HostMessage>,
) {
    let Ok(mut stack) = view_stack.lock() else {
        tracing::error!("views: stack lock poisoned; close_view_request dropped");
        return;
    };
    let Some(top) = stack.top() else {
        tracing::debug!(%plugin, handle, "views: close_view_request on empty stack");
        return;
    };

    if top.plugin_name != plugin || top.handle != handle {
        tracing::warn!(
            %plugin,
            handle,
            top_plugin = %top.plugin_name,
            top_handle = top.handle,
            "views: close_view_request for non-top frame; ignored",
        );
        return;
    }
    let frame = stack.pop().expect("top present");
    drop(stack);
    tracing::info!(plugin = %frame.plugin_name, handle = frame.handle, "views: render-null triggered close");

    if host_tx
        .send(HostMessage::ViewClose {
            plugin: frame.plugin_name,
            handle: frame.handle,
        })
        .is_err()
    {
        tracing::error!("views: runtime thread exited; view close dropped");
    }
}

/// Push a freshly-minted [`ViewFrame`] onto the shared stack and dispatch
/// a [`HostMessage::ViewInit`] to the runtime thread so the plugin's JS
/// runtime can run `setup → first render → mounted`. A rejected push
/// (stack at cap) logs an ERROR and does *not* send the `ViewInit` — the
/// action is observably dropped instead of silently lost.
fn push_view_frame(
    view_stack: &Arc<Mutex<ViewStack>>,
    host_tx: &mpsc::UnboundedSender<HostMessage>,
    plugin_name: &str,
    handle: u64,
    props: serde_json::Value,
    reset: bool,
) {
    let Ok(mut stack) = view_stack.lock() else {
        tracing::error!("views: stack lock poisoned; push dropped");
        return;
    };
    let props_for_init = props.clone();
    let frame = ViewFrame::new(plugin_name.to_owned(), handle, props);

    match stack.push(frame, reset) {
        Ok(()) => {
            tracing::info!(
                plugin = %plugin_name,
                handle,
                reset,
                depth = stack.depth(),
                "views: frame pushed",
            );
            // Drop the stack lock before posting — the runtime thread
            // may immediately call back into the bridge globals and try
            // to grab the same lock.
            drop(stack);

            if host_tx
                .send(HostMessage::ViewInit {
                    plugin: plugin_name.to_owned(),
                    handle,
                    props: props_for_init,
                })
                .is_err()
            {
                tracing::error!("views: runtime thread exited; view init dropped");
            }
        }
        Err(PushError::AtCap) => tracing::error!(
            plugin = %plugin_name,
            handle,
            depth = stack.depth(),
            "views: push rejected at stack cap",
        ),
    }
}

/// Pop the topmost frame off the shared stack and dispatch
/// [`HostMessage::ViewClose`] so the plugin's JS runtime runs
/// `unmounted` and clears its per-handle state. A no-op on an empty
/// stack (and not an error — a stale `closeView` after the user already
/// Esc'd out is a benign race).
fn pop_view_frame(view_stack: &Arc<Mutex<ViewStack>>, host_tx: &mpsc::UnboundedSender<HostMessage>) {
    let Ok(mut stack) = view_stack.lock() else {
        tracing::error!("views: stack lock poisoned; pop dropped");
        return;
    };

    let Some(frame) = stack.pop() else {
        tracing::debug!("views: pop on empty stack");
        return;
    };
    tracing::info!(
        plugin = %frame.plugin_name,
        handle = frame.handle,
        depth = stack.depth(),
        "views: frame popped",
    );
    drop(stack);

    if host_tx
        .send(HostMessage::ViewClose {
            plugin: frame.plugin_name,
            handle: frame.handle,
        })
        .is_err()
    {
        tracing::error!("views: runtime thread exited; view close dropped");
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
