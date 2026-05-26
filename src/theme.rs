//! User-editable theme loaded from `theme.toml` in the platform config dir.
//!
//! Each theme describes two appearances in one file. Top-level sections
//! (`[colors]`, `[font]`, `[window]`) hold the base values; nested
//! sub-tables (`[colors.dark]`, `[colors.light]`, `[font.dark]`, ...)
//! override those for the matching system appearance. Anything a mode
//! sub-table doesn't set falls back to the base — so the simplest theme
//! file still parses and looks identical in both modes.
//!
//! The user's [`crate::settings::Settings::theme_mode`] decides which
//! variant `apply_theme` paints. Reload is restart-only; in `Auto` mode a
//! background watcher (see [`crate::os_appearance`]) repaints on system
//! flips without a restart.
//!
//! Token surface mirrors `QueryWindow`'s `in-out` properties. Missing or
//! malformed file falls back to the bundled yosemite-spotlight default.

use std::fs;
use std::path::PathBuf;

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

/// Lenient, infallible parse — the read-side counterpart of [`as_str`].
///
/// Matched case-insensitively (so the title-cased settings-UI labels and a
/// hand-capitalised `settings.toml` both work) and **total**: any unknown
/// string degrades to [`ThemeMode::Auto`] rather than erroring, so a typo
/// in the config never blocks daemon startup. That silent fallback is the
/// deliberate trade — this is why it's `From` (infallible) and not
/// `FromStr` (which would force a `Result` every caller just unwraps to
/// the default anyway).
///
/// [`as_str`]: ThemeMode::as_str
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
    /// Forwards to [`Self::default_bundled`] so the auto-derived path can't
    /// accidentally produce a theme where the dark variant is silently
    /// equal to the light one.
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

/// Light-mode defaults — also serve as the `base` everywhere no override
/// is provided.
impl Default for Colors {
    fn default() -> Self {
        // These hex literals are the runtime source of truth — the bundled
        // `themes/yosemite-spotlight.toml` and the .slint property defaults
        // exist for parity; only Rust reads at runtime. `expect` (not
        // `unwrap`) so a typo in one of these literals panics with a
        // message that names the offender at the panic site.
        let parse = |hex: &str| parse_hex_color(hex).unwrap_or_else(|| panic!("default theme: invalid hex {hex:?}"));
        Self {
            background: parse("#ffffffea"),
            foreground: parse("#1d1d1f"),
            muted: parse("#86868b"),
            highlight: parse("#0a84ff"),
            selection: parse("#0a84ff33"),
            border: parse("#00000010"),
        }
    }
}

