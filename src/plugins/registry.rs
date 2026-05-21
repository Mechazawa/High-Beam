//! Live mutable set of loaded plugins.
//!
//! The dispatcher consumes a cheap snapshot of the plugin list; reload paths
//! swap one or all entries under a write lock. Wrapping the list in
//! `Arc<RwLock<Vec<Arc<LoadedPlugin>>>>` lets reload happen from any thread
//! (e.g. a tokio task spawned by an action executor) without coordinating
//! with the dispatcher — the next query rolls onto the new context.
//!
//! Settings (enabled/disabled, options) and frecency are not held here: they
//! live in `settings.toml` and `SQLite` respectively, so they survive a swap
//! for free.

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::plugins::loader::{self, LoaderOptions};
use crate::plugins::runtime::LoadedPlugin;
use crate::settings::Settings;

/// Thread-safe, snapshot-cheap handle to the live plugin set.
///
/// `Clone` is intentional — every consumer (dispatcher, action executor,
/// install/update tasks) holds its own handle into the same shared list.
#[derive(Clone)]
pub struct PluginRegistry {
    inner: Arc<Inner>,
}

struct Inner {
    plugins: RwLock<Vec<Arc<LoadedPlugin>>>,
    options: LoaderOptions,
}

impl PluginRegistry {
    /// Build a registry pre-populated with `plugins`. The caller has already
    /// done the first `loader::load_all` pass; the registry takes ownership
    /// of the result and remembers `options` so subsequent reload paths
    /// look in the same directory.
    #[must_use]
    pub fn new(options: LoaderOptions, plugins: Vec<Arc<LoadedPlugin>>) -> Self {
        Self {
            inner: Arc::new(Inner {
                plugins: RwLock::new(plugins),
                options,
            }),
        }
    }

    /// Cheap clone of the current plugin list.
    ///
    /// Holds the read lock only long enough to clone the `Vec<Arc<_>>`, so
    /// dispatchers never block reload writers for the duration of a query.
    pub async fn snapshot(&self) -> Vec<Arc<LoadedPlugin>> {
        self.inner.plugins.read().await.clone()
    }

    /// The loader options the registry was built with — exposed so install
    /// paths know where to drop new plugin directories.
    #[must_use]
    pub fn options(&self) -> &LoaderOptions {
        &self.inner.options
    }

    /// Re-load every plugin from disk and swap the result in atomically.
    ///
    /// Returns the list of plugin names that ended up loaded — useful for
    /// progress reporting.
    pub async fn reload_all(&self, settings: &Settings) -> Vec<String> {
        let plugins = loader::load_all(&self.inner.options, settings).await;
        let names: Vec<String> = plugins.iter().map(|p| p.manifest.name.clone()).collect();
        *self.inner.plugins.write().await = plugins;
        names
    }

    /// Re-load a single plugin by name (case-insensitive). Other plugins are
    /// left untouched.
    ///
    /// # Errors
    ///
    /// Returns an error if no plugin by that name was loaded (the user can
    /// only reload what's currently live — for a brand-new directory the
    /// `install` flow is the right entry point) or if the new load failed.
    pub async fn reload_one(&self, name: &str, settings: &Settings) -> Result<Arc<LoadedPlugin>, ReloadError> {
        let (idx, plugin_dir) = {
            let guard = self.inner.plugins.read().await;
            let pos = guard
                .iter()
                .position(|p| p.manifest.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| ReloadError::NotFound(name.to_owned()))?;
            (pos, guard[pos].plugin_dir.clone())
        };
        let fresh = loader::load_one_for_reload(&plugin_dir, settings)
            .await
            .map_err(|err| ReloadError::Failed {
                name: name.to_owned(),
                reason: err,
            })?;
        let mut guard = self.inner.plugins.write().await;
        if let Some(slot) = guard.get_mut(idx) {
            *slot = Arc::clone(&fresh);
        } else {
            guard.push(Arc::clone(&fresh));
        }
        Ok(fresh)
    }

    /// Insert a freshly-loaded plugin into the registry, replacing any
    /// existing entry with the same `name`. Used by the install path after
    /// it lays down a new plugin directory and loads it.
    pub async fn install(&self, plugin: Arc<LoadedPlugin>) {
        let name = plugin.manifest.name.clone();
        let mut guard = self.inner.plugins.write().await;
        match guard.iter().position(|p| p.manifest.name == name) {
            Some(idx) => guard[idx] = plugin,
            None => guard.push(plugin),
        }
    }
}

