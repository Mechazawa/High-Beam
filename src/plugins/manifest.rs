//! `manifest.json` parsing.
//!
//! Stage 3 only consumes a subset of the v1 manifest spec (`docs/02-plugin-sdk.md`):
//!
//! ```json
//! {
//!   "name": "echo",
//!   "displayName": "Echo",
//!   "version": "0.1.0",
//!   "description": "Echo plugin",
//!   "entry": "plugin.js",
//!   "timeoutMs": 500,
//!   "memoryMb": 32,
//!   "capabilities": ["actions"]
//! }
//! ```
//!
//! Unknown fields are tolerated (`#[serde(default)]` + no `deny_unknown_fields`)
//! so Stage 4+ can add `debounceMs`, `fs.*`, etc. without breaking Stage 3
//! plugins.

use std::path::{Path, PathBuf};

use serde::Deserialize;

const DEFAULT_ENTRY: &str = "plugin.js";
const DEFAULT_TIMEOUT_MS: u64 = 500;
const DEFAULT_MEMORY_MB: u32 = 32;
const DEFAULT_DEBOUNCE_MS: u64 = 0;

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
    /// after the latest keystroke before invoking `query()`. `0` (the
    /// default) dispatches every keystroke immediately. Stage 4 clamps the
    /// effective value to [`crate::plugins::dispatch::MAX_DEBOUNCE_MS`].
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    #[serde(default)]
    pub capabilities: Vec<String>,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_stage3_manifest() {
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
}
