//! Slint window-callback wiring: query edits, action invocation, history
//! cycling, install-confirmation routing. The runtime thread reaches in via
//! a `mpsc` channel; this module owns the UI-thread side of the bridge.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tokio::sync::mpsc;

use crate::QueryWindow;
use crate::frecency::FrecencyDb;
use crate::logging::LogErr;
use crate::plugins::actions;
use crate::plugins::result::{Action, RankedResult};
use crate::query_history::{InputAction, QueryHistoryDb, QueryHistoryState};
use crate::sdk::view::RuntimeBridge;
use crate::settings_ui::SettingsController;
use crate::ui::{ResultRow, ViewBlock};
use crate::views::{PushError, ViewFrame, ViewStack};
use crate::window;

use super::host_view::{self as host_view_mod, HostView};
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
    pub host_view: HostView,
}

// Window callback wiring is a flat sequence of per-callback blocks;
// splitting helpers per chunk just adds indirection without reducing
// total surface.
#[allow(clippy::too_many_lines)]
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
        host_view,
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
    let host_view_for_invoke = Arc::clone(&host_view);

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
            &host_view_for_invoke,
            alt_held,
        );
    });

    wire_history_callbacks(window, &history_state, history_db.as_ref(), &settings);

    // Esc inside a pushed plugin view pops the top frame instead of
    // hiding the launcher. The frame's close_signal fires (so the
    // plugin's JS unmounted runs); when the last frame leaves the
    // stack pop_view_frame switches the window back to VIEW-QUERY.
    // A live host view (e.g. update progress) takes precedence and
    // closes before the JS stack is touched.
    let weak_for_pop = window.as_weak();
    let view_stack_for_pop = Arc::clone(&view_stack);
    let host_view_for_pop = Arc::clone(&host_view);
    let tx_for_pop = tx.clone();

    window.on_pop_view(move || {
        pop_view_frame(&host_view_for_pop, &view_stack_for_pop, &tx_for_pop, &weak_for_pop);
    });

    // Fires just before the launcher hides for any reason (Esc on
    // root, focus loss, action-induced HideWindow). Drain the view
    // stack so every per-view spawn_view task gets its unmounted
    // hook called before its QuickJS context is torn down.
    let view_stack_for_clear = Arc::clone(&view_stack);
    let host_view_for_clear = Arc::clone(&host_view);
    let tx_for_clear = tx.clone();
    let weak_for_clear = window.as_weak();

    window.on_clear_view_stack(move || {
        clear_view_stack_for_hide(
            &host_view_for_clear,
            &view_stack_for_clear,
            &tx_for_clear,
            &weak_for_clear,
        );
    });

    // Block-level events from inside a pushed view (button click,
    // input change, submit). The callback id was substituted in by
    // the SDK render walker; the host pairs it with the top frame's
    // (plugin, handle) and dispatches HostMessage::ViewEvent.
    let view_stack_for_click = Arc::clone(&view_stack);
    let tx_for_click = tx.clone();

    window.on_view_block_clicked(move |callback_id| {
        send_view_event(
            &view_stack_for_click,
            &tx_for_click,
            callback_id,
            serde_json::Value::Null,
        );
    });

    let view_stack_for_change = Arc::clone(&view_stack);
    let tx_for_change = tx.clone();

    window.on_view_block_changed(move |callback_id, text| {
        send_view_event(
            &view_stack_for_change,
            &tx_for_change,
            callback_id,
            serde_json::Value::String(text.into()),
        );
    });

    let view_stack_for_submit = Arc::clone(&view_stack);
    let tx_for_submit = tx.clone();

    window.on_view_block_submitted(move |callback_id, text| {
        send_view_event(
            &view_stack_for_submit,
            &tx_for_submit,
            callback_id,
            serde_json::Value::String(text.into()),
        );
    });

    let confirm_state_install = Arc::clone(&confirm_state);
    let host_view_for_confirm_install = Arc::clone(&host_view);
    let weak_confirm_install = window.as_weak();

    window.on_confirm_install(move || {
        send_confirm_decision(
            &confirm_state_install,
            &host_view_for_confirm_install,
            true,
            &weak_confirm_install,
        );
    });

    let confirm_state_cancel = confirm_state;
    let host_view_for_confirm_cancel = Arc::clone(&host_view);
    let weak_confirm_cancel = window.as_weak();

    window.on_confirm_cancel(move || {
        send_confirm_decision(
            &confirm_state_cancel,
            &host_view_for_confirm_cancel,
            false,
            &weak_confirm_cancel,
        );
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
/// Then restore the originating view (launcher by default, host view when
/// one is live — e.g. during the `update` flow's per-plugin cap prompts).
fn send_confirm_decision(state: &ConfirmState, host_view: &HostView, decision: bool, weak: &slint::Weak<QueryWindow>) {
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
    let host_view_active = host_view.lock().is_ok_and(|g| g.is_some());
    let host_view = Arc::clone(host_view);
    let weak = weak.clone();
    slint::invoke_from_event_loop(move || {
        if host_view_active {
            host_view_mod::paint(&host_view, &weak);
        } else if let Some(w) = weak.upgrade() {
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
    host_view: &HostView,
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
                    pop_view_frame(host_view, view_stack, host_tx, weak);
                }
                actions::ActionOutcome::ShowUpdateView => {
                    open_update_view(host_view, host_tx, &w);
                }
            }
        }
        Err(err) => {
            tracing::error!(plugin = %plugin_name, %err, "plugins: action failed");
            window::hide_and_persist_position(&w, settings);
        }
    }
}

/// Seed the host view slot with a fresh [`UpdateViewState`], paint the
/// initial "Checking…" frame, and post `HostMessage::UpdateAll` so the
/// runtime thread starts walking plugins. Replaces any existing host
/// view (firing its cancel token first) so two rapid Enters don't
/// stack updates.
fn open_update_view(host_view: &HostView, host_tx: &mpsc::UnboundedSender<HostMessage>, window: &QueryWindow) {
    {
        let Ok(mut guard) = host_view.lock() else {
            tracing::error!("update: host_view lock poisoned on open");
            return;
        };
        if let Some(prev) = guard.take() {
            prev.cancel.cancel();
        }
        *guard = Some(host_view_mod::UpdateViewState::new());
    }
    // Initial paint so the user sees the view immediately even before the
    // runtime thread fetches the plugin list.
    let weak = window.as_weak();
    host_view_mod::paint(host_view, &weak);

    if host_tx.send(HostMessage::UpdateAll).is_err() {
        tracing::error!("update: runtime thread exited; UpdateAll dropped");
    }
}

/// Drain every frame off the view stack, sending
/// [`HostMessage::ViewClose`] for each so the per-view `spawn_view`
/// tasks run `unmounted` + tear down their JS state. Switches the
/// window back to `VIEW-QUERY` once the stack is empty. Also clears
/// any live host view (firing its cancel token). Idempotent.
fn clear_view_stack_for_hide(
    host_view: &HostView,
    view_stack: &Arc<Mutex<ViewStack>>,
    host_tx: &mpsc::UnboundedSender<HostMessage>,
    weak: &slint::Weak<QueryWindow>,
) {
    let host_view_was_live = host_view_mod::take_and_cancel(host_view);

    let popped: Vec<ViewFrame> = {
        let Ok(mut stack) = view_stack.lock() else {
            tracing::error!("views: stack lock poisoned; clear dropped");
            return;
        };
        stack.clear()
    };

    if popped.is_empty() && !host_view_was_live {
        return;
    }
    for frame in &popped {
        if host_tx
            .send(HostMessage::ViewClose {
                plugin: frame.plugin_name.clone(),
                handle: frame.handle,
            })
            .is_err()
        {
            tracing::error!("views: runtime thread exited; view close dropped during clear");
        }
    }
    tracing::info!(
        count = popped.len(),
        host_view_was_live,
        "views: stack drained on launcher hide",
    );

    if let Some(window) = weak.upgrade() {
        sync_view_blocks_model(&window, Vec::new());
        window.invoke_show_query();
    }
}

/// Resolve a Slint-side `callback_id` against the top view frame and
/// post a [`HostMessage::ViewEvent`] to the runtime thread. A click /
/// change / submit can only fire on the visible (top) frame, so the
/// stack's top entry is authoritative for `(plugin, handle)`.
fn send_view_event(
    view_stack: &Arc<Mutex<ViewStack>>,
    host_tx: &mpsc::UnboundedSender<HostMessage>,
    callback_id: i32,
    value: serde_json::Value,
) {
    let id = match u64::try_from(callback_id) {
        Ok(0) => return, // 0 is the sentinel for "no handler"
        Ok(id) => id,
        Err(_) => {
            tracing::warn!(callback_id, "views: negative callback id; dropped");
            return;
        }
    };
    let Ok(stack) = view_stack.lock() else {
        tracing::error!("views: stack lock poisoned; event dropped");
        return;
    };
    let Some(top) = stack.top() else {
        tracing::debug!(callback_id = id, "views: event with empty stack; dropped");
        return;
    };
    let plugin = top.plugin_name.clone();
    let handle = top.handle;
    drop(stack);

    if host_tx
        .send(HostMessage::ViewEvent {
            plugin,
            handle,
            callback_id: id,
            value,
        })
        .is_err()
    {
        tracing::error!("views: runtime thread exited; event dropped");
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
    slint_weak: &slint::Weak<QueryWindow>,
) -> Arc<RuntimeBridge> {
    let paint_tree = {
        let plugin = plugin_name.to_owned();
        let weak = slint_weak.clone();
        Box::new(move |handle: u64, tree_json: String| {
            let plugin = plugin.clone();
            let weak = weak.clone();
            slint::invoke_from_event_loop(move || {
                paint_view_tree(&plugin, handle, &tree_json, &weak);
            })
            .log_debug("views: paint_tree invoke_from_event_loop");
        })
    };
    let paint_error = {
        let plugin = plugin_name.to_owned();
        let weak = slint_weak.clone();
        Box::new(move |handle: u64, message: String, stack: String| {
            let plugin = plugin.clone();
            let weak = weak.clone();
            slint::invoke_from_event_loop(move || {
                paint_view_error(&plugin, handle, &message, &stack, &weak);
            })
            .log_debug("views: paint_error invoke_from_event_loop");
        })
    };
    let dispatch = {
        let plugin = plugin_name.to_owned();
        let view_stack = Arc::clone(&view_stack);
        let host_tx = host_tx.clone();
        let weak = slint_weak.clone();
        Box::new(move |action_json: String| {
            let plugin = plugin.clone();
            let view_stack = Arc::clone(&view_stack);
            let host_tx = host_tx.clone();
            let weak = weak.clone();
            slint::invoke_from_event_loop(move || {
                handle_view_dispatch(&plugin, &action_json, &view_stack, &host_tx, &weak);
            })
            .log_debug("views: dispatch invoke_from_event_loop");
        })
    };
    let close_request = {
        let plugin = plugin_name.to_owned();
        let weak = slint_weak.clone();
        Box::new(move |handle: u64| {
            let plugin = plugin.clone();
            let view_stack = Arc::clone(&view_stack);
            let host_tx = host_tx.clone();
            let weak = weak.clone();
            slint::invoke_from_event_loop(move || {
                handle_view_close_request(&plugin, handle, &view_stack, &host_tx, &weak);
            })
            .log_debug("views: close_view_request invoke_from_event_loop");
        })
    };
    Arc::new(RuntimeBridge {
        plugin_name: plugin_name.to_owned(),
        close_signal: tokio_util::sync::CancellationToken::new(),
        paint_tree,
        paint_error,
        dispatch,
        close_request,
    })
}

/// Slint-thread handler for `__highbeam_paint_error(handle, message,
/// stack)`. Builds a synthetic error frame body in place of the
/// plugin's normal render, so an uncaught throw surfaces visibly
/// instead of leaving the view stuck on its last successful paint.
/// Esc dismisses the error frame the same way it dismisses any
/// other view (the JS side already ran `closeFrame` before firing
/// `paint_error`).
fn paint_view_error(plugin: &str, handle: u64, message: &str, stack: &str, weak: &slint::Weak<QueryWindow>) {
    tracing::error!(%plugin, handle, %message, "views: render error");
    if !stack.is_empty() {
        tracing::error!(%plugin, handle, stack = %stack, "views: render error stack");
    }
    let Some(window) = weak.upgrade() else { return };

    // `text` carries the plugin name + summary; `label` carries the
    // collapsed stack. Slint's ViewBlock renders kind="error" with a
    // red banner and monospaced stack body.
    let blocks = vec![ViewBlock {
        kind: "error".into(),
        text: message.into(),
        has_text: true,
        label: stack.into(),
        has_label: !stack.is_empty(),
        tone: String::new().into(),
        size: String::new().into(),
        value: String::new().into(),
        id: String::new().into(),
        on_click_id: 0,
        on_change_id: 0,
        on_submit_id: 0,
        has_value: false,
        has_on_click: false,
        has_on_change: false,
        has_on_submit: false,
        progress_value: -1.0,
        has_progress_value: false,
    }];
    window.set_view_frame_title(format!("{plugin} crashed").into());
    window.set_view_has_title(true);
    sync_view_blocks_model(&window, blocks);
    window.invoke_show_view_frame();
}

/// Slint-thread handler for `__highbeam_paint_tree(handle, tree_json)`.
/// Parses the JSON tree the plugin's `render()` produced, flattens it
/// into a list of `ViewBlock`s the Slint side iterates, and pushes the
/// result onto the `QueryWindow`'s view-frame properties (switching
/// `current-view` to `VIEW-VIEWS` if not already).
///
/// Stage 4c's flattener is deliberately lossy: `Stack` containers
/// contribute their children inline (no direction / gap / align yet);
/// non-rendering blocks (`button`, `input`, `textarea`, `image`, `row`,
/// `divider`) come through as their `kind` only — Slint paints them as
/// muted placeholders.
fn paint_view_tree(plugin: &str, handle: u64, tree_json: &str, weak: &slint::Weak<QueryWindow>) {
    let parsed: serde_json::Value = match serde_json::from_str(tree_json) {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(%plugin, handle, %err, "views: tree parse failed");
            return;
        }
    };
    let title = parsed
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let mut blocks = Vec::new();

    if let Some(body) = parsed.get("body").and_then(serde_json::Value::as_array) {
        for entry in body {
            flatten_block(entry, &mut blocks);
        }
    }
    let Some(window) = weak.upgrade() else {
        tracing::debug!(%plugin, handle, "views: paint dropped — window gone");
        return;
    };
    let has_title = title.is_some();

    window.set_view_frame_title(title.unwrap_or_default().into());
    window.set_view_has_title(has_title);
    sync_view_blocks_model(&window, blocks);
    window.invoke_show_view_frame();
}

/// Update the persistent [`VecModel`] backing the window's
/// `view_blocks` property in place. Slint's `for`-loop preserves
/// child-element identity per row index — so as long as the model
/// itself is the same `Rc`, the per-row `LineEdit` / `FocusScope`
/// instances survive across re-renders. Replacing the whole
/// `ModelRc` (the previous behaviour) rebuilt every child, which
/// stole focus from an Input mid-typing on every keystroke.
pub(super) fn sync_view_blocks_model(window: &QueryWindow, blocks: Vec<ViewBlock>) {
    // One persistent model per Slint thread (which is also the only
    // thread that calls this).
    thread_local! {
        static MODEL: RefCell<Option<Rc<VecModel<ViewBlock>>>> = const { RefCell::new(None) };
    }

    let model = MODEL.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if let Some(existing) = borrow.as_ref() {
            return Rc::clone(existing);
        }
        let fresh = Rc::new(VecModel::<ViewBlock>::default());
        window.set_view_blocks(ModelRc::from(Rc::clone(&fresh)));
        *borrow = Some(Rc::clone(&fresh));
        fresh
    });

    let new_len = blocks.len();
    let cur_len = model.row_count();
    let common = cur_len.min(new_len);

    for (i, block) in blocks.iter().take(common).enumerate() {
        model.set_row_data(i, block.clone());
    }
    for block in blocks.into_iter().skip(common) {
        model.push(block);
    }
    while model.row_count() > new_len {
        model.remove(model.row_count() - 1);
    }
}

