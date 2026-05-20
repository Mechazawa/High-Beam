use std::process::ExitCode;

use clap::Parser;
use high_beam::cli::Args;
use high_beam::daemon::{self, Options};
use high_beam::ipc::{self, Command};
use high_beam::paths;

fn main() -> ExitCode {
    let args = Args::parse();
    let socket_path = match paths::socket_path() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("could not resolve socket path: {err}");
            return ExitCode::FAILURE;
        }
    };

    // `--open` first tries to contact a running daemon; if there isn't one,
    // it falls through and starts a daemon that opens immediately.
    if args.open && ipc::send(&socket_path, Command::Open).is_ok() {
        return ExitCode::SUCCESS;
    }

    let options = Options {
        open_on_start: args.open,
        socket_path,
        plugins_dir: args.plugins_dir,
    };

    match daemon::run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("high-beam: {err}");
            ExitCode::FAILURE
        }
    }
}
