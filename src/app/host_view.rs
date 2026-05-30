//! Host-driven view-frame painting.
//!
//! The view-frame system in `src/sdk/view.rs` is plumbed for JS plugins:
//! `setup → render → mounted` runs inside a `QuickJS` context and the
//! rendered tree arrives at the host through a `RuntimeBridge` callback.
//! Some flows — currently the Core built-in's `update` verb — need the
//! same Slint surface but originate in Rust, where there's no plugin
//! context to drive a render loop. This module owns those flows.
//!
//! Shape: [`HostView`] holds a single optional state value. While `Some`,
//! the host view is "live" — it owns `current-view = VIEW-VIEWS` and any
//! Esc / hide event closes it (firing its `cancel` token so the Rust task
//! producing updates bails) instead of falling through to the JS plugin
//! view stack.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::callbacks::sync_view_blocks_model;
use crate::QueryWindow;
use crate::logging::LogErr;
use crate::ui::ViewBlock;

/// Shared slot. `None` when no host view is live.
pub(super) type HostView = Arc<Mutex<Option<UpdateViewState>>>;

/// Build a fresh empty slot for `AppState::start`.
pub(super) fn new_slot() -> HostView {
    Arc::new(Mutex::new(None))
}

/// Per-plugin update progress, painted as a single view-frame.
pub(super) struct UpdateViewState {
    pub entries: Vec<UpdateEntry>,
    pub summary: Option<UpdateSummary>,
    pub cancel: CancellationToken,
}

impl UpdateViewState {
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            summary: None,
            cancel: CancellationToken::new(),
        }
    }

    /// Index of the entry matching `name`, or `None` if absent.
    pub(super) fn position(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.name == name)
    }
}

/// Clear the slot, firing the captured cancel token. Returns `true` when
/// a view was actually live so callers can branch on "was anything
/// closed?" without re-locking. Lock-poisoning and an already-empty slot
/// both report `false`.
pub(super) fn take_and_cancel(slot: &HostView) -> bool {
    let Ok(mut guard) = slot.lock() else {
        tracing::error!("host_view: lock poisoned; take skipped");
        return false;
    };
    let Some(state) = guard.take() else { return false };

    state.cancel.cancel();
    true
}

/// One plugin's row in the update view.
pub(super) struct UpdateEntry {
    pub name: String,
    pub local_version: String,
    pub status: EntryStatus,
}

#[derive(Clone)]
pub(super) enum EntryStatus {
    /// Discovered but not yet checked.
    Queued,
    /// Hitting `manifestUrl`.
    Checking,
    /// Local == remote (or remote isn't newer).
    UpToDate,
    /// Local plugin has no `manifestUrl`, can't auto-update.
    Skipped { reason: String },
    /// Download + extract in flight.
    Updating { new_version: String },
    /// Install finished.
    Updated { new_version: String },
    /// Check or install failed.
    Failed { error: String },
}

/// End-of-run tallies.
pub(super) struct UpdateSummary {
    pub updated: usize,
    pub up_to_date: usize,
    pub skipped: usize,
    pub failed: usize,
}

/// Paint the host view's current state to the window. Called on the Slint
/// thread. No-op when the slot is empty (`take()` already cleared it for
/// a pending close), which keeps a stray repaint after Esc benign.
pub(super) fn paint(slot: &HostView, weak: &slint::Weak<QueryWindow>) {
    let Some(window) = weak.upgrade() else { return };
    let Ok(guard) = slot.lock() else {
        tracing::error!("host_view: slot lock poisoned; paint skipped");
        return;
    };
    let Some(state) = guard.as_ref() else { return };

    paint_update(&window, state);
}

/// Schedule a `paint` on the Slint event loop. Safe to call from the
/// runtime thread.
pub(super) fn schedule_paint(slot: &HostView, weak: &slint::Weak<QueryWindow>) {
    let slot = Arc::clone(slot);
    let weak = weak.clone();
    slint::invoke_from_event_loop(move || {
        paint(&slot, &weak);
    })
    .log_debug("host_view: schedule_paint invoke_from_event_loop");
}

