//! `manifest.json` parsing.
//!
//! Unknown fields are tolerated (no `deny_unknown_fields`) so new fields can
//! land without breaking older plugins.

use std::path::{Path, PathBuf};

use serde::Deserialize;

const DEFAULT_ENTRY: &str = "plugin.js";
const DEFAULT_TIMEOUT_MS: u64 = 500;
const DEFAULT_MEMORY_MB: u32 = 32;
const DEFAULT_DEBOUNCE_MS: u64 = 0;

/// Platforms the host gates on. `&[&str]` (rather than an enum) so unknown
/// future values like `"windows"` round-trip unchanged and are flagged only
/// at gating time.
const KNOWN_PLATFORMS: &[&str] = &["macos", "linux"];

/// Plugin metadata as it appears on disk.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_memory_mb")]
    pub memory_mb: u32,
    /// Per-plugin debounce in milliseconds — the dispatcher waits this long
    /// after the latest keystroke before invoking `query()`. `0` dispatches
    /// every keystroke immediately. Effective value capped at
    /// [`crate::plugins::dispatch::MAX_DEBOUNCE_MS`].
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Platforms this plugin opts into:
    ///   * `None` — loads everywhere (back-compat default).
    ///   * `Some(vec![])` — explicit shelving: never loads.
    ///   * `Some(list)` — load only where `std::env::consts::OS` matches.
    ///     Unknown strings parse but never match.
    #[serde(default)]
    pub platforms: Option<Vec<String>>,
}

fn default_entry() -> String {
    DEFAULT_ENTRY.to_owned()
}

const fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

const fn default_memory_mb() -> u32 {
    DEFAULT_MEMORY_MB
}

const fn default_debounce_ms() -> u64 {
    DEFAULT_DEBOUNCE_MS
}

