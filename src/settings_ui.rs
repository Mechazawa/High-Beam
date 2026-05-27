//! Settings-view controller: turns plugin manifests + persisted `Settings`
//! into the Slint models the settings view renders, and the inverse —
//! callbacks from the view feed back into a `Mutex<Settings>` that owns
//! disk persistence.
//!
//! Lives in its own module so `app.rs` stays focused on the launcher
//! pipeline; the settings view talks to the same `QueryWindow` but its
//! state is independent of the query/results state.

use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread;

use serde_json::Value as JsonValue;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::QueryWindow;
use crate::daemon::HotkeyRegistration;
use crate::hotkey::{MOD_ALT, MOD_CONTROL, MOD_META, MOD_SHIFT, format_hotkey_spec, slint_flag_for_modifier};
use crate::logging::LogErr;
use crate::plugins::manifest::{Manifest, OptionDef, OptionKind};
use crate::settings::{Settings, WindowPosition};
use crate::ui::{PluginOption, PluginSlot};

/// Display metadata for a plugin, extracted from its manifest and handed to
/// the settings view so the right pane can show a header.
#[derive(Debug, Clone, PartialEq)]
struct PluginMetadata {
    /// Human-readable name — `display_name` when present, otherwise `name`.
    display_name: String,
    /// Optional semver string, ready to render with a `v` prefix.
    version: Option<String>,
    /// Optional human-readable description.
    description: Option<String>,
}

/// Extract display metadata from a manifest for the settings right pane.
fn plugin_metadata(manifest: &Manifest) -> PluginMetadata {
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
    /// Mutators send `()` here; the writer thread drains the queue and
    /// flushes once per burst.
    dirty_tx: mpsc::Sender<()>,
}

