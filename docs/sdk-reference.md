# SDK reference

Complete reference for every `highbeam:*` module. Mirrors the `.d.ts` files in
`sdk/highbeam/` — when those drift, this drifts. The host has a CI test
(`tests/sdk_shape.rs`) that pins the exported symbols, so signatures in this
document stay accurate.

Companion docs: [plugin-tutorial.md](./plugin-tutorial.md) (step-by-step
build), [plugin-cookbook.md](./plugin-cookbook.md) (recipes),
[internals.md](./internals.md) (host internals).

## Conventions

- Modules are imported under the `highbeam:` scheme, or the `node:` scheme
  for the supported Node built-ins (`node:path`, `node:fs`,
  `node:fs/promises`). Anything else (`import 'fs'`, `import 'lodash'`,
  even `import 'node:os'`) is rejected at load time.
- A module loads when the plugin declares *any* capability the module
  recognises. Functions within the module may gate themselves further — e.g.
  `highbeam:clipboard` loads on either `clipboard.read` or `clipboard.write`,
  but `write()` from a `clipboard.read`-only plugin throws a
  `CapabilityError`.
- Functions that do I/O accept an `AbortSignal` via `opts.signal`. The signal
  the host passes into `query(input, signal)` is the one to propagate —
  aborting it cascades into the I/O.
- Relative paths in `highbeam:fs.*` resolve against the plugin directory.
- Errors surface as JavaScript exceptions. Catch them in the plugin if you
  want to render a "failed" row; otherwise they propagate, the host logs
  them to `plugin.log`, and the query yields nothing.

## Shared types

Defined in `sdk/highbeam/types.d.ts`. Every other module re-exports the bits
it needs.

```ts
interface Result {
    key: string;             // stable per (plugin, conceptual result)
    title: string;
    subtitle?: string;
    icon?: string;           // `data:image/<type>;base64,...` URI
    weight?: number;         // 0..100; higher ranks first
    pinned?: boolean;        // sort above non-pinned regardless of weight
    action: Action;
    altAction?: Action;      // optional alternate (Alt/Shift/Cmd/Ctrl + Enter)
    altTitle?: string;       // shown in place of `title` while alt-mod held
    altSubtitle?: string;    // shown in place of `subtitle` while alt-mod held
}

// Actions plugin code can construct. Host-only variants (quit,
// openSettings, reloadPlugin, installPlugin, updatePlugins) are produced
// by the Core built-in only; the host rejects them in plugin-yielded
// results and view dispatches.
type Action =
    | { kind: 'openUrl'; url: string }
    | { kind: 'copy'; text: string }
    | { kind: 'exec'; cmd: string; args: readonly string[] }
    | { kind: 'reveal'; path: string }
    | { kind: 'showView'; view: ViewDef; props: object; reset: boolean }
    | { kind: 'closeView' }
    | { kind: 'noop' };       // inert row — Enter just dismisses the launcher

interface AbortSignal {
    readonly aborted: boolean;
    readonly reason: unknown;    // a DOMException once aborted
    addEventListener(type: 'abort', listener: () => void): void;
    removeEventListener(type: 'abort', listener: () => void): void;
    throwIfAborted(): void;      // throws signal.reason if aborted
}

interface AbortController {
    readonly signal: AbortSignal;
    abort(reason?: unknown): void;
}

type QueryFn = (
    input: string,
    signal: AbortSignal,
) => AsyncIterable<Result>;
```

Notes:

- `Result.key` is the frecency key for `(plugin_name, key)`. Don't fold the
  user's current input into it — that defeats frecency.
- `Result.icon` must be a `data:image/<type>;base64,...` URI. Bare
  filesystem paths are treated as missing — pre-resolve via
  `highbeam:icons.forPath(...)`.
- `Action` has host-only variants (`quit`, `openSettings`, `reloadPlugin`,
  `installPlugin`, `updatePlugins`) emitted by the Core built-in. The host
  rejects them in plugin-yielded results and view dispatches.

## `highbeam:actions`

Action builders. Plain functions that return the `Action` shape the host
knows how to execute.

```js
import { openUrl, copy, exec, reveal } from 'highbeam:actions';
```

**Capability:** `actions`.

### `openUrl(url: string): Action`

```js
openUrl('https://example.com')
// => { kind: 'openUrl', url: 'https://example.com' }
```

