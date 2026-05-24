//! User settings — which plugins are disabled and per-plugin option values.
//!
//! Persisted as TOML under the platform config dir
//! (`~/Library/Application Support/high-beam/settings.toml` on macOS,
//! `$XDG_CONFIG_HOME/high-beam/settings.toml` on Linux). The loader reads on
//! startup; the settings UI calls back into the daemon to write.
//!
//! Writes are atomic (write to a tempfile, rename) so a crashed/SIGKILLed
//! daemon never leaves a half-written TOML on disk. The default state —
//! missing file, empty file, malformed file — is "all plugins enabled, no
//! user overrides" so a typo or first launch never breaks the launcher.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::paths;

const SETTINGS_FILENAME: &str = "settings.toml";

/// Default global hotkey string. Kept here (not at the call site) so the
/// fallback path on a malformed user value matches the schema default
/// byte-for-byte.
pub const DEFAULT_HOTKEY: &str = "Shift+Space";

/// Default modifier for the alt-action binding. Alt collides least with
/// system shortcuts (Cmd is reserved by macOS for menus; Ctrl by terminals
/// on Linux); plugin authors can opt rows into a secondary verb without
/// the user worrying about accidental triggers.
pub const DEFAULT_ALT_ACTION_MODIFIER: &str = "Alt";

/// The modifier strings the alt-action setting accepts. Matched
/// case-insensitively; anything outside this set falls back to the
/// default at load time.
pub const ALT_ACTION_MODIFIER_CHOICES: &[&str] = &["Alt", "Shift", "Cmd", "Ctrl"];

/// User-edited launcher state.
///
/// The on-disk shape is the [`SettingsFile`] TOML schema; this is the in-memory
/// projection the rest of the host reads/writes through.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    /// Path the settings were loaded from / will be written back to. `None`
    /// means we couldn't resolve a platform config dir — reads still work
    /// against the in-memory defaults, but [`Self::save`] will fail.
    path: Option<PathBuf>,
    /// App-wide settings (hotkey today; room for `max_rows`, blur behaviour,
    /// etc.). Kept as a separate struct so adding new fields is a one-line
    /// `serde` change without touching call sites.
    global: GlobalSettings,
    /// Per-plugin slot. Missing entries default to enabled + no overrides.
    plugins: HashMap<String, PluginSettings>,
}

/// Global launcher settings — anything that isn't per-plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct GlobalSettings {
    /// Accelerator string parsed by `global_hotkey::HotKey::from_str`
    /// (e.g. `"Shift+Space"`, `"Cmd+K"`). Validated at daemon start; a
    /// malformed value never blocks launch — see `daemon::parse_or_default`.
    pub hotkey: String,
    /// Last user-positioned launcher window origin in physical pixels.
    /// `None` means "no saved position" — the host falls back to the
    /// centered default. Using `Option` rather than e.g. `(0, 0)` avoids
    /// the ambiguity of "did the user actually drop the window at (0, 0)
    /// or have they never moved it?".
    pub launcher_position: Option<WindowPosition>,
    /// Maximum number of entries kept in the persistent query-history DB.
    /// Older rows are deleted when this cap is exceeded on insert.
    pub query_history_max_entries: usize,
    /// Modifier the user holds at Enter / click to run the result's
    /// `altAction` instead of the primary action. One of
    /// [`ALT_ACTION_MODIFIER_CHOICES`].
    pub alt_action_modifier: String,
}

/// Saved outer-position of the launcher window, in physical pixels relative
/// to the virtual desktop origin (top-left of the primary display on
/// macOS/Windows; varies by WM on X11/Wayland). Stored as `i32` because
/// winit's `set_outer_position` takes `i32` directly and negative values
/// are valid for windows on a secondary display placed left/above primary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowPosition {
    pub x: i32,
    pub y: i32,
}

