//! Result + Action types — the cross-plugin schema rendered in the window.
//!
//! JS plugins yield objects that the host parses via `serde` into
//! [`PluginResult`]. The `Action` wire shape uses a `kind` discriminator so
//! the Rust enum and the SDK `highbeam:actions` builders stay aligned without
//! bespoke (de)serialization.
//!
//! `Quit` is host-only — only the Core built-in produces it; the
//! `highbeam:actions` module never exposes a builder for it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One result row.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginResult {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    /// Optional icon — `data:image/...;base64,...` URI. Bare filesystem paths
    /// are treated as missing; the plugin is expected to pre-resolve via
    /// `highbeam:icons.forPath(...)`.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub weight: f64,
    #[serde(default)]
    pub pinned: bool,
    pub action: Action,
}

/// Variants of [`Action`] the host knows how to execute.
///
/// Wire shape (set by [`crate::sdk::actions`]):
/// ```json
/// { "kind": "openUrl", "url": "https://…" }
/// { "kind": "copy",    "text": "hello" }
/// { "kind": "exec",    "cmd": "/usr/bin/say", "args": ["hello"] }
/// { "kind": "reveal",  "path": "/Users/me/file.pdf" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Action {
    #[serde(rename = "openUrl")]
    OpenUrl { url: String },
    #[serde(rename = "copy")]
    Copy { text: String },
    #[serde(rename = "exec")]
    Exec {
        cmd: String,
        #[serde(default)]
        args: Vec<String>,
    },
    #[serde(rename = "reveal")]
    Reveal { path: PathBuf },
    /// Quit the High Beam daemon. Host-only: never serialised over the JS
    /// boundary, only produced by built-in plugins.
    #[serde(rename = "quit")]
    Quit,
    /// Open the settings view in place of the launcher view. Host-only —
    /// produced by the Core `settings` verb; the JS `highbeam:actions`
    /// module deliberately doesn't expose a builder for it.
    #[serde(rename = "openSettings")]
    OpenSettings,
    /// Reload a plugin (or all plugins) without restarting the daemon.
    /// Host-only — produced by the Core `reload` verb.
    #[serde(rename = "reloadPlugin")]
    ReloadPlugin {
        /// `None` ⇒ reload every plugin; `Some(name)` ⇒ reload just that one.
        #[serde(default)]
        name: Option<String>,
    },
    /// Install a plugin from a manifest URL. Host-only — produced by the
    /// Core `install <url>` verb.
    #[serde(rename = "installPlugin")]
    InstallPlugin { url: String },
    /// Check every loaded plugin with a `manifestUrl` against its remote
    /// counterpart and install any with a strictly higher version. Host-only
    /// — produced by the Core `update` verb.
    #[serde(rename = "updatePlugins")]
    UpdatePlugins,
    /// A no-op result that just sits in the list (e.g. version readout).
    #[serde(rename = "noop")]
    Noop,
}

/// A result enriched with the plugin name that produced it. Two plugins
/// emitting the same `key` string don't collide because frecency is keyed on
/// `(plugin_name, key)`.
#[derive(Debug, Clone)]
pub struct RankedResult {
    pub plugin_name: String,
    pub result: PluginResult,
    /// Insertion order, used to break ties stably.
    pub order: usize,
}

impl RankedResult {
    /// `<plugin>:<result_key>` — used by the UI as a row id.
    #[must_use]
    pub fn composite_key(&self) -> String {
        format!("{}:{}", self.plugin_name, self.result.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_roundtrip_open_url() {
        let action = Action::OpenUrl {
            url: "https://example.com".into(),
        };
        let s = serde_json::to_string(&action).unwrap();
        let parsed: Action = serde_json::from_str(&s).unwrap();
        assert!(matches!(parsed, Action::OpenUrl { url } if url == "https://example.com"));
    }

    #[test]
    fn action_roundtrip_copy() {
        let json = r#"{"kind":"copy","text":"hello"}"#;
        let parsed: Action = serde_json::from_str(json).unwrap();
        assert!(matches!(parsed, Action::Copy { text } if text == "hello"));
    }

    #[test]
    fn action_roundtrip_exec() {
        let json = r#"{"kind":"exec","cmd":"/usr/bin/say","args":["hello","world"]}"#;
        let parsed: Action = serde_json::from_str(json).unwrap();
        match parsed {
            Action::Exec { cmd, args } => {
                assert_eq!(cmd, "/usr/bin/say");
                assert_eq!(args, vec!["hello".to_owned(), "world".to_owned()]);
            }
            other => panic!("expected Exec, got {other:?}"),
        }
    }

    #[test]
    fn action_exec_args_default_to_empty() {
        let json = r#"{"kind":"exec","cmd":"/usr/bin/true"}"#;
        let parsed: Action = serde_json::from_str(json).unwrap();
        match parsed {
            Action::Exec { args, .. } => assert!(args.is_empty()),
            other => panic!("expected Exec, got {other:?}"),
        }
    }

    #[test]
    fn action_roundtrip_reveal() {
        let json = r#"{"kind":"reveal","path":"/tmp/file.pdf"}"#;
        let parsed: Action = serde_json::from_str(json).unwrap();
        match parsed {
            Action::Reveal { path } => assert_eq!(path, PathBuf::from("/tmp/file.pdf")),
            other => panic!("expected Reveal, got {other:?}"),
        }
    }

    #[test]
    fn result_parses_minimal_shape() {
        let json = r#"{"key":"k","title":"t","action":{"kind":"copy","text":"x"}}"#;
        let parsed: PluginResult = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.key, "k");
        assert_eq!(parsed.title, "t");
        assert!(parsed.subtitle.is_none());
        assert!(!parsed.pinned);
        assert!((parsed.weight - 0.0).abs() < f64::EPSILON);
    }
}