Opens with the system handler — `/usr/bin/open` on macOS, `xdg-open` on
Linux. Works for `https://`, `mailto:`, `file://`, and any
URL-handler-registered scheme. On macOS, also works for `.app` bundle paths
(see `plugins/app-launcher`).

### `copy(text: string): Action`

```js
copy('hello world')
// => { kind: 'copy', text: 'hello world' }
```

Copies the text to the system clipboard. The most common action — most
plugins ship at least one `copy()` row.

### `exec(cmd: string, args: readonly string[]): Action`

```js
exec('sh', ['-c', 'echo hi'])
// => { kind: 'exec', cmd: 'sh', args: ['-c', 'echo hi'] }
```

Fire-and-forget subprocess. No stdout capture, no exit-code propagation —
this is the right tool for "launch this app" / "kick off this script", not
for "compute something and use the result". Use `highbeam:system.exec` (cap
`system.exec`) when you need stdout / exit code.

`plugins/app-launcher` uses `exec('sh', ['-c', command])` on Linux
to invoke `.desktop` Exec lines while preserving shell quoting.

### `reveal(path: string): Action`

```js
reveal('/Users/me/Documents/notes.txt')
// => { kind: 'reveal', path: '/Users/me/Documents/notes.txt' }
```

Opens the file's parent directory in the system file manager with the file
selected.

- macOS: `open -R <path>` — Finder's "select this file" mode.
- Linux: best-effort `xdg-open <parent_dir>`; no selection (the freedesktop
  spec doesn't have a portable equivalent).

## `fetch`

Standard WHATWG `fetch`. No import: declaring the `http` capability installs
it (and `Headers`, `Request`, `Response`, `FormData`) as a global. Real
classes, binary bodies, and a `ReadableStream` on `response.body`.

**Capability:** `http`.

```js
const res = await fetch('https://xkcd.com/info.0.json', { signal });
if (!res.ok) throw new Error(`HTTP ${res.status}`);
const data = await res.json();
```

`fetch(input, init?)` takes a URL string or `Request` and the usual `init`
(`method`, `headers`, `body`, `signal`, …). `body` accepts a string,
`Uint8Array`/`ArrayBuffer`, `Blob`, `FormData`, or `URLSearchParams`. The
`Response` exposes `.ok`, `.status`, `.statusText`, `.headers`, and the
async readers `.json()`, `.text()`, `.arrayBuffer()`, `.blob()`, plus a
`.body` `ReadableStream`.

```js
const res = await fetch('https://api.example.com/items', {
    method: 'POST',
    headers: { 'authorization': 'bearer ...', 'content-type': 'application/json' },
    body: JSON.stringify({ name: 'x' }),
    signal,
});
```

### Behavior notes

- Every fetch gets a 30 s timeout, injected host-side by joining your
  `signal` (if any) with a deadline via `AbortSignal.any`. Your own signal
  can abort sooner (`AbortSignal.timeout(5000)`, a keystroke cancel) but
  cannot extend past the 30 s ceiling.
- Cancellation: pass an `AbortSignal` via `init.signal`. The signal the host
  hands to `query(input, signal)` is the one to propagate, so the next
  keystroke cancels the in-flight request.
- The body readers (`.json()`, `.text()`, `.arrayBuffer()`, `.blob()`) are
  async. There is no transfer cap; a response body is bounded by the
  plugin's `memoryMb`.
- A non-2xx status resolves normally (check `.ok` / `.status`). `fetch`
  rejects only on transport failure or abort.

### Example

```js
import { openUrl } from 'highbeam:actions';

export async function* query(input, signal) {
    if (!/^xkcd latest$/i.test(input)) return;
    const res = await fetch('https://xkcd.com/info.0.json', { signal });
    if (!res.ok) return;
    const comic = await res.json();
    yield {
        key: `xkcd-${comic.num}`,
        title: `${comic.num}: ${comic.title}`,
        subtitle: comic.alt,
        action: openUrl(`https://xkcd.com/${comic.num}/`),
    };
}
```

See `plugins/xkcd` for a full fetch-driven plugin with caching.

### Migrating from `highbeam:http`

The `highbeam:http` module is gone. Translate calls to `fetch`:

- `get(url, opts)` → `fetch(url, opts)`.
- `post(url, body, opts)` → `fetch(url, { method: 'POST', body, ...opts })`.
  An object body is no longer JSON-stringified for you: call
  `JSON.stringify(body)` and set `content-type: application/json` yourself.
- `res.json()` / `res.text()` are now async: `await res.json()`.
- The `timeoutMs` option is gone. Use `AbortSignal.timeout(ms)` (composed
  with the host signal via `AbortSignal.any` when you also want
  keystroke cancellation).

## `highbeam:clipboard`

Read / write the system clipboard.

```js
import { read, write } from 'highbeam:clipboard';
```

**Capabilities:** `clipboard.read` and / or `clipboard.write`. The module
loads if you declare either. Each function additionally gates itself on its
own capability.

### `read(): Promise<string>`

```js
const current = await read();
```

Returns the current clipboard text. Requires `clipboard.read`. Non-text
clipboard contents resolve to an empty string.

### `write(text: string): Promise<void>`

```js
await write('hello');
```

Sets the clipboard. Requires `clipboard.write`.

Most plugins prefer the action builder `copy(text)` from `highbeam:actions`
over imperative `write()` — the action runs after Enter is pressed, which
matches the user's mental model. Use `clipboard.write` when you really do
want a side effect during `query()` (rare).

## `highbeam:fs`

Read files, walk directories, and use a plugin-scoped cache.

```js
import { readDir, readFile, readText, readCache, writeCache, basename } from 'highbeam:fs';
```

**Capabilities:** `fs.read` for the file readers, `fs.cache` for the cache
helpers. Both can be declared independently. `basename` is a pure string
helper available with either. Declaring the broad `fs` capability (full
filesystem access, see [`node:fs`](#nodefs--nodefspromises)) unlocks every
helper here too, without naming `fs.read` / `fs.cache` separately.

Relative paths passed to `readDir` / `readFile` / `readText` resolve against
the plugin's own directory, so `readText('./bundled.json')` works regardless
of the daemon's cwd.

### `readDir(path: string, opts?: ReadDirOptions): AsyncIterable<DirEntry>`

```ts
interface ReadDirOptions {
    recursive?: boolean;       // default false
    signal?: AbortSignal;
}

