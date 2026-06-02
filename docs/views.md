# Views

Plugins can return *views* — dynamic, stateful screens that replace the
launcher's result list. A view is a JavaScript object with `setup`,
`render`, and optional `mounted` / `unmounted` hooks. State is reactive:
mutating it re-renders. Buttons and inputs talk back via closures or by
returning actions.

Companion docs: [sdk-reference.md](./sdk-reference.md) (`highbeam:view`
and `highbeam:actions` reference), [plugin-cookbook.md](./plugin-cookbook.md)
(recipes), [internals.md](./internals.md) (host stack / frame protocol).

## When to use a view

A `query()` result row is the right tool when "Enter does one
thing". Reach for a view when:

- you want to show progress while async work runs;
- the user picks from a list whose contents need their own filter / refresh;
- a single Enter would have to fan into "open this, abort that, see status";
- you want forms, multi-step flows, image previews, or anything else the
  one-row Result shape can't carry.

A view is not free — it replaces the result list, the user is
intentionally driven into it via a `showView` action, and Esc pops back.
Don't push a view from a query result that *could* have been a normal
row.

## The shape

```js
// plugin.js
import { showView } from 'highbeam:actions';
import { Heading, Text, Button, Stack } from 'highbeam:view';

const Pipeline = {
    setup: (props) => ({
        id: props.id,
        pipeline: null,
        refreshing: false,

        async load() {
            this.refreshing = true;
            this.pipeline = await fetchPipeline(this.id);
            this.refreshing = false;
        },
    }),

    async mounted({ signal }) {
        await this.load();
    },

    render() {
        if (!this.pipeline) return { body: [Spinner({ label: 'Loading…' })] };
        const p = this.pipeline;
        return { title: `Pipeline #${p.id}`, body: [
            Heading({ text: p.name }),
            Text({ text: `Status: ${p.status}`,
                   tone: p.status === 'failed' ? 'error' : undefined }),
            Stack({ direction: 'h', gap: 'sm', children: [
                Button({ label: 'Refresh', onClick: () => this.load() }),
            ]}),
        ]};
    },
};

export async function* query(input, signal) {
    yield { key: 'p1', title: 'Pipeline #1',
        action: showView(Pipeline, { id: 1 }) };
}
```

Views are values. The plugin holds the object literal, an action carries
an opaque handle, and the host bounces events back through the handle.
There is no global `views` registry, no name lookup, no per-name
versioning hazard.

**Capability:** declaring `views` is not required. A view inherits the
plugin's existing capabilities (`http`, `fs.read`, etc.) — the things
methods can *do* gate themselves the same way they would in `query()`.

## The contract

```ts
interface ViewDef<P = object> {
    setup: (props: P) => Record<string, unknown>;
    mounted?:  (this: any, ctx: { signal: AbortSignal }) => void | Promise<void>;
    unmounted?: (this: any) => void;
    render: (this: any) => ViewNode | null;
}

