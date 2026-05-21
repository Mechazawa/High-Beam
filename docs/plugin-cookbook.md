# Plugin cookbook

Copy-pasteable recipes for the patterns that come up over and over. Each
recipe is self-contained: a sentence describing when you want it, the
code, and a pointer to the example plugin it's modelled on.

For the API surface itself, see [sdk-reference.md](./sdk-reference.md). For
the end-to-end build flow, see [plugin-tutorial.md](./plugin-tutorial.md).

## Trigger on a keyword

Use this when the plugin should stay silent until the user types a specific
word, then yield results for the rest of the input.

```js
import { copy } from 'highbeam:actions';

const TRIGGER = /^\s*hex\s+(.+)$/i;

export async function* query(input, _signal) {
    const match = TRIGGER.exec(input);
    if (!match) return;
    const arg = match[1].trim();
    if (!arg) return;

    const num = Number.parseInt(arg, 10);
    if (!Number.isFinite(num)) return;

    const hex = `0x${num.toString(16)}`;
    yield {
        key: 'hex',
        title: hex,
        subtitle: `decimal: ${num}`,
        action: copy(hex),
    };
}
```

Notes:

- The regex bails before any allocation when the trigger doesn't match.
  Every keystroke fans out to every loaded plugin in parallel; cheap
  rejection paths are how a plugin stays invisible to other queries.
- `^\s*keyword\s+(.+)$` is the standard shape — leading whitespace
  tolerated, exact keyword, at least one space, then the rest.
- The `i` flag makes the trigger case-insensitive without a separate
  `.toLowerCase()` call.

Real plugin: `plugins/http-codes` (regex `/^http\s*(\d*)/i`),
`plugins/paper-size` (regex `/^\s*paper\s+/i`).

## Fuzzy-match a bundled list

Use this when you have a known list of items (commands, presets, contacts)
and want to rank them against the user's query.

```js
import { openUrl } from 'highbeam:actions';
import { fuzzy } from 'highbeam:match';

const ITEMS = [
    { name: 'Mercury', href: 'https://example.com/mercury' },
    { name: 'Venus',   href: 'https://example.com/venus' },
    { name: 'Earth',   href: 'https://example.com/earth' },
    // ...
];

export async function* query(input, _signal) {
    if (!input.trim()) return;

    const ranked = fuzzy(ITEMS, input, {
        key: (item) => item.name,
        threshold: 0.05,
        limit: 10,
    });

    for (const { item, score } of ranked) {
        yield {
            key: item.href,
            title: item.name,
            weight: score * 100,
            action: openUrl(item.href),
        };
    }
}
```

Notes:

- `key: (item) => item.name` tells the matcher which field to fuzz against.
- `threshold: 0.05` drops irrelevant matches — `fuzzy.score` is normalised
  to `[0, 1]`, and `0.05` is a good "barely relevant" floor.
- `limit: 10` caps the result count. Yielding 200 rows is rude to the
  renderer.
- `score * 100` maps the normalised score onto the host's `0..100` weight
  range so frecency can combine fuzz score with pick history.

Real plugin: `plugins/dnd`.

## Load bundled data via `fs.readText`

Use this when the data is too big to inline cleanly. Lazy-load on first
match; cache the parsed result in module scope.

```js
import { readText } from 'highbeam:fs';

const DATA_PATH = './codes.json';
let dataPromise = null;

function loadData() {
    if (!dataPromise) {
        dataPromise = readText(DATA_PATH).then((text) => JSON.parse(text));
    }
    return dataPromise;
}

export async function* query(input, _signal) {
    if (!input.startsWith('code ')) return;
    const data = await loadData();
    // ... use data ...
}
```

Notes:

- Relative paths in `highbeam:fs.*` resolve against the plugin directory,
  so `./codes.json` works regardless of the daemon's cwd.
