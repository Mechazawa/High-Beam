//! Discover and load plugins from a directory.
//!
//! Each plugin gets its own [`LoadedPlugin`] (independent JS runtime +
//! context). Load failures are logged and the plugin is skipped — one bad
//! manifest must not take the whole launcher down. See
//! [`LoaderOptions::resolve`] for the directory search order.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use directories::ProjectDirs;

use crate::plugins::log::{LogLevel, PluginLog};
use crate::plugins::manifest::Manifest;
use crate::plugins::runtime::{LifecycleReason, LoadedPlugin};
use crate::sdk::capability;
use crate::settings::Settings;

/// A plugin loaded successfully, alongside the lifecycle hook (if any) the
/// caller should fire post-load.
///
/// `reason = None` when the plugin's recorded version in
/// `settings.last_loaded_version` already matches the manifest — the loader
/// has nothing to announce. `Some(Install)` for plugins the host has never
/// seen, `Some(Update)` when the manifest version moved on disk. Plugins
/// without a manifest `version` always get `None` (no way to detect
/// updates), even on first load — opting out of lifecycle hooks is a
/// reasonable default for dev plugins.
pub struct LoadOutcome {
    pub plugin: Arc<LoadedPlugin>,
    pub reason: Option<LifecycleReason>,
}

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
    /// The platform dir is NOT created if missing — letting the loader
    /// yield zero plugins is fine; eagerly creating it would litter the
    /// filesystem with empty dirs on first runs that never install a plugin.
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
        Self { plugins_dir: platform }
    }
}

fn platform_plugins_dir() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("", "", "high-beam")?;
    Some(dirs.data_dir().join("plugins"))
}

