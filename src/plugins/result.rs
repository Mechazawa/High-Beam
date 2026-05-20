//! Result + Action types — the cross-plugin schema rendered in the window.
//!
//! Stage 4 supports the full v1 action set: `openUrl`, `copy`, `exec`,
//! `reveal`. (Stage 7 may add `push` for nested detail views, post-v1.)
//!
//! Icons are still unimplemented (Stage 7 wires up native icons).
//!
//! JS plugins yield objects that the host parses via `serde` into [`Result`].
//! The wire shape mirrors the doc'd `Action` tagged-union (`kind` discriminator)
//! so the SDK module exports in `crate::sdk::actions` and the Rust enum stay
//! aligned without bespoke (de)serialization code.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One result row.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginResult {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
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
}

/// A result enriched with the plugin name that produced it.
///
/// We carry the plugin name through the dispatch pipeline so future stages
/// can key frecency / per-plugin logging on it. Stage 3 only uses it for the
/// row `key` so different plugins emitting the same `key` string don't
/// collide.
#[derive(Debug, Clone)]
pub struct RankedResult {
    pub plugin_name: String,
    pub result: PluginResult,
    /// Insertion order across the merge, used to break ties stably.
    pub order: usize,
}

impl RankedResult {
    /// Composite key: `<plugin>:<result_key>`. Used by the UI as a row id.
    #[must_use]
    pub fn composite_key(&self) -> String {
        format!("{}:{}", self.plugin_name, self.result.key)
    }
}

/// Merge results from multiple plugins into a single ordered list.
///
/// Stage 3 ranking rules:
///   1. pinned-first
///   2. then by descending `weight`
///   3. then by insertion order (stable)
///
/// Stage 5 will replace step 2 with a frecency-aware score.
#[must_use]
pub fn sort_merged(mut all: Vec<RankedResult>) -> Vec<RankedResult> {
    all.sort_by(|a, b| {
        b.result
            .pinned
            .cmp(&a.result.pinned)
            .then_with(|| {
                b.result
                    .weight
                    .partial_cmp(&a.result.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.order.cmp(&b.order))
    });
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rr(name: &str, key: &str, weight: f64, pinned: bool, order: usize) -> RankedResult {
        RankedResult {
            plugin_name: name.to_owned(),
            result: PluginResult {
                key: key.to_owned(),
                title: key.to_owned(),
                subtitle: None,
                weight,
                pinned,
                action: Action::Copy {
                    text: key.to_owned(),
                },
            },
            order,
        }
    }

    #[test]
    fn pinned_sorts_above_unpinned_regardless_of_weight() {
        let merged = sort_merged(vec![
            rr("a", "high", 100.0, false, 0),
            rr("a", "low-pinned", 0.0, true, 1),
        ]);
        assert_eq!(merged[0].result.key, "low-pinned");
        assert_eq!(merged[1].result.key, "high");
    }

    #[test]
    fn unpinned_sorts_by_descending_weight() {
        let merged = sort_merged(vec![
            rr("a", "low", 1.0, false, 0),
            rr("a", "high", 10.0, false, 1),
            rr("a", "mid", 5.0, false, 2),
        ]);
        let keys: Vec<_> = merged.iter().map(|r| r.result.key.clone()).collect();
        assert_eq!(keys, ["high", "mid", "low"]);
    }

    #[test]
    fn equal_weight_ties_break_by_insertion_order() {
        let merged = sort_merged(vec![
            rr("a", "second", 5.0, false, 1),
            rr("a", "first", 5.0, false, 0),
            rr("a", "third", 5.0, false, 2),
        ]);
        let keys: Vec<_> = merged.iter().map(|r| r.result.key.clone()).collect();
        assert_eq!(keys, ["first", "second", "third"]);
    }

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
