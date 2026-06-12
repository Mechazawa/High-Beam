//! Core launcher verbs: quit, settings, plugin install/reload/update, version
//! readout.
//!
//! Keyword prefix-match, case-insensitive; score scales by typed fraction
//! (`query_len / keyword_len`), capped to 100.

use std::sync::LazyLock;

use crate::plugins::dispatch::StreamedResult;
use crate::plugins::result::{Action, PluginResult};

pub const NAME: &str = "core";

const VERSION: &str = env!("CARGO_PKG_VERSION");

const INSTALL_VERB: &str = "install";
const RELOAD_VERB: &str = "reload";
/// Verb the user types to check every loaded plugin against its remote
/// manifest and re-install any with a newer version.
const UPDATE_VERB: &str = "update";

struct Keyword {
    label: &'static str,
    subtitle: Option<&'static str>,
    make_action: fn() -> Action,
}

static KEYWORDS: LazyLock<Vec<Keyword>> = LazyLock::new(|| {
    vec![
        Keyword {
            label: "exit High Beam",
            subtitle: Some("quit the launcher daemon"),
            make_action: || Action::Quit,
        },
        Keyword {
            label: "settings",
            subtitle: Some("open the High Beam settings screen"),
            make_action: || Action::OpenSettings,
        },
        Keyword {
            label: "check for updates",
            subtitle: Some("not implemented yet"),
            make_action: || Action::Noop,
        },
        // Label synthesised at query time so it always reflects the running
        // binary's version.
        Keyword {
            label: "__version_placeholder__",
            subtitle: None,
            make_action: || Action::Noop,
        },
    ]
});

#[must_use]
pub(crate) fn query(input: &str, plugin_names: &[&str]) -> Vec<StreamedResult> {
    let q = input.trim();

    if q.is_empty() {
        return Vec::new();
    }
    let q_lower = q.to_lowercase();

    let mut out = Vec::new();
    let version_label = format!("version v{VERSION}");

    for kw in KEYWORDS.iter() {
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
                    alt_action: None,
                    alt_title: None,
                    alt_subtitle: None,
                },
            });
        }
    }

    out.extend(install_rows(&q_lower, q));
    out.extend(reload_rows(&q_lower, q, plugin_names));
    out.extend(update_rows(&q_lower));
    out
}

/// `install <url>` — one row, only when the verb prefix matches and an
/// argument is present. The row's action carries the verbatim URL; the
/// installer validates scheme/format later.
fn install_rows(q_lower: &str, q: &str) -> Vec<StreamedResult> {
    let Some(rest) = strip_verb(q, INSTALL_VERB) else {
        return verb_only_row(q_lower, INSTALL_VERB, "type a manifest URL after `install`");
    };
    let trimmed = rest.trim();

    if trimmed.is_empty() {
        return verb_only_row(q_lower, INSTALL_VERB, "type a manifest URL after `install`");
    }
    vec![row(
        "install".to_owned(),
        format!("install {trimmed}"),
        Some("download + install plugin"),
        100.0,
        Action::InstallPlugin {
            url: trimmed.to_owned(),
        },
    )]
}

/// `reload` (no arg) → "Reload all plugins". `reload <prefix>` → one row per
/// loaded plugin whose name starts with the prefix. `reload` with an empty
/// list of plugins still shows the "all plugins" affordance so the user can
/// reload at any time.
fn reload_rows(q_lower: &str, q: &str, plugin_names: &[&str]) -> Vec<StreamedResult> {
    let Some(rest) = strip_verb(q, RELOAD_VERB) else {
        return verb_only_row(
            q_lower,
            RELOAD_VERB,
            "reload one plugin (type a name after `reload`) or all",
        );
    };
    let trimmed = rest.trim();
    let mut rows = vec![row(
        "reload-all".to_owned(),
        "Reload all plugins".to_owned(),
        Some("re-scan the plugin directory + re-evaluate every plugin"),
        100.0,
        Action::ReloadPlugin { name: None },
    )];
    let target = trimmed.to_ascii_lowercase();

    for name in plugin_names {
        if !target.is_empty() && !name.to_ascii_lowercase().starts_with(&target) {
            continue;
        }
        rows.push(row(
            format!("reload-{name}"),
            format!("Reload {name}"),
            Some("re-evaluate this plugin in place"),
            90.0,
            Action::ReloadPlugin {
                name: Some((*name).to_owned()),
            },
        ));
    }
    rows
}

