//! `manifest.json` parsing.
//!
//! Unknown fields are tolerated (no `deny_unknown_fields`) so new fields can
//! land without breaking older plugins.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value as JsonValue;

const DEFAULT_ENTRY: &str = "plugin.js";
const DEFAULT_TIMEOUT_MS: u64 = 500;
const DEFAULT_MEMORY_MB: u32 = 32;
const DEFAULT_DEBOUNCE_MS: u64 = 0;
const DEFAULT_DEFAULT_ENABLED: bool = true;

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
    /// Whether this plugin loads by default when no explicit user toggle is set.
    /// Plugins that require external tooling (e.g. a CLI) ship with `false` so
    /// they don't produce errors for users who haven't installed the dependency.
    #[serde(default = "default_default_enabled", rename = "defaultEnabled")]
    pub default_enabled: bool,
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
    /// User-facing options the settings UI will render. Each entry declares
    /// a `key`, a primitive `type`, a human label, and a typed default. The
    /// raw shape is permissive so we can warn-and-drop malformed entries
    /// rather than failing to load — see [`Manifest::parsed_options`].
    #[serde(default)]
    pub options: Vec<JsonValue>,
    /// Archive URL (`.tar.gz`, `.tgz`, `.tar`, or `.zip`) the installer
    /// downloads when the user runs `install <manifestUrl>`. Mutually
    /// exclusive with [`Self::entry_url`] — a published manifest must
    /// declare exactly one (or neither, for local-only plugins).
    #[serde(default)]
    pub archive_url: Option<String>,
    /// Single-file install URL pointing at the plugin's entry script.
    /// For plugins that fit in one JS file with no sibling data — the
    /// common case — this skips archive packaging: the installer
    /// downloads just this file and writes it to `<plugin>/<entry>`
    /// alongside the fetched manifest. Mutually exclusive with
    /// [`Self::archive_url`].
    #[serde(default)]
    pub entry_url: Option<String>,
    /// Canonical URL hosting *this* manifest. `update` re-fetches it and
    /// compares versions to decide whether a new install is due. Absent ⇒
    /// the plugin opts out of update checks; once `install` succeeds the
    /// installer backfills this field with the URL the user ran against.
    #[serde(default)]
    pub manifest_url: Option<String>,
    /// Memoised result of [`Self::parsed_options`]. Settings callbacks call
    /// `parsed_options` once per option-set event, and re-running the
    /// JSON-shape validation each time is wasted work.
    #[serde(skip, default)]
    parsed_options_cache: OnceLock<ParsedOptions>,
}

/// One option a plugin author declared in `manifest.json` and the settings
/// UI should render.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionDef {
    pub key: String,
    pub label: String,
    pub kind: OptionKind,
}

/// Primitive option types the settings UI knows how to render.
///
/// Unknown types declared in a manifest get logged + dropped — they don't
/// fail the load. New variants need a matching input widget on the UI side.
#[derive(Debug, Clone, PartialEq)]
pub enum OptionKind {
    String {
        default: String,
    },
    Bool {
        default: bool,
    },
    Int {
        default: i64,
        min: Option<i64>,
        max: Option<i64>,
    },
    Enum {
        default: String,
        choices: Vec<String>,
    },
}

impl OptionDef {
    /// Default value rendered as a JSON-friendly scalar, so callers can stash
    /// it next to user-set values without needing to branch on `kind`.
    #[must_use]
    pub fn default_json(&self) -> JsonValue {
        match &self.kind {
            OptionKind::String { default } | OptionKind::Enum { default, .. } => JsonValue::String(default.clone()),
            OptionKind::Bool { default } => JsonValue::Bool(*default),
            OptionKind::Int { default, .. } => JsonValue::Number((*default).into()),
        }
    }
}

/// Result of validating [`Manifest::options`]: the option definitions the
/// host should expose to the settings UI, plus any warnings the loader should
/// surface in `plugin.log`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ParsedOptions {
    pub defs: Vec<OptionDef>,
    pub warnings: Vec<String>,
}