- The lazy-init pattern (`dataPromise = readText(...).then(...)`) parses
  the file exactly once, on the first matching keystroke. Subsequent
  queries hit the in-memory `dataPromise` for free.
- The plugin must declare the `fs.read` capability in `manifest.json`.

Manifest:

```json
{
    "name": "codes",
    "capabilities": ["actions", "fs.read"]
}
```

Real plugin: `plugins/http-codes` (`http.json`),
`plugins/dnd` (`5eSpells.json`).

## Cache expensive computations via `fs.cache`

Use this when the data is expensive to build (HTTP fetches, large sort,
filesystem walk) and rebuilding on every daemon start is wasteful. The
cache survives across restarts; the in-memory cache (above) only survives
across queries within one daemon run.

```js
import { readCache, writeCache } from 'highbeam:fs';

const CACHE_NAME = 'index.json';
const CACHE_TTL_MS = 24 * 60 * 60 * 1000;

let inMemoryCache = null;

async function loadFromCache() {
    if (inMemoryCache) return inMemoryCache;
    const raw = await readCache(CACHE_NAME);
    if (!raw) return null;
    try {
        const text = typeof raw === 'string'
            ? raw
            : new TextDecoder().decode(raw);
        const parsed = JSON.parse(text);
        if (typeof parsed.last_updated !== 'number') return null;
        if (Date.now() - parsed.last_updated > CACHE_TTL_MS) return null;
        inMemoryCache = parsed;
        return parsed;
    } catch {
        return null;
    }
}

async function rebuildAndCache(signal) {
    const fresh = await buildExpensiveIndex(signal);
    const payload = { last_updated: Date.now(), data: fresh };
    try {
        await writeCache(CACHE_NAME, JSON.stringify(payload));
    } catch {
        // Cache writes aren't load-bearing — return the data anyway.
    }
    inMemoryCache = payload;
    return payload;
}

async function getIndex(signal) {
    const cached = await loadFromCache();
    if (cached) return cached;
    return rebuildAndCache(signal);
}
```

Notes:

- `readCache` returns `null` on cache miss — never throws "missing file".
- `readCache` returns `Uint8Array | null`. The `TextDecoder` shim above
  handles the case where the stub during tests returns a string and the
  real host returns bytes.
- Cache locations:
  - macOS: `~/Library/Caches/high-beam/plugins/<plugin_name>/`
  - Linux: `$XDG_CACHE_HOME/high-beam/plugins/<plugin_name>/`
- `name` can't contain `/`, `..`, or start with `.` — single path
  components only. Plugins can't see each other's caches.
- Cache-write failures are non-fatal — wrap in `try / catch` and return the
  computed data either way.

Manifest:

```json
{
    "name": "your-plugin",
    "capabilities": ["actions", "fs.cache", "http"]
}
```

Real plugin: `plugins/xkcd` builds a title index lazily and
refreshes on a 24h TTL.

## HTTP request with timeout and abort

Use this when fetching from a remote API. Propagate the host's `signal` so
the next keystroke cancels the in-flight request.

```js
import { openUrl } from 'highbeam:actions';
import { get } from 'highbeam:http';

const TRIGGER = /^pkg\s+(.+)$/i;

export async function* query(input, signal) {
    const match = TRIGGER.exec(input);
    if (!match) return;
    const term = match[1].trim();
    if (!term) return;

    let res;
    try {
        res = await get(
            `https://registry.example.com/search?q=${encodeURIComponent(term)}`,
            { signal, timeoutMs: 3000 },
        );
    } catch (err) {
        // Aborted, timed out, or transport failure — render nothing.
        if (err.name === 'AbortError') return;
        throw err;
    }
    if (!res.ok) return;

    const body = res.json();
    for (const pkg of body.results ?? []) {
        if (signal.aborted) return;
        yield {
            key: pkg.name,
            title: pkg.name,
            subtitle: pkg.description ?? '',
            action: openUrl(pkg.url),
        };
    }
}
```

Notes:

- Pass `signal` into `get`/`post` so the abort cascades from "user typed
  another keystroke" into the reqwest future.
- `timeoutMs: 3000` overrides the 30 s default for this call. Pick
  aggressively — autocomplete UI feels bad waiting 30 seconds for a
  failing fetch.
- Catch `AbortError` and return silently; that's the cancellation path.
- Check `signal.aborted` between yields if you produce many rows after a
  slow await; otherwise the next keystroke's rows can interleave with this
  one's.
- The plugin must declare the `http` capability.

Real plugin: `plugins/xkcd` (HTTP + abort + cache).

## Cross-platform behavior

Use this when the plugin's data sources differ between macOS and Linux —
e.g. app discovery via `.app` bundles vs `.desktop` files.

```js
import { isMacOS, isLinux } from 'highbeam:platform';

