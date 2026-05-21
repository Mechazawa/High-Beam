//! Pure state machine for in-session query-history cycling.
//!
//! No I/O, no Slint types — everything here can be unit-tested without a
//! running window.
//!
//! Up enters preview mode only when the live input is empty; once in
//! preview, Up walks older and Down walks newer. Down past the newest
//! entry exits preview to an empty input (no draft to restore because the
//! empty-input precondition was what let us enter preview in the first
//! place). Any edit or submit also exits preview.

/// In-session history cycling state. Kept on the Slint event-loop thread;
/// no `Send` requirement.
#[derive(Debug, Default)]
pub(crate) struct QueryHistoryState {
    /// Loaded from the DB at startup / after each submit. Oldest first.
    entries: Vec<String>,
    /// Index into `entries` while previewing; `None` means "not previewing".
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
        Self { entries, cursor: None }
    }

    /// Handle Up. Only enters preview mode when the input is empty — once
    /// the user has typed anything, Up is reserved for result-list nav.
    /// Stepping older while already in preview always works.
    ///
    /// - Empty input + non-empty history + not yet cycling: enter preview at
    ///   the most-recent entry.
    /// - Already cycling and not at the oldest: step one entry older.
    /// - Otherwise: no-op (non-empty live input, oldest already, empty
    ///   history).
    pub(crate) fn history_up(&mut self, current_input: &str) -> InputAction {
        match self.cursor {
            None => {
                if self.entries.is_empty() || !current_input.is_empty() {
                    return InputAction::NoChange;
                }
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

    /// Handle Down. Only meaningful while in preview.
    ///
    /// - Cycling and not at the newest entry: step one entry newer.
    /// - At the newest entry: exit preview, clear input (the live draft was
    ///   empty — that's the gating precondition for entering preview).
    /// - Not cycling: no-op (regular Down navigates results elsewhere).
    pub(crate) fn history_down(&mut self) -> InputAction {
        match self.cursor {
            None => InputAction::NoChange,
            Some(idx) if idx + 1 < self.entries.len() => {
                let new_idx = idx + 1;
                self.cursor = Some(new_idx);
                InputAction::SetTo(self.entries[new_idx].clone())
            }
            Some(_) => {
                self.cursor = None;
                InputAction::SetTo(String::new())
            }
        }
    }

    /// Whether the user is currently viewing a history entry (a preview that
    /// hasn't been committed by editing or pressing Enter). The UI layer
    /// uses this to render the input with the muted-text colour.
    #[must_use]
    pub(crate) fn is_preview(&self) -> bool {
        self.cursor.is_some()
    }

    /// Notify the state machine that the user has edited the input text.
    /// If previewing, this commits — the cycle ends and the input becomes
    /// the new live text. The original history entries are never mutated.
    pub(crate) fn mark_edited(&mut self) {
        self.cursor = None;
    }

    /// Record a submitted query. Mirrors what the DB just did: dedup
    /// against the last entry, append, trim to `max_entries`. Resets the
    /// preview cursor.
    pub(crate) fn on_submit(&mut self, query: &str, max_entries: usize) {
        self.cursor = None;
        if query.is_empty() {
            return;
        }
        let last_matches = self.entries.last().map(String::as_str) == Some(query);
        if !last_matches {
            self.entries.push(query.to_owned());
        }
        let excess = self.entries.len().saturating_sub(max_entries);
        if excess > 0 {
            self.entries.drain(..excess);
        }
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
        let action = s.history_up("");
        assert_eq!(action, InputAction::NoChange);
        assert!(!s.is_preview());
    }

    #[test]
    fn up_with_non_empty_input_is_noop() {
        // The empty-input precondition is what reserves regular Up for
        // result-list navigation once the user has started typing.
        let mut s = state_with(&["a", "b"]);
        let action = s.history_up("typing");
        assert_eq!(action, InputAction::NoChange);
        assert!(!s.is_preview());
    }

    #[test]
    fn up_from_empty_enters_preview_at_newest() {
        let mut s = state_with(&["a", "b", "c"]);
        let action = s.history_up("");
        assert_eq!(action, InputAction::SetTo("c".into()));
        assert!(s.is_preview());
    }

    #[test]
    fn up_from_newest_moves_to_older() {
        let mut s = state_with(&["old", "new"]);
        s.history_up("");
        let action = s.history_up("new"); // current_input ignored once cycling
        assert_eq!(action, InputAction::SetTo("old".into()));
    }

    #[test]
    fn up_at_oldest_is_clamped_noop() {
        let mut s = state_with(&["only"]);
        s.history_up("");
        let action = s.history_up("only");
        assert_eq!(action, InputAction::NoChange);
        assert!(s.is_preview());
    }

    // ---- history_down ------------------------------------------------------

    #[test]
    fn down_when_not_previewing_is_noop() {
        let mut s = QueryHistoryState::default();
        let action = s.history_down();
        assert_eq!(action, InputAction::NoChange);
    }

    #[test]
    fn down_from_older_entry_moves_toward_newest() {
        let mut s = state_with(&["a", "b", "c"]);
        s.history_up(""); // cursor → 2 ("c")
        s.history_up("c"); // cursor → 1 ("b")
        s.history_up("b"); // cursor → 0 ("a")
        let action = s.history_down(); // cursor → 1 ("b")
        assert_eq!(action, InputAction::SetTo("b".into()));
    }

    #[test]
    fn down_past_newest_exits_preview_with_empty_input() {
        let mut s = state_with(&["a", "b"]);
        s.history_up(""); // cursor → 1 ("b")
        let action = s.history_down(); // exit preview
        assert_eq!(action, InputAction::SetTo(String::new()));
        assert!(!s.is_preview());
    }

    #[test]
    fn up_then_down_returns_to_empty_input() {
        let mut s = state_with(&["old query"]);
        let action_up = s.history_up("");
        assert_eq!(action_up, InputAction::SetTo("old query".into()));
        let action_down = s.history_down();
        assert_eq!(action_down, InputAction::SetTo(String::new()));
        assert!(!s.is_preview());
    }

    // ---- mark_edited -------------------------------------------------------

    #[test]
    fn editing_while_previewing_exits_preview() {
        let mut s = state_with(&["entry"]);
        s.history_up("");
        assert!(s.is_preview());
        s.mark_edited();
        assert!(!s.is_preview());
    }

    #[test]
    fn editing_when_not_previewing_is_noop() {
        let mut s = state_with(&["entry"]);
        s.mark_edited();
        assert!(!s.is_preview());
    }

    // ---- on_submit ---------------------------------------------------------

    #[test]
    fn submit_resets_cursor() {
        let mut s = state_with(&["old"]);
        s.history_up("");
        assert!(s.is_preview());
        s.on_submit("new query", 100);
        assert!(!s.is_preview());
    }

    #[test]
    fn submit_appends_to_entries() {
        let mut s = state_with(&["a"]);
        s.on_submit("b", 100);
        let action = s.history_up("");
        assert_eq!(action, InputAction::SetTo("b".into()));
    }

    #[test]
    fn submit_dedups_against_last_entry() {
        let mut s = state_with(&["same"]);
        s.on_submit("same", 100);
        assert_eq!(s.entries, vec!["same"]);
    }

    #[test]
    fn submit_skips_empty_query() {
        let mut s = QueryHistoryState::default();
        s.on_submit("", 100);
        assert!(s.entries.is_empty());
    }

    #[test]
    fn submit_trims_to_max_entries() {
        let mut s = state_with(&["a", "b", "c"]);
        s.on_submit("d", 3);
        assert_eq!(s.entries, vec!["b", "c", "d"]);
    }
}
