//! Settings-view controller: turns plugin manifests + persisted `Settings`
//! into the Slint models the settings view renders, and the inverse —
//! callbacks from the view feed back into a `Mutex<Settings>` that owns
//! disk persistence.
//!
//! Lives in its own module so `app.rs` stays focused on the launcher
//! pipeline; the settings view talks to the same `QueryWindow` but its
//! state is independent of the query/results state.

use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value as JsonValue;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::QueryWindow;
use crate::daemon::HotkeyRegistration;
use crate::plugins::manifest::{Manifest, OptionDef, OptionKind};
use crate::settings::{Settings, WindowPosition};
use crate::ui::{PluginOption, PluginSlot};

/// Display metadata for a plugin, extracted from its manifest and handed to
/// the settings view so the right pane can show a header.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginMetadata {
    /// Human-readable name — `display_name` when present, otherwise `name`.
    pub display_name: String,
    /// Optional semver string, ready to render with a `v` prefix.
    pub version: Option<String>,
    /// Optional human-readable description.
    pub description: Option<String>,
}

/// Extract display metadata from a manifest for the settings right pane.
#[must_use]
pub fn plugin_metadata(manifest: &Manifest) -> PluginMetadata {
    PluginMetadata {
        display_name: manifest.display_name.clone().unwrap_or_else(|| manifest.name.clone()),
        version: manifest.version.clone(),
        description: manifest.description.clone(),
    }
}

/// Shared state for the settings view. Cloned into every callback closure;
/// internally `Arc`-wrapped so writes from the UI thread land in one place.
#[derive(Clone)]
pub struct SettingsController {
    inner: Arc<Inner>,
}

struct Inner {
    /// All manifests we found in the plugins dir. The settings view shows
    /// every entry — even ones the runtime didn't load (because the user
    /// disabled them) — so toggling the switch can re-enable them.
    manifests: Vec<Manifest>,
    /// Persisted state. Mutex because multiple callbacks (toggle, set
    /// option) can fire from different Slint events; no concurrent writes
    /// in practice but the borrow checker doesn't care.
    settings: Mutex<Settings>,
    /// Handle to the OS hotkey registration, plumbed in by `daemon::run`
    /// after `spawn_hotkey_listener` succeeds. `None` until set (e.g.
    /// during early init), and on Linux where the WM owns the binding.
    /// `OnceLock` so the controller's already-wired callbacks see the
    /// registration as soon as the daemon installs it.
    hotkey_registration: OnceLock<Arc<HotkeyRegistration>>,
}

impl SettingsController {
    /// Build a controller from the manifest scan + initial loaded settings.
    #[must_use]
    pub fn new(manifests: Vec<Manifest>, settings: Settings) -> Self {
        Self {
            inner: Arc::new(Inner {
                manifests,
                settings: Mutex::new(settings),
                hotkey_registration: OnceLock::new(),
            }),
        }
    }

    /// Wire the live OS hotkey registration so capture / reset paths can
    /// re-bind without a daemon restart. Called by `daemon::run` after the
    /// hotkey listener thread is up. Idempotent at the type level — the
    /// `OnceLock` swallows a second set silently.
    pub fn attach_hotkey_registration(&self, registration: Arc<HotkeyRegistration>) {
        let _ = self.inner.hotkey_registration.set(registration);
    }

    fn reregister_hotkey(&self, spec: &str) {
        if let Some(reg) = self.inner.hotkey_registration.get() {
            reg.reregister(spec);
        }
    }