/// Walk one tree node, appending the rendered block(s) it represents to
/// `out`. `stack` containers inline their children; anything else
/// becomes a single [`ViewBlock`] entry. Interactive blocks (button,
/// input) pull their callback ids out of the `__callbackId`
/// placeholders the SDK render walker left in place of `on*`
/// closures.
fn flatten_block(value: &serde_json::Value, out: &mut Vec<ViewBlock>) {
    let Some(obj) = value.as_object() else { return };
    let kind = obj.get("kind").and_then(serde_json::Value::as_str).unwrap_or("");

    if kind == "stack" {
        if let Some(children) = obj.get("children").and_then(serde_json::Value::as_array) {
            for child in children {
                flatten_block(child, out);
            }
        }
        return;
    }
    let read_str = |key: &str| -> String {
        obj.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };

    // For Button + Row the visible "text" is what the SDK calls
    // `label` — types.slint already documents text as doubling for
    // the button label, so route the JSON field accordingly here so
    // the Slint side only ever reads `block.text` for visible
    // rendering.
    let text = if kind == "button" || kind == "row" {
        read_str("label")
    } else {
        read_str("text")
    };
    let tone = read_str("tone");
    let size = read_str("size");
    let value_text = read_str("value");
    let id = read_str("id");
    // `label` doubles as the Input/TextArea `placeholder` since the
    // two are mutually exclusive per block kind. Spinner/Progress
    // keep their own `label` JSON field. Button/Row already
    // consumed `label` above into `text`, so they leave this empty.
    let label = if kind == "input" || kind == "textarea" {
        read_str("placeholder")
    } else if kind == "spinner" || kind == "progress" {
        read_str("label")
    } else {
        String::new()
    };

    let on_click_id = read_callback_id(obj.get("onClick"));
    let on_change_id = read_callback_id(obj.get("onChange"));
    let on_submit_id = read_callback_id(obj.get("onSubmit"));

    // ProgressBar values live in [0, 1]; truncation in this range
    // round-trips exactly. `-1.0` is the indeterminate sentinel.
    let progress_value = if kind == "progress" {
        obj.get("value")
            .and_then(serde_json::Value::as_f64)
            .map_or(-1.0_f32, |v| {
                #[allow(clippy::cast_possible_truncation)]
                {
                    v.clamp(-1.0, 1.0) as f32
                }
            })
    } else {
        -1.0_f32
    };

    out.push(ViewBlock {
        kind: kind.into(),
        has_text: !text.is_empty(),
        text: text.into(),
        tone: tone.into(),
        size: size.into(),
        progress_value,
        has_label: !label.is_empty(),
        label: label.into(),
        has_value: !value_text.is_empty(),
        value: value_text.into(),
        id: id.into(),
        // Slint structs only carry `int` (i32). Callback ids are
        // minted from a per-view counter that starts at 1 — reaching
        // i32::MAX would require ~2B renders against the same frame.
        // Saturating to 0 (the no-handler sentinel) would silently
        // drop a handler, so flag the overflow in debug builds.
        on_click_id: clamp_callback_id(on_click_id),
        on_change_id: clamp_callback_id(on_change_id),
        on_submit_id: clamp_callback_id(on_submit_id),
        has_on_click: on_click_id > 0,
        has_on_change: on_change_id > 0,
        has_on_submit: on_submit_id > 0,
        has_progress_value: kind == "progress" && obj.contains_key("value"),
    });
}