impl SettingsController {
    /// Build a controller and spawn the writer thread that drains the
    /// dirty-signal channel.
    #[must_use]
    pub fn new(manifests: Vec<Manifest>, settings: Settings) -> Self {
        let (dirty_tx, dirty_rx) = mpsc::channel::<()>();
        let inner = Arc::new(Inner {
            manifests,
            settings: Mutex::new(settings),
            hotkey_registration: OnceLock::new(),
            dirty_tx,
        });

        spawn_writer_thread(Arc::downgrade(&inner), dirty_rx);

        Self { inner }
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
    /// once per window. `theme` lets the theme-mode handler re-apply the
    /// variant on pick, without waiting for the next OS-appearance flip.
    pub fn wire(&self, window: &QueryWindow, theme: Arc<crate::theme::Theme>) {
        // Populate before the user first opens settings.
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

        let weak = window.as_weak();

        window.on_close_settings(move || {
            // Each mutator already queued a dirty signal, so the writer
            // thread will flush whatever's outstanding. Closing the view
            // doesn't need its own write — the queued one is sufficient.
            if let Some(w) = weak.upgrade() {
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

        let ctrl = self.clone();
        let weak = window.as_weak();

        window.on_set_alt_action_modifier(move |value| {
            ctrl.set_alt_action_modifier(value.as_str());

            if let Some(w) = weak.upgrade() {
                ctrl.refresh_global(&w);
            }
        });

        self.wire_theme_mode_select(window, theme);

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
        window.set_alt_action_modifier(SharedString::from(settings.alt_action_modifier()));
        window.set_alt_action_modifier_flag(SharedString::from(slint_flag_for_modifier(
            settings.alt_action_modifier(),
        )));
        window.set_theme_mode(SharedString::from(theme_mode_label(settings.theme_mode())));
    }

    fn set_hotkey(&self, value: &str) {
        {
            let mut settings = self.inner.settings.lock().expect("settings lock");
            settings.set_hotkey(value);
        }

        self.queue_persist();
    }

    fn set_alt_action_modifier(&self, value: &str) {
        {
            let mut settings = self.inner.settings.lock().expect("settings lock");
            settings.set_alt_action_modifier(value);
        }

        self.queue_persist();
    }

    /// Hook the theme-mode dropdown into `window`. Split from [`Self::wire`]
    /// to keep it under the line cap.
    fn wire_theme_mode_select(&self, window: &QueryWindow, theme: Arc<crate::theme::Theme>) {
        let ctrl = self.clone();
        let weak = window.as_weak();
        window.on_set_theme_mode(move |value| {
            ctrl.set_theme_mode(value.as_str());

            if let Some(w) = weak.upgrade() {
                ctrl.refresh_global(&w);
                // Re-apply now rather than waiting for the next poll tick.
                crate::window::apply_theme(
                    &w,
                    theme.variant_for(ctrl.theme_mode(), crate::os_appearance::current()),
                );
            }
        });
    }

    fn set_theme_mode(&self, value: &str) {
        {
            let mut settings = self.inner.settings.lock().expect("settings lock");
            settings.set_theme_mode(value);
        }

        self.queue_persist();
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

    /// Whether the configured alt-action modifier is set in the modifier
    /// bitfield Slint hands us with an Enter / click event. `mods` follows
    /// the same packing as the hotkey-capture path
    /// (`MOD_META | MOD_CONTROL | …`), so callers OR the four Slint flags
    /// into one byte at the callback boundary.
    ///
    /// Resolves the platform-specific Slint Cmd↔Ctrl swap (the same one
    /// the hotkey formatter has to handle).
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned. See
    /// [`Self::launcher_position`] for the rationale.
    #[must_use]
    pub fn alt_modifier_held(&self, mods: u8) -> bool {
        let settings = self.inner.settings.lock().expect("settings lock");

        match settings.alt_action_modifier() {
            "Alt" => mods & MOD_ALT != 0,
            "Shift" => mods & MOD_SHIFT != 0,
            // Slint reports the physical Cmd key as `control` on macOS;
            // everywhere else, it's `meta` (Super / Win key).
            "Cmd" => {
                #[cfg(target_os = "macos")]
                {
                    mods & MOD_CONTROL != 0
                }
                #[cfg(not(target_os = "macos"))]
                {
                    mods & MOD_META != 0
                }
            }
            "Ctrl" => {
                #[cfg(target_os = "macos")]
                {
                    mods & MOD_META != 0
                }
                #[cfg(not(target_os = "macos"))]
                {
                    mods & MOD_CONTROL != 0
                }
            }
            _ => false,
        }
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

    /// Cheap clone of the current settings, for read-only consumers (the
    /// plugin loader, dispatcher). Persist via the controller's own
    /// `set_*` / `record_loaded_versions` methods rather than mutating the
    /// snapshot — modifications to the returned `Settings` are discarded.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned. See
    /// [`Self::launcher_position`] for the rationale.
    #[must_use]
    pub fn snapshot(&self) -> Settings {
        self.inner.settings.lock().expect("settings lock").clone()
    }

    /// Current `theme_mode` setting. Read by the OS-appearance watcher on
    /// every tick so toggling between `Auto` / `Dark` / `Light` in a
    /// future settings-UI surface takes effect without a daemon restart.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned. See
    /// [`Self::launcher_position`] for the rationale.
    #[must_use]
    pub fn theme_mode(&self) -> crate::theme::ThemeMode {
        self.inner.settings.lock().expect("settings lock").theme_mode()
    }

    /// Record the manifest version each `(plugin, Some(version))` pair was
    /// last loaded with, then persist once. The lifecycle-hook layer uses
    /// this so a crash mid-hook can't replay the work on the next boot.
    /// Empty input is a no-op. A `version` of `None` clears the slot
    /// (e.g. on uninstall).
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned. See
    /// [`Self::launcher_position`] for the rationale.
    pub fn record_loaded_versions(&self, entries: &[(String, Option<String>)]) {
        if entries.is_empty() {
            return;
        }

        {
            let mut s = self.inner.settings.lock().expect("settings lock");

            for (name, version) in entries {
                s.set_last_loaded_version(name, version.clone());
            }
        }

        self.queue_persist();
    }

    /// Record a new launcher window origin and queue a disk flush.
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

        self.queue_persist();
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

        self.queue_persist();
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

        self.queue_persist();
    }

    fn set_option(&self, plugin: &str, key: &str, value: JsonValue) {
        {
            let mut settings = self.inner.settings.lock().expect("settings lock");
            settings.set_plugin_option(plugin, key, value);
        }

        self.queue_persist();
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
                // Stdlib `i64::clamp` would panic if `min > max`; the
                // manifest schema doesn't forbid the inversion, so clamp
                // each bound separately instead.
                let mut clamped = parsed;

                if let Some(lo) = *min {
                    clamped = clamped.max(lo);
                }

                if let Some(hi) = *max {
                    clamped = clamped.min(hi);
                }
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

    fn queue_persist(&self) {
        self.inner
            .dirty_tx
            .send(())
            .log_debug("settings: writer thread receiver gone");
    }

    /// Synchronously save settings to disk, bypassing the writer thread.
    /// For tests that assert on-disk state right after a mutation.
    ///
    /// # Panics
    ///
    /// Panics if the settings mutex is poisoned. See
    /// [`Self::launcher_position`] for the rationale.
    pub fn flush(&self) {
        let settings = self.inner.settings.lock().expect("settings lock");
        settings.save().log_warn("settings: flush failed");
    }
}

/// Block on the dirty-signal channel and flush after each burst. Exits
/// when the last `SettingsController` clone drops the sender.
fn spawn_writer_thread(weak_inner: Weak<Inner>, rx: mpsc::Receiver<()>) {
    let spawn_result = thread::Builder::new()
        .name("highbeam-settings-writer".into())
        .spawn(move || {
            while rx.recv().is_ok() {
                // Drain coalesced signals before each save.
                while rx.try_recv().is_ok() {}

                let Some(inner) = weak_inner.upgrade() else {
                    break;
                };
                let snapshot = match inner.settings.lock() {
                    Ok(g) => g.clone(),
                    Err(err) => {
                        tracing::error!(?err, "settings: writer saw poisoned lock");
                        break;
                    }
                };
                drop(inner);
                snapshot.save().log_warn("settings: writer save failed");
            }
        });

    spawn_result.log_warn("settings: writer thread spawn failed");
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

/// Return the choice immediately after `current` in `choices`, wrapping
/// around at the end. Used to implement the v1 "click-to-cycle" enum widget;
/// proper dropdowns are post-v1.
fn next_choice(current: &str, choices: &[String]) -> String {
    let idx = choices.iter().position(|c| c == current).unwrap_or(0);
    let next = (idx + 1) % choices.len().max(1);

    choices.get(next).cloned().unwrap_or_else(|| current.to_owned())
}

/// Title-cased label for the theme-mode pill in the settings UI.
/// [`crate::theme::ThemeMode::as_str`] returns the lowercase on-disk
/// spelling; this helper provides the display variant the user reads in
/// the click-to-cycle row.
fn theme_mode_label(mode: crate::theme::ThemeMode) -> &'static str {
    use crate::theme::ThemeMode;
    match mode {
        ThemeMode::Auto => "Auto",
        ThemeMode::Dark => "Dark",
        ThemeMode::Light => "Light",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Mutators queue an async write; flush synchronously so we can
        // assert on the on-disk state without racing the writer thread.
        ctrl.flush();

        let reloaded = Settings::load_from(&path);
        let opts = reloaded.plugin_options("p");
        assert_eq!(opts.get("limit"), Some(&JsonValue::Number(20.into())));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn alt_modifier_held_follows_setting() {
        let ctrl = SettingsController::new(Vec::new(), Settings::default());

        // Default setting is "Alt" — only the alt flag triggers.
        assert!(ctrl.alt_modifier_held(MOD_ALT));
        assert!(!ctrl.alt_modifier_held(MOD_META));
        assert!(!ctrl.alt_modifier_held(MOD_CONTROL));
        assert!(!ctrl.alt_modifier_held(MOD_SHIFT));
        assert!(!ctrl.alt_modifier_held(0));

        ctrl.set_alt_action_modifier("Shift");
        assert!(ctrl.alt_modifier_held(MOD_SHIFT));
        assert!(!ctrl.alt_modifier_held(MOD_ALT));

        ctrl.set_alt_action_modifier("Cmd");
        #[cfg(target_os = "macos")]
        {
            // Slint reports physical Cmd as `control` on macOS.
            assert!(ctrl.alt_modifier_held(MOD_CONTROL));
            assert!(!ctrl.alt_modifier_held(MOD_META));
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert!(ctrl.alt_modifier_held(MOD_META));
            assert!(!ctrl.alt_modifier_held(MOD_CONTROL));
        }

        ctrl.set_alt_action_modifier("Ctrl");
        #[cfg(target_os = "macos")]
        {
            assert!(ctrl.alt_modifier_held(MOD_META));
            assert!(!ctrl.alt_modifier_held(MOD_CONTROL));
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert!(ctrl.alt_modifier_held(MOD_CONTROL));
            assert!(!ctrl.alt_modifier_held(MOD_META));
        }

        ctrl.set_alt_action_modifier("Alt");
        assert!(ctrl.alt_modifier_held(MOD_ALT));
    }
}
