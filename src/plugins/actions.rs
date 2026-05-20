//! Host-side execution of plugin [`Action`] variants.
//!
//! `Exec` here is fire-and-forget; the live-capture variant lives in
//! `highbeam:system.exec`. `Reveal` opens the parent dir with the file
//! selected (macOS `open -R`); Linux opens the parent dir, no selection.

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
            let mut clipboard = arboard::Clipboard::new()?;
            clipboard.set_text(text.clone())?;
            Ok(())
        }
        Action::Exec { cmd, args } => {
            Command::new(cmd).args(args).spawn()?;
            Ok(())
        }
        Action::Reveal { path } => {
            reveal(path)?;
            Ok(())
        }
        Action::Quit => {
            // Hard exit — bypasses Drop for in-flight resources, but every
            // owned resource (tokio runtime, SQLite handle, rquickjs context)
            // is designed to survive abrupt termination.
            std::process::exit(0);
        }
        Action::Noop => Ok(()),
    }
}

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
