//! Per-plugin `plugin.log` writer.
//!
//! ```text
//! [2026-05-20T15:30:42.123Z] [INFO ] message goes here
//!     continuation lines indent four spaces
//! ```
//!
//! Lazy create on first write (plugins that never log leave no file behind).
//! Append-only, no rotation in v1.
//!
//! Writes are non-blocking from the caller's perspective: each
//! `PluginLog::write` formats the line on the calling thread and hands it to
//! a dedicated writer thread over an unbounded channel. The writer owns the
//! file handle and does the actual `write_all` — that way a slow disk
//! (NFS / fsync stall) can't block the tokio runtime thread that drives the
//! plugin JS event loop.

use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use chrono::SecondsFormat;

/// Log severity. Rendered padded to 5 chars so columns align in `tail -f`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn padded(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO ",
            Self::Warn => "WARN ",
            Self::Error => "ERROR",
        }
    }
}

/// Messages the writer thread understands. `Line` is a formatted log line;
/// `Flush` is a synchronisation barrier — the writer pings the supplied ack
/// channel after it has processed every prior `Line`, so callers can wait
/// for buffered writes to land before reading the file.
enum LogMsg {
    Line(String),
    Flush(std_mpsc::Sender<()>),
}

/// Append-only writer for one plugin's `plugin.log`. Cheap to clone via `Arc`.
///
/// `tx` and `writer` are `Mutex<Option<...>>` purely so `Drop` can move them
/// out — dropping the sender first is what tells the writer thread to drain
/// its queue and exit, then `join` blocks until the file is flushed.
pub struct PluginLog {
    path: PathBuf,
    tx: Mutex<Option<std_mpsc::Sender<LogMsg>>>,
    writer: Mutex<Option<thread::JoinHandle<()>>>,
}

impl PluginLog {
    /// Build a writer pointing at `<plugin_dir>/plugin.log`. The file is
    /// created lazily on the writer thread's first message.
    ///
    /// # Panics
    ///
    /// Panics if the OS refuses to spawn the writer thread (memory
    /// exhaustion or process thread-cap reached). Other thread-spawn sites
    /// in the daemon log and continue, but a plugin without its logfile
    /// surface would silently swallow every `console.*` call from JS —
    /// preferable to fail loud here than debug the consequences later.
    #[must_use]
    pub fn for_plugin_dir(plugin_dir: &Path) -> Arc<Self> {
        let path = plugin_dir.join("plugin.log");
        let (tx, rx) = std_mpsc::channel::<LogMsg>();
        let writer_path = path.clone();
        let handle = thread::Builder::new()
            .name("highbeam-plugin-log".into())
            .spawn(move || run_writer(&writer_path, &rx))
            .expect("spawn plugin log writer thread");
        Arc::new(Self {
            path,
            tx: Mutex::new(Some(tx)),
            writer: Mutex::new(Some(handle)),
        })
    }

    /// Path the writer targets. Exposed for tests.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append a single log line. Best-effort: the writer thread surfaces I/O
    /// errors via stderr so a failing disk doesn't take the daemon down.
    /// Returns immediately — the line lands on the writer thread's queue,
    /// not the file.
    pub fn write(&self, level: LogLevel, message: &str) {
        let line = format_line(level, message);
        if let Ok(guard) = self.tx.lock()
            && let Some(tx) = guard.as_ref()
        {
            // `Disconnected` means the writer thread already exited (e.g.
            // during shutdown after Drop ran on another Arc clone) — dropping
            // the line is the only sensible move.
            let _ = tx.send(LogMsg::Line(line));
        }
    }

    /// Block until every line submitted via `write` so far has been written
    /// to disk. Tests call this before asserting on file contents; production
    /// code does not need it (the `Drop` impl flushes on shutdown).
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = std_mpsc::channel::<()>();
        let sent = {
            let Ok(guard) = self.tx.lock() else {
                return;
            };
            match guard.as_ref() {
                Some(tx) => tx.send(LogMsg::Flush(ack_tx)).is_ok(),
                None => false,
            }
        };
        if sent {
            // Sentinel travels FIFO with every prior `Line`, so by the time
            // the writer answers the ack every earlier write is on disk.
            let _ = ack_rx.recv();
        }
    }
}