interface ViewNode {
    title?: string;
    body: Block[];
}
```

- `setup(props)` runs once, returns the working object — **data and
  methods mixed in the same bag.** The SDK wraps it in a `Proxy`; that
  proxy becomes `this` everywhere else.
- `mounted({ signal })` runs once, on the next microtask after the first
  render paints. `signal` aborts on view close.
- `unmounted()` runs synchronously when the frame is popped. Use it to
  release file handles, abort sub-controllers, etc. Return value is
  ignored; if you start async work, fire-and-forget — the host doesn't
  wait.
- `render()` runs synchronously; returns a `ViewNode` or `null`.
  Returning `null` closes the current frame (and the launcher if it was
  the only frame).

## Reactivity rules

The proxy traps every property `set` on the working object (including
nested objects and arrays — deep by default). Mutations queue a
re-render on the next microtask. Multiple sync mutations coalesce:

```js
this.x = 1;
this.y = 2;
this.z = 3;
// → one render
```

Across `await`:

```js
this.x = 1;
await something();
this.y = 2;
// → two renders (microtask boundary at the await)
```

Things to know:

- **Nested mutations re-render.** `this.user.name = 'Bas'`,
  `this.list.push(x)`, `this.matrix[2][3] = 'x'` — all trip the proxy.
- **Reassigning a function slot re-renders too.** `this.load = newFn`
  marks the object dirty. The function value itself isn't watched —
  closures captured by callbacks see whichever value was in the slot at
  the time of the previous render.
- **Computed values and watchers are not in v1.** Recompute inside
  `render()` (cheap at launcher scale); run side effects from methods.

## Events: `on*` handlers

`Button`, `Input`, `TextArea`, and `Row` accept `on*` props. They can be
plain functions (closures) or bare `Action` values (shorthand). Both
collapse to one path on the wire: each handler becomes a callback id
that the host fires back as `{ id, kind, value? }`.

```js
Button({ label: 'Abort',   onClick: () => this.abort() })       // closure
Button({ label: 'Open',    onClick: openUrl(p.url) })           // bare Action shorthand
Button({ label: 'Refresh', onClick: () => this.load() })
Input ({ id: 'q',          onChange: (v) => { this.filter = v; } })
```

A closure's **return value, if an Action, is dispatched after the
closure settles.** For mid-flow dispatching use `dispatch(action)`. The
two compose:

```js
async onClick() {
    dispatch(copy(this.url));         // fire mid-flow
    await this.cleanup();
    return closeView;                 // dispatched after the closure resolves
}
```

If a closure resolves *after* the frame has already been torn down (e.g.
the user closed the launcher mid-await), subsequent `this.x = ...`
writes are silently dropped and logged once per closure at `INFO`.

## Showing and closing a view

`highbeam:actions` exposes two new builders:

```ts
showView(view: ViewDef, props?: object, opts?: { reset?: boolean }): Action
closeView: Action
```

- `showView(view, props)` pushes a new frame on top of the stack.
- `showView(view, props, { reset: true })` clears the stack first, so
  the new frame is the only frame.
- `closeView` (no call — it's a constant) pops the top frame.
  `render → null` does the same thing.

The wire shape:

```rust
Action::ShowView { handle: u64, props: Value, reset: bool }
Action::CloseView
```

The `handle` is opaque to the host. The SDK keeps a per-plugin map
`handle → ViewDef`; when the host bounces back "render handle 42 with
this event", the SDK looks it up. Pushing the same view twice creates
two frames — every push is a new frame, no deduping.

The stack cap is **16 frames**. Pushing a 17th frame without `{ reset:
true }` is rejected: ERROR in `plugin.log`, the action no-ops.

## Lifecycle order

```
1. setup(props)            sync
2. proxy wraps result
3. render()                sync — first paint shipped to host
4. user sees first frame
5. mounted({ signal })     scheduled on next microtask
6. mounted's sync mutations batch → one render
7. mounted's post-await mutations → one render each
... (interactive phase: events → methods → state mutations → renders)
N. frame popped or launcher hidden:
   a. signal aborts
   b. unmounted() runs sync
   c. callback ids freed, proxy released
