use std::env;
use std::process::ExitCode;

use clap::Parser;
use high_beam::cli::Args;
use high_beam::daemon::{self, Options};
use high_beam::ipc::{self, Command};
use high_beam::logging;
use high_beam::paths;

fn main() -> ExitCode {
    let args = Args::parse();
    // Init here so the short-circuit `--open` path (which doesn't reach
    // `daemon::run`) still has a working subscriber. `daemon::run` calls
    // `try_init` too; the guard keeps the second call harmless.
    logging::try_init();

    let socket_path = match paths::socket_path() {
        Ok(p) => p,
        Err(err) => {
            tracing::error!(%err, "could not resolve socket path");
            return ExitCode::FAILURE;
        }
    };

    // The Wayland focus-grab path needs an `XDG_ACTIVATION_TOKEN` for the
    // compositor to honor the request. On cold-start the token is consumed
    // by `daemon::run` directly; on the IPC path we forward it across the
    // socket so the already-running daemon can use it as the analog of
    // macOS's `NSApp.activate(ignoringOtherApps:)`. Either way we remove
    // the env var immediately so child processes (plugins, `open::that(…)`,
    // etc.) don't inherit a stale token.
    let activation_token = consume_activation_token();

    // `--open` first tries to contact a running daemon; if there isn't one
    // it falls through and starts a daemon that opens immediately.
    //
    // `--once` deliberately skips the IPC fast-path: every invocation
    // cold-starts a fresh process that exits on dismiss, no single-instance
    // lock, no socket. That sidesteps the Slint-1.16 Wayland re-show issues
    // entirely. `--once` implies `--open`.
    if args.open && !args.once {
        let cmd = Command::Open {
            activation_token: activation_token.clone(),
        };

        if ipc::send(&socket_path, &cmd).is_ok() {
            return ExitCode::SUCCESS;
        }
    }

    let options = Options {
        open_on_start: args.open || args.once,
        once: args.once,
        activation_token,
        socket_path,
        plugins_dir: args.plugins_dir,
    };

    match daemon::run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(%err, "daemon exited with error");
            ExitCode::FAILURE
        }
    }
}

/// Read `XDG_ACTIVATION_TOKEN` (Wayland) or `DESKTOP_STARTUP_ID` (X11) from
/// the process environment and clear them. The freedesktop startup-notify
/// spec requires the consumer to unset the variable so it isn't inherited
/// by unrelated child processes — see [`winit::platform::startup_notify`].
fn consume_activation_token() -> Option<String> {
    // Prefer the Wayland variable; fall back to the X11 one for the rare
    // cross-protocol launcher (e.g., XWayland app launching us). The
    // variable name is opaque to the daemon — it just forwards whichever
    // it got.
    let token = env::var("XDG_ACTIVATION_TOKEN")
        .ok()
        .or_else(|| env::var("DESKTOP_STARTUP_ID").ok());
    // SAFETY: documented as the canonical consume-and-clear pattern for
    // these vars; running synchronously in `main` before any threads spawn
    // means no other thread can observe a torn read.
    unsafe {
        env::remove_var("XDG_ACTIVATION_TOKEN");
        env::remove_var("DESKTOP_STARTUP_ID");
    }
    token.filter(|s| !s.is_empty())
}