/// Default maximum query-history entries. Matches the spec (100) and is
/// intentionally small enough to load quickly into memory at startup.
pub const DEFAULT_QUERY_HISTORY_MAX_ENTRIES: usize = 100;

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY.to_owned(),
            launcher_position: None,
            query_history_max_entries: DEFAULT_QUERY_HISTORY_MAX_ENTRIES,
            alt_action_modifier: DEFAULT_ALT_ACTION_MODIFIER.to_owned(),
        }
    }
}

/// Normalise an alt-action modifier string to one of [`ALT_ACTION_MODIFIER_CHOICES`].
/// Returns the default when the input doesn't match any known choice.
#[must_use]
pub fn normalize_alt_action_modifier(raw: &str) -> String {
    ALT_ACTION_MODIFIER_CHOICES
        .iter()
        .find(|c| c.eq_ignore_ascii_case(raw))
        .map_or_else(|| DEFAULT_ALT_ACTION_MODIFIER.to_owned(), |c| (*c).to_owned())
}

/// Per-plugin settings as we store them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginSettings {
    pub enabled: bool,
    /// User-set option values keyed by manifest option `key`. Stored as
    /// `serde_json::Value` so the UI and the SDK module can speak the same
    /// scalar shapes the manifest declares (`string`/`bool`/`int`/`enum`)
    /// without each consumer needing to branch on the option's declared type.
    pub options: HashMap<String, JsonValue>,
    /// Manifest `version` we last loaded for this plugin. Used by the
    /// lifecycle-hook layer to tell "first install" from "version bump" — a
    /// load that finds this `None` infers `Install`, a load that finds a
    /// different value infers `Update`. `None` when the plugin was never
    /// loaded (or pre-dates this field).
    pub last_loaded_version: Option<String>,
}

impl PluginSettings {
    /// Default plugin slot — enabled, no option overrides. Used as the
    /// fallback whenever the settings file omits a plugin entirely.
    #[must_use]
    pub fn enabled_default() -> Self {
        Self {
            enabled: true,
            options: HashMap::new(),
            last_loaded_version: None,
        }
    }
}

impl Settings {
    /// Load from the platform config path. Missing file / unresolvable config
    /// dir / malformed TOML all fall back to the default — settings must
    /// never block startup.
    #[must_use]
    pub fn load_or_default() -> Self {
        let Some(path) = default_settings_path() else {
            tracing::warn!("settings: could not resolve config dir; running with defaults");
            return Self::default();
        };
        Self::load_from(&path)
    }

