# Plugin authoring

Plugins are single-file JS programs the host loads at startup. No npm, no
bundler, no toolchain at runtime — just `manifest.json` + `plugin.js`. The
host loader resolves `highbeam:*` specifiers; everything else is rejected.

This page is the table of contents. The detailed material lives in three
sibling docs:

| Doc                                            | Read it when you want to                                      |
|------------------------------------------------|----------------------------------------------------------------|
| [plugin-tutorial.md](./plugin-tutorial.md)     | Build your first plugin start to finish, with vitest          |
| [sdk-reference.md](./sdk-reference.md)         | Look up signatures, capabilities, behaviour of `highbeam:*`   |
| [plugin-cookbook.md](./plugin-cookbook.md)     | Find a copy-pasteable recipe for a specific pattern           |

For the host's internals — query dispatch, threading, cancellation — see
[architecture.md](./architecture.md).

## At a glance

```
plugins/
  my-plugin/
    manifest.json          required
    plugin.js              required (or whatever manifest.entry points at)
    plugin.log             written by the host on demand
    [your data files, like spells.json or http.json]
    [optional dev-time files: package.json, vitest.config.ts, *.test.js]
```

```json
{
    "name": "my-plugin",
    "displayName": "My Plugin",
    "version": "0.1.0",
    "description": "What it does",
    "entry": "plugin.js",
    "debounceMs": 0,
    "timeoutMs": 500,
    "memoryMb": 32,
    "capabilities": ["actions"],
    "platforms": ["macos", "linux"],
    "options": [
        { "key": "github_username", "type": "string", "label": "GitHub username", "default": "" },
        { "key": "result_limit", "type": "int", "label": "Max results", "default": 10, "min": 1, "max": 50 }
    ]
}
```

```js
import { copy } from 'highbeam:actions';

export async function* query(input, signal) {
    if (!input) return;
    yield {
        key: 'hello',
        title: `hello: ${input}`,
        subtitle: 'press Enter to copy',
        action: copy(input),
    };
}
```

That's the entire surface area. Everything else is detail.

## Where the host looks for plugins

In priority order:

1. `--plugins-dir <path>` (test override).
2. `./plugins/` next to the binary's cwd (dev convenience), if present.
3. Platform default:
   - macOS: `~/Library/Application Support/high-beam/plugins/`
   - Linux: `$XDG_DATA_HOME/high-beam/plugins/`

Drop a plugin directory into the chosen location and restart High Beam.
There is no hot reload in v1.

## Manifest cheat sheet

Full schema lives in `src/plugins/manifest.rs`. Unknown fields are
tolerated so new fields can land without breaking older plugins.

- `name` — required; unique; lowercase-kebab; frecency key prefix and
  cache directory name.
- `displayName`, `version`, `description` — optional metadata.
- `entry` — optional; defaults to `plugin.js`.
- `debounceMs` — wait this long after the latest keystroke before
  invoking `query()`. `0` = no debounce. Capped at 2000.
- `timeoutMs` — wall-clock kill for `query()`. Default 500.
- `memoryMb` — QuickJS context memory cap. Default 32.
- `capabilities` — array of capability strings. See SDK reference for the
  full table.
- `platforms` — optional array. Absent loads everywhere; `["macos"]`
  shelves it on Linux; `[]` shelves it entirely.