```

This ordering is **load-bearing**: returning `{loading: true}` from
`setup` paints a spinner before `mounted` even starts its first HTTP
request. That's the difference between perceived snappiness and a blank
frame.

## Keyboard and focus

- **Tab / Shift+Tab** cycles between interactive blocks (`Input`,
  `TextArea`, `Button`, `Row`) in document order.
- **Enter** on a focused `Button` clicks it. On a focused `Input`, fires
  `onSubmit` if set; otherwise no-op.
- **Esc** pops the top frame. Esc on the root launcher view hides the
  launcher (which pops every frame top-down on the way out).
- **Up / Down** inside an `Input` move the cursor. Outside any Input,
  they move focus between `Row`s when a row group is present, otherwise
  they scroll the body.
- **First `Input` auto-focuses** on mount. With no Inputs, focus
  defaults to the first `Button`. With neither, the body takes focus
  for scroll.

### `tabIndex`

Every interactive block accepts an optional `tabIndex?: number`.
Resolution:

- Blocks with an explicit `tabIndex` take that slot.
- Blocks without get auto-assigned in document order, each picking
  `max(explicit_so_far) + 1`.
- A negative `tabIndex` removes the block from the tab cycle entirely
  (it stays clickable / focusable via mouse).

## Errors

**Exceptions are fatal.** An unhandled throw or promise rejection from
`setup`, `mounted`, `unmounted`, `render`, or any method:

1. Pops the offending frame.
2. Pushes a built-in error frame in its place showing the plugin name,
   the error message, and the stack.
3. Logs the full stack to `plugin.log` at ERROR.

Esc pops the error frame like any other.

Authors who want graceful failure handle it themselves:

```js
async load() {
    try {
        const res = await fetch(url);
        this.pipeline = await res.json();
    } catch (e) {
        this.err = e;
    }
}
render() {
    if (this.err) return { body: [
        Heading({ text: 'Failed to load' }),
        Text({ text: String(this.err), tone: 'error' }),
    ]};
    // ...
}
```

Crashing loud beats silently rendering a stale view.

## The view stack

The host owns a frame stack `Vec<Frame>` per launcher session. A frame
carries `{ plugin, handle, props }` plus a per-frame `AbortSignal`.

- **Push:** appends a frame. Re-pushing the same view value creates a
  new frame.
- **Reset push:** pops every frame (running each `unmounted`) and
  pushes one as the only frame.
- **Pop:** removes the top frame, fires its `unmounted`, aborts its
  signal.
- **Launcher hides** (Esc on root, focus loss, action-induced hide):
  pops every frame top-down, then the launcher view. Next launcher open
  starts at the root. **Frame state does not persist across launcher
  hides** — if you need persistence, write to `fs.cache` during
  `unmounted`.
- **Plugin reload:** the host walks the stack, pops every frame owned
  by the reloading plugin, logs one `INFO` per popped frame, skips
  `unmounted` (the QuickJS context is being destroyed). The user lands
  on whatever frame remains.
- **Stack cap:** 16 frames.

## Parent → child communication

Functions are valid in `props`. They cross the host boundary as
callback ids (same mechanism `on*` uses) and reconstitute as callables
on the receiving side:

```js
// Parent view
methods: {
    pickDate() {
        showView(DatePicker, {
            initial: this.date,
            onPick: (date) => { this.date = date; },
        });
    },
},

// DatePicker view
setup: (props) => ({
    value: props.initial,

    confirm() {
        props.onPick(this.value);
        return closeView;
    },
}),
```

The child invokes `props.onPick(date)`; the SDK fires it back into the
parent's frame; the parent's bound method mutates state; the parent
re-renders. No formal "modal return value" plumbing — this fall-out
covers the case.

## Building blocks

All blocks under `highbeam:view`. Each factory returns a plain
`{ kind, ...props }` object — the same shape pattern as
`highbeam:actions`. Render output is fully testable in vitest.

```ts
type ViewNode = { title?: string; body: Block[] };

type Align = 'start' | 'center' | 'end';
type Tone  = 'default' | 'muted' | 'error' | 'success' | 'warning';
type Gap   = 'xs' | 'sm' | 'md' | 'lg';
type Size  = 'sm' | 'md' | 'lg' | 'xl';

type Block =
  | { kind: 'stack';    direction?: 'v' | 'h'; gap?: Gap; align?: Align;
                        children: Block[] }
  | { kind: 'divider' }
  | { kind: 'heading';  text: string; align?: Align }
  | { kind: 'text';     text: string; size?: Size; align?: Align; tone?: Tone }
  | { kind: 'spinner';  label?: string }
  | { kind: 'progress'; value?: number; label?: string }
  | { kind: 'button';   label: string; id?: string; tabIndex?: number;
                        tone?: 'default' | 'primary' | 'danger';
                        onClick?: Closure | Action }
  | { kind: 'input';    id: string; tabIndex?: number;
                        value?: string; placeholder?: string;
                        onChange?: Closure | Action; onSubmit?: Closure | Action }
  | { kind: 'textarea'; id: string; tabIndex?: number; rows?: number;
                        value?: string; placeholder?: string;
                        onChange?: Closure | Action }
  | { kind: 'image';    src: string; fit?: 'contain' | 'cover'; alt?: string }
  | { kind: 'row';      title: string; tabIndex?: number;
                        subtitle?: string; icon?: string;
                        onClick?: Closure | Action };