fn parse_option(raw: &JsonValue) -> Result<OptionDef, String> {
    let obj = raw.as_object().ok_or_else(|| "expected an object".to_owned())?;
    let key = obj
        .get("key")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| "missing or non-string `key`".to_owned())?
        .to_owned();
    if key.is_empty() {
        return Err("`key` must be non-empty".to_owned());
    }
    let type_str = obj
        .get("type")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| "missing or non-string `type`".to_owned())?;
    let label = obj
        .get("label")
        .and_then(JsonValue::as_str)
        .map_or_else(|| key.clone(), str::to_owned);

    let kind = match type_str {
        "string" => OptionKind::String {
            default: obj
                .get("default")
                .and_then(JsonValue::as_str)
                .map(str::to_owned)
                .unwrap_or_default(),
        },
        "bool" => OptionKind::Bool {
            default: obj.get("default").and_then(JsonValue::as_bool).unwrap_or(false),
        },
        "int" => OptionKind::Int {
            default: obj.get("default").and_then(JsonValue::as_i64).unwrap_or(0),
            min: obj.get("min").and_then(JsonValue::as_i64),
            max: obj.get("max").and_then(JsonValue::as_i64),
        },
        "enum" => {
            let choices_raw = obj
                .get("choices")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| "enum option missing `choices` array".to_owned())?;
            let choices: Vec<String> = choices_raw
                .iter()
                .filter_map(JsonValue::as_str)
                .map(str::to_owned)
                .collect();
            if choices.is_empty() {
                return Err("enum option needs at least one string in `choices`".to_owned());
            }
            // Default falls back to the first choice — every enum is guaranteed
            // a renderable value so the settings UI never has to special-case
            // a missing default.
            let default = obj
                .get("default")
                .and_then(JsonValue::as_str)
                .map_or_else(|| choices[0].clone(), str::to_owned);
            if !choices.contains(&default) {
                return Err(format!("enum default {default:?} not present in choices {choices:?}"));
            }
            OptionKind::Enum { default, choices }
        }
        other => return Err(format!("unknown option type {other:?}")),
    };

    Ok(OptionDef { key, label, kind })
}

