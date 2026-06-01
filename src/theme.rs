//! User-editable themes loaded by name from the platform themes dir.
//!
//! The user picks a theme by file stem in settings
//! ([`crate::settings::Settings::theme`]); [`Theme::load_named`] resolves it
//! to `<themes_dir>/<stem>.toml`. The reserved name
//! [`crate::settings::DEFAULT_THEME`] (and any missing/malformed file) falls
//! back to the in-Rust bundled yosemite-spotlight default, so a stale name
//! never blocks startup. [`available_theme_names`] enumerates the dir for the
//! settings dropdown.
//!
//! Each theme describes two appearances in one file. Top-level sections
//! (`[colors]`, `[font]`, `[window]`) hold the base values; nested
//! sub-tables (`[colors.dark]`, `[colors.light]`, `[font.dark]`, ...)
//! override those for the matching system appearance. Anything a mode
//! sub-table doesn't set falls back to the base — so the simplest theme
//! file still parses and looks identical in both modes.
//!
//! The user's [`crate::settings::Settings::theme_mode`] decides which
//! variant `apply_theme` paints. The settings UI swaps the active theme and
//! re-applies live (no restart); in `Auto` mode a background watcher (see
//! [`crate::os_appearance`]) repaints on system flips.
//!
//! Token surface mirrors `QueryWindow`'s `in-out` properties.

use std::fs;

use serde::Deserialize;
use slint::Color;

use crate::os_appearance::Appearance;
use crate::paths;

/// User's chosen theme-mode preference. `Auto` follows the OS appearance
/// (the default); `Dark` / `Light` pin to one variant regardless of system
/// state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    Auto,
    Dark,
    Light,
}

impl ThemeMode {
    /// The on-disk string spelling — kept in one place so the loader and
    /// the writer agree on case.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// Read-side counterpart of `as_str`: case-insensitive and total —
/// unknown input falls back to `Auto` so a config typo can't block
/// startup. `From` (not `FromStr`) because that fallback is deliberate,
/// never an error.
impl From<&str> for ThemeMode {
    fn from(raw: &str) -> Self {
        match raw.to_ascii_lowercase().as_str() {
            "dark" => Self::Dark,
            "light" => Self::Light,
            _ => Self::Auto,
        }
    }
}

/// Resolved theme — both appearance variants are fully merged at load
/// time, so [`Theme::variant_for`] is a cheap lookup. Defaults reproduce
/// the bundled yosemite-spotlight theme (light values for light, dark
/// values for dark); partial user overrides merge against them.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub dark: ThemeVariant,
    pub light: ThemeVariant,
}

impl Default for Theme {
    /// Forwards to `default_bundled` so a derived default can't yield
    /// dark == light.
    fn default() -> Self {
        Self::default_bundled()
    }
}