impl Drop for PluginLog {
    fn drop(&mut self) {
        // Drop the sender so the writer's `recv` returns `Disconnected` once
        // the queue is drained; the join below blocks until that happens,
        // guaranteeing every buffered line lands on disk before this Arc's
        // resources are released.
        drop(
            self.tx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        );
        if let Some(handle) = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = handle.join();
        }
    }
}

/// Drain the channel until the sender side hangs up. File creation is
/// deferred to the first `Line` so plugins that never log leave no file.
fn run_writer(path: &Path, rx: &std_mpsc::Receiver<LogMsg>) {
    let mut file: Option<File> = None;
    while let Ok(msg) = rx.recv() {
        match msg {
            LogMsg::Line(line) => {
                if file.is_none() {
                    match OpenOptions::new().create(true).append(true).open(path) {
                        Ok(f) => file = Some(f),
                        Err(err) => {
                            eprintln!("[plugin {}] couldn't create log: {err}", path.display());
                            continue;
                        }
                    }
                }
                if let Some(f) = file.as_mut()
                    && let Err(err) = f.write_all(line.as_bytes())
                {
                    eprintln!("[plugin {}] couldn't write log: {err}", path.display());
                }
            }
            LogMsg::Flush(ack) => {
                // Best-effort ack — receiver may already be gone if the
                // caller stopped waiting. Either way the prior Line writes
                // are guaranteed to have happened before we get here.
                let _ = ack.send(());
            }
        }
    }
}

/// Render one logfile line including the trailing newline. Continuation
/// lines are indented so `grep` for a level catches the leading line and
/// the indent visually anchors the rest.
fn format_line(level: LogLevel, message: &str) -> String {
    let timestamp = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut out = String::with_capacity(message.len() + 48);
    let mut lines = message.split('\n');
    if let Some(first) = lines.next() {
        let _ = writeln!(out, "[{timestamp}] [{}] {first}", level.padded());
    } else {
        let _ = writeln!(out, "[{timestamp}] [{}]", level.padded());
    }
    for rest in lines {
        let _ = writeln!(out, "    {rest}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let p = std::env::temp_dir().join(format!(
            "high-beam-pluginlog-{tag}-{}-{nanos}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&p).expect("mk tmp");
        p
    }

    #[test]
    fn does_not_create_file_until_first_write() {
        let dir = tmp_dir("nocreate");
        let log = PluginLog::for_plugin_dir(&dir);
        assert!(!log.path().exists(), "no write yet → no file");
        log.write(LogLevel::Info, "hello");
        log.flush();
        assert!(log.path().exists(), "first write created the file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_appended_and_formatted() {
        let dir = tmp_dir("format");
        let log = PluginLog::for_plugin_dir(&dir);
        log.write(LogLevel::Info, "first");
        log.write(LogLevel::Warn, "second");
        log.flush();
        let body = std::fs::read_to_string(log.path()).expect("read log");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("[INFO ] first"), "got {:?}", lines[0]);
        assert!(lines[1].contains("[WARN ] second"), "got {:?}", lines[1]);
        assert!(lines[0].starts_with('['));
        assert!(lines[0].contains('T'));
        assert!(lines[0].contains("Z]"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn multiline_message_indents_continuations() {
        let dir = tmp_dir("multiline");
        let log = PluginLog::for_plugin_dir(&dir);
        log.write(LogLevel::Error, "boom\nat line 2\nat line 3");
        log.flush();
        let body = std::fs::read_to_string(log.path()).expect("read log");
        let mut lines = body.lines();
        assert!(lines.next().unwrap().ends_with("boom"));
        assert_eq!(lines.next(), Some("    at line 2"));
        assert_eq!(lines.next(), Some("    at line 3"));
        assert_eq!(lines.next(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn appending_does_not_truncate_prior_content() {
        let dir = tmp_dir("append");
        {
            let log = PluginLog::for_plugin_dir(&dir);
            log.write(LogLevel::Info, "alpha");
        }
        // First log Arc dropped → writer thread joined → file fsync'd.
        {
            let log = PluginLog::for_plugin_dir(&dir);
            log.write(LogLevel::Info, "beta");
        }
        let body = std::fs::read_to_string(dir.join("plugin.log")).expect("read");
        assert!(body.contains("alpha"));
        assert!(body.contains("beta"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
