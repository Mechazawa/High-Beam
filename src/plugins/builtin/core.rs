//! Core system actions (v1 `CorePlugin.js`) reimplemented as a host built-in.
//!
//! Why not a JS plugin: a malicious or buggy plugin must not be able to power
//! off the user's machine. Shutdown/sleep/restart/lock/quit are spelled out
//! in Rust so they're auditable in one place and can't be redefined at
//! runtime.
//!
//! Surface: keyword prefix-matching, case-insensitive. Scores are scaled by
//! how much of the keyword the user has typed (`query_len / keyword_len`),
//! capped to 100 so it ranks against JS plugins on the same 0..100 scale.

use crate::plugins::dispatch::StreamedResult;
use crate::plugins::result::{Action, PluginResult};

/// The plugin name attributed to every Core result. Stable for the frecency
/// key prefix — pickling "shutdown" once shouldn't lift it forever, but the
/// current frecency model is already keyed on (plugin, key) so this Just Works.
pub const NAME: &str = "core";

/// Version string surfaced in `version v…` results.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Every keyword the Core built-in recognises, paired with the action it
/// produces when selected.
struct Keyword {
    /// What the user types (case-insensitive prefix match).
    label: &'static str,
    /// Optional subtitle to surface on the result row.
    subtitle: Option<&'static str>,
    /// Build the action. Most are pure data; only the version readout needs
    /// runtime composition.
    make_action: fn() -> Action,
}

fn keywords() -> Vec<Keyword> {
    vec![
        Keyword {
            label: "exit High Beam",
            subtitle: Some("quit the launcher daemon"),
            make_action: || Action::Quit,
        },
        Keyword {
            label: "shutdown",
            subtitle: Some("shut down this computer"),
            make_action: shutdown_action,
        },
        Keyword {
            label: "sleep",
            subtitle: Some("put this computer to sleep"),
            make_action: sleep_action,
        },
        Keyword {
            label: "restart",
            subtitle: Some("restart this computer"),
            make_action: restart_action,
        },
        Keyword {
            label: "lock",
            subtitle: Some("lock the screen"),
            make_action: lock_action,
        },
        Keyword {
            label: "check for updates",
            subtitle: Some("not implemented yet"),
            make_action: || Action::Noop,
        },
        // Synthesised at query time so the version label always matches the
        // running binary even when build-time and lookup-time drift.
        Keyword {
            label: "__version_placeholder__",
            subtitle: None,
            make_action: || Action::Noop,
        },
    ]
}

