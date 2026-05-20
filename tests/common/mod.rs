// Integration-test harness for plugin authors and stage agents.
//
// Load a plugin from disk, run `query()` against canned inputs, and assert on
// the yielded results — no manual visual verification required.
//
// Usage:
//   let tester = PluginTester::load("examples/plugins/echo").await.unwrap();
//   let results = tester.query("hello").await.unwrap();
//   assert_eq!(results[0].title, "echo: hello");
//   assert_eq!(results[0].action.as_copy().unwrap(), "hello");
//
// TODO: stage 8 — add SDK stubs here when a plugin needs fs/http/clipboard live
// (the Stage 8 agent extends this file; echo only needs `actions.copy` which is
// data-only and needs no stub).

use std::path::Path;
use std::time::Duration;

use high_beam::plugins::manifest::Manifest;
use high_beam::plugins::result::{Action, PluginResult};
use high_beam::plugins::runtime::{LoadedPlugin, PluginError};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

/// Default per-query wall-clock budget.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// A loaded plugin ready to handle test queries.
pub struct PluginTester {
    plugin: LoadedPlugin,
    timeout: Duration,
}

impl PluginTester {
    /// Load a plugin by directory path, bypassing the `plugins/` directory scan.
    ///
    /// # Errors
    ///
    /// Propagates [`PluginError`] from manifest parsing or JS evaluation.
    pub async fn load(dir: impl AsRef<Path>) -> Result<Self, PluginError> {
        Self::load_with_timeout(dir, DEFAULT_TIMEOUT).await
    }

    /// Load a plugin with a custom per-query timeout.
    ///
    /// # Errors
    ///
    /// Propagates [`PluginError`] from manifest parsing or JS evaluation.
    pub async fn load_with_timeout(
        dir: impl AsRef<Path>,
        timeout: Duration,
    ) -> Result<Self, PluginError> {
        let dir = dir.as_ref();
        let bytes = std::fs::read(dir.join("manifest.json"))?;
        let mut manifest = Manifest::parse(&bytes)
            .map_err(|err| PluginError::Js(format!("parse manifest.json: {err}")))?;
        // Override manifest timeout with the test timeout so the caller controls
        // the wall-clock budget without needing to edit the fixture file.
        manifest.timeout_ms = timeout.as_millis().try_into().unwrap_or(u64::MAX);
        let plugin = LoadedPlugin::load(dir, manifest).await?;
        Ok(Self { plugin, timeout })
    }

    /// Run `query(input)` and collect all yielded results.
    ///
    /// Returns `Ok(Vec<TestResult>)` once the plugin's iterator drains or the
    /// timeout fires. The timeout is the value set at load time (default 5s).
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Timeout`] if the plugin stalls, or
    /// [`PluginError::Js`] if the plugin throws.
    pub async fn query(&self, input: &str) -> Result<Vec<TestResult>, PluginError> {
        let cancel = CancellationToken::new();
        let mut rx: UnboundedReceiver<PluginResult> =
            self.plugin.run_query_stream(input, cancel.clone());

        // Outer timeout guards against a plugin that yields forever and never closes.
        // The inner runtime timeout (set in `load_with_timeout`) stops a stalled
        // iterator step; this outer guard catches a fast-but-infinite generator.
        let guard = self.timeout + Duration::from_millis(500);
        let collect = async {
            let mut out = Vec::new();
            while let Some(r) = rx.recv().await {
                out.push(TestResult::from(r));
            }
            out
        };

        tokio::time::timeout(guard, collect)
            .await
            .map_err(|_| PluginError::Timeout)
    }
}

/// A single result row, mirroring [`PluginResult`] with ergonomic action helpers.
#[derive(Debug, Clone)]
pub struct TestResult {
    pub key: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub weight: f64,
    pub pinned: bool,
    pub action: Action,
}

impl TestResult {
    /// Return the copy text if the action is `copy`, otherwise `None`.
    #[must_use]
    pub fn as_copy(&self) -> Option<&str> {
        match &self.action {
            Action::Copy { text } => Some(text),
            _ => None,
        }
    }

    /// Return the URL if the action is `openUrl`, otherwise `None`.
    #[must_use]
    pub fn as_open_url(&self) -> Option<&str> {
        match &self.action {
            Action::OpenUrl { url } => Some(url),
            _ => None,
        }
    }

    /// Return `(cmd, args)` if the action is `exec`, otherwise `None`.
    #[must_use]
    pub fn as_exec(&self) -> Option<(&str, &[String])> {
        match &self.action {
            Action::Exec { cmd, args } => Some((cmd, args)),
            _ => None,
        }
    }

    /// Return the path string if the action is `reveal`, otherwise `None`.
    #[must_use]
    pub fn as_reveal(&self) -> Option<&std::path::Path> {
        match &self.action {
            Action::Reveal { path } => Some(path),
            _ => None,
        }
    }
}

impl From<PluginResult> for TestResult {
    fn from(r: PluginResult) -> Self {
        Self {
            key: r.key,
            title: r.title,
            subtitle: r.subtitle,
            weight: r.weight,
            pinned: r.pinned,
            action: r.action,
        }
    }
}