    /// Load from an explicit path. Same fallback rules as
    /// [`Self::load_or_default`]; exposed so tests can point at a tmpdir.
    #[must_use]
    pub fn load_from(path: &Path) -> Self {
        let mut settings = match fs::read_to_string(path) {
            Ok(text) => Self::from_toml_or_default(&text, path),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    %err,
                    "settings: could not read; using defaults",
                );
                Self::default()
            }
        };
        settings.path = Some(path.to_path_buf());
        settings
    }

    /// Parse with fallback — malformed TOML logs and returns the default so
    /// the daemon keeps starting.
    #[must_use]
    pub fn from_toml_or_default(text: &str, source: &Path) -> Self {
        match Self::from_toml(text) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(
                    source = %source.display(),
                    %err,
                    "settings: malformed file; using defaults",
                );
                Self::default()
            }
        }
    }

    /// Parse with strict errors — public so tests can assert on specific
    /// failure modes.
    ///
    /// # Errors
    ///
    /// Returns a human-readable error string if the TOML is malformed.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let raw: SettingsFile = toml::from_str(text).map_err(|e| e.to_string())?;

        let plugins = raw
            .plugins
            .into_iter()
            .map(|(name, slot)| {
                let plugin = PluginSettings {
                    enabled: slot.enabled.unwrap_or(true),
                    options: slot
                        .options
                        .map(|tbl| tbl.into_iter().map(|(k, v)| (k, toml_to_json(v))).collect())
                        .unwrap_or_default(),
                    last_loaded_version: slot.last_loaded_version,
                };
                (name, plugin)
            })
            .collect();
        let global = {
            let raw_global = raw.global.unwrap_or_default();
            let max_entries = raw_global
                .query_history_max_entries
                .map_or(DEFAULT_QUERY_HISTORY_MAX_ENTRIES, |v| v.clamp(1, 10_000));
            let alt_mod = raw_global
                .alt_action_modifier
                .as_deref()
                .map_or_else(|| DEFAULT_ALT_ACTION_MODIFIER.to_owned(), normalize_alt_action_modifier);
            GlobalSettings {
                hotkey: raw_global.hotkey.unwrap_or_else(|| DEFAULT_HOTKEY.to_owned()),
                launcher_position: raw_global.launcher_position,
                query_history_max_entries: max_entries,
                alt_action_modifier: alt_mod,
            }
        };
        Ok(Self {
            path: None,
            global,
            plugins,
        })
    }

    /// Render the settings to TOML — exposed for tests and for the writer.
    ///
    /// # Panics
    ///
    /// Panics only if the TOML serialiser fails on a value shape we know to be
    /// representable (`bool`/`int`/`float`/`string`/`array`/`table`). This
    /// would indicate a bug, not a runtime condition.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let plugins = self
            .plugins
            .iter()
            .map(|(name, slot)| {
                let options = if slot.options.is_empty() {
                    None
                } else {
                    Some(
                        slot.options
                            .iter()
                            .map(|(k, v)| (k.clone(), json_to_toml(v.clone())))
                            .collect(),
                    )
                };
                let raw = PluginSlotFile {
                    enabled: Some(slot.enabled),
                    options,
                    last_loaded_version: slot.last_loaded_version.clone(),
                };
                (name.clone(), raw)
            })
            .collect();

        // Skip the `[global]` section when it matches defaults so a
        // freshly-installed settings file stays minimal. All fields write
        // through unconditionally once we emit the section — `hotkey` so
        // the user sees the active value on disk, `launcher_position` so
        // a remembered drag round-trips, and `query_history_max_entries`
        // so the user's customised cap survives a reload.
        let global = if self.global == GlobalSettings::default() {
            None
        } else {
            let max = self.global.query_history_max_entries;
            let alt_mod = if self.global.alt_action_modifier == DEFAULT_ALT_ACTION_MODIFIER {
                None
            } else {
                Some(self.global.alt_action_modifier.clone())
            };
            Some(GlobalFile {
                hotkey: Some(self.global.hotkey.clone()),
                launcher_position: self.global.launcher_position,
                query_history_max_entries: if max == DEFAULT_QUERY_HISTORY_MAX_ENTRIES {
                    None
                } else {
                    Some(max)
                },
                alt_action_modifier: alt_mod,
            })
        };
        let file = SettingsFile { global, plugins };
        // Unwrap is safe: SettingsFile only contains TOML-serialisable scalar
        // shapes (string/bool/int/float/array/table). serde never fails for
        // these unless a Display impl panics — none do.
        toml::to_string_pretty(&file).expect("settings TOML serialisation")
    }

    /// Persist to disk atomically: write to `<path>.tmp`, then rename.
    ///
    /// # Errors
    ///
    /// Returns an error if the settings file has no resolved path (e.g.
    /// `ProjectDirs` couldn't pick a config dir), or the underlying I/O
    /// (`create_dir_all`, write, rename) fails.
    pub fn save(&self) -> io::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "settings have no resolved path",
            ));
        };
        Self::write_to(self, path)
    }

    /// Variant of [`Self::save`] that writes to an explicit path. Used by
    /// tests and by the first-time write flow when [`Self::load_or_default`]
    /// couldn't resolve a path.
    ///
    /// # Errors
    ///
    /// Returns an error if the parent dir can't be created, the temp file
    /// can't be written, or the rename fails.
    pub fn write_to(&self, path: &Path) -> io::Result<()> {
        paths::ensure_parent_dir(path)?;
        let text = self.to_toml();
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        // Atomic on POSIX: rename within the same dir is one inode swap.
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Whether a plugin is enabled. Missing entries default to enabled.
    #[must_use]
    pub fn is_plugin_enabled(&self, name: &str) -> bool {
        self.plugins.get(name).is_none_or(|s| s.enabled)
    }

    /// Whether a plugin is enabled, falling back to `manifest_default` when
    /// the user has not set an explicit toggle. An explicit toggle in settings
    /// always wins, regardless of the manifest default.
    #[must_use]
    pub fn is_plugin_enabled_or_default(&self, name: &str, manifest_default: bool) -> bool {
        match self.plugins.get(name) {
            Some(slot) => slot.enabled,
            None => manifest_default,
        }
    }

    /// All option values the user has set for a plugin. Returns a borrowed
    /// reference to the plugin's options map, or to a shared empty map when
    /// the plugin has no overrides — callers fall back to manifest defaults
    /// at lookup time.
    #[must_use]
    pub fn plugin_options(&self, name: &str) -> &HashMap<String, JsonValue> {
        static EMPTY: LazyLock<HashMap<String, JsonValue>> = LazyLock::new(HashMap::new);
        self.plugins.get(name).map_or(&EMPTY, |s| &s.options)
    }

    /// Toggle a plugin's enabled flag. Creates the slot on first set.
    pub fn set_plugin_enabled(&mut self, name: &str, enabled: bool) {
        let slot = self
            .plugins
            .entry(name.to_owned())
            .or_insert_with(PluginSettings::enabled_default);
        slot.enabled = enabled;
    }

    /// Last manifest version recorded for this plugin, if any.
    #[must_use]
    pub fn last_loaded_version(&self, name: &str) -> Option<&str> {
        self.plugins.get(name).and_then(|s| s.last_loaded_version.as_deref())
    }

    /// Record the manifest version that just got loaded for this plugin.
    /// Pass `None` to clear (e.g. on uninstall).
    pub fn set_last_loaded_version(&mut self, name: &str, version: Option<String>) {
        let slot = self
            .plugins
            .entry(name.to_owned())
            .or_insert_with(PluginSettings::enabled_default);
        slot.last_loaded_version = version;
    }

    /// Set one option value for one plugin. Other plugins' option bags are
    /// untouched — scoping is per-plugin.
    pub fn set_plugin_option(&mut self, plugin: &str, key: &str, value: JsonValue) {
        let slot = self
            .plugins
            .entry(plugin.to_owned())
            .or_insert_with(PluginSettings::enabled_default);
        slot.options.insert(key.to_owned(), value);
    }

    /// Snapshot of the plugin slots, sorted by name. Used by the settings UI
    /// to render a deterministic plugin list.
    #[must_use]
    pub fn iter_plugin_slots(&self) -> Vec<(String, PluginSettings)> {
        let mut out: Vec<_> = self.plugins.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Read-only view of the global settings block.
    #[must_use]
    pub fn global(&self) -> &GlobalSettings {
        &self.global
    }

    /// The configured maximum query-history entry count.
    #[must_use]
    pub fn query_history_max_entries(&self) -> usize {
        self.global.query_history_max_entries
    }

    /// The configured alt-action modifier (`"Alt"`, `"Shift"`, `"Cmd"`,
    /// `"Ctrl"`). Already normalised at load time.
    #[must_use]
    pub fn alt_action_modifier(&self) -> &str {
        &self.global.alt_action_modifier
    }

    /// Replace the alt-action modifier. Input is normalised against
    /// [`ALT_ACTION_MODIFIER_CHOICES`]; unknown values reset to the
    /// default rather than persisting garbage.
    pub fn set_alt_action_modifier(&mut self, value: &str) {
        self.global.alt_action_modifier = normalize_alt_action_modifier(value);
    }

    /// Set the global hotkey accelerator string. Trimmed before storing so
    /// `"Shift+Space "` from a stray `TextInput` doesn't fail the parser later.
    pub fn set_hotkey(&mut self, hotkey: &str) {
        hotkey.trim().clone_into(&mut self.global.hotkey);
    }

    /// Record the launcher window's last user-chosen origin. Overwrites any
    /// previous saved position — the user's most recent drag is always the
    /// one we want to restore.
    pub fn set_launcher_position(&mut self, position: WindowPosition) {
        self.global.launcher_position = Some(position);
    }

    /// Forget the saved launcher position so the next show recenters.
    pub fn clear_launcher_position(&mut self) {
        self.global.launcher_position = None;
    }
}

/// The TOML wire shape. Keep optional everywhere so partial files
/// (`[plugins.foo]\nenabled = false`, nothing else) parse cleanly.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    global: Option<GlobalFile>,
    #[serde(default)]
    plugins: HashMap<String, PluginSlotFile>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct GlobalFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hotkey: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    launcher_position: Option<WindowPosition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    query_history_max_entries: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    alt_action_modifier: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PluginSlotFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    options: Option<toml::value::Table>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_loaded_version: Option<String>,
}