async function collectApps() {
    if (isMacOS()) return collectMacApps();
    if (isLinux()) return collectLinuxApps();
    return [];
}

async function collectMacApps() {
    // /Applications, ~/Applications, ...
}

async function collectLinuxApps() {
    // /usr/share/applications, ~/.local/share/applications, ...
}
```

For a single behaviour that's a no-op on one platform — e.g. AppleScript —
you don't have to gate. `highbeam:system.applescript` resolves to `null`
immediately on non-macOS rather than throwing:

```js
import { applescript } from 'highbeam:system';

// Resolves to null on Linux; no isMacOS() check needed.
const result = await applescript('tell application "Finder" to get name');
```

Limiting the manifest's `platforms` array shelves the plugin on platforms
where it can't work at all:

```json
{
    "name": "spotlight-only",
    "platforms": ["macos"]
}
```

The host won't even load it on Linux. `platforms` absent loads everywhere;
empty `[]` shelves the plugin entirely (never loads).

Real plugin: `plugins/app-launcher` (`isMacOS()` / `isLinux()` to
branch between `.app` and `.desktop` discovery).

## Read user-editable options via `highbeam:settings`

Use this when the plugin needs a knob the user can flip — a default search
engine, a result cap, a username substituted into URL templates.

In `manifest.json`:

```json
{
    "name": "quick-links",
    "options": [
        { "key": "github_username", "type": "string", "label": "GitHub username", "default": "" },
        { "key": "result_limit", "type": "int", "label": "Max results", "default": 10, "min": 1, "max": 50 }
    ]
}
```

In `plugin.js`:

```js
import { openUrl } from 'highbeam:actions';
import { getString, getInt } from 'highbeam:settings';

export async function* query(input, _signal) {
    const user = getString('github_username') ?? '';
    const limit = getInt('result_limit') ?? 10;

    // ...use `user` to expand `gh me/repo` to `gh <user>/repo`, etc.
    // ...cap yields at `limit`.
}
```

The settings UI (open via Cmd+, or by typing `settings`) renders an input
per option type — text field for `string`, toggle for `bool`, number input
for `int`, click-to-cycle for `enum`. Values persist in
`~/Library/Application Support/high-beam/settings.toml` on macOS,
`$XDG_CONFIG_HOME/high-beam/settings.toml` on Linux.

Notes:

- Each plugin only sees its OWN options. `get('foo')` from plugin A and
  plugin B return different values.
- Reading a value is "cheap" — populated into the JS context at load time.
- Reload is restart-only in v1. Editing a setting persists immediately, but
  the already-loaded plugin keeps the value it saw when it loaded.
- Use the typed variants (`getString` / `getBool` / `getInt`) when a stale
  `settings.toml` (e.g. from a manifest rename) might carry the wrong shape
  — they return `undefined` on mismatch rather than handing you the wrong
  type.

Real plugin: `plugins/quick-links` (`github_username` +
`result_limit` options).

## Mock SDK calls in vitest

Use this when testing a plugin that calls SDK functions with side effects.
The SDK ships `vi.fn()` stubs alongside the `.d.ts` files; vitest's
`resolve.alias` maps `highbeam:*` onto them.

`vitest.config.ts`:

```ts
import config from '../../sdk/highbeam/vitest.config.example';
export default config;
```

(Adjust the relative path to wherever `sdk/highbeam/` lives. The example
plugins use `../../../sdk/highbeam/`.)

Mocking `fs.readText` for a plugin that loads bundled data:

```js
import { beforeEach, describe, expect, test, vi } from 'vitest';
import * as fs from 'highbeam:fs';
import { query } from './plugin.js';

