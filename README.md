# High Beam

[![CI](https://github.com/Mechazawa/high-beam/actions/workflows/ci.yml/badge.svg)](https://github.com/Mechazawa/high-beam/actions/workflows/ci.yml)
[![Plugins](https://github.com/Mechazawa/high-beam/actions/workflows/plugins.yml/badge.svg)](https://github.com/Mechazawa/high-beam/actions/workflows/plugins.yml)

A keyboard launcher in the Spotlight / Alfred / Raycast / Ulauncher
class. Native Rust, single-file JS plugins, no npm at runtime.

Pre-release.

## Install

```
cargo install --path .
```

Packaged builds (.app, .dmg, .deb, .pacman, .rpm, tarball) via
`just bundle` / `just bundle-linux`. See
[docs/distribution.md](docs/distribution.md).

macOS post-install:

```
xattr -dr com.apple.quarantine /Applications/HighBeam.app
```

(self-signed; the command strips the download quarantine.)

## Use

- **macOS**: `Shift+Space` to open. Configurable in Settings → Global.
- **Linux**: bind `highbeam --open` to a hotkey in your WM/DE. See
  [docs/platform.md](docs/platform.md) for GNOME / KDE / sway /
  Hyprland snippets.

Type to query, ↑/↓ to highlight, Enter to invoke, Esc to dismiss. The
Core built-in handles `settings`, `install <manifest-url>`, `reload`,
`update`, `shutdown`, `sleep`, `lock`, etc.

## Plugins

Single-directory `manifest.json` + `plugin.js`. Reference plugins
under `plugins/`; bundled ones get seeded on first launch. Authoring
guide: [docs/plugin-authoring.md](docs/plugin-authoring.md). API
reference: [docs/sdk-reference.md](docs/sdk-reference.md). Dynamic,
stateful screens: [docs/views.md](docs/views.md).

## Theming

`theme.toml` in the config dir overrides the bundled
yosemite-spotlight default. Token reference:
[docs/theming.md](docs/theming.md).

## Internals

[docs/internals.md](docs/internals.md) — stack rationale, threading
model, cancellation contract, Slint gotchas. `src/lib.rs` is the
module map.

## License

MIT.
