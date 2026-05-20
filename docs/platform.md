# Platform notes

## Targets

- **macOS** — primary. Modern (14+) initially; older support is best-effort.
- **Linux** — secondary. X11 + Wayland under common compositors (GNOME, KDE,
  sway/Hyprland).

## Global hotkey

### macOS

`global-hotkey` registers `Shift+Space` at startup. The hotkey is currently
hardcoded; user configurability is post-v1.

If macOS prompts for input-monitoring or accessibility permission, grant it
once — High Beam itself doesn't need it (the `global-hotkey` crate operates
without entitlements on modern macOS), but some AppleScript-driven Core
built-ins (sleep / restart) may.

### Linux

There is no portable global-hotkey API on Wayland (the spec doesn't include
one; the GlobalShortcuts portal's coverage varies by compositor). High Beam
punts: bind `highbeam --open` in your WM / DE keyboard settings. The same
mechanism works on X11, so we don't have to maintain a separate X path.

Example (GNOME Settings → Keyboard → View and Customize Shortcuts → Custom):

| Name        | Command            | Shortcut       |
|-------------|--------------------|----------------|
| High Beam   | `highbeam --open`  | `Super+Space`  |

`highbeam --open` returns immediately if a daemon is running; otherwise it
starts one and opens the window.

## Single-instance lock

`highbeam --open` from the CLI is the canonical way to summon the window on
Linux, so the daemon needs single-instance semantics. Implementation: unix
domain socket.

- macOS: `~/Library/Application Support/high-beam/high-beam.sock`
- Linux: `$XDG_RUNTIME_DIR/high-beam.sock` (falls back to
  `$XDG_STATE_HOME/high-beam/high-beam.sock` when the runtime dir is unset)

First instance binds and listens; subsequent invocations connect, send a
command (`open\n`), and exit. The wire format is intentionally tiny —
newline-terminated ASCII commands.

If a previous daemon crashed and left a stale socket behind, the next start
detects it (connect fails) and replaces it.

## App data paths

| Purpose             | macOS                                                   | Linux                                              |
|---------------------|---------------------------------------------------------|----------------------------------------------------|
| Config (`theme.toml`) | `~/Library/Application Support/high-beam/`            | `$XDG_CONFIG_HOME/high-beam/`                      |
| Plugins             | `~/Library/Application Support/high-beam/plugins/`      | `$XDG_DATA_HOME/high-beam/plugins/`                |
| Plugin cache        | `~/Library/Caches/high-beam/plugins/<name>/`            | `$XDG_CACHE_HOME/high-beam/plugins/<name>/`        |
| Frecency DB         | `~/Library/Application Support/high-beam/frecency.sqlite` | `$XDG_DATA_HOME/high-beam/frecency.sqlite`       |
| Per-plugin log      | `<plugin_dir>/plugin.log`                               | `<plugin_dir>/plugin.log`                          |

Resolved via the `directories` crate. Parent directories are created lazily
on first write.

## Permissions

### macOS

The Core built-in's sleep / restart / shut-down actions run via
`osascript`; macOS may prompt for automation permission the first time
High Beam tries to control Finder / System Events. Grant once.

Plugins that use `highbeam:system.applescript` will likewise need the
relevant automation entitlements granted to High Beam.

App-launching (`highbeam:actions.openUrl` against a `.app` path, or the
example `app-launcher` plugin) doesn't need any entitlement.

### Linux

No special permissions. Plugins that run subprocesses via
`highbeam:system.exec` or use `highbeam:actions.exec` rely on whatever
`PATH` the daemon was started with.

## Known platform limitations

- **macOS Dock / Cmd-Tab**: the daemon currently appears briefly in the
  Dock when launched via `cargo run`. Setting
  `NSApp.setActivationPolicy(.accessory)` or shipping as a `.app` with
  `LSUIElement=1` in `Info.plist` would fix this — TODO when a real
  packaged build lands.
- **Linux Wayland global hotkeys**: no portable API yet. The WM keybind +
  `highbeam --open` flow is the v1 answer.
- **No real macOS vibrancy**: v1 uses the `background` alpha channel for
  flat translucency. `NSVisualEffectView` integration is post-v1.

## Distribution (out of scope for v1)

- macOS: build to a `.app` bundle, eventually code-signed + notarized.
- Linux: plain binary; Flatpak / AUR packaging later.
- Auto-updater: deferred. v1 is `cargo install` / manual.
