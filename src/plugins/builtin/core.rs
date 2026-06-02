//! Core system actions, implemented in Rust so a buggy plugin can't power
//! off the machine.
//!
//! Keyword prefix-match, case-insensitive. Score scales by how much of the
//! keyword has been typed (`query_len / keyword_len`), capped to 100.

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
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut kws = vec![
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
        // Alias for restart — same underlying action, different muscle memory.
        Keyword {
            label: "reboot",
            subtitle: Some("restart this computer"),
            make_action: restart_action,
        },
        Keyword {
            label: "lock",
            subtitle: Some("lock the screen"),
            make_action: lock_action,
        },
        Keyword {
            label: "log out",
            subtitle: Some("end this user session"),
            make_action: logout_action,
        },
        Keyword {
            label: "screensaver",
            subtitle: Some("start the screensaver"),
            make_action: screensaver_action,
        },
        Keyword {
            label: "display sleep",
            subtitle: Some("turn the display off without sleeping the machine"),
            make_action: display_sleep_action,
        },
        Keyword {
            label: "empty trash",
            subtitle: Some("permanently delete trashed files"),
            make_action: empty_trash_action,
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
    ];
    // `eject` only ships on macOS — `gio mount --eject` / `udisksctl` would
    // need a target device, so a bare verb makes no sense on Linux. The
    // `#[cfg]` gate keeps `eject_action` from being referenced at all on
    // non-macOS builds, where the function is not compiled.
    #[cfg(target_os = "macos")]
    kws.push(Keyword {
        label: "eject",
        subtitle: Some("eject all ejectable disks"),
        make_action: eject_action,
    });
    kws
});

fn shutdown_action() -> Action {
    #[cfg(target_os = "macos")]
    {
        Action::Exec {
            cmd: "/usr/bin/osascript".into(),
            args: vec!["-e".into(), "tell application \"Finder\" to shut down".into()],
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

fn logout_action() -> Action {
    #[cfg(target_os = "macos")]
    {
        Action::Exec {
            cmd: "/usr/bin/osascript".into(),
            args: vec!["-e".into(), "tell application \"System Events\" to log out".into()],
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // `terminate-session` needs a session id; `kill-session self` works
        // from inside the session without one. `sh -c` lets us fall back at
        // runtime: env var present → terminate; otherwise kill self.
        Action::Exec {
            cmd: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "if [ -n \"$XDG_SESSION_ID\" ]; then loginctl terminate-session \"$XDG_SESSION_ID\"; else loginctl kill-session self; fi".into(),
            ],
        }
    }
}

#[cfg(target_os = "macos")]
fn eject_action() -> Action {
    Action::Exec {
        cmd: "/usr/bin/osascript".into(),
        args: vec![
            "-e".into(),
            "tell application \"Finder\" to eject (every disk whose ejectable is true)".into(),
        ],
    }
}

fn screensaver_action() -> Action {
    #[cfg(target_os = "macos")]
    {
        Action::Exec {
            cmd: "/usr/bin/open".into(),
            args: vec!["-a".into(), "ScreenSaverEngine".into()],
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Action::Exec {
            cmd: "xdg-screensaver".into(),
            args: vec!["activate".into()],
        }
    }
}

fn display_sleep_action() -> Action {
    #[cfg(target_os = "macos")]
    {
        Action::Exec {
            cmd: "/usr/bin/pmset".into(),
            args: vec!["displaysleepnow".into()],
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Action::Exec {
            cmd: "xset".into(),
            args: vec!["dpms".into(), "force".into(), "off".into()],
        }
    }
}

fn empty_trash_action() -> Action {
    #[cfg(target_os = "macos")]
    {
        Action::Exec {
            cmd: "/usr/bin/osascript".into(),
            args: vec!["-e".into(), "tell application \"Finder\" to empty trash".into()],
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Action::Exec {
            cmd: "gio".into(),
            args: vec!["trash".into(), "--empty".into()],
        }
    }
}

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

    #[test]
    fn reboot_aliases_restart_action() {
        let restart = query("restart")
            .into_iter()
            .find(|r| r.result.title == "restart")
            .expect("restart result");
        let reboot = query("reboot")
            .into_iter()
            .find(|r| r.result.title == "reboot")
            .expect("reboot result");

        // Same Action shape — different labels, identical command + args.
        match (restart.result.action, reboot.result.action) {
            (Action::Exec { cmd: c1, args: a1 }, Action::Exec { cmd: c2, args: a2 }) => {
                assert_eq!(c1, c2);
                assert_eq!(a1, a2);
            }
            other => panic!("expected Exec/Exec, got {other:?}"),
        }
    }

    #[test]
    fn log_out_matches_full_phrase() {
        let results = query("log out");
        let r = results
            .iter()
            .find(|r| r.result.title == "log out")
            .expect("log out result");
        assert!(matches!(r.result.action, Action::Exec { .. }));
    }

    #[test]
    fn new_verbs_are_unpinned() {
        for q in ["reboot", "log out", "screensaver", "display sleep", "empty trash"] {
            let results = query(q);
            assert!(
                results.iter().any(|r| !r.result.pinned),
                "expected unpinned result for {q}"
            );
            assert!(
                results.iter().all(|r| !r.result.pinned),
                "no core result should be pinned ({q})"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn eject_is_present_on_macos() {
        let results = query("eject");
        let r = results
            .iter()
            .find(|r| r.result.title == "eject")
            .expect("eject result on macOS");

        match &r.result.action {
            Action::Exec { cmd, args } => {
                assert_eq!(cmd, "/usr/bin/osascript");
                assert!(args.iter().any(|a| a.contains("eject")));
            }
            other => panic!("expected Exec, got {other:?}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn eject_is_absent_on_linux() {
        // `gio mount --eject` needs a target device, so the bare verb has
        // no sensible Linux command — the table omits it entirely.
        let results = query("eject");
        assert!(
            results.iter().all(|r| r.result.title != "eject"),
            "eject should not appear on Linux, got {results:?}"
        );
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
