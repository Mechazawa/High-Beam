# Plugin authoring

Plugins are single-file JS programs the host loads at startup. No npm, no
bundler, no toolchain at runtime — just `manifest.json` + `plugin.js`.

## Layout on disk

```
plugins/
  my-plugin/
    manifest.json          required
    plugin.js              required (or whatever manifest.entry points at)
    plugin.log             written by the host on demand
    [your data files, like spells.json or http.json]
    [optional dev-time files: package.json, vitest.config.ts, *.test.js]
```

The host reads from one of these locations, in priority order:

1. `--plugins-dir <path>` (test override)
2. `./plugins` next to the binary's cwd (dev convenience), if present
3. Platform default:
   - macOS: `~/Library/Application Support/high-beam/plugins/`
   - Linux: `$XDG_DATA_HOME/high-beam/plugins/`

Drop a plugin directory into the chosen location and restart High Beam.

## `manifest.json`

```jsonc
{
  "name": "calculator",            // unique identifier; frecency key prefix
  "displayName": "Calculator",     // optional
  "version": "0.1.0",              // optional
  "description": "Evaluate math expressions",   // optional
  "entry": "plugin.js",            // optional, defaults to "plugin.js"

  "debounceMs": 0,                 // wait this long after the latest
                                   // keystroke before invoking query();
                                   // 0 = no debounce. Capped at 2000.
  "timeoutMs": 500,                // wall-clock kill for query(); default 500
  "memoryMb": 32,                  // QuickJS context memory cap; default 32

  "capabilities": ["actions", "http"],   // see "Capabilities" below

  "platforms": ["macos", "linux"]  // optional; absent = load everywhere.
                                   // Empty array = shelved (never loads).
}
```

Unknown fields are tolerated so new fields can land without breaking older
plugins. The full schema lives in `src/plugins/manifest.rs`.

## The plugin entry point

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

- `input` is the current query string.
- `signal` is a standard `AbortSignal`. Check `signal.aborted` between
  yields if you're doing long work. The host also passes `signal` to its
  own I/O APIs so cancellation cascades automatically.
- Yield `Result` objects (one per row). The shape:

```ts
type Result = {
  key: string;             // stable per (plugin, conceptual result)
  title: string;
  subtitle?: string;
  icon?: string;           // data:image/<type>;base64,<...> URI
  weight?: number;         // 0..100; higher ranks first
  pinned?: boolean;        // sort above non-pinned regardless of weight
  action: Action;
};
```

- Plugins can `import` only from the `highbeam:*` scheme. `import 'fs'`,
  `import 'lodash'`, etc. are rejected at load time.

## Capabilities

Declare every capability the plugin needs up front. Importing a module the
plugin didn't declare is a load-time error logged to `plugin.log`.

| Capability             | Grants                                              |
|------------------------|-----------------------------------------------------|
| `actions`              | `highbeam:actions` (the action builders)            |
| `http`                 | `highbeam:http.get` / `.post`                       |
| `clipboard.read`       | `highbeam:clipboard.read`                           |
| `clipboard.write`      | `highbeam:clipboard.write`                          |
| `fs.read`              | `highbeam:fs.readDir` / `.readFile` / `.readText`   |
| `fs.cache`             | `highbeam:fs.readCache` / `.writeCache`             |
| `icons`                | `highbeam:icons.forPath`                            |
| `system.exec`          | `highbeam:system.exec`                              |
| `system.applescript`   | `highbeam:system.applescript`                       |

`highbeam:match` and `highbeam:platform` are uncapped — no declaration
required.

A module loads if the plugin declares *any* of its required caps. Within a
module, individual functions can still gate themselves more tightly — e.g.
`highbeam:clipboard` loads on either `clipboard.read` or `clipboard.write`,
but calling `write()` from a plugin that only declared `clipboard.read`
throws a `CapabilityError`.

## The `highbeam:*` modules

