//! Platform-specific path resolution.
//!
//! We use `directories` (rather than `dirs`) so the same `<qualifier>/
//! <organization>/<app>` triple covers both platforms — with empty qualifier
//! and organization, the Linux config dir lands at `~/.config/high-beam` and
//! macOS Application Support at `~/Library/Application Support/high-beam`.

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

/// Platform-specific config directory used for `settings.toml`, the
/// `themes/` folder, and other user-edited settings.
///
/// - macOS: `~/Library/Application Support/high-beam/`
/// - Linux: `$XDG_CONFIG_HOME/high-beam/` (default `~/.config/high-beam/`)
///
/// # Errors
///
/// Returns an [`io::Error`] if the platform's project directories can't be
/// resolved (no `$HOME` etc.).
pub fn config_dir() -> io::Result<PathBuf> {
    Ok(project_dirs()?.config_dir().to_path_buf())
}

/// Platform-specific data directory used for the frecency + query-history
/// `SQLite` files.
///
/// - macOS: `~/Library/Application Support/high-beam/`
/// - Linux: `$XDG_DATA_HOME/high-beam/` (default `~/.local/share/high-beam/`)
pub(crate) fn data_dir() -> Option<PathBuf> {
    project_dirs().ok().map(|d| d.data_dir().to_path_buf())
}

/// Per-user plugin install directory under [`data_dir`]. The loader scans
/// this for `manifest.json` files at startup; the installer and the
/// first-launch bundle-seed lay plugins here.
pub(crate) fn plugins_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("plugins"))
}

/// Per-user themes directory under [`config_dir`]. The theme selector lists
/// the `*.toml` files here; the first-launch bundle-seed lays the shipped
/// themes in. Lives under the config dir (not data) because these are
/// user-editable like `settings.toml`.
pub(crate) fn themes_dir() -> Option<PathBuf> {
    config_dir().ok().map(|d| d.join("themes"))
}

/// Platform-specific cache directory. macOS: `~/Library/Caches/high-beam/`;
/// Linux: `$XDG_CACHE_HOME/high-beam/` (default `~/.cache/high-beam/`).
pub(crate) fn cache_dir() -> Option<PathBuf> {
    project_dirs().ok().map(|d| d.cache_dir().to_path_buf())
}

/// Best-effort parent-directory creation for paths we own. Returns `Ok` if the
/// directory already exists or was created.
pub(crate) fn ensure_parent_dir(path: &std::path::Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    Ok(())
}
