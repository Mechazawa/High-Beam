//! Capability gating — central truth for which `highbeam:*` modules a plugin
//! is allowed to import.
//!
//! A module is importable if the plugin's caps include any of the module's
//! `any_of`. Functions inside the module can still gate themselves tighter
//! (e.g. `clipboard` imports on either read or write, but `write()` throws if
//! the plugin only declared `clipboard.read`).
//!
//! `highbeam:match`, `highbeam:platform`, and `highbeam:settings` skip the
//! gate entirely.

pub(crate) struct ModuleCap {
    pub specifier: &'static str,
    /// At least one of these caps must be declared for the module to load.
    pub any_of: &'static [&'static str],
}

/// One row in the capability table: the manifest string, a human-readable
/// explanation rendered in the install-confirmation view.
struct Capability {
    name: &'static str,
    explanation: &'static str,
}

/// Every capability string the host recognises plus its user-facing
/// explanation. Single source of truth — drives both the unknown-cap warning
/// at load time and the rows the confirmation view renders.
const CAPABILITIES: &[Capability] = &[
    Capability {
        name: "actions",
        explanation: "open URLs, copy text, run commands, reveal files",
    },
    Capability {
        name: "http",
        explanation: "make outbound HTTP requests",
    },
    Capability {
        name: "clipboard.read",
        explanation: "read the system clipboard",
    },
    Capability {
        name: "clipboard.write",
        explanation: "write to the system clipboard",
    },
    Capability {
        name: "fs.read",
        explanation: "read files and list directories",
    },
    Capability {
        name: "fs.cache",
        explanation: "read and write a per-plugin cache directory",
    },
    Capability {
        name: "system.exec",
        explanation: "run shell commands and capture their output",
    },
    Capability {
        name: "system.applescript",
        explanation: "execute AppleScript on macOS",
    },
    Capability {
        name: "icons",
        explanation: "resolve native file / app icons",
    },
];

/// All cap-gated `highbeam:*` modules. `highbeam:match`, `highbeam:platform`,
/// and `highbeam:settings` load unconditionally — see [`is_uncapped_module`].
pub(crate) const MODULES: &[ModuleCap] = &[
    ModuleCap {
        specifier: "highbeam:actions",
        any_of: &["actions"],
    },
    ModuleCap {
        specifier: "highbeam:http",
        any_of: &["http"],
    },
    ModuleCap {
        specifier: "highbeam:clipboard",
        any_of: &["clipboard.read", "clipboard.write"],
    },
    ModuleCap {
        specifier: "highbeam:fs",
        any_of: &["fs.read", "fs.cache"],
    },
    ModuleCap {
        specifier: "highbeam:icons",
        any_of: &["icons"],
    },
    ModuleCap {
        specifier: "highbeam:system",
        any_of: &["system.exec", "system.applescript"],
    },
];

const UNCAPPED_MODULES: &[&str] = &[
    "highbeam:match",
    "highbeam:platform",
    "highbeam:settings",
    "highbeam:view",
];

#[must_use]
pub(crate) fn for_module(specifier: &str) -> Option<&'static ModuleCap> {
    MODULES.iter().find(|m| m.specifier == specifier)
}

#[must_use]
pub(crate) fn is_uncapped_module(specifier: &str) -> bool {
    UNCAPPED_MODULES.contains(&specifier)
}

#[must_use]
pub(crate) fn grants_any(caps: &[String], any_of: &[&str]) -> bool {
    caps.iter().any(|c| any_of.contains(&c.as_str()))
}

/// Whether `cap` is one of the strings the host recognises. Caps outside
/// this set are logged as unknown-cap warnings during plugin load and
/// rendered with a placeholder explanation in the install-confirmation view.
#[must_use]
pub fn is_known_cap(cap: &str) -> bool {
    CAPABILITIES.iter().any(|c| c.name == cap)
}

/// Comma-separated list of every recognised cap name. Used by the loader's
/// unknown-cap warning so the user sees what they could have declared.
#[must_use]
pub fn known_cap_names() -> Vec<&'static str> {
    CAPABILITIES.iter().map(|c| c.name).collect()
}

/// Human-readable explanation for a capability. Falls back to the raw cap
/// string for unknown values so the confirmation view always has something
/// to render.
#[must_use]
pub fn explain_cap(cap: &str) -> &str {
    CAPABILITIES
        .iter()
        .find(|c| c.name == cap)
        .map_or(cap, |c| c.explanation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_module_grants_on_actions_cap() {
        let m = for_module("highbeam:actions").expect("actions module exists");
        assert!(grants_any(&["actions".into()], m.any_of));
        assert!(!grants_any(&["http".into()], m.any_of));
    }

    #[test]
    fn clipboard_module_grants_on_either_clipboard_cap() {
        let m = for_module("highbeam:clipboard").expect("clipboard module exists");
        assert!(grants_any(&["clipboard.read".into()], m.any_of));
        assert!(grants_any(&["clipboard.write".into()], m.any_of));
        assert!(grants_any(
            &["clipboard.read".into(), "clipboard.write".into()],
            m.any_of
        ));
        assert!(!grants_any(&[], m.any_of));
        assert!(!grants_any(&["actions".into()], m.any_of));
    }

    #[test]
    fn unknown_module_returns_none() {
        assert!(for_module("highbeam:nope").is_none());
        assert!(for_module("fs").is_none());
    }

    #[test]
    fn explain_cap_falls_back_to_raw_string_for_unknown() {
        // Unknown caps render their raw name in the confirmation view — the
        // user still sees the offending string so they can grep manifests.
        assert_eq!(explain_cap("future.cap"), "future.cap");
    }
}