    /// Wire every settings callback on the given window. Idempotent; call
    /// once per window. The controller is held by both the closures and by
    /// the caller (it owns no Slint state of its own).
    pub fn wire(&self, window: &QueryWindow) {
        // Initial render so the user sees populated UI the first time they
        // open settings rather than empty placeholders.
        self.refresh_slots(window);
        self.refresh_global(window);

        let ctrl = self.clone();
        let weak = window.as_weak();
        window.on_open_settings(move || {
            if let Some(w) = weak.upgrade() {
                ctrl.refresh_slots(&w);
                ctrl.refresh_options(&w);
                ctrl.refresh_global(&w);
            }
        });

        let ctrl = self.clone();
        let weak = window.as_weak();
        window.on_close_settings(move || {
            // Persist on close — the toggle/set callbacks also persist, but
            // close acts as a final commit point if anything fell through.
            if let Some(w) = weak.upgrade() {
                let _ = ctrl.persist();
                w.set_current_view(0);
            }
        });

        let ctrl = self.clone();
        let weak = window.as_weak();
        window.on_select_plugin(move |idx| {
            if let Some(w) = weak.upgrade() {
                w.set_selected_plugin_index(idx);
                ctrl.refresh_options(&w);
            }
        });

        let ctrl = self.clone();
        let weak = window.as_weak();
        window.on_toggle_plugin(move |name, enabled| {
            ctrl.set_enabled(&name, enabled);
            if let Some(w) = weak.upgrade() {
                ctrl.refresh_slots(&w);
            }
        });

        // Per-keystroke edits intentionally skip `refresh_options` /
        // `refresh_global`. Writing the new value back into the bound
        // Slint property re-renders the TextInput and yanks focus out
        // of it on every character. Persistence happens; on next
        // settings-open the user sees the saved value.
        let ctrl = self.clone();
        window.on_set_option_string(move |plugin, key, value| {
            ctrl.set_option_for_kind(&plugin, &key, value.as_str());
        });

        let ctrl = self.clone();
        window.on_set_option_bool(move |plugin, key, value| {
            ctrl.set_option(&plugin, key.as_str(), JsonValue::Bool(value));
        });

        let ctrl = self.clone();
        window.on_set_option_int(move |plugin, key, value| {
            ctrl.set_option(&plugin, key.as_str(), JsonValue::Number(i64::from(value).into()));
        });

        // Enum cycling is a click, not a keystroke — refreshing the
        // option model is what makes the new choice visible, and there's
        // no TextInput focus to preserve.
        let ctrl = self.clone();
        let weak = window.as_weak();
        window.on_cycle_option_enum(move |plugin, key| {
            ctrl.set_option_for_kind(&plugin, &key, "");
            if let Some(w) = weak.upgrade() {
                ctrl.refresh_options(&w);
            }
        });

        let ctrl = self.clone();
        let weak = window.as_weak();
        window.on_capture_hotkey(move |meta, control, shift, alt, key| -> bool {
            let mods = (u8::from(meta) * MOD_META)
                | (u8::from(control) * MOD_CONTROL)
                | (u8::from(shift) * MOD_SHIFT)
                | (u8::from(alt) * MOD_ALT);
            let Some(spec) = format_hotkey_spec(mods, key.as_str()) else {
                return false;
            };
            ctrl.set_hotkey(&spec);
            ctrl.reregister_hotkey(&spec);
            if let Some(w) = weak.upgrade() {
                ctrl.refresh_global(&w);
            }
            true
        });

        let ctrl = self.clone();
        let weak = window.as_weak();
        window.on_reset_hotkey_to_default(move || {
            ctrl.set_hotkey(crate::settings::DEFAULT_HOTKEY);
            ctrl.reregister_hotkey(crate::settings::DEFAULT_HOTKEY);
            if let Some(w) = weak.upgrade() {
                ctrl.refresh_global(&w);
            }
        });

        let weak = window.as_weak();
        window.on_select_global(move || {
            // The Slint side already flipped `showing-global` to true; this
            // callback exists so future work (analytics, lazy reads) has a
            // hook without us having to add a property-changed watcher.
            if let Some(w) = weak.upgrade() {
                w.set_showing_global(true);
            }
        });
    }

    fn refresh_global(&self, window: &QueryWindow) {
        let settings = self.inner.settings.lock().expect("settings lock");
        window.set_hotkey_value(SharedString::from(settings.global().hotkey.as_str()));
    }

    fn set_hotkey(&self, value: &str) {
        {
            let mut settings = self.inner.settings.lock().expect("settings lock");
            settings.set_hotkey(value);
        }
        if let Err(err) = self.persist() {
            tracing::warn!(%err, "settings: persist after hotkey-set failed");
        }
    }