/// Reasons [`PluginRegistry::reload_one`] could fail.
#[derive(Debug)]
pub enum ReloadError {
    /// The registry doesn't currently hold a plugin by that name.
    NotFound(String),
    /// The plugin dir was found but the new load failed (manifest parse,
    /// JS eval, etc.). The previous instance is kept in the registry so the
    /// user isn't left with a hole.
    Failed { name: String, reason: String },
}

impl std::fmt::Display for ReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(name) => {
                write!(f, "no loaded plugin named {name:?}")
            }
            Self::Failed { name, reason } => {
                write!(f, "reload {name:?}: {reason}")
            }
        }
    }
}

impl std::error::Error for ReloadError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn fresh_tmp(tag: &str) -> PathBuf {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let p = std::env::temp_dir().join(format!("high-beam-registry-test-{tag}-{now}"));
        std::fs::create_dir_all(&p).expect("mkdir tmp");
        p
    }

    fn write_plugin(root: &Path, name: &str, manifest: &str, entry: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest).unwrap();
        std::fs::write(dir.join("plugin.js"), entry).unwrap();
    }

    fn empty_registry(opts: LoaderOptions) -> PluginRegistry {
        PluginRegistry::new(opts, Vec::new())
    }

    #[test]
    fn reload_unknown_plugin_returns_not_found() {
        let root = fresh_tmp("reload-unknown");
        let opts = LoaderOptions {
            plugins_dir: root.clone(),
        };
        let reg = empty_registry(opts);
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt")
            .block_on(reg.reload_one("nope", &Settings::default()));
        match result {
            Err(ReloadError::NotFound(name)) => assert_eq!(name, "nope"),
            Err(other) => panic!("expected NotFound, got {other:?}"),
            Ok(_) => panic!("expected NotFound, got Ok"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reload_all_picks_up_new_plugin_on_disk() {
        let root = fresh_tmp("reload-all");
        let opts = LoaderOptions {
            plugins_dir: root.clone(),
        };
        let reg = empty_registry(opts);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");

        // No plugins on disk yet -> reload returns empty list.
        let names = rt.block_on(reg.reload_all(&Settings::default()));
        assert!(names.is_empty());

        // Add a plugin and reload -> registry sees it.
        write_plugin(
            &root,
            "later",
            r#"{ "name": "later", "entry": "plugin.js" }"#,
            "export async function* query() {}",
        );
        let names = rt.block_on(reg.reload_all(&Settings::default()));
        assert_eq!(names, vec!["later".to_owned()]);
        let snap = rt.block_on(reg.snapshot());
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].manifest.name, "later");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reload_one_swaps_only_targeted_plugin() {
        let root = fresh_tmp("reload-one");
        let opts = LoaderOptions {
            plugins_dir: root.clone(),
        };
        write_plugin(
            &root,
            "alpha",
            r#"{ "name": "alpha", "entry": "plugin.js", "version": "0.1.0" }"#,
            "export async function* query(input) { yield { key: 'a', title: 'v1', action: { kind: 'noop' } }; }",
        );
        write_plugin(
            &root,
            "beta",
            r#"{ "name": "beta", "entry": "plugin.js", "version": "0.1.0" }"#,
            "export async function* query() {}",
        );
        let reg = empty_registry(opts);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        let _ = rt.block_on(reg.reload_all(&Settings::default()));

        let beta_before = rt
            .block_on(reg.snapshot())
            .iter()
            .find(|p| p.manifest.name == "beta")
            .map(Arc::clone)
            .expect("beta loaded");

        // Bump alpha's version on disk and reload only alpha.
        std::fs::write(
            root.join("alpha/manifest.json"),
            r#"{ "name": "alpha", "entry": "plugin.js", "version": "0.2.0" }"#,
        )
        .unwrap();
        rt.block_on(reg.reload_one("ALPHA", &Settings::default()))
            .expect("reload alpha");

        let snap = rt.block_on(reg.snapshot());
        let alpha = snap
            .iter()
            .find(|p| p.manifest.name == "alpha")
            .expect("alpha still present");
        assert_eq!(alpha.manifest.version.as_deref(), Some("0.2.0"));

        // The beta Arc didn't get replaced — pointer equality holds.
        let beta_after = snap
            .iter()
            .find(|p| p.manifest.name == "beta")
            .map(Arc::clone)
            .expect("beta still present");
        assert!(
            Arc::ptr_eq(&beta_before, &beta_after),
            "reload_one must not touch other plugins"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
