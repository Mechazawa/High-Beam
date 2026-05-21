//! Pure state machine for in-session query-history cycling.
//!
//! No I/O, no Slint types — everything here can be unit-tested without a
//! running window.
//!
//! The cycle model:
//! ```text
//! oldest ← entries[0] … entries[n-1] ← (draft slot)  ← newest
//! ```
//! `cursor = None`  means the user is on the draft slot (the live input).
//! `cursor = Some(i)` means they've cycled back to `entries[i]`.

/// In-session history cycling state. Kept on the Slint event-loop thread;
/// no `Send` requirement.
#[derive(Debug, Default)]
pub(crate) struct QueryHistoryState {
    /// Loaded from the DB at startup / after each submit. Oldest first.
    entries: Vec<String>,
    /// The input text that was live when the user first pressed up. Restored
    /// when down is pressed past the newest entry.
    draft: Option<String>,
    /// Index into `entries` while cycling; `None` means "on the draft slot".
    cursor: Option<usize>,
}

/// What the caller should do with the input field after a state transition.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InputAction {
    /// Leave the input text unchanged.
    NoChange,
    /// Replace the input text with the contained string.
    SetTo(String),
}

impl QueryHistoryState {
    /// Create with a pre-loaded history. `entries` must be in chronological
    /// order (oldest first), matching `QueryHistoryDb::load_recent`.
    #[must_use]
    pub(crate) fn new(entries: Vec<String>) -> Self {
        Self {
            entries,
            draft: None,
            cursor: None,
        }
    }

    /// Handle Cmd+Up (history-back).
    ///
    /// - On the draft slot with a non-empty history: save the current input as
    ///   the draft and jump to the most-recent entry.
    /// - Already cycling and not at the oldest: step one entry older.
    /// - Already at the oldest: no-op (clamped).
    pub(crate) fn history_up(&mut self, current_input: &str) -> InputAction {
        match self.cursor {
            None => {
                if self.entries.is_empty() {
                    return InputAction::NoChange;
                }
                self.draft = Some(current_input.to_owned());
                let idx = self.entries.len() - 1;
                self.cursor = Some(idx);
                InputAction::SetTo(self.entries[idx].clone())
            }
            Some(0) => InputAction::NoChange,
            Some(idx) => {
                let new_idx = idx - 1;
                self.cursor = Some(new_idx);
                InputAction::SetTo(self.entries[new_idx].clone())
            }
        }
    }

    /// Handle Cmd+Down (history-forward).
    ///
    /// - On the draft slot: no-op (already at the newest position).
    /// - Cycling and not at the newest entry: step one entry newer.
    /// - At the newest entry: restore the draft, clear cursor.
    pub(crate) fn history_down(&mut self) -> InputAction {
        match self.cursor {
            None => InputAction::NoChange,
            Some(idx) if idx + 1 < self.entries.len() => {
                let new_idx = idx + 1;
                self.cursor = Some(new_idx);
                InputAction::SetTo(self.entries[new_idx].clone())
            }
            Some(_) => {
                // At the newest entry — return to draft slot.
                let draft = self.draft.take().unwrap_or_default();
                self.cursor = None;
                InputAction::SetTo(draft)
            }
        }
    }

    /// Notify the state machine that the user has edited the input text.
    /// If cycling, this abandons the cycle and makes the edited text the new
    /// draft (cursor cleared). The original history entries are never mutated.
    pub(crate) fn mark_edited(&mut self) {
        if self.cursor.is_some() {
            self.cursor = None;
            self.draft = None;
        }
    }

    /// Record a submitted query. Updates the in-memory entries list so that
    /// subsequent cycling includes the new entry. Deduplication against the
    /// last entry is handled in the DB layer; here we just append (the DB
    /// push is the authority).
    ///
    /// Resets cursor and draft — the user is back on a fresh input.
    pub(crate) fn on_submit(&mut self, query: &str, new_entries: Vec<String>) {
        self.entries = new_entries;
        // If the submitted text happens to equal the last entry (DB dedup
        // fired), don't double-append in memory — the reload covers it.
        let last = self.entries.last().map(String::as_str);
        if last != Some(query) && !query.is_empty() {
            self.entries.push(query.to_owned());
        }
        self.cursor = None;
        self.draft = None;
    }

