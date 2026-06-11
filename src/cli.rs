//! Command-line interface.

use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "highbeam",
    about = "Native Rust keyboard launcher (Spotlight/Alfred/Raycast class).",
    version
)]
pub struct Args {
    /// Open the query window. If a daemon is already running, signal it via
    /// the unix socket. Otherwise, start the daemon and open the window.
    #[arg(long)]
    pub open: bool,

    /// Single-shot mode: open the window and exit when it's dismissed. No
    /// daemon, no IPC socket, no global-hotkey listener. Useful as a WM
    /// keybind target on Wayland — each press cold-starts a fresh process
    /// and inherits the compositor's `XDG_ACTIVATION_TOKEN` directly, which
    /// avoids the IPC-re-show issues with Slint's Wayland backend.
    /// Implies `--open`.
    #[arg(long)]
    pub once: bool,

    /// Override the plugin discovery directory. Defaults to `./plugins` (if
    /// present) or the platform plugin dir.
    #[arg(long, value_name = "PATH")]
    pub plugins_dir: Option<PathBuf>,

    /// Open the launcher with the query box pre-filled with this text, as if
    /// the user had typed it. Forwarded to a running daemon when one exists,
    /// otherwise the cold-started daemon opens with it. Implies `--open`.
    #[arg(long, value_name = "TEXT")]
    pub query: Option<String>,
}