/// Cheap synchronous scan of `plugins_dir` returning every well-formed
/// manifest. Used by the settings UI, which wants to render an option row
/// per plugin regardless of whether each one currently loads — disabled and
/// platform-gated plugins still appear in settings so the user can flip
/// the toggle.
#[must_use]
pub fn scan_manifests(options: &LoaderOptions) -> Vec<Manifest> {
    let Ok(entries) = std::fs::read_dir(&options.plugins_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("manifest.json");
        let Ok(bytes) = std::fs::read(&manifest_path) else {
            continue;
        };
        if let Ok(manifest) = Manifest::parse(&bytes) {
            out.push(manifest);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Scan `plugins/` and async-load every valid plugin we find. Plugins that
/// fail to load are skipped with a stderr message; the returned vec only
/// contains plugins ready to handle queries.
///
/// The default `settings` of [`Settings::default()`] treats every plugin as
/// enabled, so callers that don't care about user settings can pass it
/// directly.
pub async fn load_all(options: &LoaderOptions, settings: &Settings) -> Vec<LoadOutcome> {
    let plugins_dir = options.plugins_dir.clone();
    let scan = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<PathBuf>> {
        let entries = std::fs::read_dir(&plugins_dir)?;
        Ok(entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect())
    })
    .await;
    let dirs = match scan {
        Ok(Ok(dirs)) => dirs,
        Ok(Err(err)) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Ok(Err(err)) => {
            tracing::error!(
                plugins_dir = %options.plugins_dir.display(),
                %err,
                "plugins: failed to read plugins directory",
            );
            return Vec::new();
        }
        Err(join_err) => {
            tracing::error!(
                plugins_dir = %options.plugins_dir.display(),
                %join_err,
                "plugins: scan task panicked",
            );
            return Vec::new();
        }
    };

    let mut plugins = Vec::new();
    for path in dirs {
        match load_one(&path, settings).await {
            Ok((plugin, reason)) => {
                tracing::info!(
                    plugin = %plugin.manifest.name,
                    caps = plugin.manifest.capabilities.len(),
                    "plugins: loaded",
                );
                plugins.push(LoadOutcome {
                    plugin: Arc::new(plugin),
                    reason,
                });
            }
            Err(LoadError::Skipped { name, reason }) => {
                // Deliberate gate (e.g. platform), not an error condition.
                tracing::info!(plugin = %name, %reason, "plugins: skipping");
            }
            Err(LoadError::Failed(err)) => {
                tracing::warn!(path = %path.display(), %err, "plugins: skipping");
            }
        }
    }
    plugins
}

/// Reload one plugin directory in isolation.
///
/// Used by [`crate::plugins::registry::PluginRegistry::reload_one`] when the
/// user invokes the `reload` verb. Returns the loaded plugin wrapped in an
/// `Arc` (matching what the dispatcher consumes) or a flattened reason
/// string — the registry surfaces it to the user via the result row, so a
/// stringly-typed error keeps the call site free of `Box<dyn Error>`
/// formatting glue.
///
/// # Errors
///
/// Returns the same set of failures as the bulk loader, plus the platform-
/// gate / user-disabled cases that the bulk loader handles silently. The
/// caller doesn't get to differentiate — anything other than success means
/// "this plugin can't be live right now", and the previous instance stays in
/// the registry.
pub async fn load_one_for_reload(plugin_dir: &Path, settings: &Settings) -> Result<LoadOutcome, String> {
    match load_one(plugin_dir, settings).await {
        Ok((plugin, reason)) => Ok(LoadOutcome {
            plugin: Arc::new(plugin),
            reason,
        }),
        Err(LoadError::Skipped { reason, .. }) => Err(reason),
        Err(LoadError::Failed(err)) => Err(err.to_string()),
    }
}

/// Decide whether this load is an `Install`/`Update`/no-change based on the
/// manifest version vs. what we previously recorded for this plugin.
fn detect_reason(manifest_version: Option<&str>, recorded: Option<&str>) -> Option<LifecycleReason> {
    let current = manifest_version?;
    match recorded {
        None => Some(LifecycleReason::Install),
        Some(prev) if prev != current => Some(LifecycleReason::Update),
        Some(_) => None,
    }
}

async fn load_one(plugin_dir: &Path, settings: &Settings) -> Result<(LoadedPlugin, Option<LifecycleReason>), LoadError> {
    let manifest_path = plugin_dir.join("manifest.json");
    let read_path = manifest_path.clone();
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&read_path))
        .await
        .map_err(|join_err| LoadError::Failed(format!("manifest read task panicked: {join_err}").into()))?
        .map_err(|err| LoadError::Failed(format!("read {}: {err}", manifest_path.display()).into()))?;
    // Manifest parse failures are reported to stderr rather than plugin.log
    // because the manifest is the source of the plugin's name — writing into
    // a per-plugin file before the parse succeeds would require inventing one.
    let manifest =
        Manifest::parse(&bytes).map_err(|err| LoadError::Failed(format!("parse manifest.json: {err}").into()))?;

    let log = PluginLog::for_plugin_dir(plugin_dir);

    for warning in manifest.platform_warnings() {
        log.write(LogLevel::Warn, &warning);
    }

    for warning in &manifest.parsed_options().warnings {
        log.write(LogLevel::Warn, warning);
    }

    for cap in &manifest.capabilities {
        if !capability::is_known_cap(cap) {
            let known = capability::known_cap_names();
            log.write(
                LogLevel::Warn,
                &format!("ignoring unknown capability {cap:?} (known: {known:?})"),
            );
            tracing::warn!(
                plugin = %manifest.name,
                capability = %cap,
                ?known,
                "plugins: ignoring unknown capability",
            );
        }
    }

    // Gate before paying the rquickjs Context cost.
    if !manifest.supports_current_platform() {
        let reason = manifest
            .platform_skip_reason()
            .unwrap_or_else(|| "platform gate".to_owned());

        return Err(LoadError::Skipped {
            name: manifest.name,
            reason,
        });
    }

    // User-disabled plugins: same INFO-log shape as the platform gate so the
    // skip path is consistent. Restart-to-apply for v1 — disabling a plugin
    // while the daemon is running has no effect until the next launch.
    if !settings.is_plugin_enabled_or_default(&manifest.name, manifest.default_enabled) {
        return Err(LoadError::Skipped {
            name: manifest.name,
            reason: "disabled in settings".to_owned(),
        });
    }

    // Fold the user's TOML overrides onto the manifest defaults so the
    // runtime sees one ready-to-export bag — keeps the SDK module path free
    // of branching on "did the user set a value?".
    let merged_options = manifest.merged_options(settings.plugin_options(&manifest.name));

    let cache_dir = crate::plugins::runtime::default_cache_dir(&manifest.name);
    let recorded_version = settings.last_loaded_version(&manifest.name).map(str::to_owned);
    let loaded = LoadedPlugin::load_with_log(plugin_dir, manifest, cache_dir, Arc::clone(&log), merged_options)
        .await
        .map_err(|err| {
            log.write(LogLevel::Error, &format!("load failed: {err}"));
            LoadError::Failed(Box::new(err))
        })?;
    let reason = detect_reason(loaded.manifest.version.as_deref(), recorded_version.as_deref());
    Ok((loaded, reason))
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
            .block_on(load_all(&opts, &Settings::default()));
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
        let opts = LoaderOptions::resolve(None);
        assert!(!opts.plugins_dir.as_os_str().is_empty());
    }

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
        let other = if std::env::consts::OS == "macos" {
            "linux"
        } else {
            "macos"
        };
        let wrong = format!(r#"{{ "name": "wrong-os", "entry": "plugin.js", "platforms": ["{other}"] }}"#);
        write_plugin(&root, "wrong-os", &wrong, "export async function* query() {}");
        // And a matching plugin so we can assert the loader still found one.
        let right = format!(
            r#"{{ "name": "right-os", "entry": "plugin.js", "platforms": ["{}"] }}"#,
            std::env::consts::OS,
        );
        write_plugin(&root, "right-os", &right, "export async function* query() {}");

        let opts = LoaderOptions {
            plugins_dir: root.clone(),
        };
        let plugins = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt")
            .block_on(load_all(&opts, &Settings::default()));

        let names: Vec<_> = plugins.iter().map(|p| p.plugin.manifest.name.as_str()).collect();
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
    fn unknown_platform_warning_lands_in_plugin_log() {
        let root = fresh_tmp("unknown-platform-log");
        // Mix an unknown entry with the host OS so the plugin still loads and
        // we have a chance to assert against plugin.log.
        let manifest = format!(
            r#"{{ "name": "mixed", "entry": "plugin.js", "platforms": ["haiku", "{}"] }}"#,
            std::env::consts::OS,
        );
        write_plugin(&root, "mixed", &manifest, "export async function* query() {}");

        let opts = LoaderOptions {
            plugins_dir: root.clone(),
        };
        let plugins = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt")
            .block_on(load_all(&opts, &Settings::default()));
        assert_eq!(plugins.len(), 1, "matching host OS keeps the plugin loaded");

        let log_path = root.join("mixed").join("plugin.log");
        let body =
            std::fs::read_to_string(&log_path).expect("plugin.log should have been created by the warning write");
        assert!(
            body.contains("[WARN ] ignoring unknown platform"),
            "expected warn line, got: {body}",
        );
        assert!(body.contains("haiku"), "warning should name the offender: {body}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn disabled_plugin_is_skipped_by_loader() {
        let root = fresh_tmp("disabled");
        write_plugin(
            &root,
            "echo",
            r#"{ "name": "echo", "entry": "plugin.js" }"#,
            "export async function* query() {}",
        );
        write_plugin(
            &root,
            "keep",
            r#"{ "name": "keep", "entry": "plugin.js" }"#,
            "export async function* query() {}",
        );

        let mut settings = Settings::default();
        settings.set_plugin_enabled("echo", false);

        let opts = LoaderOptions {
            plugins_dir: root.clone(),
        };
        let plugins = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt")
            .block_on(load_all(&opts, &settings));

        let names: Vec<_> = plugins.iter().map(|p| p.plugin.manifest.name.as_str()).collect();
        assert!(
            !names.contains(&"echo"),
            "echo should be skipped when disabled, got {names:?}",
        );
        assert!(names.contains(&"keep"), "keep should still load, got {names:?}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn default_disabled_plugin_is_skipped_without_explicit_settings() {
        // A plugin with `"defaultEnabled": false` must be skipped when the
        // user has no explicit override — mirrors `disabled_plugin_is_skipped_by_loader`.
        let root = fresh_tmp("default-disabled");
        write_plugin(
            &root,
            "vault",
            r#"{ "name": "vault", "entry": "plugin.js", "defaultEnabled": false }"#,
            "export async function* query() {}",
        );
        write_plugin(
            &root,
            "keep",
            r#"{ "name": "keep", "entry": "plugin.js" }"#,
            "export async function* query() {}",
        );

        let opts = LoaderOptions {
            plugins_dir: root.clone(),
        };
        // Use default (empty) Settings — no explicit toggle for "vault".
        let plugins = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt")
            .block_on(load_all(&opts, &Settings::default()));

        let names: Vec<_> = plugins.iter().map(|p| p.plugin.manifest.name.as_str()).collect();
        assert!(
            !names.contains(&"vault"),
            "vault should be skipped (defaultEnabled: false, no user override), got {names:?}",
        );
        assert!(names.contains(&"keep"), "keep should still load, got {names:?}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn default_disabled_plugin_loads_when_user_explicitly_enables() {
        // An explicit `enabled = true` in settings must override `defaultEnabled: false`.
        let root = fresh_tmp("default-disabled-override");
        write_plugin(
            &root,
            "vault",
            r#"{ "name": "vault", "entry": "plugin.js", "defaultEnabled": false }"#,
            "export async function* query() {}",
        );

        let mut settings = Settings::default();
        settings.set_plugin_enabled("vault", true);

        let opts = LoaderOptions {
            plugins_dir: root.clone(),
        };
        let plugins = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt")
            .block_on(load_all(&opts, &settings));

        let names: Vec<_> = plugins.iter().map(|p| p.plugin.manifest.name.as_str()).collect();
        assert!(
            names.contains(&"vault"),
            "vault should load when user explicitly enables it, got {names:?}",
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn detect_reason_install_when_no_recorded_version() {
        assert!(matches!(
            detect_reason(Some("0.1.0"), None),
            Some(LifecycleReason::Install),
        ));
    }

    #[test]
    fn detect_reason_update_on_version_bump() {
        assert!(matches!(
            detect_reason(Some("0.2.0"), Some("0.1.0")),
            Some(LifecycleReason::Update),
        ));
    }

    #[test]
    fn detect_reason_none_on_matching_version() {
        assert!(detect_reason(Some("0.1.0"), Some("0.1.0")).is_none());
    }

    #[test]
    fn detect_reason_none_when_manifest_has_no_version() {
        // Dev plugins without a `version` opt out of lifecycle hooks — we
        // can't tell update from no-op.
        assert!(detect_reason(None, None).is_none());
        assert!(detect_reason(None, Some("0.1.0")).is_none());
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
            .block_on(load_all(&opts, &Settings::default()));

        assert!(plugins.is_empty(), "empty platforms must disable the plugin everywhere");

        let _ = std::fs::remove_dir_all(&root);
    }
}
