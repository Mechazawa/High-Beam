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
use crate::plugins::result::RankedResult;
use crate::query_history::{InputAction, QueryHistoryDb, QueryHistoryState};
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
                    push_view_frame(view_stack, &plugin_name, handle, props, reset);
                }
                actions::ActionOutcome::CloseView => {
                    pop_view_frame(view_stack);
                }
            }
        }
        Err(err) => {
            tracing::error!(plugin = %plugin_name, %err, "plugins: action failed");
            window::hide_and_persist_position(&w, settings);
        }
    }
}

/// Push a freshly-minted [`ViewFrame`] onto the shared stack. Stage 2 stops
/// at logging the push and does not yet drive the launcher window into
/// view-mode — Slint rendering arrives with the protocol + UI stages. A
/// rejected push (stack at cap) logs an ERROR so the action is observably
/// dropped instead of silently lost.
fn push_view_frame(
    view_stack: &Arc<Mutex<ViewStack>>,
    plugin_name: &str,
    handle: u64,
    props: serde_json::Value,
    reset: bool,
) {
    let Ok(mut stack) = view_stack.lock() else {
        tracing::error!("views: stack lock poisoned; push dropped");
        return;
    };
    let frame = ViewFrame::new(plugin_name.to_owned(), handle, props);

    match stack.push(frame, reset) {
        Ok(()) => tracing::info!(
            plugin = %plugin_name,
            handle,
            reset,
            depth = stack.depth(),
            "views: frame pushed",
        ),
        Err(PushError::AtCap) => tracing::error!(
            plugin = %plugin_name,
            handle,
            depth = stack.depth(),
            "views: push rejected at stack cap",
        ),
    }
}

/// Pop the topmost frame off the shared stack. A no-op on an empty stack
/// (and not an error — a stale `closeView` after the user already Esc'd
/// out is a benign race).
fn pop_view_frame(view_stack: &Arc<Mutex<ViewStack>>) {
    let Ok(mut stack) = view_stack.lock() else {
        tracing::error!("views: stack lock poisoned; pop dropped");
        return;
    };

    if let Some(frame) = stack.pop() {
        tracing::info!(
            plugin = %frame.plugin_name,
            handle = frame.handle,
            depth = stack.depth(),
            "views: frame popped",
        );
    } else {
        tracing::debug!("views: pop on empty stack");
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
