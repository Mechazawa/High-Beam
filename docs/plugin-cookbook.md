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

## fetch with timeout and abort

Use this when fetching from a remote API. Compose the host's `signal` with a
per-request timeout so the next keystroke cancels the in-flight request and a
slow server can't hang the query.

```js
import { openUrl } from 'highbeam:actions';

const TRIGGER = /^pkg\s+(.+)$/i;

export async function* query(input, signal) {
    const match = TRIGGER.exec(input);
    if (!match) return;
    const term = match[1].trim();
    if (!term) return;

    let res;
    try {
        res = await fetch(
            `https://registry.example.com/search?q=${encodeURIComponent(term)}`,
            { signal: AbortSignal.any([signal, AbortSignal.timeout(3000)]) },
        );
    } catch (err) {
        // Aborted, timed out, or transport failure — render nothing.
        if (err.name === 'AbortError' || err.name === 'TimeoutError') return;
        throw err;
    }
    if (!res.ok) return;

    const body = await res.json();
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

- `AbortSignal.any([signal, AbortSignal.timeout(3000)])` aborts on either the
  keystroke signal or a 3 s timeout, whichever fires first. Without a timeout
  signal, the host's 30 s default still applies. Pick aggressively;
  autocomplete UI feels bad waiting 30 seconds for a failing fetch.
- Pass the host `signal` so the abort cascades from "user typed another
  keystroke" into the request.
- Catch `AbortError` / `TimeoutError` and return silently; that's the
  cancellation path.
- `await res.json()` (the body readers are async).
- Check `signal.aborted` between yields if you produce many rows after a
  slow await; otherwise the next keystroke's rows can interleave with this
  one's.
- The plugin must declare the `http` capability.

Real plugin: `plugins/xkcd` (fetch + abort + cache).

## Cross-platform behavior

Use this when the plugin's data sources differ between macOS and Linux —
e.g. app discovery via `.app` bundles vs `.desktop` files.

```js
import os from 'node:os';

async function collectApps() {
    if (os.platform() === 'darwin') return collectMacApps();
    if (os.platform() === 'linux') return collectLinuxApps();
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

// Resolves to null on Linux; no platform check needed.
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

Real plugin: `plugins/app-launcher` (`os.platform() === 'darwin'` /
`=== 'linux'` to branch between `.app` and `.desktop` discovery).

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

Mocking `fetch` per-test (it's a global, so stub `globalThis.fetch` rather
than a module import):

```js
import { vi } from 'vitest';

vi.stubGlobal('fetch', vi.fn().mockResolvedValueOnce(
    new Response(JSON.stringify({ result: 'ok' }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
    }),
));
```

`Response` exists in Node 18+ (and under vitest), so the real class works in
tests. Call `vi.unstubAllGlobals()` in `afterEach` to restore.

Resetting a plugin's module-level cache between tests:

```js
async function loadPlugin() {
    vi.resetModules();
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(/* default Response */));
    const plugin = await import('./plugin.js');
    return { plugin };
}
```

Notes:

- `highbeam:actions` is not a mock — the stub returns the same plain
  objects the host does, so `expect(action).toEqual({ kind: 'copy', text:
  '...' })` works straight out.
- `node:os` is real, so `os.platform()` reflects the test host. Mock the
  whole module to drive platform detection per test (ESM namespaces are
  frozen, so replacing the module beats `spyOn`):

  ```js
  vi.mock('node:os', () => {
      const platform = vi.fn(() => 'darwin');
      return { default: { platform }, platform };
  });
  // later: vi.mocked((await import('node:os')).default.platform)
  //     .mockReturnValue('linux');
  ```
- `highbeam:match` is a faithful port of the host matcher. Order and
  highlight ranges agree with `nucleo-matcher` on realistic input.
- `vi.resetModules()` clears the plugin's module-level cache between
  tests. Without it, a `let cache = null` declared at module scope carries
  over.

Real test suites: `plugins/http-codes/http-codes.test.js`
(mocking `fs.readText`), `plugins/xkcd/xkcd.test.js`
(stubbing `fetch` + mocking `fs.readCache` + `vi.resetModules`).

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

## Open a plugin view from a result row

Push a dynamic, stateful screen onto the launcher stack instead of running a
one-shot action on Enter. Useful when one Enter would have to fan into "open
this, abort that, see status" — see [views.md](./views.md) for the full
contract.

```js
import { showView } from 'highbeam:actions';
import { Heading, Text, Spinner } from 'highbeam:view';

