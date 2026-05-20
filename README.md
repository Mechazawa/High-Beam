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
`manifest.json` and the entry file (default `plugin.js`). The plugin dir is
resolved in this priority order:

1. `--plugins-dir <path>` CLI flag (test override)
2. `./plugins` next to the binary's cwd (dev convenience), if it exists
3. Platform default — `~/Library/Application Support/high-beam/plugins/` on
   macOS, `$XDG_DATA_HOME/high-beam/plugins/` on Linux

Stage 4 ships three example plugins to smoke-test the runtime; Stage 5
adds a fourth that verifies frecency:

    cp -r examples/plugins/echo plugins/echo                       # one-shot echo
    cp -r examples/plugins/slow-echo plugins/slow-echo             # streaming + abort demo
    cp -r examples/plugins/echo-ts plugins/echo-ts                 # TypeScript variant
    cp -r examples/plugins/frecency-demo plugins/frecency-demo     # frecency re-ranker demo
    just run
    # Press Shift+Space (or run `highbeam --open`) and type — Enter copies
    # the input to your clipboard. `slow-echo` yields three rows with a
    # 300ms gap each so you can see streaming and abort behaviour.
    # `frecency-demo` always returns Alpha/Beta/Gamma (equal weight):
    # pick one, run another query, and watch it bubble to the top.

The manifest fields the host honors today (see `docs/02-plugin-sdk.md` for
the full v1 spec):

    {
      "name": "echo",
      "displayName": "Echo",
      "version": "0.1.0",
      "entry": "plugin.js",
      "timeoutMs": 500,
      "memoryMb": 32,
      "debounceMs": 0,
      "capabilities": ["actions"]
    }

Plugins can `import` only from the `highbeam:*` scheme — `import 'fs'` and
`import 'lodash'` are rejected at load time. Stage 4 ships:

| Module                | Functions                              | Capability                              |
| --------------------- | -------------------------------------- | --------------------------------------- |
| `highbeam:actions`    | `openUrl`, `copy`, `exec`, `reveal`    | `actions`                               |
| `highbeam:http`       | `get`, `post`                          | `http`                                  |
| `highbeam:clipboard`  | `read`, `write`                        | `clipboard.read` / `clipboard.write`    |

TypeScript declarations live in `sdk/highbeam/`; see the README there for
the `tsconfig.json` recipe.