```

Notes:

- **`Text.size` maps to theme tokens, not pixels.** Theme owns the
  actual size. Same for tones.
- **`ProgressBar.value`** is in `[0, 1]`. Omit it for indeterminate
  ("working, no fixed total"); set it for `N / total` step progress.
- **`Image.src` is `data:` only in v1.** Plugins `fetch` the bytes and
  base64-encode (`Buffer.from(await res.arrayBuffer()).toString('base64')`).
  For images > ~1 MB, cache via `fs.writeCache` or bump `manifest.memoryMb`
  (base64 of a 5 MB JPEG easily blows the default 32 MB cap).
- **`Row` overlaps with `Result` on purpose** so a view can be a
  picker list. Pass `onClick: showView(Detail, { id })` to make a row
  push a detail view.

## Theme

View blocks use the same `theme.toml` token set as the launcher list;
new tokens are added flat (no `[view]` sub-table). See
[theming.md](./theming.md) for the canonical reference. Token additions:

| Token group              | Purpose                                    |
|--------------------------|--------------------------------------------|
| `heading.{size,color}`   | `Heading` text                             |
| `text.{sm,md,lg,xl}.size`| `Text` per-size font size                  |
| `text.tone.{default,muted,error,success,warning}` | `Text` tones |
| `button.{default,primary,danger}.{bg,fg,border}`  | `Button` tones |
| `input.{bg,fg,border,focus.border,placeholder}`   | `Input` / `TextArea` |
| `progress.{track,fill}`  | `ProgressBar`                              |
| `divider.color`          | `Divider`                                  |
| `spinner.color`          | `Spinner`                                  |
| `view.{bg,padding,gap}`  | View frame container                       |
| `error.frame.{bg,fg,border}` | Built-in error frame                   |

## Testing with vitest

`highbeam:view` factories are pure — same pattern as `highbeam:actions`.
Drive a view by hand:

```ts
import { describe, expect, it } from 'vitest';
import { Pipeline } from './plugin.js';

describe('Pipeline view', () => {
    it('renders a spinner before mount completes', () => {
        const state = Pipeline.setup({ id: 1 });
        expect(Pipeline.render.call(state)).toMatchObject({
            body: [{ kind: 'spinner' }],
        });
    });

    it('renders pipeline info once load resolves', async () => {
        const state = Pipeline.setup({ id: 1 });
        state.pipeline = { id: 1, name: 'deploy', status: 'ok', url: '/p/1' };
        const tree = Pipeline.render.call(state);
        expect(tree.title).toBe('Pipeline #1');
        expect(tree.body[0]).toMatchObject({ kind: 'heading', text: 'deploy' });
    });
});
```

The SDK doesn't ship a reactive proxy in the vitest stub — tests
mutate the plain object returned from `setup` directly, then call
`render`. That covers the rendering contract without depending on the
flush scheduler. For end-to-end view behaviour, the host smoke test
(`tests/plugin_smoke.rs`) exercises a `view-demo` plugin in real
rquickjs.

## Memory and lifetime hygiene

- Each render mints fresh callback ids for every `on*` handler. The
  SDK eagerly frees the previous tree's ids before serialising the new
  one — closures don't pile up under spam-clicking.
- Frame state lives in the plugin's QuickJS context. Closing the frame
  releases the proxy and its underlying object; standard JS GC reclaims
  it.
- `props` passed to `showView` are deep-cloned via JSON across the
  boundary, with functions replaced by callback-id placeholders. Don't
  pass non-cloneable values (e.g. `Date` instances, `Map`, circular
  refs); the SDK throws at `showView` time if it can't serialise.

## Deferred to v2

- Computed properties / watchers
- Animations between frames
- Persisting frame state across launcher hides
- First-class modal return values (the closure-prop pattern covers the
  case for v1)
- Custom plugin settings views (separate feature)
- Remote-URL image loading via a `http` capability extension
- View-level inputs that filter the launcher search field directly
