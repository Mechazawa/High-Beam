//! Per-plugin `plugin.log` writer.
//!
//! ```text
//! [2026-05-20T15:30:42.123Z] [INFO ] message goes here
//!     continuation lines indent four spaces
//! ```
//!
//! Lazy create on first write (plugins that never log leave no file behind).
//! Append-only, no rotation in v1. `Mutex<Option<File>>` so JS-side
//! callbacks (tokio worker) and host code (loader) can share one writer.

use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

/// Append-only writer for one plugin's `plugin.log`. Cheap to clone via `Arc`.
pub struct PluginLog {
    path: PathBuf,
    file: Mutex<Option<File>>,
}

impl PluginLog {
    /// Build a writer pointing at `<plugin_dir>/plugin.log`. The file is
    /// created lazily on first `write`.
    #[must_use]
    pub fn for_plugin_dir(plugin_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            path: plugin_dir.join("plugin.log"),
            file: Mutex::new(None),
        })
    }

    /// Path the writer targets. Exposed for tests.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append a single log line. Best-effort: a filesystem error falls back to
    /// stderr so we never panic over diagnostics.
    pub fn write(&self, level: LogLevel, message: &str) {
        let line = format_line(level, message);
        let mut guard = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_none() {
            match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                Ok(file) => *guard = Some(file),
                Err(err) => {
                    eprintln!(
                        "[plugin {}] couldn't create log: {err}",
                        self.path.display(),
                    );
                    return;
                }
            }
        }
        if let Some(file) = guard.as_mut()
            && let Err(err) = file.write_all(line.as_bytes())
        {
            eprintln!("[plugin {}] couldn't write log: {err}", self.path.display());
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
        assert!(log.path().exists(), "first write created the file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_appended_and_formatted() {
        let dir = tmp_dir("format");
        let log = PluginLog::for_plugin_dir(&dir);
        log.write(LogLevel::Info, "first");
        log.write(LogLevel::Warn, "second");
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