interface DirEntry {
    name: string;              // filename only, no leading path
    path: string;              // absolute path
    isFile: boolean;
    isDir: boolean;
}
```

```js
for await (const entry of readDir('/Applications', { recursive: false })) {
    if (entry.isDir && entry.name.endsWith('.app')) {
        // ...
    }
}
```

With `{ recursive: true }`, descends into subdirectories before yielding
their siblings' children. Missing directories surface as a thrown error from
the iterator — most plugins wrap the loop in `try / catch` and treat
"unreadable directory" as "no apps here, move on" (see `app-launcher`).

**Capability:** `fs.read`.

### `readFile(path: string, opts?: ReadFileOptions): Promise<Uint8Array>`

```ts
interface ReadFileOptions {
    signal?: AbortSignal;
}
```

```js
const bytes = await readFile('./icon.png');
```

Returns the file as a `Uint8Array`. Use this for binary data; for text, use
`readText`. **Capability:** `fs.read`.

### `readText(path: string, opts?: ReadFileOptions): Promise<string>`

```js
const text = await readText('./5eSpells.json');
const spells = JSON.parse(text);
```

UTF-8 decode. Throws on invalid UTF-8. **Capability:** `fs.read`.

### `readCache(name: string): Promise<Uint8Array | null>`

```js
const raw = await readCache('xkcd-index.json');
if (raw === null) {
    // cache miss — repopulate
}
```

Reads from a plugin-scoped cache directory. Returns `null` on cache miss
(never throws for "missing"). The `name` is rejected if it contains `/`,
`..`, or starts with `.` — single path components only.

Cache locations:

- macOS: `~/Library/Caches/high-beam/plugins/<plugin_name>/`
- Linux: `$XDG_CACHE_HOME/high-beam/plugins/<plugin_name>/`

Plugins cannot see each other's caches; the directory is keyed on the
manifest `name`. **Capability:** `fs.cache`.

### `writeCache(name: string, data: Uint8Array | string): Promise<void>`

```js
await writeCache('index.json', JSON.stringify({ updated: Date.now() }));
```

Writes a blob to the plugin's cache by name. Creates the cache directory if
missing. Same naming rules as `readCache`. **Capability:** `fs.cache`.

See `plugins/xkcd` for cache-backed iteration (build an index on
first miss, serve subsequent queries from cache, refresh on TTL).

### `basename(path: string): string`

```js
basename('/Applications/Firefox.app');  // 'Firefox.app'
basename('/foo/bar/');                  // 'bar'
basename('/');                          // ''
```

Final component of `path` after stripping trailing slashes. Empty string for
the root and the empty path; `.` / `..` pass through as-is (matching Node's
`path.posix.basename`). Pure string helper — works with either `fs.*`
capability, no I/O.

## `highbeam:icons`

Native icon resolution. Returns a `data:image/png;base64,...` URI suitable
for direct use as a `Result.icon`.

```js
import { forPath } from 'highbeam:icons';
```

**Capability:** `icons`.

### `forPath(path: string, opts?: IconOptions): Promise<string>`

```ts
interface IconOptions {
    size?: number;             // longest-edge pixel size; default 128
}
```

```js
const icon = await forPath('/Applications/Safari.app', { size: 64 });
// icon === 'data:image/png;base64,iVBOR...'
```

Behavior:

- **macOS:** extracts the bundle's `CFBundleIconFile` via `sips`. Slow on
  the first call (~50ms), then instant — the host caches in-process keyed
  on `(path, size)`.
- **Linux:** best-effort. Returns a 1×1 transparent PNG fallback when the
  path can't be resolved (rather than throwing). Currently only absolute
  paths from `.desktop` `Icon=` entries; full XDG icon-theme lookup is
  post-v1.

`plugins/app-launcher` resolves icons for matched apps and assigns
them to `result.icon`.

## `highbeam:match`

Host-side fuzzy matcher. No capability required.

```js
import { fuzzy } from 'highbeam:match';
```

Backed by `nucleo-matcher` (Smith-Waterman with filename-style bonus
heuristics). Scores are normalised to `[0, 1]`.

### `fuzzy<T>(items: readonly T[], query: string, opts: FuzzyOptions<T>): Match<T>[]`

```ts
interface FuzzyOptions<T> {
    key: (item: T) => string;      // extract the haystack
    threshold?: number;            // drop matches with score < threshold
    limit?: number;                // cap returned matches
}