    /// Whether the user is currently viewing a history entry (not the draft).
    #[cfg(test)]
    pub(crate) fn is_cycling(&self) -> bool {
        self.cursor.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(entries: &[&str]) -> QueryHistoryState {
        QueryHistoryState::new(entries.iter().map(|s| (*s).to_owned()).collect())
    }

    // ---- history_up --------------------------------------------------------

    #[test]
    fn up_with_empty_history_is_noop() {
        let mut s = QueryHistoryState::default();
        let action = s.history_up("draft text");
        assert_eq!(action, InputAction::NoChange);
        assert!(!s.is_cycling());
    }

    #[test]
    fn up_from_draft_saves_draft_and_jumps_to_newest() {
        let mut s = state_with(&["a", "b", "c"]);
        let action = s.history_up("my draft");
        assert_eq!(action, InputAction::SetTo("c".into()));
        assert!(s.is_cycling());
        assert_eq!(s.draft, Some("my draft".into()));
    }

    #[test]
    fn up_from_newest_moves_to_older() {
        let mut s = state_with(&["old", "new"]);
        s.history_up("draft");
        let action = s.history_up("new"); // `current_input` ignored while cycling
        assert_eq!(action, InputAction::SetTo("old".into()));
    }

    #[test]
    fn up_at_oldest_is_clamped_noop() {
        let mut s = state_with(&["only"]);
        s.history_up("draft");
        let action = s.history_up("only");
        assert_eq!(action, InputAction::NoChange);
        assert!(s.is_cycling());
    }

    // ---- history_down ------------------------------------------------------

    #[test]
    fn down_on_draft_slot_is_noop() {
        let mut s = QueryHistoryState::default();
        let action = s.history_down();
        assert_eq!(action, InputAction::NoChange);
    }

    #[test]
    fn down_from_older_entry_moves_toward_newest() {
        let mut s = state_with(&["a", "b", "c"]);
        s.history_up("draft"); // cursor → 2 ("c")
        s.history_up("c"); // cursor → 1 ("b")
        s.history_up("b"); // cursor → 0 ("a")
        let action = s.history_down(); // cursor → 1 ("b")
        assert_eq!(action, InputAction::SetTo("b".into()));
    }

    #[test]
    fn down_past_newest_restores_draft_and_clears_cursor() {
        let mut s = state_with(&["a", "b"]);
        s.history_up("my draft"); // cursor → 1 ("b"), draft = "my draft"
        let action = s.history_down(); // cursor → None, draft restored
        assert_eq!(action, InputAction::SetTo("my draft".into()));
        assert!(!s.is_cycling());
        assert_eq!(s.draft, None);
    }

    // ---- round-trip: the user-requested must-test -------------------------

    #[test]
    fn up_then_down_restores_original_draft() {
        let mut s = state_with(&["old query"]);
        let action_up = s.history_up("partial draft");
        assert_eq!(action_up, InputAction::SetTo("old query".into()));
        let action_down = s.history_down();
        assert_eq!(action_down, InputAction::SetTo("partial draft".into()));
        assert!(!s.is_cycling());
    }

    #[test]
    fn up_multiple_then_down_all_the_way_restores_draft() {
        let mut s = state_with(&["q1", "q2", "q3"]);
        s.history_up("draft");
        s.history_up("q3");
        s.history_up("q2");
        // Now at q1 (oldest). Walk all the way back to draft.
        s.history_down();
        s.history_down();
        let action = s.history_down();
        assert_eq!(action, InputAction::SetTo("draft".into()));
        assert!(!s.is_cycling());
    }

    // ---- mark_edited -------------------------------------------------------

    #[test]
    fn editing_while_cycling_clears_cursor() {
        let mut s = state_with(&["entry"]);
        s.history_up("draft");
        assert!(s.is_cycling());
        s.mark_edited();
        assert!(!s.is_cycling());
    }

    #[test]
    fn editing_on_draft_slot_is_noop() {
        let mut s = state_with(&["entry"]);
        s.mark_edited(); // no cursor — should not panic
        assert!(!s.is_cycling());
    }

    // ---- on_submit ---------------------------------------------------------

    #[test]
    fn submit_resets_cursor_and_draft() {
        let mut s = state_with(&["old"]);
        s.history_up("draft");
        assert!(s.is_cycling());
        s.on_submit("new query", vec!["old".into(), "new query".into()]);
        assert!(!s.is_cycling());
        assert_eq!(s.draft, None);
    }

    #[test]
    fn submit_appends_to_entries_when_not_already_last() {
        let mut s = state_with(&["a"]);
        s.on_submit("b", vec!["a".into()]);
        // "b" wasn't in the reload slice (simulating what DB returns before
        // the flush lands), so on_submit appends it in-memory.
        let action = s.history_up("");
        assert_eq!(action, InputAction::SetTo("b".into()));
    }

    #[test]
    fn submit_does_not_double_append_when_reload_already_includes_entry() {
        let mut s = QueryHistoryState::default();
        // DB already returned the entry in the reload slice.
        s.on_submit("same", vec!["same".into()]);
        assert_eq!(s.entries, vec!["same"]);
    }

    #[test]
    fn submit_skips_empty_query() {
        let mut s = QueryHistoryState::default();
        s.on_submit("", vec![]);
        assert!(s.entries.is_empty());
    }
}