const Detail = {
    setup: (props) => ({ id: props.id, data: null }),
    async mounted({ signal }) {
        this.data = await fetchDetail(this.id, signal);
    },
    render() {
        if (!this.data) return { body: [Spinner({ label: 'Loading…' })] };
        return { title: this.data.name, body: [
            Heading({ text: this.data.title }),
            Text({ text: this.data.summary }),
        ]};
    },
};

export async function* query(input, _signal) {
    if (input !== 'detail') return;
    yield {
        key: 'open-detail',
        title: 'Open detail view',
        action: showView(Detail, { id: 42 }),
    };
}
```

The view's `setup` runs once on push; `mounted` runs after the first paint
so the user sees the spinner before any work starts. State mutation inside
methods (`this.data = …`) re-renders automatically.

## Cancel stale in-flight requests when re-fetching

Inside a view's methods, multiple async fetches can interleave. The
naive write — assigning to `this.data` once each fetch resolves — lets a
slow earlier call clobber a fast later one. Hold an `AbortController` per
fetch key and abort the previous before starting a new one:

```js
methods: {
    async load() {
        this.loadCtrl?.abort();
        this.loadCtrl = new AbortController();
        this.loading = true;
        try {
            const res = await fetch(this.url, { signal: this.loadCtrl.signal });
            this.data = await res.json();
        } catch (err) {
            if (err.name !== 'AbortError') this.err = err;
        }
        this.loading = false;
    },
},
```

The host's `signal` (from `mounted({ signal })`) is also fine to pass —
it aborts on view close. The per-fetch controller is what handles "user
clicked Refresh while a load is mid-flight."

## Pass a callback from a parent view to a child view

`showView(view, props)` walks `props` and substitutes any function values
with callback ids the same way `on*` handlers work. The child invokes
`props.onPick(value)` and the closure fires inside the parent's reactive
proxy — `this` is the parent's state.

```js
const Parent = {
    setup: () => ({ picked: null }),
    methods: {
        pickColor() {
            showView(ColorPicker, {
                initial: this.picked,
                onPick: (color) => { this.picked = color; },
            });
        },
    },
    render() {
        return { body: [
            Text({ text: this.picked ? `Picked: ${this.picked}` : 'Nothing picked' }),
            Button({ label: 'Pick a colour', onClick: 'pickColor' }),
        ]};
    },
};

const ColorPicker = {
    setup: (props) => ({ value: props.initial }),
    methods: {
        confirm() {
            // `props.onPick` is reconstituted as a callable on the
            // child side — calling it fires the parent's closure.
            this.props.onPick(this.value);
            return closeView;
        },
    },
    // …
};
```

No first-class modal-return plumbing — Stage v1 leaves it to the closure
pattern. The pattern survives parent-popped: if the parent's frame closed
before the child returns, the parent's `this` mutations are silently
dropped (logged once at INFO in `plugin.log`).

## Show a remote image inside a view

`Image({ src })` accepts a `data:` URI only — the same rule as
`Result.icon`. Plugins fetch with `fetch` and base64-encode the body
themselves (`Buffer` is an always-on global). Watch the size: a 5 MB JPEG
base64-encodes to ~7 MB of JS string, which blows the default 32 MB
`memoryMb` cap.

```js
import { Heading, Image, Spinner } from 'highbeam:view';

const Photo = {
    setup: (props) => ({ src: null, title: props.title }),
    async mounted({ signal }) {
        const res = await fetch(this.props.src, { signal });
        const base64 = Buffer.from(await res.arrayBuffer()).toString('base64');
        this.src = `data:image/jpeg;base64,${base64}`;
    },
    render() {
        if (!this.src) return { body: [Spinner({ label: 'Loading image…' })] };
        return { title: this.title, body: [
            Heading({ text: this.title }),
            Image({ src: this.src, fit: 'contain' }),
        ]};
    },
};
```

If your images run large, bump `memoryMb` in `manifest.json` (`64` or
`128`) and consider caching the encoded blob via `fs.writeCache` so
re-opens skip the download.