interface Match<T> {
    item: T;
    score: number;                 // [0, 1]; higher is better
    highlights: [number, number][]; // [start, end) byte ranges
}
```

```js
const spells = [ /* ... */ ];
const ranked = fuzzy(spells, 'fireball', {
    key: (s) => s.name,
    threshold: 0.05,
    limit: 10,
});
for (const { item, score, highlights } of ranked) {
    // item is the original element, untouched
    // score is normalised, higher = better
    // highlights are [start, end) byte ranges into key(item)
}
```

Notes:

- Results are returned sorted by score, best match first.
- Empty `query` returns every item with score 1 and no highlights.
- `threshold` of `0.05` is a good "barely relevant" floor for short queries.
- `highlights` use byte ranges, not character ranges — fine for ASCII keys,
  matters for multi-byte UTF-8 if you're rendering bold spans yourself.

See `plugins/dnd` and `plugins/app-launcher` for typical
usage (fuzzy-rank a bundled list and map scores onto `weight`).

## `highbeam:platform`

Host metadata. Always importable.

```js
import { os, arch, version, isMacOS, isLinux } from 'highbeam:platform';
```

**Capability:** none.

### `os: 'macos' | 'linux'`

OS identifier, matching the host's `std::env::consts::OS`.

### `arch: string`

CPU architecture. Common values: `x86_64`, `aarch64`.

### `version: string`

OS version. Best-effort:

- macOS: `sw_vers -productVersion` (e.g. `14.4.1`).
- Linux: `uname -r` (kernel release).

Returns `"unknown"` if detection fails. Never throws.

### `isMacOS(): boolean` / `isLinux(): boolean`

```js
if (isMacOS()) {
    // mac-specific path
} else if (isLinux()) {
    // linux-specific path
}
```

Convenience wrappers over `os === 'macos'` / `os === 'linux'`. Use these in
preference to manual comparisons — the intent is clearer at the call site.

## `highbeam:settings`

Read this plugin's own option values. The host scopes per plugin internally,
so `get('foo')` always returns this plugin's `foo` — never another plugin's.
Always importable, no capability required.

```js
import { get, getString, getBool, getInt } from 'highbeam:settings';
```

**Capability:** none.

Options come from the plugin's `manifest.json` (the `options` array — see
[plugin-authoring.md](./plugin-authoring.md#manifest-cheat-sheet)). The host
folds the user's saved overrides onto the manifest defaults at load time,
so the SDK never returns a value the manifest didn't declare.

### `get<T>(key: string): T | undefined`

Returns whatever the host has for `key`, or `undefined` when the key isn't
in the plugin's options bag. Use the typed variants below if you want the
SDK to also drop values whose runtime type doesn't match.

### `getString(key)` / `getBool(key)` / `getInt(key)`

Same lookup as `get`, but the SDK returns `undefined` unless the stored
value matches the expected type (string / boolean / number). Useful when
the manifest renamed an option and an older `settings.toml` carries the
wrong type — the plugin sees a clean `undefined` instead of a surprise
shape.

### Example

```js
import { copy } from 'highbeam:actions';
import { getString, getInt } from 'highbeam:settings';