const fn default_default_enabled() -> bool {
    DEFAULT_DEFAULT_ENABLED
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

    /// Whether this plugin should load on the host platform. Pure: does not
    /// emit warnings — pair with [`Self::platform_warnings`] when you have a
    /// destination for them (e.g. the per-plugin `plugin.log`).
    #[must_use]
    pub fn supports_current_platform(&self) -> bool {
        let Some(list) = &self.platforms else {
            return true;
        };
        let current = std::env::consts::OS;
        list.iter()
            .any(|entry| KNOWN_PLATFORMS.contains(&entry.as_str()) && entry == current)
    }

    /// Validated option list + any warnings about malformed/unknown entries.
    /// Malformed entries are dropped (not load failures) so a plugin author
    /// shipping a typo in `manifest.json` still gets their plugin loaded,
    /// just without the offending option.
    #[must_use]
    pub fn parsed_options(&self) -> &ParsedOptions {
        self.parsed_options_cache.get_or_init(|| {
            let mut defs = Vec::new();
            let mut warnings = Vec::new();
            for (idx, raw) in self.options.iter().enumerate() {
                match parse_option(raw) {
                    Ok(def) => defs.push(def),
                    Err(reason) => {
                        warnings.push(format!("options[{idx}]: {reason}"));
                    }
                }
            }
            ParsedOptions { defs, warnings }
        })
    }

    /// Diagnostic strings the loader should record for this manifest, one per
    /// unknown platform entry. Empty when `platforms` is absent or every entry
    /// is recognised. The loader funnels these into `plugin.log` after the
    /// log handle exists, so they're discoverable next to the plugin instead
    /// of buried on stderr.
    #[must_use]
    pub fn platform_warnings(&self) -> Vec<String> {
        let Some(list) = &self.platforms else {
            return Vec::new();
        };
        list.iter()
            .filter(|entry| !KNOWN_PLATFORMS.contains(&entry.as_str()))
            .map(|entry| format!("ignoring unknown platform {entry:?} (known: {KNOWN_PLATFORMS:?})"))
            .collect()
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

/// Whether `remote` is strictly newer than `local` per semver rules.
///
/// Both arguments must parse as semver for the answer to be `true`. Anything
/// else — non-semver strings, equal versions, remote older than local — yields
/// `false`. The intentional conservatism: when in doubt, do nothing rather
/// than risk replacing a working plugin with a string-equal but semantically
/// older one.
#[must_use]
pub fn is_newer_version(remote: &str, local: &str) -> bool {
    let Ok(r) = semver::Version::parse(remote) else {
        return false;
    };
    let Ok(l) = semver::Version::parse(local) else {
        return false;
    };
    r > l
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
        let json = format!(r#"{{ "name": "matched", "platforms": ["{}"] }}"#, std::env::consts::OS);
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
        let warnings = m.platform_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("haiku"));
    }

    #[test]
    fn platforms_unknown_alongside_match_still_supports() {
        let json = format!(
            r#"{{ "name": "mixed", "platforms": ["haiku", "{}"] }}"#,
            std::env::consts::OS,
        );
        let m = Manifest::parse(json.as_bytes()).unwrap();
        assert!(m.supports_current_platform());
        let warnings = m.platform_warnings();
        assert_eq!(warnings.len(), 1, "only the unknown entry warns");
        assert!(warnings[0].contains("haiku"));
    }

    #[test]
    fn platform_warnings_empty_when_no_platforms_field() {
        let m = Manifest::parse(br#"{ "name": "x" }"#).unwrap();
        assert!(m.platform_warnings().is_empty());
    }

    #[test]
    fn platform_warnings_empty_for_only_known_entries() {
        let m = Manifest::parse(br#"{ "name": "x", "platforms": ["macos", "linux"] }"#).unwrap();
        assert!(m.platform_warnings().is_empty());
    }

    #[test]
    fn options_absent_parses_to_empty() {
        let m = Manifest::parse(br#"{ "name": "x" }"#).unwrap();
        let parsed = m.parsed_options();
        assert!(parsed.defs.is_empty());
        assert!(parsed.warnings.is_empty());
    }

    #[test]
    fn options_parses_string_bool_int_enum() {
        let json = br#"{
            "name": "x",
            "options": [
                { "key": "user", "type": "string", "label": "User", "default": "alice" },
                { "key": "live", "type": "bool", "label": "Live?", "default": true },
                { "key": "limit", "type": "int", "label": "Max", "default": 5, "min": 1, "max": 50 },
                { "key": "engine", "type": "enum", "label": "Engine", "default": "ddg", "choices": ["ddg", "google"] }
            ]
        }"#;
        let m = Manifest::parse(json).unwrap();
        let parsed = m.parsed_options();
        assert!(parsed.warnings.is_empty(), "got {:?}", parsed.warnings);
        assert_eq!(parsed.defs.len(), 4);

        assert_eq!(parsed.defs[0].key, "user");
        assert_eq!(parsed.defs[0].label, "User");
        assert!(matches!(
            &parsed.defs[0].kind,
            OptionKind::String { default } if default == "alice"
        ));

        assert!(matches!(&parsed.defs[1].kind, OptionKind::Bool { default: true },));

        assert!(matches!(
            &parsed.defs[2].kind,
            OptionKind::Int {
                default: 5,
                min: Some(1),
                max: Some(50),
            },
        ));

        match &parsed.defs[3].kind {
            OptionKind::Enum { default, choices } => {
                assert_eq!(default, "ddg");
                assert_eq!(choices, &vec!["ddg".to_owned(), "google".to_owned()]);
            }
            other => panic!("expected Enum, got {other:?}"),
        }
    }

    #[test]
    fn options_unknown_type_warns_and_drops_entry() {
        let json = br##"{
            "name": "x",
            "options": [
                { "key": "ok", "type": "string", "default": "" },
                { "key": "bad", "type": "color", "default": "#fff" }
            ]
        }"##;
        let m = Manifest::parse(json).unwrap();
        let parsed = m.parsed_options();
        assert_eq!(parsed.defs.len(), 1, "only the well-typed option survives");
        assert_eq!(parsed.defs[0].key, "ok");
        assert_eq!(parsed.warnings.len(), 1);
        assert!(
            parsed.warnings[0].contains("color"),
            "warning should name the offender: {:?}",
            parsed.warnings,
        );
    }

    #[test]
    fn options_missing_key_drops_entry() {
        let json = br#"{
            "name": "x",
            "options": [
                { "type": "string", "default": "x" }
            ]
        }"#;
        let m = Manifest::parse(json).unwrap();
        let parsed = m.parsed_options();
        assert!(parsed.defs.is_empty());
        assert_eq!(parsed.warnings.len(), 1);
    }

    #[test]
    fn options_enum_without_choices_drops_entry() {
        let json = br#"{
            "name": "x",
            "options": [{ "key": "e", "type": "enum", "default": "a" }]
        }"#;
        let m = Manifest::parse(json).unwrap();
        let parsed = m.parsed_options();
        assert!(parsed.defs.is_empty());
        assert_eq!(parsed.warnings.len(), 1);
    }

    #[test]
    fn options_string_default_falls_back_to_empty() {
        let json = br#"{
            "name": "x",
            "options": [{ "key": "u", "type": "string", "label": "U" }]
        }"#;
        let m = Manifest::parse(json).unwrap();
        let parsed = m.parsed_options();
        assert_eq!(parsed.defs.len(), 1);
        assert!(matches!(
            &parsed.defs[0].kind,
            OptionKind::String { default } if default.is_empty()
        ));
    }

    #[test]
    fn options_label_falls_back_to_key() {
        let json = br#"{
            "name": "x",
            "options": [{ "key": "username", "type": "string" }]
        }"#;
        let m = Manifest::parse(json).unwrap();
        let parsed = m.parsed_options();
        assert_eq!(parsed.defs.len(), 1);
        assert_eq!(parsed.defs[0].label, "username");
    }

    #[test]
    fn default_enabled_defaults_to_true() {
        let m = Manifest::parse(br#"{ "name": "x" }"#).unwrap();
        assert!(m.default_enabled);
    }

    #[test]
    fn default_enabled_false_round_trips() {
        let json = br#"{ "name": "x", "defaultEnabled": false }"#;
        let m = Manifest::parse(json).unwrap();
        assert!(!m.default_enabled);
    }

    #[test]
    fn default_enabled_true_explicit_round_trips() {
        let json = br#"{ "name": "x", "defaultEnabled": true }"#;
        let m = Manifest::parse(json).unwrap();
        assert!(m.default_enabled);
    }

    #[test]
    fn archive_and_manifest_urls_default_to_none() {
        let m = Manifest::parse(br#"{ "name": "x" }"#).unwrap();
        assert!(m.archive_url.is_none());
        assert!(m.manifest_url.is_none());
    }

    #[test]
    fn archive_and_manifest_urls_round_trip_when_present() {
        let json = br#"{
            "name": "x",
            "archiveUrl": "https://example.com/x.tar.gz",
            "manifestUrl": "https://example.com/x/manifest.json"
        }"#;
        let m = Manifest::parse(json).unwrap();
        assert_eq!(m.archive_url.as_deref(), Some("https://example.com/x.tar.gz"));
        assert_eq!(m.manifest_url.as_deref(), Some("https://example.com/x/manifest.json"));
        assert!(m.entry_url.is_none());
    }

    #[test]
    fn entry_url_parses_when_present() {
        let json = br#"{
            "name": "x",
            "entryUrl": "https://example.com/x.js"
        }"#;
        let m = Manifest::parse(json).unwrap();
        assert_eq!(m.entry_url.as_deref(), Some("https://example.com/x.js"));
        assert!(m.archive_url.is_none());
    }

    #[test]
    fn is_newer_version_compares_semver() {
        assert!(is_newer_version("1.2.4", "1.2.3"));
        assert!(is_newer_version("2.0.0", "1.9.9"));
        assert!(!is_newer_version("1.2.3", "1.2.3"));
        assert!(!is_newer_version("1.0.0", "1.0.1"));
    }

    #[test]
    fn is_newer_version_false_on_non_semver() {
        // Conservatism: an unparseable version string never triggers an update.
        assert!(!is_newer_version("v1.2.3", "1.2.3"));
        assert!(!is_newer_version("1.2.3", "not-a-version"));
        assert!(!is_newer_version("", "1.0.0"));
    }

    #[test]
    fn options_default_json_matches_kind() {
        let m = Manifest::parse(
            br#"{
            "name": "x",
            "options": [
                { "key": "s", "type": "string", "default": "hi" },
                { "key": "b", "type": "bool", "default": true },
                { "key": "i", "type": "int", "default": 7 },
                { "key": "e", "type": "enum", "choices": ["a", "b"], "default": "b" }
            ]
        }"#,
        )
        .unwrap();
        let parsed = m.parsed_options();
        assert_eq!(parsed.defs[0].default_json(), JsonValue::String("hi".into()));
        assert_eq!(parsed.defs[1].default_json(), JsonValue::Bool(true));
        assert_eq!(parsed.defs[2].default_json(), JsonValue::Number(7.into()));
        assert_eq!(parsed.defs[3].default_json(), JsonValue::String("b".into()));
    }
}
