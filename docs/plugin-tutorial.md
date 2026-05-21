# Plugin tutorial: build a greetings plugin

This walks you from an empty directory to a working High Beam plugin that says
hello in a chosen language when the user types `greet <name>`. Plan an hour;
most of that will be reading. Real authoring time is ~15 minutes once you've
done it once.

By the end you will have:

- A `plugins/greetings/` directory the daemon loads on startup.
- A `manifest.json` declaring the capabilities the plugin needs.
- A `plugin.js` exporting an `async function* query()` that yields rows.
- A vitest suite covering the trigger and a couple of edge cases.
- The plugin running live in the launcher.

If you already know the shape and just want a reference, jump to
[sdk-reference.md](./sdk-reference.md). If you want copy-pasteable recipes for
specific patterns, jump to [plugin-cookbook.md](./plugin-cookbook.md).

## What you're building

The plugin triggers on the word `greet`. Type `greet Alice` and the launcher
shows three rows — one English, one Dutch, one Spanish — each of which copies
the rendered greeting to the clipboard when picked.

```
> greet Alice
  Hello, Alice!         press Enter to copy        [English]
  Hallo, Alice!         press Enter to copy        [Dutch]
  ¡Hola, Alice!         press Enter to copy        [Spanish]
```

A language can also be pinned to the top by typing `greet <lang> <name>`
(`greet nl Alice`) — that's the wrinkle we'll iterate on after the first
working version.

## Step 1 — scaffold the directory

Pick where the daemon should pick it up from. The order the host scans:

1. `--plugins-dir <path>` (test override; not what you want here).
2. `./plugins/` next to the binary's cwd (handy during dev).
3. Platform default:
   - macOS: `~/Library/Application Support/high-beam/plugins/`
   - Linux: `$XDG_DATA_HOME/high-beam/plugins/`

For this tutorial, use the repo-local `./plugins/` next to the daemon — that
way you can edit in place and restart without copying around.

```bash
mkdir -p plugins/greetings
cd plugins/greetings
```

Two files are required:

```
plugins/greetings/
  manifest.json
  plugin.js
```

That's it. No `package.json`, no `node_modules`, no compile step — the host
loads `plugin.js` straight into a QuickJS context at startup. The vitest
plumbing we'll add later is dev-only.

## Step 2 — write `manifest.json`

The manifest tells the host how to load the plugin and what it's allowed to
do. Capabilities are the important part: every `highbeam:*` module needs a
matching declared capability, and importing one without it is a load-time
error logged to `plugin.log`.

```json
{
  "name": "greetings",
  "displayName": "Greetings",
  "version": "0.1.0",
  "description": "Say hello in a chosen language",
  "entry": "plugin.js",
  "debounceMs": 0,
  "timeoutMs": 200,
  "memoryMb": 16,
  "capabilities": ["actions"]
}
```

Field-by-field:

- `name` — unique identifier. Becomes the frecency-table key prefix and the
  cache-dir name; keep it lowercase-kebab.
- `displayName` — what shows in error messages and (eventually) settings.
- `entry` — defaults to `plugin.js`; setting it explicitly is harmless and
  obvious.
- `debounceMs: 0` — invoke `query()` on every keystroke. Plugins that hit the
  network or scan disk should raise this; ours is in-memory only.
- `timeoutMs: 200` — wall-clock kill switch. The default is 500 ms; we don't
  need that much.
- `memoryMb: 16` — QuickJS context memory cap. Default is 32; this plugin is
  tiny.
- `capabilities: ["actions"]` — the only module we'll import is
  `highbeam:actions` for the `copy()` action builder.

Full schema lives in `src/plugins/manifest.rs`. Unknown fields are tolerated.

