# Internals

Load-bearing context that isn't obvious from the code: rationale for the
big choices, threading model, cancellation contract, and the Slint
gotchas that took a day to debug the first time around. Read this when
you're about to touch the host, not when you're using it.

For the module layout, `src/lib.rs` is the source of truth — listing
modules here just drifts on every rename. For the plugin-author API,
see [plugin-authoring.md](./plugin-authoring.md) and
[sdk-reference.md](./sdk-reference.md).

## Stack

| Concern | Choice | Why |
|---|---|---|
| Language | Rust (edition 2024) | Native binary, no GC, mature ecosystem for the pieces we need |
| UI toolkit | Slint | Declarative `.slint` files with runtime-bound properties (good fit for theme tokens); winit windowing |
| Plugin runtime | rquickjs (QuickJS binding) | ~1 MB embed, pure ECMAScript (not Node), per-context memory caps, interrupt hooks for timeouts |
| Persistence | SQLite via `rusqlite` | Frecency + query history. Boring, reliable. |
| Global hotkey | `global-hotkey` (macOS); CLI `--open` (Linux) | Wayland has no portable global-hotkey API; punt to the WM. |

## Threading

Three threads carry the protocol; a handful of small ones do
fire-and-forget housekeeping.

- **Slint event loop (main)** — owns the window, runs Slint callbacks,
  owns the `global-hotkey` manager on macOS. All Slint operations must
  run here; other threads route through `slint::invoke_from_event_loop`.
- **`highbeam-plugin-runtime`** — single-threaded tokio runtime that
  owns every loaded `LoadedPlugin` (rquickjs `AsyncContext`). Plugins
  live here because rquickjs futures are `!Send` across `async_with!`
  under the `parallel` feature, so they can't cross threads.
- **`highbeam-ipc`** — blocks on the unix socket; hops back to the
  Slint thread to show the window.

The hotkey listener thread (`highbeam-hotkey`, macOS only) is a tiny
adapter that drains `global-hotkey` events. The plugin-log writer
(`highbeam-plugin-log`), the launcher-position persister
(`highbeam-settings-position`), and the frecency bumper
(`highbeam-frecency-bump`) are fire-and-forget per their respective
write paths.

## Streaming + cancellation contract

- Plugins yield results via `AsyncIterable`; the host iterates and
  renders progressively.
- On every keystroke (post-debounce), the host calls `cancel.cancel()`
  on the previous dispatch token, which both fires JS-side
  `AbortSignal` listeners and sets the rquickjs interrupt flag.
- All host APIs that do I/O (`http.get`, `fs.readDir`, `system.exec`,
  etc.) accept an optional `signal` and honor it.
- CPU-bound plugins that block synchronously can't be cancelled
  gracefully — the per-plugin `timeoutMs` is the hard kill via the
  rquickjs interrupt hook. The watchdog runs on the blocking thread
  pool so a `while(true){}` JS body can't starve it.

## Frecency

- SQLite table `picks` keyed on `(plugin_name, result_key)`. Columns:
  `picks` count, `last_picked_at` (Unix seconds).
- Ranking: pinned first (sorted by `weight`); non-pinned sorted by
  `weight * frecency_modifier(picks, age)`.
- Modifier (`src/frecency/score.rs`):
  `1.0 + 0.10 * picks * 2^(-age/14d)`. Half-life 14 days; one fresh
  pick is roughly a 10% bump; modifier decays back to 1.0 as picks age.

## Built-in plugins live in the host

Core system actions (shutdown / restart / lock / sleep / exit High Beam
/ install / reload / update / settings / version readout) are
implemented in Rust as in-process built-ins
(`src/plugins/builtin/core.rs`). They appear alongside JS plugins in
the result list but never go through rquickjs — a buggy plugin must
not be able to power off the user's machine.

## Slint gotchas

These are load-bearing for future UI work — they're easy to relearn
the hard way.

### Daemon-shaped event loops: do NOT use `ComponentHandle::run()`