    /// Last saved launcher window origin, or `None` if the user has never
    /// dragged the window (or just cleared it via Recenter).
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned — a previous panic while
    /// holding the lock corrupted the shared state and continuing risks
    /// writing the corruption to disk.
    #[must_use]
    pub fn launcher_position(&self) -> Option<WindowPosition> {
        let settings = self.inner.settings.lock().expect("settings lock");
        settings.global().launcher_position
    }

    /// Current `query_history.max_entries` setting. Read on every history
    /// push so the cap takes effect without a daemon restart.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned. See
    /// [`Self::launcher_position`] for the rationale.
    #[must_use]
    pub fn query_history_max_entries(&self) -> usize {
        let settings = self.inner.settings.lock().expect("settings lock");
        settings.query_history_max_entries()
    }

    /// Record a new launcher window origin and flush to disk. Used by the
    /// window layer after the user finishes a drag; persistence is best-
    /// effort because losing the latest position is preferable to a
    /// blocked hide path.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned. See
    /// [`Self::launcher_position`] for the rationale.
    pub fn set_launcher_position(&self, position: WindowPosition) {
        {
            let mut settings = self.inner.settings.lock().expect("settings lock");
            settings.set_launcher_position(position);
        }
        if let Err(err) = self.persist() {
            tracing::warn!(%err, "settings: persist after launcher-position-set failed");
        }
    }

    /// Forget the saved launcher position. The next show recenters via the
    /// existing focused-display math.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned. See
    /// [`Self::launcher_position`] for the rationale.
    pub fn clear_launcher_position(&self) {
        {
            let mut settings = self.inner.settings.lock().expect("settings lock");
            settings.clear_launcher_position();
        }
        if let Err(err) = self.persist() {
            tracing::warn!(%err, "settings: persist after launcher-position-clear failed");
        }
    }

    fn refresh_slots(&self, window: &QueryWindow) {
        let settings = self.inner.settings.lock().expect("settings lock");
        let slots: Vec<PluginSlot> = self
            .inner
            .manifests
            .iter()
            .map(|m| PluginSlot {
                name: SharedString::from(m.name.as_str()),
                display_name: SharedString::from(m.display_name.as_deref().unwrap_or("")),
                enabled: settings.is_plugin_enabled_or_default(&m.name, m.default_enabled),
            })
            .collect();
        window.set_plugin_slots(ModelRc::new(VecModel::from(slots)));
    }

    fn refresh_options(&self, window: &QueryWindow) {
        let idx = usize::try_from(window.get_selected_plugin_index().max(0)).unwrap_or(0);
        let Some(manifest) = self.inner.manifests.get(idx) else {
            window.set_plugin_options(ModelRc::new(VecModel::from(Vec::<PluginOption>::new())));
            window.set_selected_plugin_version(SharedString::default());
            window.set_selected_plugin_description(SharedString::default());
            return;
        };

        let meta = plugin_metadata(manifest);
        window.set_selected_plugin_version(SharedString::from(
            meta.version
                .as_deref()
                .map(|v| format!("v{v}"))
                .unwrap_or_default()
                .as_str(),
        ));
        window.set_selected_plugin_description(SharedString::from(meta.description.as_deref().unwrap_or_default()));

        let defs = &manifest.parsed_options().defs;
        let settings = self.inner.settings.lock().expect("settings lock");
        let user_opts = settings.plugin_options(&manifest.name);
        let options: Vec<PluginOption> = defs
            .iter()
            .map(|def| option_row(&manifest.name, def, user_opts))
            .collect();
        window.set_plugin_options(ModelRc::new(VecModel::from(options)));
    }

    fn set_enabled(&self, plugin: &str, enabled: bool) {
        {
            let mut settings = self.inner.settings.lock().expect("settings lock");
            settings.set_plugin_enabled(plugin, enabled);
        }
        if let Err(err) = self.persist() {
            tracing::warn!(plugin, %err, "settings: persist after toggle failed");
        }
    }

    fn set_option(&self, plugin: &str, key: &str, value: JsonValue) {
        {
            let mut settings = self.inner.settings.lock().expect("settings lock");
            settings.set_plugin_option(plugin, key, value);
        }
        if let Err(err) = self.persist() {
            tracing::warn!(plugin, key, %err, "settings: persist after option-set failed");
        }
    }