impl Colors {
    /// Hardcoded dark counterpart of [`Self::default`] — the runtime
    /// source of truth for the dark variant of the bundled theme.
    /// Hand-picked to track the macOS Spotlight dark aesthetic: near-black
    /// translucent panel, light text, same blue accents.
    #[must_use]
    fn default_dark() -> Self {
        let parse =
            |hex: &str| parse_hex_color(hex).unwrap_or_else(|| panic!("default dark theme: invalid hex {hex:?}"));
        Self {
            background: parse("#1d1d1faa"),
            foreground: parse("#f5f5f7"),
            muted: parse("#86868b"),
            highlight: parse("#0a84ff"),
            selection: parse("#0a84ff33"),
            border: parse("#ffffff10"),
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
    /// Load the user's `theme.toml` from the platform config path. Missing
    /// file → silent default; malformed → warning + default. A typo in the
    /// theme must not prevent the daemon from starting.
    #[must_use]
    pub fn load_or_default() -> Self {
        let Some(path) = default_theme_path() else {
            tracing::warn!("theme: could not resolve config dir; using default");
            return Self::default_bundled();
        };

        match fs::read_to_string(&path) {
            Ok(text) => Self::from_toml_or_default(&text, &path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default_bundled(),
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "theme: could not read; using default");
                Self::default_bundled()
            }
        }
    }

    /// In-Rust fallback when the file is missing or unreadable. Light
    /// variant uses [`Colors::default`]; dark variant uses
    /// [`Colors::default_dark`]. Font/window are appearance-agnostic, so
    /// both variants share the base values.
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

        // The user's `[colors]` flat fields are *per-field* overrides on
        // top of the mode-specific defaults — so nudging one base colour
        // (e.g. `background = "#fafafa"`) doesn't drop the bundled dark
        // palette for the untouched fields. Each variant gets its own
        // base resolution from its own appearance defaults, then the
        // matching `[colors.dark]` / `[colors.light]` sub-table layers on
        // top.
        //
        // Backwards-compat note: a pre-dark-mode theme file that sets
        // *every* base field renders identically in both modes (the user
        // explicitly took ownership of every value). A file that sets
        // only some base fields gets mode-specific defaults for the
        // rest — almost always what the user wants.
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

    /// Pick the variant to paint, given the user's preference and the
    /// current OS appearance. `Auto` follows the system; `Dark` / `Light`
    /// pin regardless of what the OS reports.
    ///
    /// When the OS reports [`Appearance::Unspecified`] (Linux without an
    /// `org.freedesktop.portal.Settings` responder, headless tests, the
    /// rare error path of the platform probe), `Auto` falls back to the
    /// light variant. The fallback lives at this site rather than at the
    /// `os_appearance` boundary so it's discoverable next to the rest of
    /// the variant-selection policy, and so a future
    /// `preferred_fallback_appearance` setting can be slotted in here
    /// without touching the watcher.
    #[must_use]
    pub fn variant_for(&self, mode: ThemeMode, system: Appearance) -> &ThemeVariant {
        match (mode, system) {
            (ThemeMode::Dark, _) | (ThemeMode::Auto, Appearance::Dark) => &self.dark,
            (ThemeMode::Light, _) | (ThemeMode::Auto, Appearance::Light | Appearance::Unspecified) => &self.light,
        }
    }
}

/// Path the daemon reads on startup. `None` when the project dir can't be
/// resolved (no `$HOME` etc.).
fn default_theme_path() -> Option<PathBuf> {
    paths::config_dir().ok().map(|dir| dir.join("theme.toml"))
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
    use std::path::Path;

    #[test]
    fn default_light_variant_matches_yosemite_spotlight_values() {
        let theme = Theme::default_bundled();
        let light = &theme.light;
        assert_eq!(light.colors.background, Color::from_argb_u8(0xEA, 0xFF, 0xFF, 0xFF));
        assert_eq!(light.colors.foreground, Color::from_argb_u8(0xFF, 0x1D, 0x1D, 0x1F));
        assert_eq!(light.colors.muted, Color::from_argb_u8(0xFF, 0x86, 0x86, 0x8B));
        assert_eq!(light.colors.highlight, Color::from_argb_u8(0xFF, 0x0A, 0x84, 0xFF));
        assert_eq!(light.colors.selection, Color::from_argb_u8(0x33, 0x0A, 0x84, 0xFF));
        assert_eq!(light.colors.border, Color::from_argb_u8(0x10, 0x00, 0x00, 0x00));
        assert!((light.font.size_query - 32.0).abs() < f32::EPSILON);
        assert!((light.font.size_title - 14.0).abs() < f32::EPSILON);
        assert!((light.font.size_subtitle - 12.0).abs() < f32::EPSILON);
        assert!((light.window.width - 760.0).abs() < f32::EPSILON);
        assert!((light.window.border_radius - 14.0).abs() < f32::EPSILON);
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
            Color::from_argb_u8(0xAA, 0x1D, 0x1D, 0x1F)
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
    fn bundled_yosemite_spotlight_matches_default() {
        // Drift between the bundled theme file and the in-Rust defaults
        // would be a silent UX surprise — this test catches it.
        let text = include_str!("../themes/yosemite-spotlight.toml");
        let theme = Theme::from_toml(text).expect("bundled theme parses");
        assert_eq!(theme, Theme::default_bundled());
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