impl Manifest {
    /// Parse a manifest from raw JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns the underlying `serde_json` error if the payload is malformed,
    /// missing required fields (`name`), or contains incorrectly typed values.
    pub fn parse(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Resolve the plugin entry point relative to its directory.
    #[must_use]
    pub fn entry_path(&self, plugin_dir: &Path) -> PathBuf {
        plugin_dir.join(&self.entry)
    }

    /// Whether this plugin should load on the host platform. Warnings for
    /// unknown platform strings are emitted here (not at parse time) so the
    /// manifest stays a passive data type.
    #[must_use]
    pub fn supports_current_platform(&self) -> bool {
        match &self.platforms {
            None => true,
            Some(list) => {
                let current = std::env::consts::OS;
                let mut matched = false;
                for entry in list {
                    if KNOWN_PLATFORMS.contains(&entry.as_str()) {
                        if entry == current {
                            matched = true;
                        }
                    } else {
                        eprintln!(
                            "plugins: {}: ignoring unknown platform {entry:?} (known: {KNOWN_PLATFORMS:?})",
                            self.name,
                        );
                    }
                }
                matched
            }
        }
    }

    /// Human-readable reason describing why the plugin was gated out.
    /// `None` when the platform actually matches.
    #[must_use]
    pub fn platform_skip_reason(&self) -> Option<String> {
        let list = self.platforms.as_ref()?;
        Some(format!(
            "declares platforms={list:?}, running on {}",
            std::env::consts::OS,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_manifest() {
        let json = br#"{
            "name": "echo",
            "displayName": "Echo",
            "version": "0.1.0",
            "description": "Echo plugin",
            "entry": "plugin.js",
            "timeoutMs": 750,
            "memoryMb": 16,
            "capabilities": ["actions"]
        }"#;
        let m = Manifest::parse(json).expect("parse");
        assert_eq!(m.name, "echo");
        assert_eq!(m.display_name.as_deref(), Some("Echo"));
        assert_eq!(m.entry, "plugin.js");
        assert_eq!(m.timeout_ms, 750);
        assert_eq!(m.memory_mb, 16);
        assert_eq!(m.capabilities, vec!["actions".to_owned()]);
    }

    #[test]
    fn applies_defaults_for_optional_fields() {
        let json = br#"{ "name": "minimal" }"#;
        let m = Manifest::parse(json).expect("parse");
        assert_eq!(m.entry, "plugin.js");
        assert_eq!(m.timeout_ms, 500);
        assert_eq!(m.memory_mb, 32);
        assert_eq!(m.debounce_ms, 0);
        assert!(m.capabilities.is_empty());
    }

    #[test]
    fn parses_debounce_ms() {
        let json = br#"{ "name": "slow", "debounceMs": 250 }"#;
        let m = Manifest::parse(json).expect("parse");
        assert_eq!(m.debounce_ms, 250);
    }

    #[test]
    fn tolerates_unknown_fields() {
        let json = br#"{
            "name": "future",
            "debounceMs": 100,
            "fs": { "read": ["~/foo"] }
        }"#;
        Manifest::parse(json).expect("future fields are ignored, not rejected");
    }

    #[test]
    fn rejects_missing_name() {
        let json = br#"{ "version": "1.0.0" }"#;
        assert!(Manifest::parse(json).is_err());
    }

    #[test]
    fn entry_path_joins_with_plugin_dir() {
        let m = Manifest::parse(br#"{ "name": "x", "entry": "main.js" }"#).unwrap();
        let path = m.entry_path(Path::new("/tmp/plugins/x"));
        assert_eq!(path, PathBuf::from("/tmp/plugins/x/main.js"));
    }

    #[test]
    fn platforms_absent_supports_every_platform() {
        let m = Manifest::parse(br#"{ "name": "old" }"#).unwrap();
        assert!(m.platforms.is_none());
        assert!(m.supports_current_platform());
        assert!(m.platform_skip_reason().is_none());
    }

    #[test]
    fn platforms_null_supports_every_platform() {
        let m = Manifest::parse(br#"{ "name": "null", "platforms": null }"#).unwrap();
        assert!(m.platforms.is_none());
        assert!(m.supports_current_platform());
    }

    #[test]
    fn platforms_empty_array_is_shelved() {
        let m = Manifest::parse(br#"{ "name": "shelved", "platforms": [] }"#).unwrap();
        assert_eq!(m.platforms.as_deref(), Some(&[][..]));
        assert!(!m.supports_current_platform());
        assert!(m.platform_skip_reason().is_some());
    }

    #[test]
    fn platforms_matching_current_os_supports() {
        let json = format!(
            r#"{{ "name": "matched", "platforms": ["{}"] }}"#,
            std::env::consts::OS,
        );
        let m = Manifest::parse(json.as_bytes()).unwrap();
        assert!(m.supports_current_platform());
    }

    #[test]
    fn platforms_excluding_current_os_does_not_support() {
        let other = if std::env::consts::OS == "macos" {
            "linux"
        } else {
            "macos"
        };
        let json = format!(r#"{{ "name": "wrong-os", "platforms": ["{other}"] }}"#);
        let m = Manifest::parse(json.as_bytes()).unwrap();
        assert!(!m.supports_current_platform());
        let reason = m.platform_skip_reason().expect("skip reason");
        assert!(reason.contains(other));
        assert!(reason.contains(std::env::consts::OS));
    }

    #[test]
    fn platforms_unknown_string_is_warned_and_ignored() {
        let m = Manifest::parse(br#"{ "name": "future", "platforms": ["haiku"] }"#).unwrap();
        assert_eq!(m.platforms.as_deref().map(<[String]>::len), Some(1));
        assert!(!m.supports_current_platform());
    }

    #[test]
    fn platforms_unknown_alongside_match_still_supports() {
        let json = format!(
            r#"{{ "name": "mixed", "platforms": ["haiku", "{}"] }}"#,
            std::env::consts::OS,
        );
        let m = Manifest::parse(json.as_bytes()).unwrap();
        assert!(m.supports_current_platform());
    }
}
