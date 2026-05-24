//! Keyboard-modifier bitfield + hotkey-spec formatting.
//!
//! Lives between Slint (which surfaces modifier flags + the typed key) and
//! `global_hotkey` (which parses an accelerator string like `"Cmd+Space"`).
//! Pure functions, no state — the daemon registers the formatted spec; the
//! settings UI captures the next press and round-trips through here.

/// Bitfield for the four keyboard modifier flags Slint surfaces on a key
/// event. Packed into a `u8` so the formatter signature stays small and
/// dodges pedantic clippy's `struct_excessive_bools` /
/// `fn_params_excessive_bools` lints. Constructed via OR of the constants
/// below at the call site.
pub type KeyMods = u8;
pub const MOD_META: KeyMods = 1 << 0;
pub const MOD_CONTROL: KeyMods = 1 << 1;
pub const MOD_SHIFT: KeyMods = 1 << 2;
pub const MOD_ALT: KeyMods = 1 << 3;

/// Turn a Slint key-pressed event into the `global_hotkey`-compatible
/// spec string. Returns `None` for modifier-only / unprintable inputs the
/// hotkey parser couldn't accept anyway.
///
/// Slint encodes special keys as private-use Unicode codepoints — see the
/// private `canonical_key` mapping table below.
#[must_use]
pub fn format_hotkey_spec(mods: KeyMods, key: &str) -> Option<String> {
    let canonical = canonical_key(key)?;

    // Require at least one modifier unless the key is a function key. F1–F24
    // are explicit shortcut keys and reasonable to use bare; letting plain
    // Space / a / Enter register would turn every typed key into a launcher
    // trigger and lock the user out of the capture flow.
    if mods == 0 && !is_function_key(&canonical) {
        return None;
    }

    // Slint's documented macOS quirk: it remaps the physical Command key
    // onto `event.modifiers.control` and the physical Control key onto
    // `event.modifiers.meta` so cross-platform code can read one flag for
    // "the conventional shortcut modifier". The hotkey persistence layer
    // wants the physical-key spec — un-swap here. On Linux + Windows the
    // flags map straight through.
    #[cfg(target_os = "macos")]
    let (meta_label, control_label) = ("Control", "Cmd");
    #[cfg(not(target_os = "macos"))]
    let (meta_label, control_label) = ("Super", "Control");

    let mut parts: Vec<&str> = Vec::with_capacity(5);

    if mods & MOD_META != 0 {
        parts.push(meta_label);
    }

    if mods & MOD_CONTROL != 0 {
        parts.push(control_label);
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
        c @ 0xF704..=0xF71B => Some(format!("F{}", c - 0xF703)),
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

/// Map a stored modifier choice (`"Alt"`, `"Shift"`, `"Cmd"`, `"Ctrl"`)
/// onto the Slint flag name Slint reports for that physical key. The
/// macOS swap (Slint reports physical Cmd as `control` and physical Ctrl
/// as `meta`) is resolved here so the Slint side can branch on a stable
/// `"alt"` / `"shift"` / `"control"` / `"meta"` string.
#[must_use]
pub fn slint_flag_for_modifier(setting: &str) -> &'static str {
    match setting {
        "Shift" => "shift",
        "Cmd" => {
            #[cfg(target_os = "macos")]
            {
                "control"
            }
            #[cfg(not(target_os = "macos"))]
            {
                "meta"
            }
        }
        "Ctrl" => {
            #[cfg(target_os = "macos")]
            {
                "meta"
            }
            #[cfg(not(target_os = "macos"))]
            {
                "control"
            }
        }
        // "Alt" + any unknown value falls back to the default — the
        // setting normalisation already canonicalises on load.
        _ => "alt",
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

    #[cfg(target_os = "macos")]
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
    fn format_hotkey_spec_unswaps_macos_modifiers() {
        // The bug being guarded: Slint maps physical Cmd onto its
        // `modifiers.control` flag for cross-platform consistency. Our
        // formatter un-swaps so the persisted spec reads as the user
        // physically pressed it.
        let cmd_press = format_hotkey_spec(MOD_CONTROL, "k").expect("control flag");
        let ctrl_press = format_hotkey_spec(MOD_META, "k").expect("meta flag");

        #[cfg(target_os = "macos")]
        {
            assert_eq!(cmd_press, "Cmd+K", "Slint control flag = physical Cmd on macOS");
            assert_eq!(ctrl_press, "Control+K", "Slint meta flag = physical Ctrl on macOS");
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(cmd_press, "Control+K");
            assert_eq!(ctrl_press, "Super+K");
        }
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
    fn format_hotkey_spec_skips_escape_and_unmapped_text() {
        // 0x1B is Slint's Escape — handled separately by the reset-to-default
        // callback. Empty / multi-char `event.text` is ambiguous so we refuse.
        assert_eq!(format_hotkey_spec(0, "\u{1b}"), None);
        assert_eq!(format_hotkey_spec(MOD_META, ""), None);
        assert_eq!(format_hotkey_spec(0, "ab"), None);
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
