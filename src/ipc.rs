//! Unix-domain-socket IPC for single-instance coordination.
//!
//! Wire format is intentionally tiny: newline-terminated ASCII commands. Right
//! now there's exactly one — `open` — so a fixed read buffer is fine. Stage 3+
//! will add more; we'll move to length-prefixed framing if we ever carry
//! payloads bigger than a few bytes.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// Commands accepted by the running daemon. Keep the wire representation
/// stable across stages — `Display` is the format that goes over the socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Open the query window (or focus it if already open).
    Open,
}

impl Command {
    fn as_wire(self) -> &'static str {
        match self {
            Self::Open => "open",
        }
    }

    fn parse(line: &str) -> Result<Self, ParseError> {
        match line.trim() {
            "open" => Ok(Self::Open),
            other => Err(ParseError::Unknown(other.to_owned())),
        }
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

/// Server side of the single-instance lock.
///
/// `bind` removes a stale socket file if it exists and the previous owner is
/// gone; if a live daemon is listening, `bind` returns an error and callers
/// should switch to `send`.
#[derive(Debug)]
pub(crate) struct Server {
    listener: UnixListener,
    path: PathBuf,
}

impl Server {
    /// Bind to `path`. If a daemon is already listening, returns
    /// `io::ErrorKind::AddrInUse` so the caller can fall back to client mode.
    pub(crate) fn bind(path: &Path) -> io::Result<Self> {
        crate::paths::ensure_parent_dir(path)?;

        match UnixListener::bind(path) {
            Ok(listener) => Ok(Self {
                listener,
                path: path.to_path_buf(),
            }),
            Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
                // Either a live daemon owns this, or it's a stale file from a crash.
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
    ///
    /// Returns only on accept-loop error. The intended pattern is to spawn a
    /// dedicated thread that owns the `Server` and forwards commands to the
    /// main thread (which owns the UI).
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
                Err(err) => eprintln!("ipc: {err}"),
            }
        }
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Best-effort cleanup. If we crashed instead of dropping, the stale
        // socket gets removed on next start (see `Server::bind`).
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Client side: connect and send one command.
///
/// # Errors
///
/// Returns an [`io::Error`] if connecting or writing fails. The most common
/// case is `ConnectionRefused` / `NotFound`, which means no daemon is
/// currently listening — callers should fall back to starting one.
pub fn send(path: &Path, command: Command) -> io::Result<()> {
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
        path.push(format!(
            "high-beam-test-{}-{}.sock",
            name,
            std::process::id()
        ));
        // Clean up any leftovers from a prior run.
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn command_roundtrip_string() {
        assert_eq!(Command::Open.as_wire(), "open");
        assert_eq!(Command::parse("open\n").unwrap(), Command::Open);
        assert_eq!(Command::parse("  open  ").unwrap(), Command::Open);
        assert!(Command::parse("nope").is_err());
    }

    #[test]
    fn server_receives_client_command() {
        let path = tmp_socket("roundtrip");
        let server = Server::bind(&path).expect("bind server");

        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            // The server runs until the listener closes; here we just need one
            // event, so we listen on a worker and shut down after recv.
            let _ = server.run(move |cmd| {
                let _ = tx.send(cmd);
            });
        });

        // Tiny race window before the listener is ready; this is enough.
        thread::sleep(Duration::from_millis(50));
        send(&path, Command::Open).expect("client send");

        let received = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("receive command");
        assert_eq!(received, Command::Open);

        // Drop the listener thread by deleting the socket; the accept loop will
        // exit on the next iteration. We don't join — the test process exit
        // will reap it.
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
            // Drop runs and removes the file. Simulate a crash instead by
            // shadow-creating a leftover file with no listener.
        }
        std::fs::File::create(&path).expect("create stale socket file");
        // Second bind should detect the stale file and replace it.
        let _second = Server::bind(&path).expect("bind succeeds over stale file");
        let _ = std::fs::remove_file(&path);
    }
}
