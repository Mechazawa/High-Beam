//! Capability gating — central truth for which `highbeam:*` modules a plugin
//! is allowed to import, plus the per-function gates within a module.
//!
//! Stage 4 generalizes Stage 3's one-off `if cap == "actions"` check. Each
//! `highbeam:*` module advertises the set of capabilities that grant it. A
//! plugin can import the module if *any* of those caps is in its declared
//! list; within the module, individual functions can still gate themselves
//! tighter (e.g. `clipboard` loads if the plugin has either `clipboard.read`
//! or `clipboard.write`, but `write()` will throw if only `clipboard.read`
//! was declared).
//!
//! Stage 9 will route capability-violation errors to the per-plugin logfile;
//! Stage 4 returns a [`JsError`] that the loader will surface to stderr.

/// One row in the capability table.
pub struct ModuleCap {
    /// The `highbeam:foo` import specifier.
    pub specifier: &'static str,
    /// At least one of these caps must be declared for the module to load.
    pub any_of: &'static [&'static str],
}

/// All `highbeam:*` modules recognised by the host, mapped to the caps that
/// grant them. The runtime walks this table when resolving a load.
pub const MODULES: &[ModuleCap] = &[
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
];

/// Every capability string the host *recognises*. Anything outside this list
/// is logged as an unknown-cap warning at plugin load time. Stage 4+ extends
/// this; the underscore prefix is for Stage 7 "I exist but aren't wired" caps.
pub const KNOWN_CAPABILITIES: &[&str] = &[
    "actions",
    "http",
    "clipboard.read",
    "clipboard.write",
    // Reserved for future stages — accepted in manifests without warning,
    // but no module gates on them yet.
    "fs.read",
    "fs.cache",
    "system.exec",
    "system.applescript",
    "icons",
];

/// Look up the capability requirements for a module specifier.
#[must_use]
pub fn for_module(specifier: &str) -> Option<&'static ModuleCap> {
    MODULES.iter().find(|m| m.specifier == specifier)
}

/// True if `caps` grants *any* of the capability strings in `any_of`.
#[must_use]
pub fn grants_any(caps: &[String], any_of: &[&str]) -> bool {
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
    fn known_capabilities_includes_stage4_set() {
        for cap in ["actions", "http", "clipboard.read", "clipboard.write"] {
            assert!(
                KNOWN_CAPABILITIES.contains(&cap),
                "expected `{cap}` in KNOWN_CAPABILITIES"
            );
        }
    }
}
