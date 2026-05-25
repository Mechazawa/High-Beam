//! Host-side `tracing` subscriber bootstrap.
//!
//! Single fmt-formatted subscriber wired to stderr, filtered through
//! `RUST_LOG` (defaulting to `info` when unset). Plugin-side logs flow
//! through `crate::plugins::log::PluginLog` and are intentionally not
//! routed here.
//!
//! Also exposes [`LogErr`] — an extension trait that lets call sites
//! replace `let _ = some_result;` with `.log_warn("ctx")` or
//! `.log_debug("ctx")`, logging the `Err` side through tracing instead
//! of swallowing it silently.

use std::fmt::Display;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

/// Install the host stderr subscriber. Idempotent — `try_init` makes the
/// second call a no-op rather than a panic so tests that exercise
/// `daemon::run` paths don't blow up on re-init.
///
/// `cargo test` skips init entirely: each test gets its own captured
/// stderr from the harness, and a process-wide subscriber would leak
/// host log output into tests that don't want it.
pub fn try_init() {
    if cfg!(test) {
        return;
    }

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(true)
        .compact()
        .try_init();
}

/// Log-and-discard helpers for `Result`. The trait deliberately returns
/// `Option<T>` (not `Result<T, E>`) so a bare `.log_debug("ctx");` is a
/// valid statement without triggering the `must_use` lint.
///
/// Events are emitted with `target = "high_beam::logging"` — the macro
/// captures the *call site* of `warn!`/`debug!`, which is here, not at
/// the caller. Use the `context` string to identify the site; reach for
/// `.inspect_err(|e| tracing::warn!(...))` directly when you need the
/// caller's module path or richer structured fields.
pub trait LogErr<T> {
    /// Log a `warn!` on `Err` and return `None`. Use for failures that
    /// suggest something is off and a future debugger would want to see.
    fn log_warn(self, context: &str) -> Option<T>;

    /// Log a `debug!` on `Err` and return `None`. Use for best-effort
    /// paths (cleanup, post-shutdown sends, racey cancellations) where
    /// the failure is uninteresting on the happy path but worth seeing
    /// when you've turned debug logging on.
    fn log_debug(self, context: &str) -> Option<T>;
}

impl<T, E: Display> LogErr<T> for Result<T, E> {
    fn log_warn(self, context: &str) -> Option<T> {
        self.inspect_err(|e| tracing::warn!(error = %e, "{}", context)).ok()
    }

    fn log_debug(self, context: &str) -> Option<T> {
        self.inspect_err(|e| tracing::debug!(error = %e, "{}", context)).ok()
    }
}
