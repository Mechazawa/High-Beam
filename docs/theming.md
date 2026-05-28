# Theming

High Beam reads themes from a `themes/` folder in the platform config
dir. Each `<name>.toml` in that folder is a selectable theme.

- macOS: `~/Library/Application Support/high-beam/themes/`
- Linux: `$XDG_CONFIG_HOME/high-beam/themes/` (default
  `~/.config/high-beam/themes/`)

On first launch from a packaged build, the shipped themes are seeded
into this folder (new shipped themes from later updates are added too,
but a file you've edited is never overwritten).

Pick the active theme in Settings → Global → Theme. The dropdown lists
`default` (the builtin yosemite-spotlight, used when no file is selected)
plus every `<name>.toml` in the folder. After editing a theme file, hit
**Reload theme** next to the dropdown to re-read and apply it without a
restart. The list itself refreshes whenever you open Settings, so a file
you just dropped in (or removed) shows up on the next open.

A missing or malformed file silently falls back to the builtin, so a
typo or a stale selection can't prevent the daemon from starting.

To author a new theme, copy any seeded file (e.g.
`themes/yosemite-spotlight.toml`) and edit it.

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
backend pick the OS default — that's how the builtin `default` theme matches
the pre-theming build visually.

The `size_*` fields are pixels.

### Window

`width` is the fixed window width in pixels. The window is non-resizable;
height grows with result count up to a hardcoded cap (~9 rows).

`border_radius` is in pixels.

## Dark / light mode

Each `[colors]`, `[font]`, and `[window]` table may carry `.dark` / `.light`
sub-tables that override the base for the matching appearance; anything a
sub-table omits inherits from the base above it. The `theme_mode` setting in
`settings.toml` chooses which to paint — `"auto"` (default: follows the OS
and repaints live when it flips), `"dark"`, or `"light"`.

TOML requires the base fields before any sub-table:

```toml
[colors]
background = "#ffffffea"   # light base
foreground = "#1d1d1f"

[colors.dark]              # applied in dark mode
background = "#1c1c1eee"
foreground = "#f5f5f7"
```
