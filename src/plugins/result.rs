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
    /// Optional alternate action invoked when the user holds the modifier
    /// configured in settings (default Alt) while pressing Enter or
    /// clicking. `None` ⇒ the primary `action` is used regardless of
    /// modifier state.
    #[serde(default, rename = "altAction")]
    pub alt_action: Option<Action>,
    /// Optional title to render in place of `title` while the alt-action
    /// modifier is held. `None` ⇒ the primary title stays put.
    #[serde(default, rename = "altTitle")]
    pub alt_title: Option<String>,
    /// Optional subtitle to render in place of `subtitle` while the
    /// alt-action modifier is held. Use this to surface the alt action
    /// to the user without inventing a separate row.
    #[serde(default, rename = "altSubtitle")]
    pub alt_subtitle: Option<String>,
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
    /// Push a view frame for the producing plugin. `handle` is opaque to the
    /// host — the SDK mints it per plugin context and looks it up via
    /// `__highbeam_view_registry` on `globalThis` when the host asks for a
    /// render. `props` is whatever the caller passed; functions inside are
    /// silently dropped by serde for v1 (the closure-prop pattern lands with
    /// the reactivity runtime). `reset = true` clears the stack before pushing
    /// so the new frame becomes the only frame.
    #[serde(rename = "showView")]
    ShowView {
        handle: u64,
        #[serde(default)]
        props: serde_json::Value,
        #[serde(default)]
        reset: bool,
    },
    /// Pop the top view frame. `render → null` from inside a view does the
    /// same thing.
    #[serde(rename = "closeView")]
    CloseView,
}

impl Action {
    /// The wire `kind` when this variant is host-only (Core built-in only),
    /// `None` for plugin-legal variants. The serde derive happily parses
    /// every variant from plugin JSON, so the boundaries that accept
    /// plugin-supplied actions (`stream_query`, view dispatch) reject these
    /// explicitly — otherwise any plugin could yield `{"kind":"quit"}` and
    /// kill the daemon on Enter.
    #[must_use]
    pub fn host_only_kind(&self) -> Option<&'static str> {
        match self {
            Self::Quit => Some("quit"),
            Self::OpenSettings => Some("openSettings"),
            Self::ReloadPlugin { .. } => Some("reloadPlugin"),
            Self::InstallPlugin { .. } => Some("installPlugin"),
            Self::UpdatePlugins => Some("updatePlugins"),
            Self::OpenUrl { .. }
            | Self::Copy { .. }
            | Self::Exec { .. }
            | Self::Reveal { .. }
            | Self::Noop
            | Self::ShowView { .. }
            | Self::CloseView => None,
        }
    }
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
    /// `<plugin>:<result_key>` — used by the UI as a row id. Hot enough
    /// (one call per visible row per render) that we hand-roll the
    /// concatenation with an exact-capacity allocation instead of going
    /// through `format!`'s formatter machinery.
    #[must_use]
    pub fn composite_key(&self) -> String {
        let mut out = String::with_capacity(self.plugin_name.len() + 1 + self.result.key.len());
        out.push_str(&self.plugin_name);
        out.push(':');
        out.push_str(&self.result.key);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_exec_args_default_to_empty() {
        // `args` is `#[serde(default)]` so a builder that omits the array
        // round-trips to an empty Vec. Guards against the default being
        // accidentally dropped.
        let json = r#"{"kind":"exec","cmd":"/usr/bin/true"}"#;
        let parsed: Action = serde_json::from_str(json).unwrap();

        match parsed {
            Action::Exec { args, .. } => assert!(args.is_empty()),
            other => panic!("expected Exec, got {other:?}"),
        }
    }

    #[test]
    fn result_parses_minimal_shape() {
        // The required-fields contract: key, title, action. Everything else
        // must default sanely so a minimal yielded object lands without
        // forcing plugin authors to spell out every property.
        let json = r#"{"key":"k","title":"t","action":{"kind":"copy","text":"x"}}"#;
        let parsed: PluginResult = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.key, "k");
        assert_eq!(parsed.title, "t");
        assert!(parsed.subtitle.is_none());
        assert!(!parsed.pinned);
        assert!((parsed.weight - 0.0).abs() < f64::EPSILON);
        // Absent `altAction` parses as `None` so the dispatch path can
        // fall back to `action` unconditionally.
        assert!(parsed.alt_action.is_none());
    }

    #[test]
    fn result_parses_alt_action_when_present() {
        let json = r#"{
            "key":"k",
            "title":"t",
            "action":{"kind":"openUrl","url":"https://primary.example"},
            "altAction":{"kind":"openUrl","url":"https://explain.example"}
        }"#;
        let parsed: PluginResult = serde_json::from_str(json).unwrap();
        match parsed.alt_action {
            Some(Action::OpenUrl { url }) => assert_eq!(url, "https://explain.example"),
            other => panic!("expected Some(OpenUrl), got {other:?}"),
        }
    }

    #[test]
    fn show_view_round_trips_with_handle_props_and_reset() {
        let json = r#"{"kind":"showView","handle":42,"props":{"id":1},"reset":true}"#;
        let parsed: Action = serde_json::from_str(json).unwrap();

        match parsed {
            Action::ShowView { handle, props, reset } => {
                assert_eq!(handle, 42);
                assert_eq!(props, serde_json::json!({ "id": 1 }));
                assert!(reset);
            }
            other => panic!("expected ShowView, got {other:?}"),
        }
    }

    #[test]
    fn show_view_defaults_props_and_reset_when_omitted() {
        let json = r#"{"kind":"showView","handle":7}"#;
        let parsed: Action = serde_json::from_str(json).unwrap();

        match parsed {
            Action::ShowView { handle, props, reset } => {
                assert_eq!(handle, 7);
                assert_eq!(props, serde_json::Value::Null);
                assert!(!reset);
            }
            other => panic!("expected ShowView, got {other:?}"),
        }
    }

    #[test]
    fn close_view_round_trips() {
        let json = r#"{"kind":"closeView"}"#;
        let parsed: Action = serde_json::from_str(json).unwrap();

        assert!(matches!(parsed, Action::CloseView));
    }
}