    /// String callback handles two kinds: actual `string` options (pass
    /// through), and `int`/`enum` options where Slint hands us text we have
    /// to interpret. Looking the def up keeps each call type-correct.
    fn set_option_for_kind(&self, plugin: &str, key: &str, raw: &str) {
        let Some(manifest) = self.inner.manifests.iter().find(|m| m.name == plugin) else {
            return;
        };
        let Some(def) = manifest.parsed_options().defs.iter().find(|d| d.key == key) else {
            return;
        };
        let value = match &def.kind {
            OptionKind::Int { min, max, .. } => {
                let Ok(parsed) = raw.trim().parse::<i64>() else {
                    return;
                };
                let clamped = clamp_int(parsed, *min, *max);
                JsonValue::Number(clamped.into())
            }
            // For enums, the Slint-side cycles by re-emitting the CSV of
            // choices; we pick the next one after the current value.
            OptionKind::Enum { default, choices } => {
                let current = {
                    let settings = self.inner.settings.lock().expect("settings lock");
                    settings
                        .plugin_options(plugin)
                        .get(key)
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| default.clone())
                };
                let next = next_choice(&current, choices);
                JsonValue::String(next)
            }
            // Bool flows through `set_option_bool` in practice; if Slint
            // ever routes through the string callback we still want the
            // literal raw string preserved (same as the `String` case).
            OptionKind::String { .. } | OptionKind::Bool { .. } => JsonValue::String(raw.to_owned()),
        };

        self.set_option(plugin, key, value);
    }

    fn persist(&self) -> std::io::Result<()> {
        let settings = self.inner.settings.lock().expect("settings lock");
        settings.save()
    }
}

fn option_row(
    plugin_name: &str,
    def: &OptionDef,
    user_opts: &std::collections::HashMap<String, JsonValue>,
) -> PluginOption {
    let value = user_opts.get(&def.key).cloned().unwrap_or_else(|| def.default_json());

    let base = PluginOption {
        plugin_name: SharedString::from(plugin_name),
        key: SharedString::from(def.key.as_str()),
        label: SharedString::from(def.label.as_str()),
        kind: SharedString::default(),
        value_string: SharedString::default(),
        value_bool: false,
        value_int: 0,
        int_min: 0,
        int_max: 0,
        has_int_min: false,
        has_int_max: false,
        enum_choices: SharedString::default(),
    };

    match &def.kind {
        OptionKind::String { .. } => PluginOption {
            kind: SharedString::from("string"),
            value_string: SharedString::from(value.as_str().unwrap_or_default()),
            ..base
        },
        OptionKind::Bool { .. } => PluginOption {
            kind: SharedString::from("bool"),
            value_bool: value.as_bool().unwrap_or(false),
            ..base
        },
        OptionKind::Int { min, max, .. } => {
            let raw = value.as_i64().unwrap_or(0);
            PluginOption {
                kind: SharedString::from("int"),
                value_string: SharedString::from(raw.to_string()),
                value_int: i32::try_from(raw).unwrap_or(0),
                int_min: min.and_then(|m| i32::try_from(m).ok()).unwrap_or(0),
                int_max: max.and_then(|m| i32::try_from(m).ok()).unwrap_or(0),
                has_int_min: min.is_some(),
                has_int_max: max.is_some(),
                ..base
            }
        }
        OptionKind::Enum { choices, .. } => PluginOption {
            kind: SharedString::from("enum"),
            value_string: SharedString::from(value.as_str().unwrap_or_default()),
            enum_choices: SharedString::from(choices.join(",")),
            ..base
        },
    }
}

fn clamp_int(v: i64, min: Option<i64>, max: Option<i64>) -> i64 {
    let mut out = v;
    if let Some(lo) = min {
        out = out.max(lo);
    }
    if let Some(hi) = max {
        out = out.min(hi);
    }
    out
}

/// Return the choice immediately after `current` in `choices`, wrapping
/// around at the end. Used to implement the v1 "click-to-cycle" enum widget;
/// proper dropdowns are post-v1.
fn next_choice(current: &str, choices: &[String]) -> String {
    let idx = choices.iter().position(|c| c == current).unwrap_or(0);
    let next = (idx + 1) % choices.len().max(1);
    choices.get(next).cloned().unwrap_or_else(|| current.to_owned())
}