```ts
// highbeam:actions
export function openUrl(url: string): Action;
export function copy(text: string): Action;
export function exec(cmd: string, args: readonly string[]): Action;
export function reveal(path: string): Action;

// highbeam:http        (cap: http)
export function get(url: string, opts?: HttpOpts): Promise<HttpResponse>;
export function post(url: string, body: unknown, opts?: HttpOpts): Promise<HttpResponse>;

// highbeam:clipboard   (caps: clipboard.read / clipboard.write)
export function read(): Promise<string>;
export function write(text: string): Promise<void>;

// highbeam:fs          (caps: fs.read / fs.cache)
export function readDir(path: string, opts?: { recursive?: boolean; signal?: AbortSignal }): AsyncIterable<DirEntry>;
export function readFile(path: string, opts?: { signal?: AbortSignal }): Promise<Uint8Array>;
export function readText(path: string, opts?: { signal?: AbortSignal }): Promise<string>;
export function writeCache(name: string, data: Uint8Array | string): Promise<void>;
export function readCache(name: string): Promise<Uint8Array | null>;

// highbeam:icons       (cap: icons)
export function forPath(path: string, opts?: { size?: number }): Promise<string>;   // data URI

// highbeam:match
export function fuzzy<T>(items: T[], query: string, opts: {
  key: (t: T) => string;
  threshold?: number;   // 0..1
  limit?: number;
}): { item: T; score: number; highlights: [number, number][] }[];

// highbeam:system      (caps: system.exec / system.applescript)
export function exec(cmd: string, args: readonly string[], opts?: { signal?: AbortSignal; timeoutMs?: number; cwd?: string }): Promise<{ stdout: string; stderr: string; code: number | null }>;
export function applescript(script: string, opts?: { signal?: AbortSignal; timeoutMs?: number }): Promise<string | null>;

// highbeam:platform    (no capability)
export const os: 'macos' | 'linux';
export const arch: string;
export const version: string;
export function isMacOS(): boolean;
export function isLinux(): boolean;
```

Notes:

- Relative paths passed to `fs.readDir` / `fs.readFile` / `fs.readText` are
  resolved against the plugin's directory, so `readText('./bundled.json')`
  works regardless of the daemon's cwd.
- `fs.writeCache` / `readCache` live in a host-managed per-plugin cache dir.
  Names that contain `..` / `/` / leading `.` are rejected.
- `system.applescript` is a no-op on non-macOS — it resolves with `null`
  rather than throwing, so you don't have to gate every call site.
- Numeric scores from `match.fuzzy` are normalised into `[0, 1]`; ordering is
  best match first.

## Console + logging

`console.log/info/warn/error/debug` from your plugin are captured to
`<plugin_dir>/plugin.log`, prefixed with timestamp + level. No rotation.

Format:
```
[2026-05-20T15:30:42.123Z] [INFO ] hello world
[2026-05-20T15:30:42.456Z] [ERROR] something broke
    continuation lines indent four spaces
```

## TypeScript

Hand-written `.d.ts` files live in `sdk/highbeam/`. A minimal
`tsconfig.json` for a TS plugin:

```jsonc
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

## Testing with vitest

The same `sdk/highbeam/` directory ships Node-compatible stubs alongside the
`.d.ts` files, so you can test your plugin with plain vitest. Add to your
plugin:

```jsonc
// package.json
{
  "name": "high-beam-plugin-my-plugin",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": { "test": "vitest run" },
  "devDependencies": { "vitest": "^1.6.0" }
}
```

```ts
// vitest.config.ts — re-exports the recipe in sdk/highbeam/
import config from '../../../sdk/highbeam/vitest.config.example';
export default config;
```

```js
// my-plugin.test.js
import { describe, expect, test } from 'vitest';
import { query } from './plugin.js';

async function collect(iter) {
  const out = [];
  for await (const item of iter) out.push(item);
  return out;
}

describe('my plugin', () => {
  test('yields a row for non-empty input', async () => {
    const results = await collect(query('hello', { aborted: false }));
    expect(results).toHaveLength(1);
    expect(results[0].action).toEqual({ kind: 'copy', text: 'hello' });
  });
});
```

Then `npm install && npm test`. SDK modules with side effects
(`highbeam:http`, `highbeam:clipboard`, `highbeam:system`, …) ship as
`vi.fn()`s so you can spy on them and override per-call with
`vi.mocked(get).mockResolvedValueOnce(...)`.

## Failure handling

- `query()` throws → log to `plugin.log`, drop yielded results from this
  query, render nothing.
- Exceeded `timeoutMs` → interrupt the context, log a `WARN`, drop results.
- Exceeded `memoryMb` → log an `ERROR`, drop results.
- Capability violation at load time → log an `ERROR`, plugin doesn't load.
- Auto-disable on repeated failures is post-v1; every query gets a fresh try.

## Examples

Every plugin in `examples/plugins/` is a real, tested plugin you can copy as
a starting point:

- `echo` — minimal `copy(input)`.
- `echo-ts` — TypeScript variant with `tsconfig.json`.
- `slow-echo` — streaming + abort demo.
- `frecency-demo` — equal-weight rows to demonstrate pick-bumping.
- `calculator` — pinned, inline math evaluator (npm-free shunting-yard).
- `http-codes` — uses `highbeam:fs.readText` to load its bundled `http.json`,
  opens MDN.
- `paper-size` — inlined data, `copy` action.
- `dnd` — bundled `5eSpells.json`, `match.fuzzy` ranking, `openUrl`.
- `app-launcher` — `fs.readDir`, `icons.forPath`, `match.fuzzy`,
  cross-platform Spotlight equivalent.