/// One concrete appearance — what `window::apply_theme` actually paints.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ThemeVariant {
    pub colors: Colors,
    pub font: Font,
    pub window: Window,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Colors {
    pub background: Color,
    pub foreground: Color,
    pub muted: Color,
    pub highlight: Color,
    pub selection: Color,
    pub border: Color,
    /// Tone colours for the plugin-view block set. Used by `Text`'s
    /// `tone` prop and the matching button tones; default values
    /// reproduce the standard macOS error/success/warning hues.
    pub error: Color,
    pub success: Color,
    pub warning: Color,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Font {
    pub family: String,
    pub size_query: f32,
    pub size_title: f32,
    pub size_subtitle: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub width: f32,
    pub border_radius: f32,
}

/// Light-mode defaults; also the `base` wherever no override is given.
impl Default for Colors {
    fn default() -> Self {
        // Runtime source of truth — the bundled .toml mirrors these for
        // parity. `panic` (not silent) names a bad literal at its site.
        let parse = |hex: &str| parse_hex_color(hex).unwrap_or_else(|| panic!("default theme: invalid hex {hex:?}"));
        Self {
            background: parse("#ffffffea"),
            foreground: parse("#1d1d1f"),
            muted: parse("#86868b"),
            highlight: parse("#007aff"),
            selection: parse("#007aff33"),
            border: parse("#00000010"),
            error: parse("#d70015"),
            success: parse("#0c8430"),
            warning: parse("#b75f00"),
        }
    }
}

impl Colors {
    /// Dark counterpart of [`Self::default`] — the bundled dark palette.
    #[must_use]
    fn default_dark() -> Self {
        let parse =
            |hex: &str| parse_hex_color(hex).unwrap_or_else(|| panic!("default dark theme: invalid hex {hex:?}"));
        Self {
            background: parse("#1d1d1fd0"),
            foreground: parse("#f5f5f7"),
            muted: parse("#86868b"),
            highlight: parse("#007aff"),
            selection: parse("#007aff33"),
            border: parse("#ffffff10"),
            // Tones mirror the light palette — like muted/highlight/selection
            // above, only background/foreground/border shift for dark mode.
            error: parse("#d70015"),
            success: parse("#0c8430"),
            warning: parse("#b75f00"),
        }
    }
}

impl Default for Font {
    fn default() -> Self {
        Self {
            // Empty family → Slint picks the OS default.
            family: String::new(),
            size_query: 32.0,
            size_title: 14.0,
            size_subtitle: 12.0,
        }
    }
}

impl Default for Window {
    fn default() -> Self {
        Self {
            width: 760.0,
            border_radius: 14.0,
        }
    }
}

impl Theme {
    /// Load the theme the user selected by `name`. The reserved
    /// [`crate::settings::DEFAULT_THEME`] resolves to the in-Rust builtin; any
    /// other name reads `<themes_dir>/<name>.toml`. Missing file → silent
    /// default; unresolvable dir / malformed file → warning + default. A
    /// stale name must not prevent the daemon from starting.
    #[must_use]
    pub fn load_named(name: &str) -> Self {
        if name == crate::settings::DEFAULT_THEME {
            return Self::default_bundled();
        }

        let Some(dir) = paths::themes_dir() else {
            tracing::warn!(theme = name, "theme: could not resolve themes dir; using default");

            return Self::default_bundled();
        };

        Self::load_named_in(&dir, name)
    }

    /// Read `<dir>/<name>.toml`, falling back to the bundled default on any
    /// read or parse error. Split from [`Self::load_named`] so the on-disk
    /// path is testable against a fixture dir; production resolves `dir` via
    /// [`paths::themes_dir`]. A malformed file must never abort startup.
    #[must_use]
    fn load_named_in(dir: &std::path::Path, name: &str) -> Self {
        let path = dir.join(format!("{name}.toml"));

        match fs::read_to_string(&path) {
            Ok(text) => Self::from_toml_or_default(&text, &path),
            // Not-found (stale selection) and any other read error both fall
            // back to the builtin — no behavioural split, so one arm.
            Err(err) => {
                tracing::warn!(theme = name, path = %path.display(), %err, "theme: could not read; using default");

                Self::default_bundled()
            }
        }
    }

    /// In-Rust fallback for a missing/unreadable file. Font + window are
    /// appearance-agnostic, so both variants share them.
    #[must_use]
    pub fn default_bundled() -> Self {
        Self {
            dark: ThemeVariant {
                colors: Colors::default_dark(),
                font: Font::default(),
                window: Window::default(),
            },
            light: ThemeVariant::default(),
        }
    }

    /// Parse with fallback. Parse errors (malformed TOML, bad hex) log and
    /// return the bundled default so the app stays startable.
    #[must_use]
    pub fn from_toml_or_default(text: &str, source: &std::path::Path) -> Self {
        match Self::from_toml(text) {
            Ok(theme) => theme,
            Err(err) => {
                tracing::warn!(
                    source = %source.display(),
                    %err,
                    "theme: malformed config; using default",
                );
                Self::default_bundled()
            }
        }
    }

    /// Parse with strict errors — public so tests can assert on specific
    /// failure modes. Production uses [`Self::from_toml_or_default`].
    ///
    /// # Errors
    ///
    /// Human-readable error string if the TOML is malformed or any color
    /// value isn't a parseable hex literal.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let raw: RawTheme = toml::from_str(text).map_err(|e| e.to_string())?;
        let defaults = Self::default_bundled();
        let raw_colors = raw.colors.unwrap_or_default();
        let raw_font = raw.font.unwrap_or_default();
        let raw_window = raw.window.unwrap_or_default();

        // `[colors]` flat fields apply per-field over each mode's defaults,
        // so nudging one base colour doesn't drop the dark palette for the
        // rest. The `[colors.dark]` / `[colors.light]` sub-tables layer on top.
        let light_base_colors = raw_colors.base.apply(&defaults.light.colors, "colors")?;
        let dark_base_colors = raw_colors.base.apply(&defaults.dark.colors, "colors")?;
        let light_base_font = raw_font.base.apply(&defaults.light.font);
        let dark_base_font = raw_font.base.apply(&defaults.dark.font);
        let light_base_window = raw_window.base.apply(&defaults.light.window);
        let dark_base_window = raw_window.base.apply(&defaults.dark.window);

        let dark = ThemeVariant {
            colors: match &raw_colors.dark {
                Some(d) => d.apply(&dark_base_colors, "colors.dark")?,
                None => dark_base_colors,
            },
            font: match &raw_font.dark {
                Some(d) => d.apply(&dark_base_font),
                None => dark_base_font,
            },
            window: match &raw_window.dark {
                Some(d) => d.apply(&dark_base_window),
                None => dark_base_window,
            },
        };
        let light = ThemeVariant {
            colors: match &raw_colors.light {
                Some(l) => l.apply(&light_base_colors, "colors.light")?,
                None => light_base_colors,
            },
            font: match &raw_font.light {
                Some(l) => l.apply(&light_base_font),
                None => light_base_font,
            },
            window: match &raw_window.light {
                Some(l) => l.apply(&light_base_window),
                None => light_base_window,
            },
        };

        Ok(Self { dark, light })
    }

    /// Pick the variant to paint. `Auto` follows the system; `Dark` /
    /// `Light` pin regardless. `Auto` + `Unspecified` falls back to light —
    /// the fallback policy lives here, not at the `os_appearance` boundary.
    #[must_use]
    pub fn variant_for(&self, mode: ThemeMode, system: Appearance) -> &ThemeVariant {
        match (mode, system) {
            (ThemeMode::Dark, _) | (ThemeMode::Auto, Appearance::Dark) => &self.dark,
            (ThemeMode::Light, _) | (ThemeMode::Auto, Appearance::Light | Appearance::Unspecified) => &self.light,
        }
    }
}