/// Bitfield for the four keyboard modifier flags Slint surfaces on a key
/// event. Packed into a `u8` so the formatter signature stays small and
/// dodges pedantic clippy's `struct_excessive_bools` /
/// `fn_params_excessive_bools` lints. Constructed via OR of the constants
/// below at the call site.
type KeyMods = u8;
const MOD_META: KeyMods = 1 << 0;
const MOD_CONTROL: KeyMods = 1 << 1;
const MOD_SHIFT: KeyMods = 1 << 2;
const MOD_ALT: KeyMods = 1 << 3;

/// Turn a Slint key-pressed event into the `global_hotkey`-compatible
/// spec string. Returns `None` for modifier-only / unprintable inputs the
/// hotkey parser couldn't accept anyway.
///
/// Slint encodes special keys as private-use Unicode codepoints — see
/// [`canonical_key`] for the mapping table.
fn format_hotkey_spec(mods: KeyMods, key: &str) -> Option<String> {
    // `global_hotkey` accepts `Cmd` and `Super` for the meta modifier but
    // rejects "Meta". Picking the platform-natural label keeps the settings
    // field readable for the local user — both parse identically on either
    // OS so a settings.toml carried across platforms still works.
    #[cfg(target_os = "macos")]
    let meta_mod = "Cmd";
    #[cfg(not(target_os = "macos"))]
    let meta_mod = "Super";

    let canonical = canonical_key(key)?;

    // Require at least one modifier unless the key is a function key. F1–F24
    // are explicit shortcut keys and reasonable to use bare; letting plain
    // Space / a / Enter register would turn every typed key into a launcher
    // trigger and lock the user out of the capture flow.
    if mods == 0 && !is_function_key(&canonical) {
        return None;
    }

    let mut parts: Vec<&str> = Vec::with_capacity(5);

    if mods & MOD_META != 0 {
        parts.push(meta_mod);
    }
    if mods & MOD_CONTROL != 0 {
        parts.push("Control");
    }
    if mods & MOD_ALT != 0 {
        parts.push("Alt");
    }
    if mods & MOD_SHIFT != 0 {
        parts.push("Shift");
    }

    let mut out = parts.join("+");

    if !out.is_empty() {
        out.push('+');
    }
    out.push_str(&canonical);
    Some(out)
}

/// Map a Slint `event.text` string to the canonical key name
/// `global_hotkey` parses. Returns `None` for the things we deliberately
/// refuse to capture (Escape, raw modifier presses, multi-character text).
fn canonical_key(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }

    let mut chars = text.chars();
    let first = chars.next()?;

    if chars.next().is_some() {
        // Multi-character event.text — Slint's special-key codepoints are
        // single chars; anything else is ambiguous and we'd rather refuse.
        return None;
    }

    // Codepoint table mirrors `i-slint-common/key_codes.rs`. The W3C-shaped
    // names on the right column are what `global_hotkey` accepts. Modifier-
    // only presses (0x10-0x18) deliberately fall through to None so the
    // capture handler waits for a real key.
    match first as u32 {
        0x1B => None, // Escape — handled in Slint
        0x08 => Some("Backspace".into()),
        0x09 => Some("Tab".into()),
        0x0A | 0x0D => Some("Enter".into()),
        0x20 => Some("Space".into()),
        0x7F => Some("Delete".into()),
        0xF700 => Some("ArrowUp".into()),
        0xF701 => Some("ArrowDown".into()),
        0xF702 => Some("ArrowLeft".into()),
        0xF703 => Some("ArrowRight".into()),
        // F1..=F24 — Slint allocates F1 at 0xF704, contiguous through F24.
        c if (0xF704..=0xF71B).contains(&c) => Some(format!("F{}", c - 0xF703)),
        0xF727 => Some("Insert".into()),
        0xF729 => Some("Home".into()),
        0xF72B => Some("End".into()),
        0xF72C => Some("PageUp".into()),
        0xF72D => Some("PageDown".into()),
        0xF72F => Some("ScrollLock".into()),
        0xF730 => Some("Pause".into()),
        0xF735 => Some("ContextMenu".into()),
        _ if first.is_ascii_alphabetic() => Some(first.to_ascii_uppercase().to_string()),
        _ if first.is_ascii_digit() => Some(first.to_string()),
        _ => None,
    }
}

