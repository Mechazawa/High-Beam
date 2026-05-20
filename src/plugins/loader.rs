//! Discover and load plugins from a directory.
//!
//! Stage 4 prefers the platform plugin dir from `docs/04-platform.md`:
//!   * macOS: `~/Library/Application Support/high-beam/plugins/`
//!   * Linux: `$XDG_DATA_HOME/high-beam/plugins/`
//!
//! …but if a `./plugins` directory exists next to the binary's cwd, that
//! wins (dev convenience). A `--plugins-dir <path>` CLI flag overrides
//! everything for testing. See [`LoaderOptions::resolve`].
//!
//! Each plugin gets its own [`LoadedPlugin`] (independent JS runtime +
//! context), wrapped in `Arc` so the dispatcher can clone the handle into
//! per-plugin spawned tasks. Failures during load are logged to stderr and
//! the bad plugin is skipped; one syntax error shouldn't take the whole
//! launcher down. Stage 9 routes these to per-plugin logfiles.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use directories::ProjectDirs;

use crate::plugins::manifest::Manifest;
use crate::plugins::runtime::LoadedPlugin;
use crate::sdk::capability::KNOWN_CAPABILITIES;

/// Where to look for plugins.
#[derive(Debug, Clone)]
pub struct LoaderOptions {
    pub plugins_dir: PathBuf,
}

impl LoaderOptions {
    /// Resolve the effective plugin directory.
    ///
    /// Priority order:
    ///   1. `cli_override` from `--plugins-dir <path>` (always wins)
    ///   2. `./plugins` next to the binary's cwd, if that dir exists
    ///   3. Platform default from `directories::ProjectDirs` data dir
    ///
    /// We don't *create* the platform dir if it's missing — letting the
    /// directory not exist (and the loader yield zero plugins) is fine,
    /// and creating it eagerly would litter the user's filesystem with
    /// empty dirs on a first run that never installs a plugin.
    #[must_use]
    pub fn resolve(cli_override: Option<PathBuf>) -> Self {
        if let Some(p) = cli_override {
            return Self { plugins_dir: p };
        }
        let dev = PathBuf::from("plugins");
        if dev.is_dir() {
            return Self { plugins_dir: dev };
        }
        let platform = platform_plugins_dir().unwrap_or_else(|| PathBuf::from("plugins"));
        Self {
            plugins_dir: platform,
        }
    }
}

/// Platform plugin dir per `docs/04-platform.md`. Returns `None` if
/// `ProjectDirs` couldn't be resolved (extremely unusual).
fn platform_plugins_dir() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("", "", "high-beam")?;
    Some(dirs.data_dir().join("plugins"))
}

/// Scan `plugins/` and async-load every valid plugin we find.
///
/// Plugins that fail to load are skipped with a stderr message; the returned
/// vec only contains plugins ready to handle queries. Each plugin is wrapped
/// in `Arc` so the dispatcher can hand clones to per-plugin spawned tasks.
pub async fn load_all(options: &LoaderOptions) -> Vec<Arc<LoadedPlugin>> {
    let entries = match std::fs::read_dir(&options.plugins_dir) {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            eprintln!(
                "plugins: failed to read {}: {err}",
                options.plugins_dir.display()
            );
            return Vec::new();
        }
    };

    let mut plugins = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        match load_one(&path).await {
            Ok(plugin) => {
                eprintln!(
                    "plugins: loaded {} ({} caps)",
                    plugin.manifest.name,
                    plugin.manifest.capabilities.len(),
                );
                plugins.push(Arc::new(plugin));
            }
            Err(err) => {
                eprintln!("plugins: skipping {}: {err}", path.display());
            }
        }
    }
    plugins
}

async fn load_one(plugin_dir: &Path) -> Result<LoadedPlugin, Box<dyn std::error::Error>> {
    let manifest_path = plugin_dir.join("manifest.json");
    let bytes = std::fs::read(&manifest_path)
        .map_err(|err| format!("read {}: {err}", manifest_path.display()))?;
    let manifest = Manifest::parse(&bytes).map_err(|err| format!("parse manifest.json: {err}"))?;

    for cap in &manifest.capabilities {
        if !KNOWN_CAPABILITIES.contains(&cap.as_str()) {
            eprintln!(
                "plugins: {}: ignoring unknown capability {cap:?} (known: {KNOWN_CAPABILITIES:?})",
                manifest.name,
            );
        }
    }

    let loaded = LoadedPlugin::load(plugin_dir, manifest).await?;
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_plugins_dir_returns_empty() {
        let opts = LoaderOptions {
            plugins_dir: PathBuf::from("/tmp/high-beam-does-not-exist-xyzzy"),
        };
        let plugins = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt")
            .block_on(load_all(&opts));
        assert!(plugins.is_empty());
    }

    #[test]
    fn cli_override_wins() {
        let p = PathBuf::from("/tmp/forced");
        let opts = LoaderOptions::resolve(Some(p.clone()));
        assert_eq!(opts.plugins_dir, p);
    }

    #[test]
    fn resolve_returns_a_path() {
        // We can't assert much about the resolved path without messing with
        // CWD, but it should always produce *some* path the loader can read.
        let opts = LoaderOptions::resolve(None);
        // The plugins dir under our control either exists or doesn't —
        // either way `resolve` should hand us a non-empty PathBuf.
        assert!(!opts.plugins_dir.as_os_str().is_empty());
    }
}