/// `update` — single row; pressing Enter iterates every plugin with a
/// `manifestUrl` and reports per-plugin progress.
fn update_rows(q_lower: &str) -> Vec<StreamedResult> {
    if !UPDATE_VERB.starts_with(q_lower) {
        return Vec::new();
    }
    #[allow(clippy::cast_precision_loss)]
    let weight = ((q_lower.len() as f64) / UPDATE_VERB.len() as f64 * 100.0).min(100.0);
    vec![row(
        "update".to_owned(),
        "Update plugins".to_owned(),
        Some("check every plugin's manifestUrl and install any newer version"),
        weight,
        Action::UpdatePlugins,
    )]
}

/// Strip a leading verb token off the trimmed input. Returns the remainder
/// (including the leading separator whitespace) iff the verb matched at the
/// start, case-insensitive.
fn strip_verb<'a>(input: &'a str, verb: &str) -> Option<&'a str> {
    let lower = input.to_ascii_lowercase();

    if lower == verb {
        return Some("");
    }

    if let Some(rest) = lower.strip_prefix(verb)
        && rest.starts_with(char::is_whitespace)
    {
        // Index off the original to preserve the user's casing in `rest`.
        return Some(&input[verb.len()..]);
    }
    None
}

/// Row shown for a verb the user is mid-typing — the action is `Noop` so
/// pressing Enter on the placeholder does nothing destructive.
fn verb_only_row(q_lower: &str, verb: &str, subtitle: &str) -> Vec<StreamedResult> {
    let Some(weight) = match_weight(q_lower, verb) else {
        return Vec::new();
    };
    vec![row(
        verb.to_owned(),
        verb.to_owned(),
        Some(subtitle),
        weight,
        Action::Noop,
    )]
}

fn row(key: String, title: String, subtitle: Option<&str>, weight: f64, action: Action) -> StreamedResult {
    StreamedResult {
        plugin_name: NAME.to_owned(),
        result: PluginResult {
            key,
            title,
            subtitle: subtitle.map(str::to_owned),
            icon: None,
            weight,
            pinned: false,
            action,
            alt_action: None,
            alt_title: None,
            alt_subtitle: None,
        },
    }
}

