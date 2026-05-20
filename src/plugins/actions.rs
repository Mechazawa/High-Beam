//! Host-side execution of plugin [`Action`] variants.
//!
//! Stage 3 supports `openUrl` (via the `open` crate) and `copy` (via
//! `arboard`). Stage 4 will add `exec` + `reveal`.

use std::error::Error;

use crate::plugins::result::Action;

/// Execute an action.
///
/// # Errors
///
/// Returns an error if the underlying system call fails (no app to open the
/// URL, clipboard backend unavailable, etc.).
pub fn execute(action: &Action) -> Result<(), Box<dyn Error>> {
    match action {
        Action::OpenUrl { url } => {
            open::that(url)?;
            Ok(())
        }
        Action::Copy { text } => {
            // arboard's Clipboard is connection-per-call style; cheap enough
            // for the one-shot case. Stage 4 wires clipboard.read/write into
            // the SDK and can share a single instance there.
            let mut clipboard = arboard::Clipboard::new()?;
            clipboard.set_text(text.clone())?;
            Ok(())
        }
    }
}
