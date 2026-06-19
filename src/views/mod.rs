//! View stack — host-side state for the dynamic-view system.
//!
//! Each frame represents a pushed view from a plugin. The handle is opaque
//! to the host: the SDK's view registry on the producing plugin's
//! `globalThis` mints it on `showView()` (`__highbeam_view_registry`)
//! and looks it up again when the host asks for a render. The frame's
//! `cancel` token aborts when the frame is popped so any in-flight
//! `mounted`-driven I/O bails on its own.
//!
//! Stage 2 ships only the data structure + push/pop semantics. The
//! plugin-runtime protocol (init / render / event / unmount messages)
//! and the Slint rendering pipeline land in subsequent stages.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Maximum number of frames a stack will hold. Pushing past this without
/// `reset = true` is rejected with [`PushError::AtCap`] so a pathological
/// plugin can't grow the stack without bound.
pub const STACK_CAP: usize = 16;

#[derive(Debug)]
pub struct ViewFrame {
    /// Producing plugin name. Used to route render/event messages back to
    /// the right `QuickJS` context and to fire targeted teardown on plugin
    /// reload.
    pub plugin_name: String,
    /// SDK-minted handle, unique within `plugin_name`'s context.
    pub handle: u64,
    /// Props passed to `setup(props)`. JSON for now — closure props are
    /// substituted with callback ids by the reactivity runtime in a later
    /// stage, then live elsewhere in the protocol.
    pub props: Value,
    /// Aborts when the frame is popped. Plugin code that does I/O during
    /// `mounted` or method calls propagates this signal via the existing
    /// `AbortSignal` plumbing.
    pub cancel: CancellationToken,
}

impl ViewFrame {
    #[must_use]
    pub fn new(plugin_name: String, handle: u64, props: Value) -> Self {
        Self {
            plugin_name,
            handle,
            props,
            cancel: CancellationToken::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct ViewStack {
    frames: Vec<ViewFrame>,
    cap: usize,
}

/// Outcome of attempting to push a frame past the depth cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError {
    /// Stack is at [`STACK_CAP`] and the push was not `reset`. The caller
    /// logs an ERROR and drops the action; the user sees no frame change.
    AtCap,
}

impl ViewStack {
    /// Empty stack with the default cap of [`STACK_CAP`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            frames: Vec::new(),
            cap: STACK_CAP,
        }
    }

    /// Push a frame on top of the stack. `reset = true` clears every
    /// existing frame (firing their cancel tokens) first so the new frame
    /// becomes the only frame.
    ///
    /// # Errors
    ///
    /// Returns [`PushError::AtCap`] when the stack is at cap and `reset`
    /// is `false`.
    pub fn push(&mut self, frame: ViewFrame, reset: bool) -> Result<(), PushError> {
        if reset {
            self.clear();
        }

        if self.frames.len() >= self.cap {
            return Err(PushError::AtCap);
        }
        self.frames.push(frame);
        Ok(())
    }

    /// Pop the top frame, firing its cancel token. Returns `None` when
    /// the stack was empty.
    pub fn pop(&mut self) -> Option<ViewFrame> {
        let frame = self.frames.pop()?;
        frame.cancel.cancel();
        Some(frame)
    }

    /// Pop every frame in top-down order, firing each cancel token. The
    /// returned vector is in pop order (topmost first) so the caller can
    /// run `unmounted` hooks before signals propagate further.
    pub fn clear(&mut self) -> Vec<ViewFrame> {
        let mut popped = Vec::with_capacity(self.frames.len());

        while let Some(frame) = self.pop() {
            popped.push(frame);
        }
        popped
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    #[must_use]
    pub fn top(&self) -> Option<&ViewFrame> {
        self.frames.last()
    }

    /// Whether `(plugin, handle)` identifies the visible (top) frame.
    #[must_use]
    pub fn is_top(&self, plugin: &str, handle: u64) -> bool {
        self.top()
            .is_some_and(|top| top.plugin_name == plugin && top.handle == handle)
    }

    /// Iterate every frame bottom-up.
    pub fn iter(&self) -> std::slice::Iter<'_, ViewFrame> {
        self.frames.iter()
    }
}

impl<'a> IntoIterator for &'a ViewStack {
    type Item = &'a ViewFrame;
    type IntoIter = std::slice::Iter<'a, ViewFrame>;

    fn into_iter(self) -> Self::IntoIter {
        self.frames.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(plugin: &str, handle: u64) -> ViewFrame {
        ViewFrame::new(plugin.into(), handle, json!({}))
    }

    #[test]
    fn push_appends_to_top() {
        let mut stack = ViewStack::new();
        stack.push(frame("a", 1), false).expect("push 1");
        stack.push(frame("a", 2), false).expect("push 2");

        assert_eq!(stack.depth(), 2);
        assert_eq!(stack.top().unwrap().handle, 2);
    }

    #[test]
    fn pop_returns_top_and_fires_cancel() {
        let mut stack = ViewStack::new();
        stack.push(frame("a", 1), false).expect("push");
        let cancel = stack.top().unwrap().cancel.clone();

        let popped = stack.pop().expect("pop returns frame");
        assert_eq!(popped.handle, 1);
        assert!(cancel.is_cancelled());
        assert_eq!(stack.depth(), 0);
    }

    #[test]
    fn pop_on_empty_returns_none() {
        let mut stack = ViewStack::new();
        assert!(stack.pop().is_none());
    }

    #[test]
    fn reset_push_clears_existing_frames_and_fires_cancels() {
        let mut stack = ViewStack::new();
        stack.push(frame("a", 1), false).expect("push 1");
        stack.push(frame("a", 2), false).expect("push 2");
        let first_cancel = stack.iter().next().unwrap().cancel.clone();
        let second_cancel = stack.iter().nth(1).unwrap().cancel.clone();

        stack.push(frame("b", 9), true).expect("reset push");

        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.top().unwrap().handle, 9);
        assert!(first_cancel.is_cancelled());
        assert!(second_cancel.is_cancelled());
    }

    #[test]
    fn push_past_cap_returns_at_cap_error() {
        let mut stack = ViewStack::new();

        for handle in 1..=u64::try_from(STACK_CAP).unwrap() {
            stack.push(frame("a", handle), false).expect("push within cap");
        }
        let err = stack.push(frame("a", 99), false).expect_err("at cap");

        assert_eq!(err, PushError::AtCap);
        assert_eq!(stack.depth(), STACK_CAP);
    }

    #[test]
    fn reset_push_at_cap_succeeds_because_the_stack_clears_first() {
        let mut stack = ViewStack::new();

        for handle in 1..=u64::try_from(STACK_CAP).unwrap() {
            stack.push(frame("a", handle), false).expect("push within cap");
        }
        stack.push(frame("b", 99), true).expect("reset push at cap");

        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.top().unwrap().handle, 99);
    }

    #[test]
    fn clear_returns_frames_in_top_down_order() {
        let mut stack = ViewStack::new();
        stack.push(frame("a", 1), false).expect("push 1");
        stack.push(frame("a", 2), false).expect("push 2");
        stack.push(frame("b", 3), false).expect("push 3");

        let cleared = stack.clear();

        let handles: Vec<u64> = cleared.iter().map(|f| f.handle).collect();
        assert_eq!(handles, vec![3, 2, 1]);
        assert_eq!(stack.depth(), 0);
    }
}
