# Theming

High Beam reads a single `theme.toml` from the platform config dir at
startup. Edit it and restart the daemon to apply changes.

- macOS: `~/Library/Application Support/high-beam/theme.toml`
- Linux: `$XDG_CONFIG_HOME/high-beam/theme.toml` (default
  `~/.config/high-beam/theme.toml`)

A missing file silently falls back to the bundled default. A malformed
file logs a warning and still falls back, so a typo can't prevent the
daemon from starting.

The bundled default lives at `themes/yosemite-spotlight.toml`; copy it
as a starting point.

## Token reference

Every field is optional — partial overrides merge over the default.

```toml
[colors]
background = "#ffffffea"     # window fill (alpha permitted)
foreground = "#1d1d1f"       # primary text
muted      = "#86868b"       # subtitle / placeholder text
highlight  = "#0a84ff"       # accent (e.g. selected-row label)
selection  = "#0a84ff33"     # selected-row fill
border     = "#00000010"     # window outline
error      = "#d70015"       # Text tone="error", danger Button bg, error-frame banner
success    = "#0c8430"       # Text tone="success"
warning    = "#b75f00"       # Text tone="warning"

[font]
family       = "SF Pro Display, Inter, system-ui"
size_query   = 32            # input font size, px
size_title   = 14            # result row title
size_subtitle = 12           # result row subtitle

[window]
width         = 760
border_radius = 14
```

### Colors

Accepted forms:

- `#rgb`     — shorthand, alpha implied 0xFF
- `#rrggbb`  — alpha implied 0xFF
- `#rrggbbaa` — explicit alpha

Anything else (missing `#`, wrong length, non-hex chars) is rejected with a
warning and the default value is used for that field.

### Fonts

`family` accepts a CSS-style fallback list. An empty string lets Slint's
backend pick the OS default — that's how a missing `theme.toml` matches the
pre-theming build visually.

The `size_*` fields are pixels.

### Window

`width` is the fixed window width in pixels. The window is non-resizable;
height grows with result count up to a hardcoded cap (~9 rows).

`border_radius` is in pixels.

## Example: dark mode

```toml
[colors]
background = "#1c1c1eee"
foreground = "#f5f5f7"
muted      = "#8e8e93"
highlight  = "#0a84ff"
selection  = "#0a84ff44"
border     = "#ffffff10"
```
