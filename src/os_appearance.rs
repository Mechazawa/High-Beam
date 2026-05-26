//! OS appearance detection for the theme auto-switch.
//!
//! Wraps `dark-light` and adds a polling watcher. `dark-light` 2.0
//! dropped its upstream `subscribe()` helper, so [`Watcher::start`]
//! spawns a background thread that re-detects every [`POLL_INTERVAL`] and
//! fires the callback only on transitions. A 2 s ceiling on toggle-to-repaint
//! latency is well below what a user can perceive for a theme flip; the
//! cost is one extra wakeup every 2 s while the launcher daemon is
//! running.
//!
//! `dark-light` can return `Mode::Unspecified` when the platform can't
//! tell (older Linux desktops without the portal, headless tests). We
//! surface that as [`Appearance::Unspecified`] so the caller can apply a
//! user-controlled fallback policy in [`crate::theme::Theme::variant_for`]
//! instead of silently picking light.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use dark_light::Mode;

/// Poll interval for the background watcher. Short enough that a theme
/// flip feels instant, long enough that the wakeup cost is negligible.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Resolved OS appearance.
///
/// Three-valued: `Unspecified` is surfaced (not collapsed) so the theme
/// layer can pick a fallback that respects the user's preference rather
/// than silently defaulting to light at this boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
    /// The OS didn't report a preference (e.g. Linux desktop without an
    /// `org.freedesktop.portal.Settings` responder). Theme resolution
    /// falls back to its documented policy in
    /// [`crate::theme::Theme::variant_for`].
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

/// One-shot read of the current OS appearance. Errors from the platform
/// query degrade to [`Appearance::Unspecified`] with a warning — the
/// daemon must never refuse to start because the probe failed, and the
/// caller already has a fallback for the unspecified case.
#[must_use]
pub fn current() -> Appearance {
    match dark_light::detect() {
        Ok(mode) => mode.into(),
        Err(err) => {
            tracing::warn!(%err, "os-appearance: detect failed; reporting Unspecified");
            Appearance::Unspecified
        }
    }
}

/// Background watcher for OS appearance changes. A live RAII handle:
/// [`Watcher::start`] spawns the polling thread, and dropping the
/// returned `Watcher` stops it. The thread sees the stop flag on its next
/// loop iteration, so shutdown is bounded by [`POLL_INTERVAL`] — no
/// `join()` is awaited; the OS reaps the thread when the process exits.
pub struct Watcher {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Watcher {
    /// Start watching for OS appearance changes. The callback fires only
    /// on transitions (not on the initial value — the caller already has
    /// that from [`current`]), runs on the watcher thread, and must
    /// therefore be `Send + 'static`.
    ///
    /// The watcher polls every [`POLL_INTERVAL`]; OS-level notification
    /// APIs vary too much across platforms to wrap cheaply, and a 2 s
    /// ceiling on repaint latency is fine for a theme flip.
    ///
    /// Thread-spawn failure (resource exhaustion, `EAGAIN` from `clone`,
    /// container `RLIMIT_NPROC`) degrades gracefully: the returned handle
    /// is inert (no thread, callback never fires) and a warning is
    /// logged. Matches the daemon-wide doctrine that a single failing
    /// subsystem must not refuse the launcher's startup — the variant
    /// `daemon::run` already painted at startup remains in place; only
    /// live auto-switch is lost.
    #[must_use = "dropping the returned Watcher stops the thread — discarding it terminates auto-switch immediately"]
    pub fn start<F>(callback: F) -> Self
    where
        F: Fn(Appearance) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let initial = current();

        let handle = match thread::Builder::new()
            .name("highbeam-os-appearance".into())
            .spawn(move || Self::run(&stop_thread, initial, callback))
        {
            Ok(handle) => Some(handle),
            Err(err) => {
                tracing::warn!(%err, "os-appearance: could not spawn watcher; auto-switch disabled this session");
                // Pre-stop so a future `Drop` is idempotent — there's
                // nothing to signal, but the invariant "stop is true
                // after Drop" still holds.
                stop.store(true, Ordering::Relaxed);
                None
            }
        };

        Self { stop, handle }
    }

    fn run<F>(stop: &Arc<AtomicBool>, initial: Appearance, callback: F)
    where
        F: Fn(Appearance) + Send + 'static,
    {
        let mut last = initial;
        // Sleep at the end of the loop body — checking `stop` once per
        // iteration is enough because `Drop` sets it before the next poll
        // wakes up. Net iteration shape: check → poll → fire? → sleep.
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
        // Detach: joining would block up to POLL_INTERVAL on daemon
        // shutdown. The thread is well-behaved and the process is about
        // to exit anyway.
        drop(self.handle.take());
    }
}

// No unit tests here on purpose. The `From<Mode>` impl is one match arm
// per variant — exhaustiveness checks catch deletions at compile time, so
// per-arm asserts are tautologies. `current()` and `subscribe()` are thin
// wrappers around `dark_light` + `thread::spawn` whose only observable
// behaviour involves real platform IPC and a real OS thread; testing
// either in isolation forces hardware-dependent fixtures or pays for a
// detached watcher thread + DBus probe per test run. The integration
// surface (theme variant resolution, settings round-trip) is exercised
// from `theme.rs` and `settings.rs` instead.
