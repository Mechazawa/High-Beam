# Architecture

A contributor's tour of the High Beam host.

## Stack

| Concern | Choice | Why |
|---|---|---|
| Language | Rust (edition 2024) | Native binary, no GC, mature ecosystem for the pieces we need |
| UI toolkit | Slint | Declarative `.slint` files with runtime-bound properties (good fit for theme tokens); winit windowing |
| Plugin runtime | rquickjs (QuickJS binding) | ~1 MB embed, pure ECMAScript (not Node), per-context memory caps, interrupt hooks for timeouts |
| Persistence | SQLite via `rusqlite` | Frecency, future settings. Boring, reliable. |
| Global hotkey | `global-hotkey` (macOS); CLI `--open` (Linux) | Wayland has no portable global-hotkey API; punt to the WM. |

## Module map

```
src/
  main.rs        binary front door
  lib.rs         crate root + re-exports
  cli.rs         clap CLI: `highbeam` / `highbeam --open` / `--plugins-dir`
  daemon.rs      owns the Slint event loop, IPC listener, hotkey listener
  app.rs         plugin host: tokio runtime, query dispatch, callback wiring
  window.rs      Slint window lifecycle, macOS activation dance, icon decode
  ipc.rs         unix-domain-socket single-instance protocol
  paths.rs       platform path resolution (config / socket)
  theme.rs       theme.toml parsing + the bundled default
  ui/            Slint-generated types (`include_modules!`)

  plugins/
    mod.rs
    manifest.rs  manifest.json schema + parsing
    loader.rs    scans `plugins/` for valid plugin dirs
    runtime.rs   per-plugin rquickjs Context, query() driver, capability gate
    dispatch.rs  per-keystroke fan-out + per-plugin debounce + merge/sort
    result.rs    PluginResult / Action types (host enum)
    actions.rs   host-side execution of Action variants
    log.rs       per-plugin plugin.log writer
    builtin/
      core.rs    host built-in (shutdown / sleep / quit High Beam / …)

  sdk/           host implementations of the `highbeam:*` modules
    actions.rs   action builders
    clipboard.rs read/write (gated by clipboard.read / clipboard.write)
    fs.rs        readDir / readFile / readText / read|writeCache
    http.rs      get / post (gated by http)
    icons.rs     forPath (macOS uses `sips`, Linux returns a placeholder)
    match.rs     fuzzy ranking via nucleo-matcher
    platform.rs  os / arch / version metadata (no capability)
    system.rs    exec / applescript (each gated separately)
    abort.rs     AbortController polyfill + Rust handle
    console.rs   console.{log,info,warn,error,debug} → plugin.log
    timers.rs    setTimeout polyfill
    errors.rs    structured-throw helpers (CapabilityError, AbortError, …)
    capability.rs central truth for which cap grants which module
    js/          tiny bootstrap snippets evaluated into each context

  frecency/
    mod.rs
    db.rs        SQLite open/schema/upsert + per-query Snapshot
    score.rs     pure decay-modifier formula

tests/           integration tests (separate from `cfg(test)` modules
                 because each one needs a real tokio runtime to drive
                 rquickjs)
ui/query.slint   the Slint markup; `slint::include_modules!` generates
                 the Rust types in `src/ui/mod.rs`
themes/yosemite-spotlight.toml   the bundled default theme
```

## Core flow

1. Global hotkey fires (or `highbeam --open` signals the running daemon)
2. Frameless, always-on-top window centers on the focused display
3. User types into a single text input
4. For each keystroke, after each plugin's debounce, the host aborts the
   previous query (via `AbortController`) and dispatches a fresh one to
   every loaded plugin in parallel
5. Plugins return `AsyncIterable<Result>` — the host iterates and emits rows
   to the renderer as they arrive
6. Frecency re-ranks results across plugins; pinned results sort to the top
7. Arrow keys / mouse highlight a row; Enter invokes its `action`
8. Host executes the variant (`openUrl`, `copy`, `exec`, `reveal`, `quit`,
   `noop`); the window hides
9. `(plugin_name, result.key)` is bumped in the frecency table

## Threading

Three threads matter:

- **Slint event loop (main)** — owns the window, runs Slint callbacks, owns
  the global hotkey manager (macOS).
- **`highbeam-plugin-runtime`** — single-threaded tokio runtime that owns
  every loaded `LoadedPlugin` (rquickjs `AsyncContext`). Plugins live here
  because rquickjs futures are `!Send` across `async_with!` under the
  `parallel` feature, so they can't cross threads.
- **`highbeam-ipc`** — blocks on the unix socket; hops back to the Slint
  thread via `slint::invoke_from_event_loop` to show the window.

The hotkey listener thread is a tiny adapter that drains `global-hotkey`
events and likewise hops to the Slint thread.

## Result schema

