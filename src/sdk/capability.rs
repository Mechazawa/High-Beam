//! Capability gating — central truth for which `highbeam:*` modules a plugin
//! is allowed to import.
//!
//! A module is importable if the plugin's caps include any of the module's
//! `any_of`. Functions inside the module can still gate themselves tighter
//! (e.g. `clipboard` imports on either read or write, but `write()` throws if
//! the plugin only declared `clipboard.read`).
//!
//! `highbeam:match` and `highbeam:platform` skip the gate entirely.

pub(crate) struct ModuleCap {
    pub specifier: &'static str,
    /// At least one of these caps must be declared for the module to load.
    pub any_of: &'static [&'static str],
}

/// All cap-gated `highbeam:*` modules. `highbeam:match` and `highbeam:platform`
/// load unconditionally — see [`is_uncapped_module`].
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

const UNCAPPED_MODULES: &[&str] = &["highbeam:match", "highbeam:platform", "highbeam:settings"];

/// Every capability string the host recognises. Anything else is logged as
/// an unknown-cap warning at load time.
pub(crate) const KNOWN_CAPABILITIES: &[&str] = &[
    "actions",
    "http",
    "clipboard.read",
    "clipboard.write",
    "fs.read",
    "fs.cache",
    "system.exec",
    "system.applescript",
    "icons",
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
    fn known_capabilities_includes_baseline_set() {
        for cap in ["actions", "http", "clipboard.read", "clipboard.write"] {
            assert!(
                KNOWN_CAPABILITIES.contains(&cap),
                "expected `{cap}` in KNOWN_CAPABILITIES"
            );
        }
    }
}