fn paint_update(window: &QueryWindow, state: &UpdateViewState) {
    let total = state.entries.len();
    let done = state
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e.status,
                EntryStatus::UpToDate
                    | EntryStatus::Updated { .. }
                    | EntryStatus::Skipped { .. }
                    | EntryStatus::Failed { .. }
            )
        })
        .count();

    let mut blocks = Vec::with_capacity(2 + state.entries.len() * 2 + 1);

    let heading_text = if state.summary.is_some() {
        "Update complete".to_owned()
    } else if total == 0 {
        "Checking for plugins…".to_owned()
    } else {
        format!("Updating plugins — {done} of {total}")
    };

    blocks.push(text_block("heading", &heading_text, "", ""));

    let progress_value = if state.summary.is_some() {
        1.0
    } else if total == 0 {
        -1.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        {
            done as f64 / total as f64
        }
    };
    blocks.push(progress_block(progress_value));

    for entry in &state.entries {
        let (line, tone) = format_entry(entry);
        blocks.push(text_block("text", &line, tone, ""));

        if matches!(entry.status, EntryStatus::Updating { .. } | EntryStatus::Checking) {
            blocks.push(spinner_block(""));
        }
    }

    if let Some(summary) = state.summary.as_ref() {
        let parts: Vec<String> = [
            (summary.updated, "updated"),
            (summary.up_to_date, "up to date"),
            (summary.skipped, "skipped"),
            (summary.failed, "failed"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, label)| format!("{n} {label}"))
        .collect();

        let line = if parts.is_empty() {
            "Nothing to update".to_owned()
        } else {
            parts.join(", ")
        };
        let tone = if summary.failed > 0 { "error" } else { "success" };
        blocks.push(text_block("text", &line, tone, "lg"));
    }

    window.set_view_frame_title("Update plugins".into());
    window.set_view_has_title(true);
    // Reuse the same persistent VecModel JS view paints use — keeps the
    // Slint child elements alive across status mutations instead of
    // rebuilding every block on every paint.
    sync_view_blocks_model(window, blocks);
    window.invoke_show_view_frame();
}

fn format_entry(entry: &UpdateEntry) -> (String, &'static str) {
    let name = &entry.name;
    let local = if entry.local_version.is_empty() {
        String::new()
    } else {
        format!(" v{}", entry.local_version)
    };

    match &entry.status {
        EntryStatus::Queued => (format!("{name}{local} — queued"), "muted"),
        EntryStatus::Checking => (format!("{name}{local} — checking…"), "muted"),
        EntryStatus::UpToDate => (format!("{name}{local} — up to date"), "muted"),
        EntryStatus::Skipped { reason } => (format!("{name}{local} — skipped: {reason}"), "muted"),
        EntryStatus::Updating { new_version } => (format!("{name}{local} → v{new_version} — updating…"), "warning"),
        EntryStatus::Updated { new_version } => (format!("{name}{local} → v{new_version} — updated"), "success"),
        EntryStatus::Failed { error } => (format!("{name}{local} — failed: {error}"), "error"),
    }
}

/// Construct a heading/text block. `kind` is "heading" or "text".
fn text_block(kind: &'static str, text: &str, tone: &str, size: &str) -> ViewBlock {
    ViewBlock {
        kind: kind.into(),
        text: text.into(),
        has_text: !text.is_empty(),
        tone: tone.into(),
        size: size.into(),
        progress_value: -1.0,
        ..ViewBlock::default()
    }
}

fn spinner_block(label: &str) -> ViewBlock {
    ViewBlock {
        kind: "spinner".into(),
        label: label.into(),
        has_label: !label.is_empty(),
        progress_value: -1.0,
        ..ViewBlock::default()
    }
}

#[allow(clippy::cast_possible_truncation)]
fn progress_block(value: f64) -> ViewBlock {
    let has_value = value >= 0.0;
    ViewBlock {
        kind: "progress".into(),
        progress_value: if has_value { value.clamp(0.0, 1.0) as f32 } else { -1.0 },
        has_progress_value: has_value,
        ..ViewBlock::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, status: EntryStatus) -> UpdateEntry {
        UpdateEntry {
            name: name.into(),
            local_version: "1.0.0".into(),
            status,
        }
    }

    #[test]
    fn format_entry_uses_warning_tone_during_update() {
        let (_, tone) = format_entry(&entry(
            "foo",
            EntryStatus::Updating {
                new_version: "1.1.0".into(),
            },
        ));
        assert_eq!(tone, "warning");
    }

    #[test]
    fn format_entry_uses_error_tone_on_failure() {
        let (line, tone) = format_entry(&entry("foo", EntryStatus::Failed { error: "boom".into() }));
        assert_eq!(tone, "error");
        assert!(line.contains("boom"));
    }

    #[test]
    fn format_entry_uses_success_tone_after_update() {
        let (_, tone) = format_entry(&entry(
            "foo",
            EntryStatus::Updated {
                new_version: "1.1.0".into(),
            },
        ));
        assert_eq!(tone, "success");
    }

    #[test]
    fn position_returns_index_of_matching_entry() {
        let mut state = UpdateViewState::new();
        state.entries.push(entry("a", EntryStatus::Queued));
        state.entries.push(entry("b", EntryStatus::Queued));
        assert_eq!(state.position("b"), Some(1));
        assert_eq!(state.position("c"), None);
    }
}