beforeEach(() => {
    vi.mocked(fs.readText).mockReset();
    vi.mocked(fs.readText).mockResolvedValue(JSON.stringify([
        { name: 'foo', value: 1 },
        { name: 'bar', value: 2 },
    ]));
});

test('loads data and yields matching rows', async () => {
    // ...
});
```

Mocking `highbeam:settings` for a plugin that reads option values:

```js
import { describe, expect, test, vi } from 'vitest';
import { getString, getInt } from 'highbeam:settings';
import { query } from './plugin.js';

test('uses the user-set username', async () => {
    vi.mocked(getString).mockImplementation((key) =>
        key === 'github_username' ? 'octocat' : undefined,
    );
    vi.mocked(getInt).mockImplementation((key) =>
        key === 'result_limit' ? 5 : undefined,
    );
    // ...drive the plugin...
});
```

The SDK stubs are `vi.fn()`s that return `undefined` by default, so any
unmocked `get*` call falls back to the plugin's `?? default` branch.

Mocking `http.get` per-test:

```js
import { vi } from 'vitest';
import { get } from 'highbeam:http';

vi.mocked(get).mockResolvedValueOnce({
    status: 200,
    statusText: 'OK',
    headers: {},
    body: JSON.stringify({ result: 'ok' }),
    ok: true,
    json() { return { result: 'ok' }; },
    text() { return this.body; },
});
```

Resetting a plugin's module-level cache between tests:

```js
async function loadPlugin() {
    vi.resetModules();
    const http = await import('highbeam:http');
    vi.mocked(http.get).mockReset();
    vi.mocked(http.get).mockResolvedValue(/* default */);
    const plugin = await import('./plugin.js');
    return { plugin, http };
}
```

Notes:

- `highbeam:actions` is not a mock — the stub returns the same plain
  objects the host does, so `expect(action).toEqual({ kind: 'copy', text:
  '...' })` works straight out.
- `highbeam:platform` is real (reads `process.platform` and friends), so
  `isMacOS()` reflects the test host. Stub it explicitly when you need
  cross-platform coverage.
- `highbeam:match` is a faithful port of the host matcher. Order and
  highlight ranges agree with `nucleo-matcher` on realistic input.
- `vi.resetModules()` clears the plugin's module-level cache between
  tests. Without it, a `let cache = null` declared at module scope carries
  over.

Real test suites: `plugins/http-codes/http-codes.test.js`
(mocking `fs.readText`), `plugins/xkcd/xkcd.test.js`
(mocking `http.get` + `fs.readCache` + `vi.resetModules`).

## Stream results vs return all at once

Use streaming when each row takes meaningful work (network, disk) and the
user benefits from seeing partial results. Use one-shot when the work is
in-memory and finishes in microseconds anyway.

Streaming — yields as it goes; the renderer paints rows as they arrive:

```js
export async function* query(input, signal) {
    if (!input) return;
    for (const item of expensiveSource) {
        if (signal.aborted) return;
        await someWork(item);
        yield { key: item.id, title: item.name, action: copy(item.name) };
    }
}
```

One-shot — all at once; simpler for cheap in-memory work:

```js
export async function* query(input, _signal) {
    if (!input) return;
    const matches = items.filter((i) => i.name.includes(input));
    for (const item of matches) {
        yield { key: item.id, title: item.name, action: copy(item.name) };
    }
}
```

Notes:

- Both shapes use `async function*` — that's the contract the host
  expects. "All at once" still yields, just in tight succession.
- Streaming + `signal.aborted` checks let the user cancel mid-yield by
  typing another keystroke.
- Don't `await new Promise(r => setTimeout(r, 0))` between every yield —
  that's superstition. The host iterates as fast as you yield.

Real plugin: `plugins/slow-echo` is the streaming + abort smoke
test (three rows, 300ms apart).

## Pinned results vs frecency-ranked

Use `pinned: true` when the result is a "first-class answer" that should
beat anything frecency would surface — calculator output, syntax-error
rows, status codes.

```js
yield {
    key: `calc:${expr}`,
    title: result,
    weight: 100,
    pinned: true,
    action: copy(result),
};
```

Use plain `weight` (no `pinned`) when the result is one of many and
frecency should decide ranking based on user picks.

```js
yield {
    key: `app:${app.path}`,
    title: app.name,
    weight: score * 100,
    action: openUrl(app.path),
};
```

Notes:

- `pinned: true` bypasses frecency entirely and sorts above every
  non-pinned row on screen.
- Among pinned rows, `weight` is the tie-breaker. The host caps pinned
  weight at 100.
- Among non-pinned rows, the host computes
  `weight * frecency_modifier(picks, age)` and sorts. The modifier starts
  at 1.0, jumps by ~10% per pick, decays back over a 14-day half-life.
- `key` must be stable per (plugin, conceptual result) — that's what the
  frecency table joins on. Don't fold the user's current input into it.

Real plugins:

- `plugins/calculator` — pinned, weight 100.
- `plugins/http-codes` — pinned, weight scales with prefix length.
- `plugins/dnd` — non-pinned, weight from `match.fuzzy` score.
- `plugins/frecency-demo` — three equal-weight rows; pick one and
  watch it bubble up.

## Tune `debounceMs` / `timeoutMs` / `memoryMb`

Use the manifest knobs to keep the launcher responsive when the plugin
isn't trivial.

`debounceMs` — wait this long after the latest keystroke before invoking
`query()`. Default 0 (every keystroke); capped at 2000.

- `0` for in-memory work that finishes in microseconds (echo, calculator,
  paper-size).
- `10–50` for "loads bundled JSON on first hit" — the load is amortised
  but the parse on first match is non-zero
  (`plugins/http-codes`: 10, `plugins/dnd`: 50).
- `100+` for plugins that scan disk or hit the network — gives the user
  time to finish typing before the work starts
  (`plugins/app-launcher`: 100).

`timeoutMs` — wall-clock kill switch for `query()`. The rquickjs interrupt
hook fires after this elapses, killing the context's current async
operation. Default 500 ms.

- `100` for pure computation that should be instant (calculator: 100).
- `200–800` for "load bundled data, fuzzy-rank, maybe hit an icon
  resolver" (dnd: 200, app-launcher: 800).
- Up to a few seconds when you intentionally do slow work like a network
  fetch — but raise `debounceMs` correspondingly so the timeout isn't
  starting from scratch on every keystroke.

`memoryMb` — QuickJS context memory cap. Default 32.

- `16` for trivial plugins (echo, calculator, paper-size).
- `32` for plugins that load a JSON blob (the default).
- `64` for plugins that index a couple hundred items in memory
  (app-launcher, dnd).

Diagnosis:

- A plugin hitting `timeoutMs` logs `WARN` to `plugin.log` and its results
  are dropped for that query.
- A plugin hitting `memoryMb` logs `ERROR` to `plugin.log` and likewise
  drops results. Repeated failures don't auto-disable in v1 — every query
  gets a fresh try.
- Plugins that are slow but should still complete should raise
  `debounceMs` first (avoid the work entirely), then `timeoutMs` (give it
  longer when it does run), and only finally `memoryMb` (which is rarely
  the actual problem).