```ts
type Result = {
  key: string                    // stable per (plugin, conceptual result) — frecency key
  title: string
  subtitle?: string
  icon?: string                  // `data:image/<type>;base64,<...>` URI
  weight?: number                // plugin's self-assessed score, 0..100
  pinned?: boolean               // bypass frecency; sort to top among other pinned by weight
  action: Action                 // primary action; alt-action is post-v1
}

type Action =
  | { kind: 'openUrl', url: string }
  | { kind: 'copy', text: string }
  | { kind: 'exec', cmd: string, args: readonly string[] }
  | { kind: 'reveal', path: string }
  // host-only:
  // { kind: 'quit' }            produced by the Core built-in
  // { kind: 'noop' }            sit-still placeholder (e.g. version row)
```

## Streaming + cancellation contract

- Plugins yield results via `AsyncIterable`; the host iterates and renders
  progressively.
- On every keystroke (post-debounce), the host calls `cancel.cancel()` on the
  previous dispatch token, which both fires JS-side `AbortSignal` listeners
  and sets the rquickjs interrupt flag.
- All host APIs that do I/O (`http.get`, `fs.readDir`, etc.) accept an
  optional `signal` and honor it.
- CPU-bound plugins that block synchronously can't be cancelled gracefully —
  the per-plugin `timeoutMs` is the hard kill via the rquickjs interrupt hook.

## Frecency

- SQLite table `picks` keyed on `(plugin_name, result_key)`. Columns: `picks`
  count, `last_picked_at` (Unix seconds).
- Ranking: pinned first (sorted by `weight`); non-pinned sorted by
  `weight * frecency_modifier(picks, age)`.
- Modifier (`src/frecency/score.rs`): `1.0 + 0.10 * picks * 2^(-age/14d)`.
  Half-life 14 days; one fresh pick is roughly a 10% bump; modifier decays
  back to 1.0 as picks age.

## Built-in plugins live in the host

Core system actions (shutdown / restart / lock / sleep / exit High Beam /
version readout) are implemented in Rust as in-process built-ins
(`src/plugins/builtin/core.rs`). They appear alongside JS plugins in the
result list but never go through rquickjs — a buggy plugin must not be able
to power off the user's machine.

## Slint gotchas

These are load-bearing for future UI work — they're easy to relearn the
hard way.

### Daemon-shaped event loops: do NOT use `ComponentHandle::run()`

`ComponentHandle::run()` calls `show()` → `run_event_loop()` → `hide()` and
couples the event-loop lifetime to the window's lifetime. The first time the
last window closes, the loop ends and the daemon dies — taking the IPC
listener and global hotkey with it.

Use `slint::run_event_loop_until_quit()`. Show/hide windows freely; the loop
only ends on explicit `quit_event_loop()`.

### One-way `text: root.foo` bindings get severed on widget self-mutation

In `.slint`:

```slint
TextInput {
    text: root.query-text;
    edited => { root.query-text = self.text; }
}
```

The moment `self.text` is written from inside the widget (e.g. on `edited`),
Slint severs the inbound binding. Subsequent writes to `root.query-text`
from Rust no longer propagate into the widget.

Workaround: declare an `invoke_clear_input()` callback in `.slint` that
directly assigns `input.text = ""` and call it from Rust.

### Auto-focus on window open: macOS needs explicit activation

`set_focus(true)` / `forward-focus: input` cannot land focus when the
OS-level window/process aren't already key. Summoning via global hotkey is
exactly that case.

In `src/window.rs` (gated `#[cfg(target_os = "macos")]`):

1. `NSApplication.sharedApplication().activate(ignoringOtherApps: true)`
2. `nsWindow.makeKeyAndOrderFront(nil)`
3. Slint's focus call — now sticks.

`activateIgnoringOtherApps:` is deprecated on macOS 14+ in favor of
cooperative `activate()`. Launchers explicitly want non-cooperative
activation; the call is wrapped in `#[allow(deprecated)]` with a one-line
rationale.

### Window operations must run on the main thread

All Slint window operations must happen on the main thread. Hotkey events
(from `global-hotkey`) and IPC events arrive on their own threads — route
them through `slint::invoke_from_event_loop`.

### Dock / Cmd-Tab hiding is still TODO

`activate(ignoringOtherApps:)` + `makeKeyAndOrderFront:` only govern
frontmost/key state. To hide from Dock / Cmd-Tab we'd need
`NSApp.setActivationPolicy(.accessory)` or `LSUIElement=1` in `Info.plist`
when we ship a real `.app`. Currently a no-op TODO in `src/window.rs`.

## Why not WASM for plugins

Considered seriously. Rejected because:

- Async + debugging DX are still rough in WASI Preview 2 / Component Model.
- Sandboxing is theoretical — Alfred (900+ workflows), Raycast (2000+
  extensions), Albert, Ulauncher all forgo it. Without sandboxing as a
  load-bearing reason, the DX cost is too high.
- Polyglot plugins are a feature we don't need.

## Why not Node compat

Considered. Rejected because once `import 'fs'` works, plugin authors reach
for npm and we're back to the bundle problem that motivated rejecting
Electron. The whole point of single-file plugins is to make that impossible.

Plugins import from `highbeam:*` modules only. The host loader rejects every
other specifier.
