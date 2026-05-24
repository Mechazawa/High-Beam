//! Unix-domain-socket IPC for single-instance coordination.
//!
//! Newline-terminated ASCII commands; today there's exactly one (`open`),
//! so a fixed read buffer is fine. Length-prefixed framing waits until we
//! carry payloads bigger than a few bytes.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// Commands accepted by the running daemon. Wire format is stable; do not
/// rename without considering compat with running daemons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open the query window (or focus it if already open).
    ///
    /// `activation_token` carries an `XDG_ACTIVATION_TOKEN` forwarded from
    /// the invoking process so the daemon can grab focus on Wayland (the
    /// analog of macOS's `NSApp.activate(ignoringOtherApps:)`). `None` is
    /// expected for invocations that have no token to offer — e.g. a WM
    /// keybind that runs `high-beam --open` from a context where
    /// `XDG_ACTIVATION_TOKEN` was already consumed, or non-Wayland callers.
    Open { activation_token: Option<String> },
}

impl Command {
    /// Wire format:
    /// * `"open"` — no token (legacy + WM keybind path)
    /// * `"open <token>"` — with token; `<token>` may not contain whitespace
    ///   (real XDG activation tokens are opaque ASCII without spaces).
    fn as_wire(&self) -> String {
        match self {
            Self::Open { activation_token: None } => "open".to_owned(),
            Self::Open {
                activation_token: Some(t),
            } => format!("open {t}"),
        }
    }

    fn parse(line: &str) -> Result<Self, ParseError> {
        let trimmed = line.trim();
        if trimmed == "open" {
            return Ok(Self::Open { activation_token: None });
        }
        if let Some(rest) = trimmed.strip_prefix("open ") {
            let token = rest.trim();
            if token.is_empty() {
                return Ok(Self::Open { activation_token: None });
            }
            return Ok(Self::Open {
                activation_token: Some(token.to_owned()),
            });
        }
        Err(ParseError::Unknown(trimmed.to_owned()))
    }
}

#[derive(Debug)]
enum ParseError {
    Unknown(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(cmd) => write!(f, "unknown IPC command: {cmd:?}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Server side of the single-instance lock. `bind` removes a stale socket
/// file if the previous owner is gone; if a live daemon is listening it
/// returns `AddrInUse` so callers can switch to `send`.
#[derive(Debug)]
pub(crate) struct Server {
    listener: UnixListener,
    path: PathBuf,
}

impl Server {
    pub(crate) fn bind(path: &Path) -> io::Result<Self> {
        crate::paths::ensure_parent_dir(path)?;

        match UnixListener::bind(path) {
            Ok(listener) => Ok(Self {
                listener,
                path: path.to_path_buf(),
            }),
            Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
                // Probe by connecting; if we can't, the socket is stale.
                if UnixStream::connect(path).is_err() {
                    std::fs::remove_file(path)?;
                    let listener = UnixListener::bind(path)?;
                    Ok(Self {
                        listener,
                        path: path.to_path_buf(),
                    })
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "high-beam daemon already running",
                    ))
                }
            }
            Err(err) => Err(err),
        }
    }

    /// Block on the listener, calling `handler` for each parsed command.
    /// Intended pattern: dedicate a thread that owns the `Server` and
    /// forwards commands to the UI thread.
    pub(crate) fn run<F>(self, mut handler: F) -> io::Result<()>
    where
        F: FnMut(Command) + Send + 'static,
    {
        for stream in self.listener.incoming() {
            let stream = stream?;
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                continue;
            }
            match Command::parse(&line) {
                Ok(cmd) => handler(cmd),
                Err(err) => tracing::warn!(%err, "ipc: rejecting unknown command"),
            }
        }
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // If we crashed instead of dropping, `Server::bind` clears the stale
        // file on next start.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Client side: connect and send one command.
///
/// # Errors
///
/// Returns an [`io::Error`] if connecting or writing fails. The common case
/// is `ConnectionRefused`/`NotFound` — no daemon listening, caller should
/// fall back to starting one.
pub fn send(path: &Path, command: &Command) -> io::Result<()> {
    let mut stream = UnixStream::connect(path)?;
    writeln!(stream, "{}", command.as_wire())?;
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    fn tmp_socket(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("high-beam-test-{}-{}.sock", name, std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn command_roundtrip_string() {
        let bare = Command::Open { activation_token: None };
        assert_eq!(bare.as_wire(), "open");
        assert_eq!(Command::parse("open\n").unwrap(), bare);
        assert_eq!(Command::parse("  open  ").unwrap(), bare);
        // Empty token after the space degrades to no-token rather than an
        // empty-string token — the compositor would reject empty anyway.
        assert_eq!(Command::parse("open ").unwrap(), bare);
        assert!(Command::parse("nope").is_err());
    }

    #[test]
    fn command_with_activation_token_roundtrips() {
        let with_token = Command::Open {
            activation_token: Some("xdg-foo-bar-123".to_owned()),
        };
        assert_eq!(with_token.as_wire(), "open xdg-foo-bar-123");
        assert_eq!(Command::parse("open xdg-foo-bar-123\n").unwrap(), with_token);
    }

    #[test]
    fn server_receives_client_command() {
        let path = tmp_socket("roundtrip");
        let server = Server::bind(&path).expect("bind server");

        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let _ = server.run(move |cmd| {
                let _ = tx.send(cmd);
            });
        });

        // Tiny race window before the listener is ready.
        thread::sleep(Duration::from_millis(50));
        let cmd = Command::Open { activation_token: None };
        send(&path, &cmd).expect("client send");

        let received = rx.recv_timeout(Duration::from_secs(1)).expect("receive command");
        assert_eq!(received, cmd);

        drop(handle);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn second_bind_when_first_alive_errors() {
        let path = tmp_socket("alive");
        let _first = Server::bind(&path).expect("first bind succeeds");
        let err = Server::bind(&path).expect_err("second bind must fail");
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn stale_socket_is_replaced() {
        let path = tmp_socket("stale");
        {
            let _first = Server::bind(&path).expect("bind first");
        }
        // Simulate a crash: leftover file with no listener.
        std::fs::File::create(&path).expect("create stale socket file");
        let _second = Server::bind(&path).expect("bind succeeds over stale file");
        let _ = std::fs::remove_file(&path);
    }
}
