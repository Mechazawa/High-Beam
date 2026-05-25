//! System appearance detection for the theme auto-switch.
//!
//! Wraps `dark-light` and adds a polling subscription. `dark-light` 2.0
//! dropped its upstream `subscribe()` helper, so [`subscribe`] spawns a
//! background thread that re-detects every [`POLL_INTERVAL`] and fires the
//! callback only on transitions. A 2 s ceiling on toggle-to-repaint
//! latency is well below what a user can perceive for a theme flip; the
//! cost is one extra wakeup every 2 s while the launcher daemon is
//! running.
//!
//! `dark-light` returns `Mode::Unspecified` when the platform can't tell
//! (older Linux desktops without the portal, headless tests). We collapse
//! that to [`SystemAppearance::Light`] — it matches the historic bundled
//! theme so users who upgrade don't see a surprise palette flip.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use dark_light::Mode;

/// Poll interval for the background watcher. Short enough that a theme
/// flip feels instant, long enough that the wakeup cost is negligible.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Resolved system appearance.
///
/// Two-valued because that's what the UI cares about — `Unspecified`
/// gets normalised to [`Self::Light`] at the boundary so the rest of the
/// app only ever sees `Dark` or `Light`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAppearance {
    Dark,
    Light,
}

impl From<Mode> for SystemAppearance {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Dark => Self::Dark,
            Mode::Light | Mode::Unspecified => Self::Light,
        }
    }
}

/// One-shot read of the current system appearance. Errors from the
/// platform query degrade to [`SystemAppearance::Light`] with a warning —
/// the daemon must never refuse to start because the dark-mode probe
/// failed.
#[must_use]
pub fn current() -> SystemAppearance {
    match dark_light::detect() {
        Ok(mode) => mode.into(),
        Err(err) => {
            tracing::warn!(%err, "dark-mode: detect failed; assuming light");
            SystemAppearance::Light
        }
    }
}

/// Subscribe to system appearance changes. The callback fires only on
/// transitions (not on the initial value — the caller already has that
/// from [`current`]), runs on the watcher thread, and must therefore be
/// `Send + 'static`. Drop the returned guard to stop the watcher.
///
/// The watcher polls every [`POLL_INTERVAL`]; OS-level notification APIs
/// vary too much across platforms to wrap cheaply, and a 2 s ceiling on
/// repaint latency is fine for a theme flip.
///
/// # Panics
///
/// Panics if the OS refuses to spawn the watcher thread (resource
/// exhaustion). The launcher needs the watcher to honour `theme_mode =
/// "auto"`, so failing fast here surfaces the problem at startup rather
/// than producing a launcher that silently never repaints.
pub fn subscribe<F>(callback: F) -> SubscriptionGuard
where
    F: Fn(SystemAppearance) + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let initial = current();

    let handle = thread::Builder::new()
        .name("highbeam-dark-mode".into())
        .spawn(move || run_watcher(&stop_thread, initial, callback))
        .expect("spawn dark-mode watcher thread");

    SubscriptionGuard {
        stop,
        handle: Some(handle),
    }
}

fn run_watcher<F>(stop: &Arc<AtomicBool>, initial: SystemAppearance, callback: F)
where
    F: Fn(SystemAppearance) + Send + 'static,
{
    let mut last = initial;
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(POLL_INTERVAL);

        if stop.load(Ordering::Relaxed) {
            break;
        }

        let next = current();

        if next != last {
            last = next;
            callback(next);
        }
    }
}

/// Drop-guard that stops the watcher thread on Drop. The thread sees the
/// flag on its next loop iteration, so shutdown is bounded by
/// [`POLL_INTERVAL`] — no `join()` is awaited; the OS reaps the thread
/// when the process exits.
pub struct SubscriptionGuard {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Detach: joining would block up to POLL_INTERVAL on daemon
        // shutdown. The thread is well-behaved and the process is about
        // to exit anyway.
        drop(self.handle.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unspecified_normalises_to_light() {
        assert_eq!(SystemAppearance::from(Mode::Unspecified), SystemAppearance::Light);
    }

    #[test]
    fn mode_round_trip() {
        assert_eq!(SystemAppearance::from(Mode::Dark), SystemAppearance::Dark);
        assert_eq!(SystemAppearance::from(Mode::Light), SystemAppearance::Light);
    }

    #[test]
    fn subscription_guard_drop_stops_watcher() {
        // Smoke test: dropping the guard sets the stop flag so the thread
        // exits on its next wakeup. We can't observe the thread directly
        // without joining (which we explicitly don't do), but the flag is
        // the only signal the thread reads and we can assert on that.
        let guard = subscribe(|_| {});
        let stop = Arc::clone(&guard.stop);
        assert!(!stop.load(Ordering::Relaxed));
        drop(guard);
        assert!(stop.load(Ordering::Relaxed));
    }
}
