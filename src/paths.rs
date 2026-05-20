//! Platform-specific path resolution.
//!
//! See `docs/04-platform.md`. We use `directories` rather than `dirs` to get
//! the convenience wrapper that already knows our `<qualifier>/<organization>/<app>`
//! triple — we keep qualifier and organization empty so the Linux config dir
//! lands at `~/.config/high-beam` and the macOS Application Support folder at
//! `~/Library/Application Support/high-beam`.

use std::io;
use std::path::PathBuf;

use directories::ProjectDirs;

const QUALIFIER: &str = "";
const ORGANIZATION: &str = "";
const APPLICATION: &str = "high-beam";

fn project_dirs() -> io::Result<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve platform project directories",
        )
    })
}

/// Path to the unix domain socket used for single-instance coordination.
///
/// - macOS: `~/Library/Application Support/high-beam/high-beam.sock`
/// - Linux: `$XDG_RUNTIME_DIR/high-beam.sock`, falling back to
///   `$XDG_STATE_HOME/high-beam/high-beam.sock` when the runtime dir is unset.
///
/// # Errors
///
/// Returns an [`io::Error`] if the platform's project directories can't be
/// resolved, or — on Linux — if neither `$XDG_RUNTIME_DIR` nor a state
/// directory is available.
pub fn socket_path() -> io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let dirs = project_dirs()?;
        Ok(dirs.config_dir().join("high-beam.sock"))
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(base) = directories::BaseDirs::new()
            && let Some(runtime) = base.runtime_dir()
        {
            return Ok(runtime.join("high-beam.sock"));
        }
        let dirs = project_dirs()?;
        let state = dirs.state_dir().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no XDG_RUNTIME_DIR and no state directory available",
            )
        })?;
        Ok(state.join("high-beam.sock"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "high-beam supports macOS and Linux only",
        ))
    }
}

/// Best-effort parent-directory creation for paths we own. Returns `Ok` if the
/// directory already exists or was created.
pub(crate) fn ensure_parent_dir(path: &std::path::Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_ends_in_sock() {
        let path = socket_path().expect("resolve socket path");
        assert_eq!(
            path.extension().and_then(std::ffi::OsStr::to_str),
            Some("sock"),
            "socket path should end in .sock; got {}",
            path.display()
        );
    }

    #[test]
    fn socket_path_filename_is_app_specific() {
        let path = socket_path().expect("resolve socket path");
        let filename = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("socket path has a filename");
        assert!(
            filename.contains("high-beam"),
            "socket filename should mention the app; got {filename}"
        );
    }
}