fn match_weight(query_lower: &str, label: &str) -> Option<f64> {
    let label_lower = label.to_lowercase();

    if !label_lower.starts_with(query_lower) {
        return None;
    }
    // Keyword labels are tiny strings — well under f64 mantissa precision.
    #[allow(clippy::cast_precision_loss)]
    let coverage = query_lower.len() as f64 / label.len() as f64;
    Some((coverage * 100.0).min(100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Most existing test cases don't care about the plugin list; this
    /// shim keeps them call-site clean.
    fn query(input: &str) -> Vec<StreamedResult> {
        super::query(input, &[])
    }

    #[test]
    fn empty_query_yields_nothing() {
        assert!(query("").is_empty());
        assert!(query("   ").is_empty());
    }

    #[test]
    fn prefix_match_is_case_insensitive() {
        let results = query("SET");
        assert!(results.iter().any(|r| r.result.title == "settings"));
    }

    #[test]
    fn exact_match_scores_full() {
        let results = query("settings");
        let settings = results
            .iter()
            .find(|r| r.result.title == "settings")
            .expect("settings result");
        assert!((settings.result.weight - 100.0).abs() < 1e-6);
    }

    #[test]
    fn partial_match_scores_proportionally() {
        let results = query("se");
        let settings = results
            .iter()
            .find(|r| r.result.title == "settings")
            .expect("settings result");
        #[allow(clippy::cast_precision_loss)]
        let expected = (2.0 / "settings".len() as f64) * 100.0;
        assert!((settings.result.weight - expected).abs() < 1e-6);
    }

    #[test]
    fn settings_verb_produces_open_settings_action() {
        let results = query("settings");
        let r = results
            .iter()
            .find(|r| r.result.title == "settings")
            .expect("settings result");
        assert!(matches!(r.result.action, Action::OpenSettings));
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
    fn core_verbs_are_unpinned() {
        // Core results never pin; frecency ranks them like any other result.
        for q in ["settings", "exit", "check for updates"] {
            let results = query(q);
            assert!(results.iter().any(|r| !r.result.pinned), "expected a result for {q}");
            assert!(
                results.iter().all(|r| !r.result.pinned),
                "no core result should be pinned ({q})"
            );
        }
    }

    #[test]
    fn power_verbs_not_owned_by_core() {
        // Power verbs belong to the `system` plugin, not core.
        for q in ["shutdown", "reboot", "eject", "log out"] {
            assert!(
                query(q).iter().all(|r| r.result.title != q),
                "core must not own the {q:?} verb"
            );
        }
    }

    #[test]
    fn reload_with_no_arg_offers_all_plugins() {
        let results = super::query("reload", &["alpha", "beta"]);
        let titles: Vec<_> = results.iter().map(|r| r.result.title.clone()).collect();
        assert!(titles.contains(&"Reload all plugins".to_owned()));
        assert!(titles.contains(&"Reload alpha".to_owned()));
        assert!(titles.contains(&"Reload beta".to_owned()));
    }

    #[test]
    fn reload_with_prefix_filters_plugin_list() {
        let results = super::query("reload al", &["alpha", "beta"]);
        let titles: Vec<_> = results.iter().map(|r| r.result.title.clone()).collect();
        // "Reload all plugins" always shows; "Reload alpha" matches the
        // prefix; "Reload beta" does not.
        assert!(titles.contains(&"Reload all plugins".to_owned()));
        assert!(titles.contains(&"Reload alpha".to_owned()));
        assert!(!titles.contains(&"Reload beta".to_owned()));
    }

    #[test]
    fn reload_single_carries_plugin_name_in_action() {
        let results = super::query("reload echo", &["echo"]);
        let echo = results
            .iter()
            .find(|r| r.result.title == "Reload echo")
            .expect("Reload echo row");

        match &echo.result.action {
            Action::ReloadPlugin { name } => assert_eq!(name.as_deref(), Some("echo")),
            other => panic!("expected ReloadPlugin, got {other:?}"),
        }
    }

    #[test]
    fn reload_all_action_has_none_name() {
        let results = super::query("reload", &[]);
        let all = results
            .iter()
            .find(|r| r.result.title == "Reload all plugins")
            .expect("Reload all row");
        assert!(matches!(all.result.action, Action::ReloadPlugin { name: None }));
    }

    #[test]
    fn install_with_url_produces_install_action() {
        let results = super::query("install https://example.com/p/manifest.json", &[]);
        let row = results
            .iter()
            .find(|r| r.result.title.starts_with("install https://"))
            .expect("install row");

        match &row.result.action {
            Action::InstallPlugin { url } => {
                assert_eq!(url, "https://example.com/p/manifest.json");
            }
            other => panic!("expected InstallPlugin, got {other:?}"),
        }
    }

    #[test]
    fn install_without_url_only_shows_hint_row() {
        let results = super::query("install", &[]);
        // No InstallPlugin action emitted when the URL is missing.
        assert!(
            !results
                .iter()
                .any(|r| matches!(r.result.action, Action::InstallPlugin { .. })),
            "install without URL must not produce an InstallPlugin action",
        );
        // The hint row sits there with a Noop so accidental Enter is harmless.
        let hint = results
            .iter()
            .find(|r| r.result.title == "install")
            .expect("install hint row");
        assert!(matches!(hint.result.action, Action::Noop));
    }

    #[test]
    fn update_verb_produces_update_action() {
        let results = super::query("update", &[]);
        let row = results
            .iter()
            .find(|r| r.result.title == "Update plugins")
            .expect("update row");
        assert!(matches!(row.result.action, Action::UpdatePlugins));
    }
}