fn shutdown_action() -> Action {
    #[cfg(target_os = "macos")]
    {
        Action::Exec {
            cmd: "/usr/bin/osascript".into(),
            args: vec![
                "-e".into(),
                "tell application \"Finder\" to shut down".into(),
            ],
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Action::Exec {
            cmd: "systemctl".into(),
            args: vec!["poweroff".into()],
        }
    }
}

fn sleep_action() -> Action {
    #[cfg(target_os = "macos")]
    {
        Action::Exec {
            cmd: "/usr/bin/osascript".into(),
            args: vec!["-e".into(), "tell application \"Finder\" to sleep".into()],
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Action::Exec {
            cmd: "systemctl".into(),
            args: vec!["suspend".into()],
        }
    }
}

fn restart_action() -> Action {
    #[cfg(target_os = "macos")]
    {
        Action::Exec {
            cmd: "/usr/bin/osascript".into(),
            args: vec!["-e".into(), "tell application \"Finder\" to restart".into()],
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Action::Exec {
            cmd: "systemctl".into(),
            args: vec!["reboot".into()],
        }
    }
}

fn lock_action() -> Action {
    #[cfg(target_os = "macos")]
    {
        Action::Exec {
            cmd: "/usr/bin/osascript".into(),
            args: vec![
                "-e".into(),
                "tell application \"System Events\" to keystroke \"q\" using {control down, command down}".into(),
            ],
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Action::Exec {
            cmd: "loginctl".into(),
            args: vec!["lock-session".into()],
        }
    }
}

/// Match `query` against every Core keyword and produce matching results.
/// Each match is wrapped in a [`StreamedResult`] so the dispatcher can fold
/// these in alongside JS plugin yields.
#[must_use]
pub(crate) fn query(input: &str) -> Vec<StreamedResult> {
    let q = input.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let q_lower = q.to_lowercase();

    let mut out = Vec::new();
    let version_label = format!("version v{VERSION}");
    for kw in keywords() {
        let (label, action, subtitle) = if kw.label == "__version_placeholder__" {
            (version_label.as_str(), Action::Noop, Some("running build"))
        } else {
            (kw.label, (kw.make_action)(), kw.subtitle)
        };
        if let Some(weight) = match_weight(&q_lower, label) {
            out.push(StreamedResult {
                plugin_name: NAME.to_owned(),
                result: PluginResult {
                    key: label.to_owned(),
                    title: label.to_owned(),
                    subtitle: subtitle.map(str::to_owned),
                    icon: None,
                    weight,
                    pinned: false,
                    action,
                },
            });
        }
    }
    out
}

/// Compute the prefix-match weight for one keyword.
///
/// Returns `None` if `query` is not a prefix of `label` (case-insensitive).
/// Returns `Some(score)` where `score` scales linearly with how much of the
/// keyword has been typed, capped at 100.
fn match_weight(query_lower: &str, label: &str) -> Option<f64> {
    let label_lower = label.to_lowercase();
    if !label_lower.starts_with(query_lower) {
        return None;
    }
    // Keyword labels never approach f64's mantissa precision (52 bits), so
    // the lossy cast is safe in practice.
    #[allow(clippy::cast_precision_loss)]
    let coverage = query_lower.len() as f64 / label.len() as f64;
    Some((coverage * 100.0).min(100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_yields_nothing() {
        assert!(query("").is_empty());
        assert!(query("   ").is_empty());
    }

    #[test]
    fn prefix_match_is_case_insensitive() {
        let results = query("SHUT");
        assert!(results.iter().any(|r| r.result.title == "shutdown"));
    }

    #[test]
    fn exact_match_scores_full() {
        let results = query("shutdown");
        let shutdown = results
            .iter()
            .find(|r| r.result.title == "shutdown")
            .expect("shutdown result");
        assert!((shutdown.result.weight - 100.0).abs() < 1e-6);
    }

    #[test]
    fn partial_match_scores_proportionally() {
        let results = query("sh");
        let shutdown = results
            .iter()
            .find(|r| r.result.title == "shutdown")
            .expect("shutdown result");
        #[allow(clippy::cast_precision_loss)]
        let expected = (2.0 / "shutdown".len() as f64) * 100.0;
        assert!((shutdown.result.weight - expected).abs() < 1e-6);
    }

    #[test]
    fn exit_high_beam_produces_quit_action() {
        let results = query("exit");
        let r = results
            .iter()
            .find(|r| r.result.title == "exit High Beam")
            .expect("exit result");
        assert!(matches!(r.result.action, Action::Quit));
    }

    #[test]
    fn check_for_updates_produces_noop() {
        let results = query("check");
        let r = results
            .iter()
            .find(|r| r.result.title == "check for updates")
            .expect("check for updates result");
        assert!(matches!(r.result.action, Action::Noop));
    }

    #[test]
    fn version_result_includes_pkg_version() {
        let results = query("ver");
        let r = results
            .iter()
            .find(|r| r.result.title.starts_with("version v"))
            .expect("version result");
        assert!(r.result.title.contains(VERSION));
        assert!(matches!(r.result.action, Action::Noop));
    }

    #[test]
    fn non_matching_query_yields_nothing() {
        let results = query("xyzzy");
        assert!(results.is_empty(), "got {results:?}");
    }

    #[test]
    fn shutdown_action_is_exec_on_macos() {
        let results = query("shutdown");
        let r = &results
            .iter()
            .find(|r| r.result.title == "shutdown")
            .expect("shutdown result")
            .result;
        match &r.action {
            Action::Exec { cmd, .. } => {
                #[cfg(target_os = "macos")]
                assert_eq!(cmd, "/usr/bin/osascript");
                #[cfg(not(target_os = "macos"))]
                assert_eq!(cmd, "systemctl");
            }
            other => panic!("expected Exec, got {other:?}"),
        }
    }
}
