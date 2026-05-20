//! Host-side execution of plugin [`Action`] variants.
//!
//! `Exec` here is fire-and-forget; the live-capture variant lives in
//! `highbeam:system.exec`. `Reveal` opens the parent dir with the file
//! selected (macOS `open -R`); Linux opens the parent dir, no selection.

use std::error::Error;
use std::path::Path;
use std::process::Command;

use crate::plugins::result::Action;

/// What the caller should do with the window after `execute` returned.
///
/// `HideWindow` is the default — invoking the row's action removes the
/// reason the launcher is on screen, so we close it. `KeepOpen` is for
/// in-window navigation actions like `OpenSettings`, which switch views
/// rather than dismissing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionOutcome {
    HideWindow,
    KeepOpen,
    OpenSettingsView,
}

/// Execute an action.
///
/// # Errors
///
/// Returns an error if the underlying system call fails (no app to open the
/// URL, clipboard backend unavailable, subprocess spawn failed, etc.).
pub fn execute(action: &Action) -> Result<ActionOutcome, Box<dyn Error>> {
    match action {
        Action::OpenUrl { url } => {
            open::that(url)?;
            Ok(ActionOutcome::HideWindow)
        }
        Action::Copy { text } => {
            let mut clipboard = arboard::Clipboard::new()?;
            clipboard.set_text(text.clone())?;
            Ok(ActionOutcome::HideWindow)
        }
        Action::Exec { cmd, args } => {
            Command::new(cmd).args(args).spawn()?;
            Ok(ActionOutcome::HideWindow)
        }
        Action::Reveal { path } => {
            reveal(path)?;
            Ok(ActionOutcome::HideWindow)
        }
        Action::Quit => {
            // Hard exit — bypasses Drop for in-flight resources, but every
            // owned resource (tokio runtime, SQLite handle, rquickjs context)
            // is designed to survive abrupt termination.
            std::process::exit(0);
        }
        Action::OpenSettings => Ok(ActionOutcome::OpenSettingsView),
        // Noop preserved the pre-outcome behaviour of hiding the window —
        // the launcher closes after Enter, even on a `Noop` row like the
        // version readout, because the user explicitly chose to act.
        Action::Noop => Ok(ActionOutcome::HideWindow),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_settings_signals_settings_view_outcome() {
        let outcome = execute(&Action::OpenSettings).expect("execute");
        assert_eq!(outcome, ActionOutcome::OpenSettingsView);
    }

    #[test]
    fn noop_still_hides_window_to_match_prior_behaviour() {
        let outcome = execute(&Action::Noop).expect("execute");
        assert_eq!(outcome, ActionOutcome::HideWindow);
    }
}