/// Theme file stems available under the themes dir, sorted. The settings
/// dropdown prepends [`crate::settings::DEFAULT_THEME`] to this; an
/// unresolvable or empty dir yields an empty list (default still selectable).
#[must_use]
pub fn available_theme_names() -> Vec<String> {
    let Some(dir) = paths::themes_dir() else {
        return Vec::new();
    };

    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("toml"))
        .filter_map(|path| path.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
        .collect();
    names.sort_unstable();

    names
}

#[derive(Debug, Default, Deserialize)]
struct RawTheme {
    colors: Option<RawColors>,
    font: Option<RawFont>,
    window: Option<RawWindow>,
}

#[derive(Debug, Default, Deserialize)]
struct RawColors {
    /// Flat fields of `[colors]` inline via flatten — keeps the on-disk
    /// shape (base keys + sub-tables side by side) without duplicating
    /// the field list.
    #[serde(flatten)]
    base: RawColorsOverride,
    dark: Option<RawColorsOverride>,
    light: Option<RawColorsOverride>,
}

#[derive(Debug, Default, Deserialize)]
struct RawColorsOverride {
    background: Option<String>,
    foreground: Option<String>,
    muted: Option<String>,
    highlight: Option<String>,
    selection: Option<String>,
    border: Option<String>,
    error: Option<String>,
    success: Option<String>,
    warning: Option<String>,
}

