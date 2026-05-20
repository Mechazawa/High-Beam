//! Discover and load plugins from a directory.
//!
//! Default location is the platform data dir:
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
//! the bad plugin is skipped — one syntax error shouldn't take the whole
//! launcher down.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use directories::ProjectDirs;

use crate::plugins::log::{LogLevel, PluginLog};
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

/// Platform default plugin dir. Returns `None` if `ProjectDirs` couldn't be
/// resolved (extremely unusual).
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
            Err(LoadError::Skipped { name, reason }) => {
                // INFO-level: deliberate gate, not an error condition.
                eprintln!("plugins: skipping {name}: {reason}");
            }
            Err(LoadError::Failed(err)) => {
                eprintln!("plugins: skipping {}: {err}", path.display());
            }
        }
    }
    plugins
}

async fn load_one(plugin_dir: &Path) -> Result<LoadedPlugin, LoadError> {
    let manifest_path = plugin_dir.join("manifest.json");
    let bytes = std::fs::read(&manifest_path).map_err(|err| {
        LoadError::Failed(format!("read {}: {err}", manifest_path.display()).into())
    })?;
    // Manifest parse failures are reported to stderr rather than plugin.log:
    // we have no per-plugin identity yet (the file is the source of truth for
    // the name), so writing into a per-plugin file before the manifest parses
    // would mean inventing a name — leaving stderr as the only honest channel.
    let manifest = Manifest::parse(&bytes)
        .map_err(|err| LoadError::Failed(format!("parse manifest.json: {err}").into()))?;

    let log = PluginLog::for_plugin_dir(plugin_dir);

    for cap in &manifest.capabilities {
        if !KNOWN_CAPABILITIES.contains(&cap.as_str()) {
            log.write(
                LogLevel::Warn,
                &format!("ignoring unknown capability {cap:?} (known: {KNOWN_CAPABILITIES:?})"),
            );
            eprintln!(
                "plugins: {}: ignoring unknown capability {cap:?} (known: {KNOWN_CAPABILITIES:?})",
                manifest.name,
            );
        }
    }

    // Gate before paying the rquickjs Context cost — that's where the expense
    // sits, and we'd be evaluating an entry module just to discard it.
    if !manifest.supports_current_platform() {
        let reason = manifest
            .platform_skip_reason()
            .unwrap_or_else(|| "platform gate".to_owned());
        return Err(LoadError::Skipped {
            name: manifest.name,
            reason,
        });
    }

    let cache_dir = crate::plugins::runtime::default_cache_dir(&manifest.name);
    let loaded = LoadedPlugin::load_with_log(plugin_dir, manifest, cache_dir, Arc::clone(&log))
        .await
        .map_err(|err| {
            log.write(LogLevel::Error, &format!("load failed: {err}"));
            LoadError::Failed(Box::new(err))
        })?;
    Ok(loaded)
}

/// Distinguishes a deliberate platform-gate skip (logged at INFO) from a real
/// load failure (logged as an error) so users don't see scary "skipping" lines
/// for plugins that did nothing wrong.
enum LoadError {
    Skipped { name: String, reason: String },
    Failed(Box<dyn std::error::Error>),
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

    /// Build a throwaway plugin tree on disk so the loader can scan it.
    /// We hand-roll a temp dir via `std::env::temp_dir` + nanos so the test
    /// has zero extra deps and doesn't fight cargo's lack of `tempfile`.
    fn write_plugin(root: &Path, name: &str, manifest: &str, entry: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest).unwrap();
        std::fs::write(dir.join("plugin.js"), entry).unwrap();
    }

    fn fresh_tmp(tag: &str) -> PathBuf {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("high-beam-loader-test-{tag}-{now}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn platform_gated_plugin_is_skipped_by_loader() {
        let root = fresh_tmp("gate");
        // A "wrong-os" plugin that declares only the OTHER known platform —
        // gating must skip it without crashing or polluting the result set.
        let other = if std::env::consts::OS == "macos" {
            "linux"
        } else {
            "macos"
        };
        let wrong =
            format!(r#"{{ "name": "wrong-os", "entry": "plugin.js", "platforms": ["{other}"] }}"#);
        write_plugin(
            &root,
            "wrong-os",
            &wrong,
            "export async function* query() {}",
        );
        // And a matching plugin so we can assert the loader still found one.
        let right = format!(
            r#"{{ "name": "right-os", "entry": "plugin.js", "platforms": ["{}"] }}"#,
            std::env::consts::OS,
        );
        write_plugin(
            &root,
            "right-os",
            &right,
            "export async function* query() {}",
        );

        let opts = LoaderOptions {
            plugins_dir: root.clone(),
        };
        let plugins = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt")
            .block_on(load_all(&opts));

        // The wrong-os plugin must be absent from the loaded set; the right-os
        // plugin must be present. Anything else (panic, both present, both
        // absent) is a regression.
        let names: Vec<_> = plugins.iter().map(|p| p.manifest.name.as_str()).collect();
        assert!(
            !names.contains(&"wrong-os"),
            "wrong-os plugin should have been gated out, got {names:?}",
        );
        assert!(
            names.contains(&"right-os"),
            "right-os plugin should have loaded, got {names:?}",
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_platforms_array_disables_plugin() {
        let root = fresh_tmp("shelved");
        write_plugin(
            &root,
            "shelved",
            r#"{ "name": "shelved", "entry": "plugin.js", "platforms": [] }"#,
            "export async function* query() {}",
        );

        let opts = LoaderOptions {
            plugins_dir: root.clone(),
        };
        let plugins = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt")
            .block_on(load_all(&opts));

        assert!(
            plugins.is_empty(),
            "empty platforms must disable the plugin everywhere",
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