/// Path the daemon reads on startup. `None` when the project dir can't be
/// resolved (no `$HOME` etc.).
#[must_use]
pub fn default_settings_path() -> Option<PathBuf> {
    paths::config_dir().ok().map(|dir| dir.join(SETTINGS_FILENAME))
}

/// Lossy TOML → JSON conversion for the values we round-trip through option
/// settings. TOML datetimes degrade to strings — we never write those.
fn toml_to_json(value: toml::Value) -> JsonValue {
    match value {
        toml::Value::String(s) => JsonValue::String(s),
        toml::Value::Integer(i) => JsonValue::Number(i.into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(f).map_or(JsonValue::Null, JsonValue::Number),
        toml::Value::Boolean(b) => JsonValue::Bool(b),
        toml::Value::Datetime(dt) => JsonValue::String(dt.to_string()),
        toml::Value::Array(arr) => JsonValue::Array(arr.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(tbl) => JsonValue::Object(tbl.into_iter().map(|(k, v)| (k, toml_to_json(v))).collect()),
    }
}

/// JSON → TOML for the same set. JSON `null` becomes an empty string — the
/// settings UI never produces `null` for an option value, so this branch only
/// fires for malformed in-memory state.
fn json_to_toml(value: JsonValue) -> toml::Value {
    match value {
        JsonValue::Null => toml::Value::String(String::new()),
        JsonValue::Bool(b) => toml::Value::Boolean(b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                toml::Value::String(n.to_string())
            }
        }
        JsonValue::String(s) => toml::Value::String(s),
        JsonValue::Array(arr) => toml::Value::Array(arr.into_iter().map(json_to_toml).collect()),
        JsonValue::Object(obj) => toml::Value::Table(obj.into_iter().map(|(k, v)| (k, json_to_toml(v))).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_tmp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("high-beam-settings-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_is_empty_and_treats_everything_as_enabled() {
        let s = Settings::default();
        assert!(s.is_plugin_enabled("anything"));
        assert!(s.plugin_options("anything").is_empty());
    }

    #[test]
    fn set_plugin_option_per_plugin_scoping() {
        // One plugin's option must not leak into another's.
        let mut s = Settings::default();
        s.set_plugin_option("plugin-a", "key", JsonValue::String("value-a".into()));
        s.set_plugin_option("plugin-b", "key", JsonValue::String("value-b".into()));

        let a = s.plugin_options("plugin-a");
        let b = s.plugin_options("plugin-b");
        assert_eq!(a.get("key"), Some(&JsonValue::String("value-a".into())));
        assert_eq!(b.get("key"), Some(&JsonValue::String("value-b".into())));
    }

    #[test]
    fn save_writes_temp_then_renames_atomically() {
        let dir = fresh_tmp("atomic");
        let path = dir.join("settings.toml");
        let s = {
            let mut s = Settings::load_from(&path);
            s.set_plugin_enabled("echo", false);
            s
        };
        s.save().expect("save");

        // No `.toml.tmp` left behind once the rename completes.
        let tmp = path.with_extension("toml.tmp");
        assert!(!tmp.exists(), "temp file should be cleaned up by rename");
        assert!(path.exists(), "settings.toml should exist");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_file_falls_back_to_default() {
        let s = Settings::from_toml_or_default("not = [valid", Path::new("/tmp/x.toml"));
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn missing_file_yields_default_with_path_set() {
        let dir = fresh_tmp("missing");
        let path = dir.join("settings.toml");
        let s = Settings::load_from(&path);
        assert!(s.is_plugin_enabled("echo"), "missing → enabled");
        // The path is remembered so a subsequent save() lands the file on disk.
        s.save().expect("save into a path we resolved");
        assert!(path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_string_round_trips_to_default() {
        let s = Settings::from_toml("").expect("empty parses");
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn global_hotkey_defaults_when_absent() {
        // No `[global]` section at all → fallback to "Shift+Space".
        let s = Settings::from_toml("").expect("empty parses");
        assert_eq!(s.global().hotkey, DEFAULT_HOTKEY);

        // `[global]` present but `hotkey` missing → still default.
        let s = Settings::from_toml("[global]\n").expect("parse");
        assert_eq!(s.global().hotkey, DEFAULT_HOTKEY);
    }

    #[test]
    fn default_settings_omit_global_section() {
        // A pristine settings file shouldn't carry a `[global]` block — it's
        // implicit and identical to the default.
        let s = Settings::default();
        let text = s.to_toml();
        assert!(
            !text.contains("[global]"),
            "default settings should not write [global]: {text}"
        );
    }

    #[test]
    fn set_hotkey_trims_whitespace() {
        let mut s = Settings::default();
        s.set_hotkey("  Shift+F1  ");
        assert_eq!(s.global().hotkey, "Shift+F1");
    }

    #[test]
    fn launcher_position_defaults_to_none() {
        // Pristine settings and an empty `[global]` block both yield None so
        // the host falls back to the centered default.
        let s = Settings::default();
        assert!(s.global().launcher_position.is_none());

        let s = Settings::from_toml("[global]\n").expect("parse");
        assert!(s.global().launcher_position.is_none());

        let s = Settings::from_toml("[global]\nhotkey = \"Cmd+K\"\n").expect("parse");
        assert!(s.global().launcher_position.is_none());
    }

    #[test]
    fn default_settings_still_omit_global_section_with_position_field() {
        // Guard against the new `launcher_position` field flipping the
        // "default settings have no [global] block" invariant — the file
        // stays minimal when nothing's been customised.
        let s = Settings::default();
        let text = s.to_toml();
        assert!(
            !text.contains("[global"),
            "default settings should not write [global*]: {text}"
        );
    }

    #[test]
    fn is_plugin_enabled_or_default_explicit_on_wins() {
        let mut s = Settings::default();
        s.set_plugin_enabled("p", true);
        assert!(s.is_plugin_enabled_or_default("p", false));
    }

    #[test]
    fn is_plugin_enabled_or_default_explicit_off_wins() {
        let mut s = Settings::default();
        s.set_plugin_enabled("p", false);
        assert!(!s.is_plugin_enabled_or_default("p", true));
    }

    #[test]
    fn is_plugin_enabled_or_default_absent_uses_manifest_default_true() {
        let s = Settings::default();
        assert!(s.is_plugin_enabled_or_default("absent", true));
    }

    #[test]
    fn is_plugin_enabled_or_default_absent_uses_manifest_default_false() {
        let s = Settings::default();
        assert!(!s.is_plugin_enabled_or_default("absent", false));
    }

    #[test]
    fn iter_plugin_slots_sorted_by_name() {
        let mut s = Settings::default();
        s.set_plugin_enabled("zebra", false);
        s.set_plugin_enabled("alpha", true);
        s.set_plugin_enabled("middle", false);
        let names: Vec<_> = s.iter_plugin_slots().into_iter().map(|(k, _)| k).collect();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }
}