impl RawColorsOverride {
    /// Apply this partial override on top of `base`. Borrowed `&self` so
    /// the same `[colors]` block can be applied against both the light
    /// and dark defaults (see [`Theme::from_toml`]).
    fn apply(&self, base: &Colors, prefix: &str) -> Result<Colors, String> {
        Ok(Colors {
            background: parse_optional(
                self.background.as_deref(),
                base.background,
                &format!("{prefix}.background"),
            )?,
            foreground: parse_optional(
                self.foreground.as_deref(),
                base.foreground,
                &format!("{prefix}.foreground"),
            )?,
            muted: parse_optional(self.muted.as_deref(), base.muted, &format!("{prefix}.muted"))?,
            highlight: parse_optional(
                self.highlight.as_deref(),
                base.highlight,
                &format!("{prefix}.highlight"),
            )?,
            selection: parse_optional(
                self.selection.as_deref(),
                base.selection,
                &format!("{prefix}.selection"),
            )?,
            border: parse_optional(self.border.as_deref(), base.border, &format!("{prefix}.border"))?,
            error: parse_optional(self.error.as_deref(), base.error, &format!("{prefix}.error"))?,
            success: parse_optional(self.success.as_deref(), base.success, &format!("{prefix}.success"))?,
            warning: parse_optional(self.warning.as_deref(), base.warning, &format!("{prefix}.warning"))?,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawFont {
    #[serde(flatten)]
    base: RawFontOverride,
    dark: Option<RawFontOverride>,
    light: Option<RawFontOverride>,
}

#[derive(Debug, Default, Deserialize)]
struct RawFontOverride {
    family: Option<String>,
    size_query: Option<f32>,
    size_title: Option<f32>,
    size_subtitle: Option<f32>,
}

impl RawFontOverride {
    /// Apply this partial override on top of `base`. See
    /// [`RawColorsOverride::apply`] for why this is `&self`.
    fn apply(&self, base: &Font) -> Font {
        Font {
            family: self.family.clone().unwrap_or_else(|| base.family.clone()),
            size_query: self.size_query.unwrap_or(base.size_query),
            size_title: self.size_title.unwrap_or(base.size_title),
            size_subtitle: self.size_subtitle.unwrap_or(base.size_subtitle),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawWindow {
    #[serde(flatten)]
    base: RawWindowOverride,
    dark: Option<RawWindowOverride>,
    light: Option<RawWindowOverride>,
}

#[derive(Debug, Default, Deserialize)]
struct RawWindowOverride {
    width: Option<f32>,
    border_radius: Option<f32>,
}

impl RawWindowOverride {
    /// Apply this partial override on top of `base`. See
    /// [`RawColorsOverride::apply`] for why this is `&self`.
    fn apply(&self, base: &Window) -> Window {
        Window {
            width: self.width.unwrap_or(base.width),
            border_radius: self.border_radius.unwrap_or(base.border_radius),
        }
    }
}

fn parse_optional(raw: Option<&str>, fallback: Color, field: &str) -> Result<Color, String> {
    match raw {
        None => Ok(fallback),
        Some(s) => parse_hex_color(s).ok_or_else(|| format!("{field}: invalid color {s:?}")),
    }
}

/// Parse `#rgb`, `#rrggbb`, or `#rrggbbaa`. Returns `None` for any other
/// shape so the caller can report a localised error.
fn parse_hex_color(spec: &str) -> Option<Color> {
    let hex = spec.strip_prefix('#')?;
    let (r, g, b, a) = match hex.len() {
        3 => {
            let r = expand_nibble(hex.as_bytes()[0])?;
            let g = expand_nibble(hex.as_bytes()[1])?;
            let b = expand_nibble(hex.as_bytes()[2])?;
            (r, g, b, 0xFF)
        }
        6 => {
            let r = byte_at(hex, 0)?;
            let g = byte_at(hex, 2)?;
            let b = byte_at(hex, 4)?;
            (r, g, b, 0xFF)
        }
        8 => {
            let r = byte_at(hex, 0)?;
            let g = byte_at(hex, 2)?;
            let b = byte_at(hex, 4)?;
            let a = byte_at(hex, 6)?;
            (r, g, b, a)
        }
        _ => return None,
    };
    Some(Color::from_argb_u8(a, r, g, b))
}

fn byte_at(hex: &str, offset: usize) -> Option<u8> {
    u8::from_str_radix(hex.get(offset..offset + 2)?, 16).ok()
}

fn expand_nibble(byte: u8) -> Option<u8> {
    let nibble = u8::try_from((byte as char).to_digit(16)?).ok()?;
    Some(nibble << 4 | nibble)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Per-test scratch directory, unique across parallel tests (distinct
    /// `tag`) and across runs (clock nanos).
    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();

        std::env::temp_dir().join(format!("high-beam-{tag}-{nanos}"))
    }

    #[test]
    fn default_dark_variant_differs_from_light() {
        let theme = Theme::default_bundled();
        assert_ne!(
            theme.dark.colors, theme.light.colors,
            "dark + light defaults should not be identical — the whole point is the swap"
        );
        // Spot-check a couple of expected swaps.
        assert_eq!(
            theme.dark.colors.background,
            Color::from_argb_u8(0xD0, 0x1D, 0x1D, 0x1F)
        );
        assert_eq!(
            theme.dark.colors.foreground,
            Color::from_argb_u8(0xFF, 0xF5, 0xF5, 0xF7)
        );
    }

    #[test]
    fn default_dark_and_light_share_appearance_agnostic_tokens() {
        // Font + window aren't appearance-bound by default — same family
        // and same width regardless of dark/light.
        let theme = Theme::default_bundled();
        assert_eq!(theme.dark.font, theme.light.font);
        assert_eq!(theme.dark.window, theme.light.window);
    }

    #[test]
    fn parse_hex_handles_three_six_and_eight_digit_forms() {
        assert_eq!(
            parse_hex_color("#fff").unwrap(),
            Color::from_argb_u8(0xFF, 0xFF, 0xFF, 0xFF)
        );
        assert_eq!(
            parse_hex_color("#0a84ff").unwrap(),
            Color::from_argb_u8(0xFF, 0x0A, 0x84, 0xFF)
        );
        assert_eq!(
            parse_hex_color("#0a84ff33").unwrap(),
            Color::from_argb_u8(0x33, 0x0A, 0x84, 0xFF)
        );
        assert_eq!(parse_hex_color("#ABCDEF").unwrap(), parse_hex_color("#abcdef").unwrap());
    }

    #[test]
    fn parse_hex_rejects_malformed_input() {
        assert!(parse_hex_color("").is_none());
        assert!(parse_hex_color("ffffff").is_none(), "missing #");
        assert!(parse_hex_color("#ff").is_none(), "wrong length");
        assert!(parse_hex_color("#ggg").is_none(), "non-hex chars");
        assert!(parse_hex_color("#0a84fg").is_none(), "non-hex char in 6");
    }

    #[test]
    fn from_toml_partial_base_override_keeps_mode_defaults_for_missing_fields() {
        // Critical regression test for the dark-mode footgun: a user who
        // nudges ONE base color must keep the bundled dark palette for
        // every field they didn't touch. Old code wholesale-seeded dark
        // from the user's light-overridden base, making dark mode
        // unreadable after a single light tweak.
        let text = "[colors]\nbackground = \"#000000\"\n";
        let theme = Theme::from_toml(text).expect("partial override parses");

        // Touched base field applies to both modes — the user wrote it
        // in `[colors]`, not in a mode-specific sub-table.
        let touched = Color::from_argb_u8(0xFF, 0x00, 0x00, 0x00);
        assert_eq!(theme.light.colors.background, touched);
        assert_eq!(theme.dark.colors.background, touched);

        // Untouched fields take *mode-specific* defaults — light keeps
        // light defaults, dark keeps the bundled dark palette.
        assert_eq!(theme.light.colors.foreground, Colors::default().foreground);
        assert_eq!(theme.dark.colors.foreground, Colors::default_dark().foreground);

        assert_eq!(theme.light.font.family, Font::default().family);
    }

    #[test]
    fn from_toml_empty_string_returns_bundled_default() {
        // Empty TOML → no base override → default light *and* default dark
        // are preserved. Without this round-trip the bundled dark palette
        // would silently collapse to light.
        let theme = Theme::from_toml("").expect("empty parses");
        assert_eq!(theme, Theme::default_bundled());
    }

    #[test]
    fn from_toml_malformed_returns_error() {
        let err = Theme::from_toml("not = [valid").expect_err("malformed should error");
        assert!(!err.is_empty());
    }

    #[test]
    fn from_toml_bad_color_returns_error() {
        let text = "[colors]\nbackground = \"not-a-color\"\n";
        let err = Theme::from_toml(text).expect_err("bad color should error");
        assert!(err.contains("colors.background"), "got: {err}");
    }

    #[test]
    fn from_toml_bad_color_in_dark_override_names_the_subtable() {
        let text = "[colors.dark]\nbackground = \"not-a-color\"\n";
        let err = Theme::from_toml(text).expect_err("bad dark color should error");
        assert!(err.contains("colors.dark.background"), "got: {err}");
    }

    #[test]
    fn from_toml_or_default_swallows_errors() {
        let path = Path::new("/tmp/does-not-exist.toml");
        let theme = Theme::from_toml_or_default("not = [valid", path);
        assert_eq!(theme, Theme::default_bundled());
    }

    #[test]
    fn from_toml_nested_dark_overrides_apply_to_dark_variant_only() {
        // Double-hash raw strings (`r##"…"##`) — single-hash would terminate
        // at the first `"#` inside a hex literal.
        let text = r##"
            [colors]
            background = "#ffffff"
            foreground = "#000000"

            [colors.dark]
            background = "#111111"
        "##;
        let theme = Theme::from_toml(text).expect("parses");
        // Dark: background overridden by dark sub-table, foreground inherits
        // from base.
        assert_eq!(
            theme.dark.colors.background,
            Color::from_argb_u8(0xFF, 0x11, 0x11, 0x11)
        );
        assert_eq!(
            theme.dark.colors.foreground,
            Color::from_argb_u8(0xFF, 0x00, 0x00, 0x00)
        );
        // Light: keeps the base values, untouched by [colors.dark].
        assert_eq!(
            theme.light.colors.background,
            Color::from_argb_u8(0xFF, 0xFF, 0xFF, 0xFF)
        );
        assert_eq!(
            theme.light.colors.foreground,
            Color::from_argb_u8(0xFF, 0x00, 0x00, 0x00)
        );
    }

    #[test]
    fn from_toml_nested_light_overrides_apply_to_light_variant_only() {
        let text = r##"
            [colors]
            background = "#ffffff"

            [colors.light]
            background = "#fafafa"

            [colors.dark]
            background = "#111111"
        "##;
        let theme = Theme::from_toml(text).expect("parses");
        assert_eq!(
            theme.light.colors.background,
            Color::from_argb_u8(0xFF, 0xFA, 0xFA, 0xFA)
        );
        assert_eq!(
            theme.dark.colors.background,
            Color::from_argb_u8(0xFF, 0x11, 0x11, 0x11)
        );
    }

    #[test]
    fn from_toml_partial_base_applies_to_both_variants_but_does_not_collapse_them() {
        // Pre-dark-mode shape (no sub-tables, only base fields) is honoured
        // per-field: the user's touched fields apply to both modes, but
        // *untouched* fields still take mode-specific defaults — so dark
        // mode never accidentally becomes a re-skin of light.
        let text = r##"
            [colors]
            background = "#aabbcc"
            foreground = "#112233"
        "##;
        let theme = Theme::from_toml(text).expect("parses");

        // Touched fields: both modes carry the user's values.
        let touched_background = Color::from_argb_u8(0xFF, 0xAA, 0xBB, 0xCC);
        let touched_foreground = Color::from_argb_u8(0xFF, 0x11, 0x22, 0x33);
        assert_eq!(theme.light.colors.background, touched_background);
        assert_eq!(theme.dark.colors.background, touched_background);
        assert_eq!(theme.light.colors.foreground, touched_foreground);
        assert_eq!(theme.dark.colors.foreground, touched_foreground);

        // Untouched fields: each mode keeps its own defaults (border
        // differs between modes in the bundled palette).
        assert_eq!(theme.light.colors.border, Colors::default().border);
        assert_eq!(theme.dark.colors.border, Colors::default_dark().border);
        assert_ne!(theme.dark.colors.border, theme.light.colors.border);
    }

    #[test]
    fn variant_for_honors_explicit_mode() {
        let theme = Theme::default_bundled();
        // Dark setting wins regardless of system.
        assert_eq!(theme.variant_for(ThemeMode::Dark, Appearance::Light), &theme.dark);
        assert_eq!(theme.variant_for(ThemeMode::Dark, Appearance::Dark), &theme.dark);
        // Light setting wins regardless of system.
        assert_eq!(theme.variant_for(ThemeMode::Light, Appearance::Light), &theme.light);
        assert_eq!(theme.variant_for(ThemeMode::Light, Appearance::Dark), &theme.light);
    }

    #[test]
    fn variant_for_auto_follows_system() {
        let theme = Theme::default_bundled();
        assert_eq!(theme.variant_for(ThemeMode::Auto, Appearance::Dark), &theme.dark);
        assert_eq!(theme.variant_for(ThemeMode::Auto, Appearance::Light), &theme.light);
    }

    #[test]
    fn variant_for_auto_unspecified_falls_back_to_light() {
        // When the OS can't tell us (Linux without a portal responder,
        // headless tests, the rare error path of the platform probe),
        // `Auto` falls back to the light variant. Pinned modes still
        // win regardless of the system signal.
        let theme = Theme::default_bundled();
        assert_eq!(
            theme.variant_for(ThemeMode::Auto, Appearance::Unspecified),
            &theme.light
        );
        // Pinned modes ignore the Unspecified signal entirely.
        assert_eq!(theme.variant_for(ThemeMode::Dark, Appearance::Unspecified), &theme.dark);
        assert_eq!(
            theme.variant_for(ThemeMode::Light, Appearance::Unspecified),
            &theme.light
        );
    }

    #[test]
    fn load_named_default_returns_bundled() {
        // The reserved name never touches disk — it's the in-Rust builtin.
        assert_eq!(
            Theme::load_named(crate::settings::DEFAULT_THEME),
            Theme::default_bundled()
        );
    }

    #[test]
    fn load_named_missing_file_falls_back_to_default() {
        // A name that resolves to a file the themes dir doesn't contain must
        // degrade to the builtin rather than panic or block startup.
        let theme = Theme::load_named("definitely-not-a-real-theme-xyz-9999");
        assert_eq!(theme, Theme::default_bundled());
    }

    #[test]
    fn load_named_in_reads_a_valid_file_from_disk() {
        // Guards the meaning of the corrupt-file test below: prove the on-disk
        // path actually parses a present file, so a fallback-to-default there
        // reflects the parse error rather than an always-default no-op.
        let dir = unique_temp_dir("theme-valid");
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        std::fs::write(dir.join("custom.toml"), "[colors]\nbackground = \"#abcdef\"\n").expect("write fixture");

        let theme = Theme::load_named_in(&dir, "custom");
        assert_eq!(
            theme.light.colors.background,
            Color::from_argb_u8(0xFF, 0xAB, 0xCD, 0xEF)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_named_in_corrupt_file_falls_back_to_default() {
        // A malformed theme file present on disk must degrade to the bundled
        // default, never abort startup — the daemon loads the selected theme
        // at boot, so one bad file would otherwise be a launch blocker.
        let dir = unique_temp_dir("theme-corrupt");
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        std::fs::write(dir.join("broken.toml"), "this = is not [valid toml").expect("write corrupt fixture");

        assert_eq!(Theme::load_named_in(&dir, "broken"), Theme::default_bundled());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundled_yosemite_spotlight_matches_default() {
        // Drift between the bundled theme file and the in-Rust defaults
        // would be a silent UX surprise — this test catches it.
        let text = include_str!("../themes/yosemite-spotlight.toml");
        let theme = Theme::from_toml(text).expect("bundled theme parses");
        assert_eq!(theme, Theme::default_bundled());
    }

    #[test]
    fn every_bundled_theme_file_parses() {
        // Every `.toml` we ship under `themes/` must parse — a typo in a
        // hex literal there would compile cleanly but produce a runtime
        // warning + silent fallback to the default on user install. Walk
        // the directory rather than enumerating filenames so new themes
        // are automatically covered.
        let themes_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("themes");
        let mut count = 0_usize;
        for entry in std::fs::read_dir(&themes_dir).expect("read themes/ dir") {
            let path = entry.expect("readdir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read theme file");
            Theme::from_toml(&text).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            count += 1;
        }
        assert!(count >= 2, "expected at least the bundled defaults under themes/");
    }

    #[test]
    fn font_and_window_partial_overrides_merge() {
        let text = r#"
            [font]
            family = "Monaco"
            size_query = 24

            [window]
            width = 900
        "#;
        let theme = Theme::from_toml(text).expect("parses");
        assert_eq!(theme.light.font.family, "Monaco");
        assert!((theme.light.font.size_query - 24.0).abs() < f32::EPSILON);
        assert!((theme.light.font.size_title - 14.0).abs() < f32::EPSILON);
        assert!((theme.light.window.width - 900.0).abs() < f32::EPSILON);
        assert!((theme.light.window.border_radius - 14.0).abs() < f32::EPSILON);
        // Dark inherits the same base because no font.dark / window.dark
        // sub-table was provided.
        assert_eq!(theme.dark.font, theme.light.font);
        assert_eq!(theme.dark.window, theme.light.window);
    }

    #[test]
    fn font_dark_override_only_affects_dark_variant() {
        let text = r"
            [font]
            size_query = 24

            [font.dark]
            size_query = 30
        ";
        let theme = Theme::from_toml(text).expect("parses");
        assert!((theme.light.font.size_query - 24.0).abs() < f32::EPSILON);
        assert!((theme.dark.font.size_query - 30.0).abs() < f32::EPSILON);
    }
}
