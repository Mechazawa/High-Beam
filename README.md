# High Beam

[![CI](https://github.com/Mechazawa/high-beam/actions/workflows/ci.yml/badge.svg)](https://github.com/Mechazawa/high-beam/actions/workflows/ci.yml)
[![Plugins](https://github.com/Mechazawa/high-beam/actions/workflows/plugins.yml/badge.svg)](https://github.com/Mechazawa/high-beam/actions/workflows/plugins.yml)

A keyboard launcher in the Spotlight / Alfred / Raycast / Ulauncher class.
Hit a global hotkey, an overlay window appears, type a query, ranked
results stream in from plugins, Enter executes the chosen action.

<!-- TODO: screenshot -->

## Status

Pre-release / personal-use. The Rust rewrite (this repository) supersedes
two earlier iterations:

- v1 — Electron + Vue + JS (kept locally in `legacy/`)
- v2 — Electron + React + TypeScript (kept locally in `legacy-v2/`)
- v3 — this native Rust binary

The major v3 pieces — daemon, plugin runtime, SDK, frecency, theming,
per-plugin logging — are all in. The pre-v1.0 backlog is in
[Roadmap](#roadmap) below.

## Install

`cargo install --path .` builds the daemon binary. For local development:

    just check       # cargo fmt + clippy + tests
    just run         # cargo run
    just lint        # strict clippy (-D warnings)
    just lint-pedantic   # clippy::pedantic suggestions
    just test-plugins    # vitest in each example plugin

Stable Rust, edition 2024. Pinned via `rust-toolchain.toml`.

## Usage

- **macOS**: hit `Shift+Space` to open the launcher (default; not yet
  user-configurable).
- **Linux**: bind `highbeam --open` to a hotkey in your WM / DE keyboard
  settings — there's no portable global hotkey API on Wayland, so High Beam
  punts to the WM. See [docs/platform.md](docs/platform.md).

Type into the input; results stream in. Up/down to highlight a row; Enter
to invoke its action. Esc or click-away closes; the daemon stays running.

Type `settings` (or press Cmd+, when the launcher is open) to open the
settings view — toggle plugins on/off and edit per-plugin options. Restart
to apply.

## What ships

The host binary plus one in-process built-in (Core: shutdown / sleep /
restart / lock / exit High Beam / version readout). Plus, in
`examples/plugins/`, eight reference plugins you can drop into your
plugins directory:

| Plugin           | What it does                                                         |
|------------------|----------------------------------------------------------------------|
| `echo`           | Minimal smoke test — copies your input to the clipboard.             |
| `echo-ts`        | TypeScript variant of `echo` with a `tsconfig.json`.                 |
| `slow-echo`      | Streaming + abort demo (three rows, 300ms apart).                    |
| `frecency-demo`  | Three equal-weight rows; pick one and watch it bubble up.            |
| `calculator`     | Pinned, inline math (`1+2*3`, `sqrt(2)`, etc.). npm-free.            |
| `http-codes`     | Type `http 404`; opens MDN.                                          |
| `paper-size`     | Type `paper A4`; copies `210 x 297 mm`.                              |
| `dnd`            | Type `spell fireball`; opens the D&D 5e reference.                   |
| `app-launcher`   | Cross-platform Spotlight equivalent (mac `.app`s + Linux `.desktop`). |

Copy or symlink any of them into your plugin directory:

```bash
cp -r examples/plugins/echo ~/Library/Application\ Support/high-beam/plugins/echo  # macOS
cp -r examples/plugins/echo "$XDG_DATA_HOME/high-beam/plugins/echo"                # Linux
```

…then restart High Beam.

## Plugin authoring

Plugins are single-file JS programs the host loads at startup. No npm at
runtime, no bundler. Each plugin lives in its own directory with a
`manifest.json` and a `plugin.js` exporting an `async function* query()`:

```js
import { copy } from 'highbeam:actions';

export async function* query(input, signal) {
  if (!input) return;
  yield {
    key: 'hello',
    title: `hello: ${input}`,
    action: copy(input),
  };
}
```

The full plugin authoring guide is in
[docs/plugin-authoring.md](docs/plugin-authoring.md) — manifest schema, the
`highbeam:*` module reference, capabilities, TypeScript setup, vitest
testing recipe.

## Theming

Drop a `theme.toml` into the config dir to override the bundled default
(which approximates macOS Yosemite Spotlight). Tokens cover colors, fonts,
spacing, window width, and border radius. Restart to apply — there is no
hot-reload in v1.

See [docs/theming.md](docs/theming.md) for the full token reference and a
dark-mode example.

## Platform notes

- macOS uses the `global-hotkey` crate for `Shift+Space`. AppleScript-backed
  Core actions (sleep / restart / shutdown) may prompt for automation
  permission the first time you invoke them.
- Linux Wayland has no portable global-hotkey API; bind `highbeam --open`
  in your WM / DE keyboard settings instead.
- App data paths follow the platform conventions
  (`~/Library/Application Support/high-beam/` on macOS,
  `$XDG_CONFIG_HOME/high-beam/` / `$XDG_DATA_HOME/high-beam/` on Linux).

Full details in [docs/platform.md](docs/platform.md).

## Architecture

[docs/architecture.md](docs/architecture.md) is the contributor's tour —
stack, module map, threading model, the cancellation contract, and the
Slint integration gotchas worth knowing before touching the UI layer.

## Roadmap

The v1 line: launcher + plugin runtime + bundled examples + theming +
logging. After v1, in rough priority order:

- Alt-action / modifier-key alternate action (Cmd+Enter = "open in finder"
  vs Enter = "open")
- `push` action / nested views (the `Action` enum reserves room)
- Forms — multi-field input view dispatched by an `Action` variant
- Detail / preview pane (Yosemite Spotlight's right-side preview)
- Live-reload of plugin options (today: restart-to-apply)
- Strike-out / auto-disable on repeated plugin failures
- Plugin store / install-by-URL (Hammerspoon-style)
- Theme live-reload (watch `theme.toml`)
- Real macOS vibrancy via `NSVisualEffectView`
- Logfile rotation
- User-configurable global hotkey (currently hardcoded)
- Wayland global hotkey via `xdg-desktop-portal`
- Windows port

Explicit non-goals:

- Becoming Node — `highbeam:*` is the only import scheme; npm gravity is
  the failure mode we're avoiding.
- Plugin sandboxing — every major launcher we surveyed (Alfred, Raycast,
  Albert, Ulauncher) relies on curation rather than isolation, so we're not
  paying the DX cost for theatrical safety.

## License

MIT — see `Cargo.toml`.
