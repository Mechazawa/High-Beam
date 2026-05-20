//! Discover and load plugins from a directory.
//!
//! Stage 3 scans `plugins/` (the repo-root one in dev; Stage 4+ swaps to the
//! platform config dir from `directories`) for subdirectories containing a
//! `manifest.json` and the entry file the manifest names. Each plugin gets
//! its own [`LoadedPlugin`] (independent JS runtime + context).
//!
//! Failures during load are logged to stderr and the bad plugin is skipped —
//! one syntax error shouldn't take the whole launcher down. Stage 9 routes
//! these to per-plugin logfiles.

use std::path::{Path, PathBuf};

use crate::plugins::manifest::Manifest;
use crate::plugins::runtime::LoadedPlugin;

/// Names of capabilities the host understands today. Anything else is parsed
/// but logged as a warning at load time. Stage 4 expands this list.
const KNOWN_CAPABILITIES: &[&str] = &["actions"];

/// Where to look for plugins.
#[derive(Debug, Clone)]
pub struct LoaderOptions {
    pub plugins_dir: PathBuf,
}

impl LoaderOptions {
    /// Default Stage 3 location: `<repo-root>/plugins`.
    ///
    /// We deliberately use the *current working directory*'s `plugins/`, not
    /// the platform config dir. Stage 4+ moves this to the proper config dir.
    #[must_use]
    pub fn dev_default() -> Self {
        Self {
            plugins_dir: PathBuf::from("plugins"),
        }
    }
}

/// Scan `plugins/` and async-load every valid plugin we find.
///
/// Plugins that fail to load are skipped with a stderr message; the returned
/// vec only contains plugins ready to handle queries.
pub async fn load_all(options: &LoaderOptions) -> Vec<LoadedPlugin> {
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
                    "plugins: loaded {} ({} cap{})",
                    plugin.manifest.name,
                    plugin.manifest.capabilities.len(),
                    if plugin.manifest.capabilities.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                );
                plugins.push(plugin);
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
                "plugins: {}: ignoring unknown capability {cap:?} (Stage 3 only knows {KNOWN_CAPABILITIES:?})",
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
        // Drive the future on a small current-thread runtime; we just want to
        // observe the empty-vec branch.
        let plugins = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt")
            .block_on(load_all(&opts));
        assert!(plugins.is_empty());
    }
}