`ComponentHandle::run()` calls `show()` → `run_event_loop()` →
`hide()` and couples the event-loop lifetime to the window's
lifetime. The first time the last window closes, the loop ends and
the daemon dies — taking the IPC listener and global hotkey with it.

Use `slint::run_event_loop_until_quit()`. Show/hide windows freely;
the loop only ends on explicit `quit_event_loop()`.

### One-way `text: root.foo` bindings get severed on widget self-mutation

In `.slint`:

```slint
TextInput {
    text: root.query-text;
    edited => { root.query-text = self.text; }
}
```

The moment `self.text` is written from inside the widget (e.g. on
`edited`), Slint severs the inbound binding. Subsequent writes to
`root.query-text` from Rust no longer propagate into the widget.

Workaround: declare an `invoke_clear_input()` callback in `.slint`
that directly assigns `input.text = ""` and call it from Rust.

### Wayland `hide()` is broken; collapse instead

Slint 1.16's Wayland `hide()` destroys the underlying winit window
via `suspend()`, and when the destroy fails (because Slint's renderer
holds extra `Arc<winit::Window>` refs we can't drop from app code)
the state still flips to `None` — so the *next* `show()` won't
re-attach the surface and the launcher silently never opens again.

The workaround in `src/window.rs::hide`: set an `is-hidden` flag in
the `.slint` file that collapses the visible content to 1×1
transparent, keeping Slint's `shown` state intact. macOS still uses
the real `window.hide()`.

### Wayland focus grab needs an activation token

`xdg_activation_v1` is the protocol the compositor uses to decide
whether a window-raise request is allowed. winit 0.30 consumes the
token at window-creation time only; for re-activations we drop down
to `wayland-client` and call `xdg_activation_v1.activate(token,
surface)` directly. The token comes from `XDG_ACTIVATION_TOKEN` /
`DESKTOP_STARTUP_ID` in the environment of whatever launched us, or
forwarded across the IPC socket when an existing daemon is asked to
open.

See `src/window_wayland.rs` for the foreign-display backend wiring.

### Auto-focus on window open: macOS needs explicit activation

`set_focus(true)` / `forward-focus: input` cannot land focus when the
OS-level window/process aren't already key. Summoning via global
hotkey is exactly that case.

In `src/window.rs` (gated `#[cfg(target_os = "macos")]`):

1. `NSApplication.sharedApplication().activate(ignoringOtherApps: true)`
2. `nsWindow.makeKeyAndOrderFront(nil)`
3. Slint's focus call — now sticks.

`activateIgnoringOtherApps:` is deprecated on macOS 14+ in favor of
cooperative `activate()`. Launchers explicitly want non-cooperative
activation; the call is wrapped in `#[allow(deprecated)]` with a
one-line rationale.

### Linux blur-grace timer

GNOME-Mutter fires a spurious `Focused(false)` event ~1s after we
appear (the launching terminal's prompt redraws, focus-stealing
prevention kicks in). The window-event handler in `src/window.rs`
debounces blurs by `BLUR_GRACE_MS` so the launcher doesn't auto-hide
during the activation handoff.

### Dock / Cmd-Tab hiding is still TODO

`activate(ignoringOtherApps:)` + `makeKeyAndOrderFront:` only govern
frontmost/key state. To hide from Dock / Cmd-Tab we'd need
`NSApp.setActivationPolicy(.accessory)` or `LSUIElement=1` in
`Info.plist` when we ship a real `.app`. Currently a no-op TODO in
`src/window.rs`.

## Why not WASM for plugins

Considered seriously. Rejected because:

- Async + debugging DX are still rough in WASI Preview 2 / Component
  Model.
- Sandboxing is theoretical — Alfred (900+ workflows), Raycast (2000+
  extensions), Albert, Ulauncher all forgo it. Without sandboxing as
  a load-bearing reason, the DX cost is too high.
- Polyglot plugins are a feature we don't need.

## Why not Node compat

Considered. Rejected because once `import 'fs'` works, plugin authors
reach for npm and we're back to the bundle problem that motivated
rejecting Electron. The whole point of single-file plugins is to make
that impossible.

Plugins import from `highbeam:*` modules only. The host loader
rejects every other specifier.
