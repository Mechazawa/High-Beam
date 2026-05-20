//! Command-line interface.
//!
//! High Beam is a daemon. `highbeam` with no args starts and registers the
//! global hotkey. `--open` either tells a running daemon to show its window,
//! or — if no daemon is running — starts one and opens the window immediately.

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_not_open() {
        let args = Args::parse_from(["highbeam"]);
        assert!(!args.open);
    }

    #[test]
    fn open_flag_parses() {
        let args = Args::parse_from(["highbeam", "--open"]);
        assert!(args.open);
    }

    #[test]
    fn rejects_unknown_flag() {
        let result = Args::try_parse_from(["highbeam", "--what"]);
        assert!(result.is_err(), "unknown flag should fail to parse");
    }
}
