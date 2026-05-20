//! User-editable theme loaded from `theme.toml` in the platform config dir.
//!
//! The token surface mirrors what `QueryWindow` exposes as `in-out` properties.
//! Loading is best-effort: a missing or malformed file falls back to the
//! bundled yosemite-spotlight default. Reload is restart-only — there is no
//! file watcher.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use slint::Color;

use crate::paths;

/// Resolved theme tokens. Field defaults reproduce the bundled
/// yosemite-spotlight theme; partial user overrides merge against them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Theme {
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

impl Default for Colors {
    fn default() -> Self {
        Self {
            // The hex literals here are the source of truth for the bundled
            // yosemite-spotlight theme. `themes/yosemite-spotlight.toml` and
            // the Slint property defaults exist for documentation/UI parity
            // but this is the only value Rust reads at runtime.
            background: parse_hex_color("#ffffffea").unwrap(),
            foreground: parse_hex_color("#1d1d1f").unwrap(),
            muted: parse_hex_color("#86868b").unwrap(),
            highlight: parse_hex_color("#0a84ff").unwrap(),
            selection: parse_hex_color("#0a84ff33").unwrap(),
            border: parse_hex_color("#00000010").unwrap(),
        }
    }
}

impl Default for Font {
    fn default() -> Self {
        Self {
            // Empty = let Slint's backend pick the OS default font; that's
            // what today's UI uses, so a missing theme.toml is visually
            // identical to the pre-theming build.
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
    /// Resolve the platform-specific config path and load the user's
    /// `theme.toml` if present. Missing file = silent default. Malformed file
    /// logs a warning and still returns the default — the daemon must not
    /// fail to start because of a typo in a theme.
    #[must_use]
    pub fn load_or_default() -> Self {
        let Some(path) = default_theme_path() else {
            eprintln!("theme: could not resolve config dir; using default");
            return Self::default();
        };
        match fs::read_to_string(&path) {
            Ok(text) => Self::from_toml_or_default(&text, &path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                eprintln!("theme: could not read {}: {err}", path.display());
                Self::default()
            }
        }
    }

    /// Parse a theme document, merging present fields over the default. Any
    /// parse error (malformed TOML, bad hex string) falls back to the full
    /// default with a one-line warning so the app remains startable.
    #[must_use]
    pub fn from_toml_or_default(text: &str, source: &std::path::Path) -> Self {
        match Self::from_toml(text) {
            Ok(theme) => theme,
            Err(err) => {
                eprintln!(
                    "theme: malformed {}: {err}; using default",
                    source.display()
                );
                Self::default()
            }
        }
    }

    /// Parse a theme document with strict error reporting. Public so tests can
    /// assert on specific failure modes; production loading routes through
    /// [`Self::from_toml_or_default`] which swallows errors.
    ///
    /// # Errors
    ///
    /// Returns a human-readable error string if the TOML is malformed or any
    /// color value is not parseable as a hex literal.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let raw: RawTheme = toml::from_str(text).map_err(|e| e.to_string())?;
        let defaults = Self::default();
        let colors = raw.colors.unwrap_or_default().apply(&defaults.colors)?;
        let font = raw.font.unwrap_or_default().apply(&defaults.font);
        let window = raw.window.unwrap_or_default().apply(&defaults.window);
        Ok(Self {
            colors,
            font,
            window,
        })
    }
}

/// Path the daemon reads on startup. `None` when the platform's project dir
/// can't be resolved (no `$HOME` etc.) — extremely rare but possible in CI.
#[must_use]
pub fn default_theme_path() -> Option<PathBuf> {
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
    background: Option<String>,
    foreground: Option<String>,
    muted: Option<String>,
    highlight: Option<String>,
    selection: Option<String>,
    border: Option<String>,
}

impl RawColors {
    fn apply(self, defaults: &Colors) -> Result<Colors, String> {
        Ok(Colors {
            background: parse_optional(self.background, defaults.background, "colors.background")?,
            foreground: parse_optional(self.foreground, defaults.foreground, "colors.foreground")?,
            muted: parse_optional(self.muted, defaults.muted, "colors.muted")?,
            highlight: parse_optional(self.highlight, defaults.highlight, "colors.highlight")?,
            selection: parse_optional(self.selection, defaults.selection, "colors.selection")?,
            border: parse_optional(self.border, defaults.border, "colors.border")?,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawFont {
    family: Option<String>,
    size_query: Option<f32>,
    size_title: Option<f32>,
    size_subtitle: Option<f32>,
}

impl RawFont {
    fn apply(self, defaults: &Font) -> Font {
        Font {
            family: self.family.unwrap_or_else(|| defaults.family.clone()),
            size_query: self.size_query.unwrap_or(defaults.size_query),
            size_title: self.size_title.unwrap_or(defaults.size_title),
            size_subtitle: self.size_subtitle.unwrap_or(defaults.size_subtitle),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawWindow {
    width: Option<f32>,
    border_radius: Option<f32>,
}

impl RawWindow {
    fn apply(self, defaults: &Window) -> Window {
        Window {
            width: self.width.unwrap_or(defaults.width),
            border_radius: self.border_radius.unwrap_or(defaults.border_radius),
        }
    }
}

fn parse_optional(raw: Option<String>, fallback: Color, field: &str) -> Result<Color, String> {
    match raw {
        None => Ok(fallback),
        Some(s) => parse_hex_color(&s).ok_or_else(|| format!("{field}: invalid color {s:?}")),
    }
}

/// Parse `#rgb`, `#rrggbb`, or `#rrggbbaa`. Returns `None` for any other
/// shape so the caller can report a localised error and fall through to the
/// default.
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
    let nibble = match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => return None,
    };
    Some(nibble << 4 | nibble)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn default_theme_matches_yosemite_spotlight_values() {
        let theme = Theme::default();
        assert_eq!(
            theme.colors.background,
            Color::from_argb_u8(0xEA, 0xFF, 0xFF, 0xFF)
        );
        assert_eq!(
            theme.colors.foreground,
            Color::from_argb_u8(0xFF, 0x1D, 0x1D, 0x1F)
        );
        assert_eq!(
            theme.colors.muted,
            Color::from_argb_u8(0xFF, 0x86, 0x86, 0x8B)
        );
        assert_eq!(
            theme.colors.highlight,
            Color::from_argb_u8(0xFF, 0x0A, 0x84, 0xFF)
        );
        assert_eq!(
            theme.colors.selection,
            Color::from_argb_u8(0x33, 0x0A, 0x84, 0xFF)
        );
        assert_eq!(
            theme.colors.border,
            Color::from_argb_u8(0x10, 0x00, 0x00, 0x00)
        );
        assert!((theme.font.size_query - 32.0).abs() < f32::EPSILON);
        assert!((theme.font.size_title - 14.0).abs() < f32::EPSILON);
        assert!((theme.font.size_subtitle - 12.0).abs() < f32::EPSILON);
        assert!((theme.window.width - 760.0).abs() < f32::EPSILON);
        assert!((theme.window.border_radius - 14.0).abs() < f32::EPSILON);
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
        // case-insensitive
        assert_eq!(
            parse_hex_color("#ABCDEF").unwrap(),
            parse_hex_color("#abcdef").unwrap()
        );
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
    fn from_toml_partial_override_keeps_defaults_for_missing_fields() {
        let text = "[colors]\nbackground = \"#000000\"\n";
        let theme = Theme::from_toml(text).expect("partial override parses");
        // overridden
        assert_eq!(
            theme.colors.background,
            Color::from_argb_u8(0xFF, 0x00, 0x00, 0x00)
        );
        // untouched
        assert_eq!(theme.colors.foreground, Theme::default().colors.foreground);
        assert_eq!(theme.font.family, Theme::default().font.family);
        assert!(theme.font.family.is_empty(), "default family is empty");
        assert!((theme.window.width - 760.0).abs() < f32::EPSILON);
    }

    #[test]
    fn from_toml_empty_string_returns_full_default() {
        let theme = Theme::from_toml("").expect("empty parses");
        assert_eq!(theme, Theme::default());
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
    fn from_toml_or_default_swallows_errors() {
        let path = Path::new("/tmp/does-not-exist.toml");
        let theme = Theme::from_toml_or_default("not = [valid", path);
        assert_eq!(theme, Theme::default());
    }

    #[test]
    fn bundled_yosemite_spotlight_matches_default() {
        // The bundled theme is the documented source of truth for what the
        // app looks like when no user theme.toml is present. Drift between
        // this file and the in-Rust defaults would be a silent UX surprise.
        let text = include_str!("../themes/yosemite-spotlight.toml");
        let theme = Theme::from_toml(text).expect("bundled theme parses");
        assert_eq!(theme, Theme::default());
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
        assert_eq!(theme.font.family, "Monaco");
        assert!((theme.font.size_query - 24.0).abs() < f32::EPSILON);
        assert!((theme.font.size_title - 14.0).abs() < f32::EPSILON);
        assert!((theme.window.width - 900.0).abs() < f32::EPSILON);
        assert!((theme.window.border_radius - 14.0).abs() < f32::EPSILON);
    }
}