const user = getString('github_username') ?? '';
const limit = getInt('result_limit') ?? 10;
// ...use `user` / `limit` in your query function.
```

### Persistence

User-set values live in `settings.toml`:

- macOS: `~/Library/Application Support/high-beam/settings.toml`
- Linux: `$XDG_CONFIG_HOME/high-beam/settings.toml`

Writes are atomic (temp file + rename). Reloading values is restart-only
for v1 — toggling an option in the settings UI persists immediately, but
running plugins keep the value they were loaded with.

## `highbeam:system`

Subprocess and AppleScript escape hatches. Two capabilities so plugins
declare only what they need.

```js
import { exec, applescript } from 'highbeam:system';
```

**Capabilities:** `system.exec` and / or `system.applescript`.

### `exec(cmd: string, args: readonly string[], opts?: ExecOptions): Promise<ExecResult>`

```ts
interface ExecOptions {
    signal?: AbortSignal;
    timeoutMs?: number;            // hard wall-clock cap; kills the child
    cwd?: string;                  // working dir for the child
}

interface ExecResult {
    stdout: string;                // truncated at 10 MB
    stderr: string;                // truncated at 10 MB
    code: number | null;           // null if killed by signal
}
```

```js
const { stdout, stderr, code } = await exec('git', ['status', '--short'], {
    cwd: '/Users/me/repo',
    timeoutMs: 2000,
    signal,
});
if (code !== 0) throw new Error(`git failed: ${stderr}`);
```

Captures stdout and stderr. Aborting the signal kills the child. Output
exceeding 10 MB is silently truncated.

This is the variant to use when you need the child's output. For
fire-and-forget launches, use the `exec` *action* from `highbeam:actions`
(no capability beyond `actions`). **Capability:** `system.exec`.

### `applescript(script: string, opts?: AppleScriptOptions): Promise<string | null>`

```ts
interface AppleScriptOptions {
    signal?: AbortSignal;
    timeoutMs?: number;
}
```

```js
const front = await applescript(
    'tell application "System Events" to get name of first process whose frontmost is true',
);
```

On macOS: runs via `osascript -e <script>` and resolves with the script's
stdout (trailing newline trimmed).

On every other platform: resolves with `null` immediately — plugins don't
have to gate every call site behind `isMacOS()`. **Capability:**
`system.applescript`.

macOS may prompt for automation permission the first time the script tries
to control a system app (Finder, System Events, etc.). Grant once.

## Always-on globals

Pure-compute Web platform APIs are installed for every plugin, no capability
and no import. They come from the llrt module crates layered on QuickJS:

- `URL` / `URLSearchParams`.
- `Buffer`, `Blob`, `File`.
- `TextEncoder` / `TextDecoder` (multi-encoding: utf-8, utf-16le, utf-16be,
  windows-1252, with BOM handling; invalid byte sequences decode to U+FFFD).
- `ReadableStream` and the rest of the web-streams family.
- `DOMException`.
- `AbortController` / `AbortSignal`, including `AbortSignal.timeout(ms)`,
  `AbortSignal.any([...])`, `AbortSignal.abort(reason)`, and on a signal
  `signal.reason` (a `DOMException`) and `signal.throwIfAborted()`.

`fetch` and its companions (`Headers`, `Request`, `Response`, `FormData`)
are not in this list: they need the `http` capability (see [`fetch`](#fetch)).

## `node:path`

Path manipulation, importable by every plugin, no capability.

```js
import { join, basename, extname } from 'node:path';
// or: import * as path from 'node:path';
```

**Capability:** none.

Exports: `basename`, `dirname`, `extname`, `format`, `parse`, `join`,
`resolve`, `relative`, `normalize`, `isAbsolute`, `delimiter`, `sep`.

```js
join('/Applications', 'Safari.app');        // '/Applications/Safari.app'
extname('notes.txt');                        // '.txt'
parse('/foo/bar.json').name;                 // 'bar'
```

Pure string operators, no filesystem access.

## `node:fs` / `node:fs/promises`

Full filesystem access: read and write any file the user can.

```js
import { readFileSync, writeFileSync } from 'node:fs';
import { readFile, writeFile } from 'node:fs/promises';
```

**Capability:** `fs`. Its user-facing meaning is "FULL filesystem access,
read and write any file your user can", so the settings UI surfaces it as a
broad grant. Declaring `fs` also unlocks all the scoped
[`highbeam:fs`](#highbeamfs) helpers (`readDir` / `readFile` / `readText` /
`readCache` / `writeCache` / `basename`) without naming `fs.read` /
`fs.cache`. The reverse does not hold: the narrower `fs.read` / `fs.cache`
caps do NOT load `node:fs`.

`node:fs` exports (sync): `accessSync`, `mkdirSync`, `mkdtempSync`,
`readdirSync`, `readFileSync`, `rmdirSync`, `rmSync`, `statSync`,
`lstatSync`, `writeFileSync`, `chmodSync`, `renameSync`, `symlinkSync`,
`constants`, `promises`.

`node:fs/promises` exports: `access`, `open`, `readFile`, `writeFile`,
`rename`, `readdir`, `mkdir`, `mkdtemp`, `rm`, `rmdir`, `stat`, `lstat`,
`chmod`, `symlink`, `constants`.

```js
import { readFile } from 'node:fs/promises';

