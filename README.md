# High Beam

[![CI](https://github.com/Mechazawa/high-beam/actions/workflows/ci.yml/badge.svg)](https://github.com/Mechazawa/high-beam/actions/workflows/ci.yml)
[![Plugins](https://github.com/Mechazawa/high-beam/actions/workflows/plugins.yml/badge.svg)](https://github.com/Mechazawa/high-beam/actions/workflows/plugins.yml)

A keyboard launcher in the Spotlight / Alfred / Raycast / Ulauncher class.
Hit a global hotkey, an overlay window appears, type a query, ranked
results stream in from plugins, Enter executes the chosen action.

<!-- TODO: screenshot -->

## Status

Pre-release. The host pieces — daemon, plugin runtime, SDK, frecency,
theming, per-plugin logging, settings UI — are all in. Pre-v1.0
backlog in [Roadmap](#roadmap).

## Install

`cargo install --path .` builds the daemon binary. For local development:

    just check       # cargo fmt + clippy + tests
    just run         # cargo run
    just lint        # strict clippy (-D warnings)
    just lint-pedantic   # clippy::pedantic suggestions
    just test-plugins    # vitest in each example plugin

Stable Rust, edition 2024. Pinned via `rust-toolchain.toml`.

### Linux

Four package formats out of `just bundle-linux` (run on a Linux host):

- **Portable tarball** — `highbeam-<ver>-linux-x86_64.tar.gz`. Untar
  anywhere, run `install.sh` (defaults to `/usr/local`,
  `PREFIX=$HOME/.local` for sudo-less installs).
- **Arch** — `.pkg.tar.zst` via `cargo packager --formats pacman`.
  Also published to the AUR as `high-beam-bin` (see
  [packaging/aur/README.md](packaging/aur/README.md)).
- **.deb** — `cargo packager --formats deb` for Debian / Ubuntu.
- **.rpm** — via [`cargo-generate-rpm`](https://github.com/cat-in-136/cargo-generate-rpm)
  for Fedora / RHEL / SUSE.

After install, enable the user daemon and bind a WM hotkey:

```bash
systemctl --user enable --now highbeam.service
# Then bind `highbeam --open` to a key in your WM / DE — examples for
# GNOME / KDE / sway / Hyprland in docs/platform.md.
```

Full details in [docs/distribution.md](docs/distribution.md).

### macOS Gatekeeper bypass (one-time, post-install)

After dragging `HighBeam.app` to `/Applications`, run:

```bash
xattr -dr com.apple.quarantine /Applications/HighBeam.app
```

High Beam is self-signed (free path — see `scripts/create-signing-cert.sh`).
The command above strips the download quarantine so macOS launches it
without the "unidentified developer" warning. Notarized builds — which
wouldn't need this step — require a $99/yr Apple Developer ID; see
[docs/distribution.md](docs/distribution.md) for the trade-off.

### Release builds

`.github/workflows/release.yml` builds macOS + Linux artifacts and
publishes a GitHub Release whenever a `v*` tag is pushed. The notes are
AI-summarised via [GitHub Models](https://github.com/marketplace/models)
(free; uses the auto-provisioned `GITHUB_TOKEN`, no extra secret) with
a raw-commit-log fallback; codesigned macOS builds need
`MACOS_CERT_P12_BASE64` + `MACOS_CERT_PASSWORD`. Both are optional — the
release still ships without them. Full setup in
[docs/distribution.md § Release workflow](docs/distribution.md#release-workflow-github-actions).

## Usage

- **macOS**: hit `Shift+Space` to open the launcher (default; configurable
  via Settings → Global → Hotkey, restart to apply).
- **Linux**: bind `highbeam --open` to a hotkey in your WM / DE keyboard
  settings — there's no portable global hotkey API on Wayland, so High Beam
  punts to the WM. See [docs/platform.md](docs/platform.md).

Type into the input; results stream in. Up/down to highlight a row; Enter
to invoke its action. Esc or click-away closes; the daemon stays running.

Type `settings` (or press Cmd+, when the launcher is open) to open the
settings view — toggle plugins on/off and edit per-plugin options. Restart
to apply.

Type `install <url>` to install a plugin from a hosted manifest, `reload`
to hot-swap a plugin's code without restarting the daemon, or `update` to
check every plugin with a `manifestUrl` against its remote counterpart.
See [docs/plugin-authoring.md](docs/plugin-authoring.md#publishing--distribution)
for the publish-side guide.

## What ships

The host binary plus one in-process built-in (Core: shutdown / sleep /
restart / lock / settings / install / reload / update / version
readout). Reference plugins live under `plugins/` in this repo —
each is a single-directory `manifest.json` + `plugin.js` pair. Browse
that directory for the current set; the bundled installers (macOS
.app, .deb, .pacman) seed a curated subset into the user plugin dir on
first launch.

Copy or symlink any plugin directory into your plugin path to use it
without an install:

```bash
cp -r plugins/calculator ~/Library/Application\ Support/high-beam/plugins/calculator  # macOS
cp -r plugins/calculator "$XDG_DATA_HOME/high-beam/plugins/calculator"                # Linux
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

[docs/internals.md](docs/internals.md) is the contributor's tour — stack
rationale, threading model, the cancellation contract, and the Slint
integration gotchas worth knowing before touching the UI layer. For the
module layout, `src/lib.rs` is the source of truth.

## Roadmap

The v1 line: launcher + plugin runtime + bundled examples + theming +
logging. After v1, in rough priority order:

- `push` action / nested views (the `Action` enum reserves room)
- Forms — multi-field input view dispatched by an `Action` variant
- Detail / preview pane (Yosemite Spotlight's right-side preview)
- Live-reload of plugin options (today: restart-to-apply)
- Strike-out / auto-disable on repeated plugin failures
- Theme live-reload (watch `theme.toml`)
- Real macOS vibrancy via `NSVisualEffectView`
- Logfile rotation
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
