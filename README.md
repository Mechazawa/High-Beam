# High Beam

A keyboard launcher (Spotlight / Alfred / Raycast class) for macOS and Linux.

This is the v3 Rust rewrite. v1 (Electron + Vue, `legacy/`) and v2 (Electron + React + TS, `legacy-v2/`) are kept locally as porting reference but not tracked in git.

Design docs and stage tracking live in `docs/` (also untracked working notes).

## Build

    just check       # fmt + clippy + test
    just run         # run the binary

## Plugins

JS plugins live in `plugins/<name>/` (untracked — they're user content). The
host scans that directory at startup; each subdirectory needs a
`manifest.json` and the entry file (default `plugin.js`).

Stage 3 ships an `echo` example plugin to smoke-test the runtime:

    cp -r examples/plugins/echo plugins/echo
    just run
    # Press Shift+Space (or run `highbeam --open`) and type — Enter copies
    # the input to your clipboard.

The minimum manifest fields the host honors today (see `docs/02-plugin-sdk.md`
for the full v1 spec):

    {
      "name": "echo",
      "displayName": "Echo",
      "version": "0.1.0",
      "entry": "plugin.js",
      "timeoutMs": 500,
      "memoryMb": 32,
      "capabilities": ["actions"]
    }

Plugins can `import` only from the `highbeam:*` scheme — `import 'fs'` and
`import 'lodash'` are rejected at load time. Stage 3 implements
`highbeam:actions` (`openUrl`, `copy`); Stage 4 adds `highbeam:http`,
`highbeam:clipboard`, etc.