const text = await readFile('/etc/hosts', 'utf-8');
```

Prefer scoped `highbeam:fs` (relative-to-plugin reads, plugin-keyed cache)
when that covers the need. Reach for `node:fs` only when a plugin genuinely
needs to touch arbitrary user paths.

## Capabilities table

| Capability             | Grants                                              |
|------------------------|-----------------------------------------------------|
| `actions`              | `highbeam:actions`                                  |
| `http`                 | global `fetch` / `Headers` / `Request` / `Response` / `FormData` |
| `clipboard.read`       | `highbeam:clipboard.read`                           |
| `clipboard.write`      | `highbeam:clipboard.write`                          |
| `fs.read`              | `highbeam:fs.readDir` / `.readFile` / `.readText`   |
| `fs.cache`             | `highbeam:fs.readCache` / `.writeCache`             |
| `fs`                   | `node:fs` / `node:fs/promises` + all `highbeam:fs.*` |
| `icons`                | `highbeam:icons.forPath`                            |
| `system.exec`          | `highbeam:system.exec`                              |
| `system.applescript`   | `highbeam:system.applescript`                       |

`highbeam:match`, `highbeam:platform`, `highbeam:settings`, and `node:path`
are uncapped, as are the always-on globals.

A module loads if the plugin declares *any* of its required caps. Within a
module, individual functions can still gate themselves more tightly.
Calling a function without its capability throws a `CapabilityError`.

## Error types

The host throws JavaScript `Error` instances with a structured `.name`:

| `error.name`        | When                                                                   |
|---------------------|------------------------------------------------------------------------|
| `CapabilityError`   | Function called without its declared capability                        |
| `AbortError`        | Operation was aborted via `AbortSignal` (a `DOMException`, also `signal.reason`) |
| `TimeoutError`      | An `AbortSignal.timeout(ms)` deadline fired, including fetch's default 30 s one (a `DOMException`) |
| `FsError`           | `highbeam:fs` read / cache failure (other than capability)             |
| `ClipboardError`    | Clipboard read / write failure                                         |
| `IconError`         | Icon resolution failed (other than the Linux placeholder fallback)     |
| `SystemError`       | Subprocess / AppleScript failure                                       |

Plain `Error` and `TypeError` cover the usual JS-level failures (bad
arguments, JSON parse errors, etc.). `fetch` rejects with a `TypeError` on
transport failure and with the aborting signal's reason otherwise: an
`AbortError` `DOMException` for an explicit abort, a `TimeoutError` one
when a deadline fired. `try / catch` is the right tool — the plugin can
render a failed-state row instead of yielding nothing.

## Versioning + drift

The `.d.ts` files in `sdk/highbeam/` are the contract. The host has a CI
test (`tests/sdk_shape.rs`) that loads each module into a real rquickjs
context and asserts that exported symbols match an expected list. Adding a
function to a `.d.ts` without updating that test (or vice versa) breaks the
build.