If you want user-editable knobs (defaults to use when the user hasn't set
anything), add an `options` array — the settings UI renders each entry and
your plugin reads the values via `highbeam:settings`. See the
[settings recipe](./plugin-cookbook.md#read-user-editable-options-via-highbeamsettings)
in the cookbook.

## Step 3 — write `plugin.js`

The host calls `query(input, signal)` on every keystroke (post-debounce) and
iterates whatever you yield until the iterator returns. The signature:

```ts
export async function* query(
  input: string,
  signal: AbortSignal,
): AsyncIterable<Result>;
```

Each yielded `Result` is a row. The shape:

```ts
type Result = {
  key: string;             // stable per (plugin, conceptual result)
  title: string;
  subtitle?: string;
  weight?: number;         // 0..100; higher ranks first
  pinned?: boolean;        // sort above non-pinned regardless of weight
  action: Action;
};
```

Here's the first cut. Drop it into `plugins/greetings/plugin.js`:

```js
import { copy } from 'highbeam:actions';

const TRIGGER = /^\s*greet(?:\s+(.+))?$/i;

const LANGUAGES = [
    { code: 'en', name: 'English', render: (name) => `Hello, ${name}!` },
    { code: 'nl', name: 'Dutch',   render: (name) => `Hallo, ${name}!` },
    { code: 'es', name: 'Spanish', render: (name) => `¡Hola, ${name}!` },
];

export async function* query(input, _signal) {
    const match = TRIGGER.exec(input);
    if (!match) return;
    const name = (match[1] ?? '').trim();
    if (!name) return;

    for (const lang of LANGUAGES) {
        const greeting = lang.render(name);
        yield {
            key: `greet:${lang.code}`,
            title: greeting,
            subtitle: lang.name,
            action: copy(greeting),
        };
    }
}
```

The shape worth highlighting:

- **Keyword gate first.** The regex bails before any allocation when the user
  isn't reaching for this plugin. Every keystroke fans out to every loaded
  plugin in parallel; cheap rejection paths are how you stay invisible.
- **Generator yields one row at a time.** The host renders them
  progressively. For three rows it doesn't matter, but the muscle memory of
  "yield, don't return an array" pays off when you wire up an HTTP fetch
  later.
- **`key` is stable per row.** `greet:en` will always be the English row,
  regardless of which name the user typed. Frecency keys on
  `(plugin_name, result.key)`, so a stable key means "pick the English row
  once" bumps the English row on future runs.
- **No `weight`, no `pinned`.** The plugin can leave both off and let the
  default ranking (weight 0 modulated by frecency) sort it out. You'd reach
  for `pinned: true` if you wanted this to always sit above non-pinned
  matches from other plugins.

## Step 4 — run it

Restart the daemon. The host scans the plugins directory once at startup, so
edits don't hot-reload.

```bash
# from the repo root
cargo run
```

Hit your launcher hotkey (`Shift+Space` on macOS; whatever you bound
`highbeam --open` to on Linux). Type `greet Alice`. Three rows should appear.

If they don't:

- Check `plugins/greetings/plugin.log`. Capability errors, parse errors,
  uncaught exceptions and `console.log/warn/error` calls land there.
- Check the daemon's stderr. Manifest parse errors don't get a `plugin.log`
  (the plugin never loaded), so they show up where the daemon's `tracing`
  output goes.
- Make sure `manifest.json` is valid JSON. The host is strict.

## Step 5 — iterate

You'll want a fast feedback loop. Three knobs:

- **`console.log` to plugin.log.** Anything you print goes to
  `plugins/greetings/plugin.log` with a timestamp and level. Useful for
  "what's the actual `input` here?" sanity checks.
- **Restart the daemon to reload.** There's no hot reload in v1. Quit and
  rerun `cargo run`.
- **Run query() under vitest.** The fastest loop. See Step 6.

Let's add the `greet <lang> <name>` wrinkle — when the user types
`greet nl Alice`, Dutch should sort to the top.

Update the regex and the body:

```js
import { copy } from 'highbeam:actions';

const TRIGGER = /^\s*greet(?:\s+(\S+))?(?:\s+(.+))?$/i;

const LANGUAGES = [
    { code: 'en', name: 'English', render: (name) => `Hello, ${name}!` },
    { code: 'nl', name: 'Dutch',   render: (name) => `Hallo, ${name}!` },
    { code: 'es', name: 'Spanish', render: (name) => `¡Hola, ${name}!` },
];

const LANG_BY_CODE = new Map(LANGUAGES.map((l) => [l.code, l]));

export async function* query(input, _signal) {
    const match = TRIGGER.exec(input);
    if (!match) return;

    const first = (match[1] ?? '').trim();
    const rest = (match[2] ?? '').trim();

    // `greet nl Alice` — first is a known language code, rest is the name.
    // `greet Alice` — first is the name, rest is empty.
    let preferred;
    let name;
    if (rest && LANG_BY_CODE.has(first.toLowerCase())) {
        preferred = first.toLowerCase();
        name = rest;
    } else {
        name = [first, rest].filter(Boolean).join(' ');
    }
    if (!name) return;

    for (const lang of LANGUAGES) {
        const greeting = lang.render(name);
        const row = {
            key: `greet:${lang.code}`,
            title: greeting,
            subtitle: lang.name,
            action: copy(greeting),
        };
        if (preferred === lang.code) {
            row.pinned = true;
            row.weight = 100;
        }
        yield row;
    }
}
```

`pinned: true` bypasses frecency and sorts above every non-pinned result on
the screen; `weight: 100` tie-breaks among pinned rows (the host caps pinned
weight at 100 anyway). Restart the daemon, type `greet nl Alice`, and Dutch
sits on top.

## Step 6 — add vitest

The SDK ships Node-compatible stubs alongside the `.d.ts` files. That means
you can `import { copy } from 'highbeam:actions'` from a `.test.js` running in
plain Node, with vitest's `resolve.alias` mapping the `highbeam:*` specifier
onto the stub.

In `plugins/greetings/`:

```json
{
  "name": "high-beam-plugin-greetings",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": { "test": "vitest run" },
  "devDependencies": { "vitest": "^1.6.0" }
}
```

```ts
// vitest.config.ts — re-exports the recipe shipped with the SDK
import config from '../../sdk/highbeam/vitest.config.example';
export default config;
```

(Adjust the relative path to wherever `sdk/highbeam/` lives relative to your
plugin; the example plugins use `../../../sdk/highbeam/` because they live
under `examples/plugins/<name>/`.)

```js
// greetings.test.js
import { describe, expect, test } from 'vitest';
import { query } from './plugin.js';

async function collect(iter) {
    const out = [];
    for await (const item of iter) out.push(item);
    return out;
}

describe('greetings plugin', () => {
    test('non-trigger input yields nothing', async () => {
        expect(await collect(query('hello', { aborted: false }))).toEqual([]);
    });

    test('bare `greet` yields nothing', async () => {
        expect(await collect(query('greet', { aborted: false }))).toEqual([]);
        expect(await collect(query('greet ', { aborted: false }))).toEqual([]);
    });

    test('`greet Alice` yields three rows in declared order', async () => {
        const results = await collect(query('greet Alice', { aborted: false }));
        expect(results).toHaveLength(3);
        expect(results.map((r) => r.subtitle)).toEqual([
            'English', 'Dutch', 'Spanish',
        ]);
        expect(results[0].title).toBe('Hello, Alice!');
        expect(results[0].action).toEqual({
            kind: 'copy', text: 'Hello, Alice!',
        });
    });

    test('`greet nl Alice` pins the Dutch row', async () => {
        const results = await collect(
            query('greet nl Alice', { aborted: false }),
        );
        const nl = results.find((r) => r.subtitle === 'Dutch');
        expect(nl.pinned).toBe(true);
        expect(nl.weight).toBe(100);
        expect(nl.title).toBe('Hallo, Alice!');
    });

    test('every row has a stable per-language key', async () => {
        const a = await collect(query('greet Alice', { aborted: false }));
        const b = await collect(query('greet Bob', { aborted: false }));
        expect(a.map((r) => r.key)).toEqual(b.map((r) => r.key));
    });
});
```

Then:

```bash
cd plugins/greetings
npm install
npm test
```

vitest watches by default; `vitest run` runs once and exits. The iteration
loop is now: edit `plugin.js`, save, vitest re-runs in <100ms.

SDK modules with side effects (`highbeam:http`, `highbeam:clipboard`,
`highbeam:fs`, `highbeam:system`, `highbeam:icons`) ship as `vi.fn()`s so you
can spy and override per-call:

```js
import { readText } from 'highbeam:fs';
vi.mocked(readText).mockResolvedValueOnce('{"hello": "world"}');
```

`highbeam:actions` is the exception — the stub returns the same plain objects
the host does, so `expect(action).toEqual({ kind: 'copy', text: '...' })`
works without mocking.

## Step 7 — ship it

Drop the directory into the platform default location:

```bash
# macOS
cp -r plugins/greetings ~/Library/Application\ Support/high-beam/plugins/
# Linux
cp -r plugins/greetings "$XDG_DATA_HOME/high-beam/plugins/"
```

Restart High Beam. Type `greet Alice`. Done.

## Step 8 — publish as install-by-URL

Once the plugin is working locally, hand it out via `install <url>`
instead of asking users to copy directories. The full publishing
checklist lives in
[plugin-authoring.md](./plugin-authoring.md#publishing--distribution);
the short version is:

1. `tar -czf greetings.tar.gz -C plugins/greetings .`
2. Upload `greetings.tar.gz` and `manifest.json` to any HTTP(S) host
   (S3, GitHub Releases, your own server).
3. Add `archiveUrl` + `manifestUrl` to `manifest.json` so the install
   flow knows where to download from and where to look for updates:

   ```json
   {
     "name": "greetings",
     "version": "1.0.0",
     "archiveUrl": "https://example.com/greetings/1.0.0/greetings.tar.gz",
     "manifestUrl": "https://example.com/greetings/manifest.json"
   }
   ```

4. Tell your users to type `install https://example.com/greetings/manifest.json`
   in the launcher. They get a streaming "Installing greetings…" → "Installed
   greetings v1.0.0" row; the plugin is live immediately, no restart.
5. When you ship a new version, bump `version` in the hosted manifest +
   archive. Users run `update` to pick it up.

The installer keeps the previous install at `<name>.backup.<unix_ms>/`
so a botched ship is recoverable by hand.

## Where to go next

- [sdk-reference.md](./sdk-reference.md) — complete SDK reference, one section
  per `highbeam:*` module. Look here when you want to know what a function
  takes and returns.
- [plugin-cookbook.md](./plugin-cookbook.md) — recipes for common patterns:
  fuzzy matching a bundled list, caching expensive computations, HTTP with
  timeout + abort, cross-platform branching, mocking SDK calls in vitest.
- [plugin-authoring.md](./plugin-authoring.md) — the overview hub if you
  need to re-find any of the above.

Real plugins to read for inspiration (all under `examples/plugins/`):

- `echo` — minimal `copy(input)`.
- `calculator` — pinned inline results, npm-free expression parser.
- `paper-size` — inlined data, substring filter.
- `http-codes` — bundled JSON loaded via `fs.readText`.
- `dnd` — bundled JSON + `match.fuzzy` ranking.
- `app-launcher` — `fs.readDir` + `icons.forPath` + `match.fuzzy`,
  cross-platform.
- `xkcd` — HTTP, `fs.cache` for an index, fuzzy title search.
- `slow-echo` — streaming + abort.
- `frecency-demo` — equal-weight rows to demonstrate pick-bumping.
- `echo-ts` — TypeScript variant with a `tsconfig.json`.