- `options` — optional array of user-editable settings the host renders
  in the settings UI. Each entry has `key`, `type` (`"string"`, `"bool"`,
  `"int"`, or `"enum"`), `label`, `default`, plus `min`/`max` (int) or
  `choices` (enum). Plugins read their own values via `highbeam:settings`
  — see [the SDK reference](./sdk-reference.md#highbeamsettings).

Tuning guidance is in [the cookbook](./plugin-cookbook.md#tune-debouncems--timeoutms--memorymb).

## Query function shape

```ts
export async function* query(
    input: string,
    signal: AbortSignal,
): AsyncIterable<Result>;
```

- `input` is the current query string.
- `signal` aborts when the user types another keystroke. Check
  `signal.aborted` between yields if you produce many rows; propagate it
  into I/O calls via `opts.signal`.
- Yield `Result` objects, one per row:

```ts
type Result = {
    key: string;             // stable per (plugin, conceptual result)
    title: string;
    subtitle?: string;
    icon?: string;           // data:image/<type>;base64,... URI
    weight?: number;         // 0..100; higher ranks first
    pinned?: boolean;        // sort above non-pinned regardless of weight
    action: Action;
};
```

Plugins can `import` only from the `highbeam:*` scheme. `import 'fs'`,
`import 'lodash'`, etc. are rejected at load time.

## Capabilities at a glance

| Capability             | Grants                                              |
|------------------------|-----------------------------------------------------|
| `actions`              | `highbeam:actions`                                  |
| `http`                 | `highbeam:http.get` / `.post`                       |
| `clipboard.read`       | `highbeam:clipboard.read`                           |
| `clipboard.write`      | `highbeam:clipboard.write`                          |
| `fs.read`              | `highbeam:fs.readDir` / `.readFile` / `.readText`   |
| `fs.cache`             | `highbeam:fs.readCache` / `.writeCache`             |
| `icons`                | `highbeam:icons.forPath`                            |
| `system.exec`          | `highbeam:system.exec`                              |
| `system.applescript`   | `highbeam:system.applescript`                       |

`highbeam:match`, `highbeam:platform`, and `highbeam:settings` are uncapped
— no declaration required. See [sdk-reference.md](./sdk-reference.md) for
per-function behavior, signatures, and examples.

## Console + logging

`console.log/info/warn/error/debug` from a plugin are captured to
`<plugin_dir>/plugin.log`, prefixed with timestamp + level. No rotation.

```
[2026-05-20T15:30:42.123Z] [INFO ] hello world
[2026-05-20T15:30:42.456Z] [ERROR] something broke
    continuation lines indent four spaces
```

This is also where capability violations, parse errors, timeouts, and
memory-cap hits surface.

## TypeScript

Hand-written `.d.ts` files live in `sdk/highbeam/`. A minimal
`tsconfig.json` for a TS plugin:

```json
{
    "compilerOptions": {
        "target": "ES2022",
        "module": "ES2022",
        "moduleResolution": "node",
        "strict": true,
        "outDir": ".",
        "paths": {
            "highbeam:*": ["./node_modules/@high-beam/sdk/*"]
        }
    },
    "include": ["plugin.ts"]
}
```

`npm install --save-dev <path-to-this-repo>/sdk/highbeam` (or symlink it)
to pull the ambient types in. Compiled `plugin.js` keeps the bare
`highbeam:*` specifiers — TypeScript only needs the types at compile time.

`examples/plugins/echo-ts` has a full working setup.

## Testing with vitest

The SDK ships Node-compatible stubs alongside the `.d.ts` files, so a
plugin can be tested in plain vitest. See
[plugin-tutorial.md](./plugin-tutorial.md#step-6--add-vitest) for the
full setup, and the [cookbook recipe](./plugin-cookbook.md#mock-sdk-calls-in-vitest)
for mocking patterns.

## Failure handling

- `query()` throws → host logs to `plugin.log`, drops yielded results
  from this query, renders nothing.
- Exceeded `timeoutMs` → host interrupts the context, logs a `WARN`,
  drops results.
- Exceeded `memoryMb` → host logs an `ERROR`, drops results.
- Capability violation at load time → host logs an `ERROR`, plugin
  doesn't load.
- Auto-disable on repeated failures is post-v1; every query gets a fresh
  try.

## Example plugins

Every plugin in `examples/plugins/` is real and tested. Copy any of them
as a starting point:

- `echo` — minimal `copy(input)`.
- `echo-ts` — TypeScript variant with `tsconfig.json`.
- `slow-echo` — streaming + abort demo.
- `frecency-demo` — equal-weight rows to demonstrate pick-bumping.
- `calculator` — pinned, inline math evaluator (npm-free shunting-yard).
- `http-codes` — uses `highbeam:fs.readText` to load bundled `http.json`,
  opens MDN.
- `paper-size` — inlined data, `copy` action.
- `dnd` — bundled `5eSpells.json`, `match.fuzzy` ranking, `openUrl`.
- `app-launcher` — `fs.readDir`, `icons.forPath`, `match.fuzzy`,
  cross-platform Spotlight equivalent.
- `xkcd` — HTTP + `fs.cache`-backed title index, fuzzy search.
