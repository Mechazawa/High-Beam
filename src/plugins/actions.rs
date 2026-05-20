//! Host-side execution of plugin [`Action`] variants.
//!
//! Stage 4 supports the full v1 set:
//!   * `OpenUrl` — `open::that(url)` (system handler)
//!   * `Copy`    — `arboard::Clipboard::set_text`
//!   * `Exec`    — spawn a subprocess, no stdout capture (Stage 7 adds the
//!     `highbeam:system.exec` story with stdout/stderr/code)
//!   * `Reveal`  — open the parent dir with the file selected (macOS `open -R`;
//!     Linux opens the parent dir best-effort, no selection)

use std::error::Error;
use std::path::Path;
use std::process::Command;

use crate::plugins::result::Action;

/// Execute an action.
///
/// # Errors
///
/// Returns an error if the underlying system call fails (no app to open the
/// URL, clipboard backend unavailable, subprocess spawn failed, etc.).
pub fn execute(action: &Action) -> Result<(), Box<dyn Error>> {
    match action {
        Action::OpenUrl { url } => {
            open::that(url)?;
            Ok(())
        }
        Action::Copy { text } => {
            // arboard's Clipboard is connection-per-call style; cheap enough
            // for the one-shot case. The Stage 4 `highbeam:clipboard` module
            // creates its own instance per call too.
            let mut clipboard = arboard::Clipboard::new()?;
            clipboard.set_text(text.clone())?;
            Ok(())
        }
        Action::Exec { cmd, args } => {
            // Stage 4 fire-and-forget: spawn and forget. Stage 7's
            // `highbeam:system.exec(...)` (the *live* call, not the action
            // variant) is where stdout/stderr/code capture lives.
            Command::new(cmd).args(args).spawn()?;
            Ok(())
        }
        Action::Reveal { path } => {
            reveal(path)?;
            Ok(())
        }
    }
}

/// Reveal a file in the system file manager.
///
/// macOS: `open -R <path>` (the dedicated Finder "select this file" mode).
/// Linux: best-effort `xdg-open <parent_dir>` — no selection.
/// Other: not supported; returns an error consistent with how the rest of the
/// daemon handles unsupported platforms.
fn reveal(path: &Path) -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "macos")]
    {
        Command::new("/usr/bin/open").arg("-R").arg(path).spawn()?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        let target = path.parent().unwrap_or(path);
        Command::new("xdg-open").arg(target).spawn()?;
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        Err("reveal is only supported on macOS and Linux".into())
    }
}
