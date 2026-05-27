//! OS appearance detection for the theme auto-switch.
//!
//! Wraps `dark-light`, whose 2.0 release dropped its `subscribe()` helper —
//! so [`Watcher::start`] polls every [`POLL_INTERVAL`] and fires on change.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use dark_light::Mode;

use crate::logging::LogErr;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Resolved OS appearance. `Unspecified` is kept distinct (not collapsed to
/// `Light`) so [`crate::theme::Theme::variant_for`] owns the fallback policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
    /// OS reported no preference (e.g. Linux without an
    /// `org.freedesktop.portal.Settings` responder).
    Unspecified,
}

impl From<Mode> for Appearance {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Dark => Self::Dark,
            Mode::Light => Self::Light,
            Mode::Unspecified => Self::Unspecified,
        }
    }
}

/// One-shot read of the current OS appearance. A failed probe degrades to
/// `Unspecified` rather than erroring — startup must not hinge on it.
#[must_use]
pub fn current() -> Appearance {
    dark_light::detect()
        .log_warn("os-appearance: detect failed; reporting Unspecified")
        .map_or(Appearance::Unspecified, Into::into)
}

/// Live RAII handle for the appearance-polling thread: [`Watcher::start`]
/// spawns it, dropping the `Watcher` stops it (within one [`POLL_INTERVAL`]).
pub struct Watcher {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Watcher {
    /// Start the watcher. `callback` fires only on transitions (the caller
    /// already has the initial value from [`current`]) and runs on the
    /// watcher thread, hence `Send + 'static`.
    ///
    /// A failed thread spawn degrades to an inert handle (no thread, no
    /// callbacks) plus a warning — a single failing subsystem must not block
    /// the daemon; only live auto-switch is lost.
    #[must_use = "dropping the Watcher stops the thread, ending auto-switch"]
    pub fn start<F>(callback: F) -> Self
    where
        F: Fn(Appearance) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let initial = current();

        let handle = thread::Builder::new()
            .name("highbeam-os-appearance".into())
            .spawn(move || Self::run(&stop_thread, initial, callback))
            .log_warn("os-appearance: spawn failed; auto-switch disabled this session");

        // No thread to signal — pre-stop keeps Drop idempotent.
        if handle.is_none() {
            stop.store(true, Ordering::Relaxed);
        }

        Self { stop, handle }
    }

    fn run<F>(stop: &Arc<AtomicBool>, initial: Appearance, callback: F)
    where
        F: Fn(Appearance) + Send + 'static,
    {
        let mut last = initial;
        // Sleep last: Drop sets `stop` before the next wake, so one check
        // per iteration suffices.
        while !stop.load(Ordering::Relaxed) {
            let next = current();

            if next != last {
                last = next;
                callback(next);
            }

            thread::sleep(POLL_INTERVAL);
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Detached, not joined: joining would block up to POLL_INTERVAL on
        // shutdown for a thread the OS reaps at exit anyway.
        drop(self.handle.take());
    }
}

// No unit tests: `From<Mode>` is one arm per variant (exhaustiveness already
// guards it), and `current` / `Watcher` only do platform IPC + a real
// thread. Behaviour is covered via theme.rs / settings.rs.