/// Extract a callback id from a value the SDK render walker may have
/// substituted (`{ "__callbackId": N }`). Returns `0` when absent —
/// callers compare against `0` as the "no handler" sentinel.
fn read_callback_id(value: Option<&serde_json::Value>) -> u64 {
    value
        .and_then(serde_json::Value::as_object)
        .and_then(|o| o.get("__callbackId"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

/// Narrow a SDK-minted callback id to the `i32` Slint structs use.
/// In practice ids stay well under `i32::MAX` (one render mints a
/// handful; a view lives for one launcher session) — overflow would
/// silently drop a handler, so the `debug_assert!` catches it during
/// development.
fn clamp_callback_id(id: u64) -> i32 {
    if let Ok(v) = i32::try_from(id) {
        v
    } else {
        debug_assert!(false, "callback id {id} exceeds i32::MAX");
        0
    }
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
    weak: &slint::Weak<QueryWindow>,
) {
    let action: Action = match serde_json::from_str(action_json) {
        Ok(a) => a,
        Err(err) => {
            tracing::error!(%plugin, %err, %action_json, "views: dispatch parse failed");
            return;
        }
    };

    // Must be rejected before execute() — Quit's effect (process::exit)
    // happens inside execute, so the outcome-level guards below come too
    // late for it.
    if let Some(kind) = action.host_only_kind() {
        tracing::warn!(%plugin, kind, "views: dispatch of host-only action ignored");
        return;
    }
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
            // A JS view dispatching closeView can only target itself —
            // host views aren't reachable from JS, so skip the host-view
            // precedence check the Esc-keypath does.
            pop_js_view_frame(view_stack, host_tx, weak);
        }
        actions::ActionOutcome::OpenSettingsView => {
            tracing::warn!(%plugin, "views: dispatch of openSettings ignored — would tear down the view");
        }
        actions::ActionOutcome::HostTask(_) | actions::ActionOutcome::ShowUpdateView => {
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
    weak: &slint::Weak<QueryWindow>,
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
    let depth = stack.depth();
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

    if depth == 0
        && let Some(window) = weak.upgrade()
    {
        // Drain the persistent view-blocks model so the next view
        // session starts with fresh Slint elements — otherwise a
        // surviving LineEdit's internal text carries over from the
        // previous session even though JS-side state is fresh.
        sync_view_blocks_model(&window, Vec::new());
        window.invoke_show_query();
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

/// Pop the topmost frame off the shared stack, dispatch
/// [`HostMessage::ViewClose`] so the plugin's JS runtime runs
/// `unmounted`, and — if that was the last frame — switch the window
/// back to the launcher view. A no-op on an empty stack (and not an
/// error — a stale `closeView` after the user already Esc'd out is a
/// benign race).
///
/// A live host view takes precedence: closing it fires its cancel token
/// (aborting whatever Rust task drives it) and routes back to the
/// launcher without touching the JS stack.
fn pop_view_frame(
    host_view: &HostView,
    view_stack: &Arc<Mutex<ViewStack>>,
    host_tx: &mpsc::UnboundedSender<HostMessage>,
    weak: &slint::Weak<QueryWindow>,
) {
    if host_view_mod::take_and_cancel(host_view) {
        if let Some(window) = weak.upgrade() {
            sync_view_blocks_model(&window, Vec::new());
            window.invoke_show_query();
        }
        return;
    }
    pop_js_view_frame(view_stack, host_tx, weak);
}

/// JS-stack pop only — host views are out of scope. Used by
/// `handle_view_dispatch` since a JS view can't legitimately close a
/// host view, and by the host-aware [`pop_view_frame`] after it has
/// verified there's no host view to take.
fn pop_js_view_frame(
    view_stack: &Arc<Mutex<ViewStack>>,
    host_tx: &mpsc::UnboundedSender<HostMessage>,
    weak: &slint::Weak<QueryWindow>,
) {
    let Ok(mut stack) = view_stack.lock() else {
        tracing::error!("views: stack lock poisoned; pop dropped");
        return;
    };

    let Some(frame) = stack.pop() else {
        tracing::debug!("views: pop on empty stack");
        return;
    };
    let depth = stack.depth();
    tracing::info!(
        plugin = %frame.plugin_name,
        handle = frame.handle,
        depth,
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

    if depth == 0
        && let Some(window) = weak.upgrade()
    {
        // Drain the persistent view-blocks model so the next view
        // session starts with fresh Slint elements — otherwise a
        // surviving LineEdit's internal text carries over from the
        // previous session even though JS-side state is fresh.
        sync_view_blocks_model(&window, Vec::new());
        window.invoke_show_query();
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
