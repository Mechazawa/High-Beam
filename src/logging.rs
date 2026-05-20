//! Host-side `tracing` subscriber bootstrap.
//!
//! Single fmt-formatted subscriber wired to stderr, filtered through
//! `RUST_LOG` (defaulting to `info` when unset). Plugin-side logs flow
//! through `crate::plugins::log::PluginLog` and are intentionally not
//! routed here.

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

#[cfg(test)]
mod tests {
    use super::try_init;

    #[test]
    fn double_try_init_does_not_panic() {
        // Under cargo test `try_init` short-circuits, but the contract — a
        // second call is a no-op rather than a panic — applies in both
        // modes. Production callers (main, daemon::run) only ever go
        // through this surface, so the test mirrors that.
        try_init();
        try_init();
    }
}
