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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_not_open() {
        let args = Args::parse_from(["highbeam"]);
        assert!(!args.open);
        assert!(args.plugins_dir.is_none());
    }

    #[test]
    fn open_flag_parses() {
        let args = Args::parse_from(["highbeam", "--open"]);
        assert!(args.open);
    }

    #[test]
    fn plugins_dir_flag_parses() {
        let args = Args::parse_from(["highbeam", "--plugins-dir", "/tmp/x"]);
        assert_eq!(args.plugins_dir, Some(PathBuf::from("/tmp/x")));
    }

    #[test]
    fn rejects_unknown_flag() {
        let result = Args::try_parse_from(["highbeam", "--what"]);
        assert!(result.is_err(), "unknown flag should fail to parse");
    }

    #[test]
    fn once_flag_parses() {
        let args = Args::parse_from(["highbeam", "--once"]);
        assert!(args.once);
    }
}