/// Whether `name` is one of the F1..=F24 keys that `canonical_key`
/// produces. Used by [`format_hotkey_spec`] to relax the "must have a
/// modifier" rule for function keys, which are explicit shortcut keys.
fn is_function_key(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('F') else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_int_respects_both_bounds() {
        assert_eq!(clamp_int(5, Some(1), Some(10)), 5);
        assert_eq!(clamp_int(0, Some(1), Some(10)), 1);
        assert_eq!(clamp_int(11, Some(1), Some(10)), 10);
        assert_eq!(clamp_int(11, None, None), 11);
        assert_eq!(clamp_int(11, None, Some(7)), 7);
        assert_eq!(clamp_int(-2, Some(0), None), 0);
    }

    #[test]
    fn next_choice_wraps_around() {
        let choices: Vec<String> = ["a", "b", "c"].iter().map(|&s| s.to_owned()).collect();
        assert_eq!(next_choice("a", &choices), "b");
        assert_eq!(next_choice("c", &choices), "a");
    }

    #[test]
    fn next_choice_unknown_starts_from_first() {
        let choices: Vec<String> = ["a", "b"].iter().map(|&s| s.to_owned()).collect();
        // If the current value isn't a known choice (manifest renamed since
        // the user's last save), restart from the first valid option.
        assert_eq!(next_choice("xxx", &choices), "b");
    }

    fn manifest_with_options(name: &str, options_json: &str) -> Manifest {
        let raw = format!(r#"{{ "name": "{name}", "options": {options_json} }}"#);
        Manifest::parse(raw.as_bytes()).expect("manifest parse")
    }

    #[test]
    fn plugin_metadata_uses_display_name_when_present() {
        let m = Manifest::parse(
            br#"{ "name": "echo", "displayName": "Echo Plugin", "version": "1.2.3", "description": "Echoes text." }"#,
        )
        .expect("parse");
        let meta = plugin_metadata(&m);
        assert_eq!(meta.display_name, "Echo Plugin");
        assert_eq!(meta.version.as_deref(), Some("1.2.3"));
        assert_eq!(meta.description.as_deref(), Some("Echoes text."));
    }

    #[test]
    fn plugin_metadata_falls_back_to_name_when_display_name_absent() {
        let m = Manifest::parse(br#"{ "name": "echo" }"#).expect("parse");
        let meta = plugin_metadata(&m);
        assert_eq!(meta.display_name, "echo");
        assert!(meta.version.is_none());
        assert!(meta.description.is_none());
    }

    #[test]
    fn refresh_slots_reflects_manifest_default_enabled() {
        // A plugin with defaultEnabled: false must show disabled in the slot
        // when the user has no explicit toggle.
        let tmp = std::env::temp_dir().join(format!(
            "high-beam-settings-default-enabled-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("settings.toml");
        let settings = Settings::load_from(&path);
        let manifest = Manifest::parse(br#"{ "name": "vault", "defaultEnabled": false }"#).expect("parse");
        let ctrl = SettingsController::new(vec![manifest], settings);

        // Reach into inner to verify the enabled value used for the slot.
        let settings_guard = ctrl.inner.settings.lock().unwrap();
        let manifest_ref = &ctrl.inner.manifests[0];
        let enabled = settings_guard.is_plugin_enabled_or_default(&manifest_ref.name, manifest_ref.default_enabled);
        assert!(!enabled, "slot should reflect manifest defaultEnabled: false");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn controller_persists_toggle() {
        let tmp = std::env::temp_dir().join(format!(
            "high-beam-settings-ui-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("settings.toml");
        let settings = Settings::load_from(&path);
        let manifests = vec![manifest_with_options("echo", "[]")];
        let ctrl = SettingsController::new(manifests, settings);

        ctrl.set_enabled("echo", false);

        let reloaded = Settings::load_from(&path);
        assert!(!reloaded.is_plugin_enabled("echo"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn controller_refresh_global_reads_back_hotkey() {
        // Whatever value is persisted on disk must round-trip through the
        // controller's view-side state — the Slint hotkey-value property is
        // populated from this read.
        let tmp = std::env::temp_dir().join(format!(
            "high-beam-settings-refresh-global-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("settings.toml");

        let mut s = Settings::load_from(&path);
        s.set_hotkey("Shift+F2");
        s.save().expect("save");

        let reloaded = Settings::load_from(&path);
        let ctrl = SettingsController::new(vec![], reloaded);
        let observed = ctrl.inner.settings.lock().unwrap().global().hotkey.clone();
        assert_eq!(observed, "Shift+F2");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn controller_persists_hotkey() {
        let tmp = std::env::temp_dir().join(format!(
            "high-beam-settings-hotkey-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("settings.toml");
        let settings = Settings::load_from(&path);
        let manifests: Vec<Manifest> = vec![];
        let ctrl = SettingsController::new(manifests, settings);

        ctrl.set_hotkey("Cmd+K");

        let reloaded = Settings::load_from(&path);
        assert_eq!(reloaded.global().hotkey, "Cmd+K");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn controller_persists_and_clears_launcher_position() {
        // The window layer writes the user-dropped origin through the
        // controller after every hide. A fresh load must observe both the
        // set and the subsequent clear (Recenter button).
        let tmp = std::env::temp_dir().join(format!(
            "high-beam-settings-launcher-pos-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("settings.toml");
        let settings = Settings::load_from(&path);
        let ctrl = SettingsController::new(vec![], settings);

        ctrl.set_launcher_position(WindowPosition { x: 320, y: 180 });
        assert_eq!(ctrl.launcher_position(), Some(WindowPosition { x: 320, y: 180 }));

        // Round-trip through disk so we know the value persisted (not
        // just lived in the in-memory Mutex).
        let reloaded = Settings::load_from(&path);
        assert_eq!(
            reloaded.global().launcher_position,
            Some(WindowPosition { x: 320, y: 180 })
        );

        ctrl.clear_launcher_position();
        assert!(ctrl.launcher_position().is_none());
        let reloaded = Settings::load_from(&path);
        assert!(reloaded.global().launcher_position.is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn controller_clamps_int_via_string_callback() {
        let tmp = std::env::temp_dir().join(format!(
            "high-beam-settings-int-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("settings.toml");
        let settings = Settings::load_from(&path);
        let manifests = vec![manifest_with_options(
            "p",
            r#"[{"key":"limit","type":"int","default":10,"min":1,"max":20}]"#,
        )];
        let ctrl = SettingsController::new(manifests, settings);

        ctrl.set_option_for_kind("p", "limit", "999");
        let reloaded = Settings::load_from(&path);
        let opts = reloaded.plugin_options("p");
        assert_eq!(opts.get("limit"), Some(&JsonValue::Number(20.into())));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn format_hotkey_spec_round_trips_through_global_hotkey() {
        // Whatever spec we produce must parse cleanly via the same path the
        // daemon uses on startup — guards against drift between our
        // formatter and `global_hotkey`'s parser, and against modifier-name
        // mismatch (the parser rejects "Meta" but accepts "Cmd"/"Super").
        use std::str::FromStr;

        let specs = [
            format_hotkey_spec(MOD_SHIFT, " ").expect("shift+space"),
            format_hotkey_spec(MOD_META, "k").expect("meta+k"),
            format_hotkey_spec(MOD_META | MOD_SHIFT, "K").expect("meta+shift+k"),
            format_hotkey_spec(MOD_CONTROL | MOD_ALT, "5").expect("ctrl+alt+5"),
            format_hotkey_spec(MOD_META | MOD_CONTROL | MOD_SHIFT | MOD_ALT, "f").expect("all-mods"),
        ];

        for spec in &specs {
            global_hotkey::hotkey::HotKey::from_str(spec).unwrap_or_else(|err| {
                panic!("global_hotkey rejected `{spec}`: {err}");
            });
        }
    }

    #[test]
    fn format_hotkey_spec_matches_default() {
        // The default-reset path writes `DEFAULT_HOTKEY` directly, so a
        // Shift+Space capture must format to the same byte sequence — keeps
        // the UI and the disk in agreement when the user later clears.
        assert_eq!(
            format_hotkey_spec(MOD_SHIFT, " ").as_deref(),
            Some(crate::settings::DEFAULT_HOTKEY),
        );
    }

    #[test]
    fn format_hotkey_spec_uppercases_letters() {
        let spec = format_hotkey_spec(MOD_META, "k").expect("meta+k");
        assert!(spec.ends_with("+K"), "expected canonical key K, got {spec}");
        // Modifier label is platform-dependent — `Cmd` on macOS, `Super` on
        // Linux. Either way, `global_hotkey` accepts it.
        assert!(spec == "Cmd+K" || spec == "Super+K", "unexpected meta label: {spec}");
    }

    #[test]
    fn format_hotkey_spec_skips_escape() {
        // 0x1B is Slint's Escape — Slint handles that path via the
        // reset-to-default callback, so the formatter refuses.
        assert_eq!(format_hotkey_spec(0, "\u{1b}"), None);
    }

    #[test]
    fn format_hotkey_spec_rejects_unmapped_text() {
        assert_eq!(format_hotkey_spec(MOD_META, ""), None);
        assert_eq!(format_hotkey_spec(0, "ab"), None);
    }

    #[test]
    fn format_hotkey_spec_rejects_bare_key_without_modifier() {
        // Plain Space / a / Enter would catch the user every time they
        // typed it — refuse unless a modifier is held.
        assert_eq!(format_hotkey_spec(0, " "), None);
        assert_eq!(format_hotkey_spec(0, "a"), None);
        assert_eq!(format_hotkey_spec(0, "\u{0a}"), None);
        assert_eq!(format_hotkey_spec(0, "1"), None);
    }

    #[test]
    fn format_hotkey_spec_allows_bare_function_key() {
        // F-keys are explicit shortcut keys; binding F12 alone is fine.
        let spec = format_hotkey_spec(0, "\u{f70f}").expect("bare F12");
        assert_eq!(spec, "F12");
    }

    #[test]
    fn is_function_key_matches_only_f_keys() {
        assert!(is_function_key("F1"));
        assert!(is_function_key("F12"));
        assert!(is_function_key("F24"));
        assert!(!is_function_key("F"));
        assert!(!is_function_key("Fa"));
        assert!(!is_function_key("ArrowUp"));
        assert!(!is_function_key("Space"));
    }

    #[test]
    fn canonical_key_maps_slint_special_codepoints() {
        assert_eq!(canonical_key("\u{f700}").as_deref(), Some("ArrowUp"));
        assert_eq!(canonical_key("\u{f704}").as_deref(), Some("F1"));
        assert_eq!(canonical_key("\u{f70f}").as_deref(), Some("F12"));
        assert_eq!(canonical_key("\u{f71b}").as_deref(), Some("F24"));
        assert_eq!(canonical_key(" ").as_deref(), Some("Space"));
        assert_eq!(canonical_key("\u{0a}").as_deref(), Some("Enter"));
        assert_eq!(canonical_key("\u{09}").as_deref(), Some("Tab"));
        assert_eq!(canonical_key("\u{08}").as_deref(), Some("Backspace"));
        assert_eq!(canonical_key("\u{7f}").as_deref(), Some("Delete"));
        assert_eq!(canonical_key("\u{f727}").as_deref(), Some("Insert"));
        assert_eq!(canonical_key("\u{f729}").as_deref(), Some("Home"));
        assert_eq!(canonical_key("\u{f72b}").as_deref(), Some("End"));
        assert_eq!(canonical_key("\u{f72c}").as_deref(), Some("PageUp"));
        assert_eq!(canonical_key("\u{f72d}").as_deref(), Some("PageDown"));
    }

    #[test]
    fn canonical_key_refuses_modifier_only_codepoints() {
        // 0x10-0x18 are Slint's Shift/Control/Alt/AltGr/CapsLock/ShiftR/
        // ControlR/Meta/MetaR — surface as None so the capture handler
        // keeps listening for a real key.
        for c in 0x10u32..=0x18 {
            let raw = char::from_u32(c).unwrap().to_string();
            assert_eq!(canonical_key(&raw), None, "codepoint {c:#x} should not map");
        }
    }
}
