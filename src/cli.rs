//! Command-line interface.
//!
//! High Beam is a daemon. `highbeam` with no args starts and registers the
//! global hotkey. `--open` either tells a running daemon to show its window,
//! or — if no daemon is running — starts one and opens the window immediately.

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

    /// Override the plugin discovery directory. Defaults to `./plugins` (if
    /// present) or the platform plugin dir from `docs/04-platform.md`.
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
}
