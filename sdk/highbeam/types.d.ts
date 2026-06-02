// Shared types referenced from multiple `highbeam:*` modules. Each module
// re-exports the bits it needs — this is the single source of truth.

/**
 * One result row a plugin yields from `query()`.
 *
 * `key` must be stable per (plugin, conceptual result) — it's the frecency
 * key. Mutable fields like the current evaluator should NOT be in it.
 */
export interface Result {
    /** Stable per (plugin, conceptual result) — used as the frecency key. */
    key: string;
    title: string;
    subtitle?: string;
    /** `data:image/...;base64,...` URI. Pre-resolve paths via `highbeam:icons`. */
    icon?: string;
    /** Plugin's self-assessed score, 0..100. */
    weight?: number;
    /** Bypass frecency; sort to the top among other pinned results by weight. */
    pinned?: boolean;
    /** Primary action; invoked on Enter / mouse-click. */
    action: Action;
    /**
     * Alternate action invoked when the user holds the alt-action
     * modifier (configurable in Settings → Global; default Alt) at the
     * moment of Enter / mouse-click. Falls back to `action` when unset,
     * so plugins can opt rows into a secondary verb individually.
     */
    altAction?: Action;
    /**
     * Title shown in place of `title` while the alt-action modifier is
     * held. Surfacing this together with `altAction` is the cheapest way
     * to telegraph "this row will do something different right now".
     */
    altTitle?: string;
    /** Subtitle shown in place of `subtitle` while the alt-action modifier is held. */
    altSubtitle?: string;
}

/** Tagged-union of actions the host knows how to execute. */
export type Action =
    | { kind: 'openUrl'; url: string }
    | { kind: 'copy'; text: string }
    | { kind: 'exec'; cmd: string; args: readonly string[] }
    | { kind: 'reveal'; path: string }
    | { kind: 'showView'; view: ViewDef; props: object; reset: boolean }
    | { kind: 'closeView' }
    /** Inert row — Enter dismisses the launcher without doing anything. */
    | { kind: 'noop' };

/**
 * A view definition — passed to `showView()` from `highbeam:actions`.
 *
 * `setup(props)` returns the working object (data and methods mixed
 * freely; both go through the same reactive proxy). `render()` produces
 * the on-screen tree; returning `null` closes the view. `mounted` runs
 * after the first paint; `unmounted` on close. See `docs/views.md`.
 */
export interface ViewDef<P = object> {
    setup: (props: P) => Record<string, unknown>;
    mounted?: (ctx: { signal: AbortSignal }) => void | Promise<void>;
    unmounted?: () => void;
    render: () => ViewNode | null;
}

/** Top-level shape `render()` returns. `null` from `render()` closes the view. */
export interface ViewNode {
    /** Optional title shown above the body. */
    title?: string;
    /** Body contents, top-to-bottom. */
    body: Block[];
}

/** Layout / content primitive a view renders. Tagged-union by `kind`. */
export type Block =
    | StackBlock
    | DividerBlock
    | HeadingBlock
    | TextBlock
    | SpinnerBlock
    | ProgressBlock
    | ButtonBlock
    | InputBlock
    | TextAreaBlock
    | ImageBlock
    | RowBlock;

export type Align = 'start' | 'center' | 'end';
export type Tone = 'default' | 'muted' | 'error' | 'success' | 'warning';
export type Gap = 'xs' | 'sm' | 'md' | 'lg';
export type TextSize = 'sm' | 'md' | 'lg' | 'xl';
export type ButtonTone = 'default' | 'primary' | 'danger';
export type ImageFit = 'contain' | 'cover';

/** Either a closure (event → re-render) or a bare `Action` (host runs it). */
export type Handler = ((value?: string) => unknown) | Action;

export interface StackBlock {
    kind: 'stack';
    direction?: 'v' | 'h';
    gap?: Gap;
    align?: Align;
    children: Block[];
}

export interface DividerBlock {
    kind: 'divider';
}

export interface HeadingBlock {
    kind: 'heading';
    text: string;
    align?: Align;
}

export interface TextBlock {
    kind: 'text';
    text: string;
    size?: TextSize;
    align?: Align;
    tone?: Tone;
}

export interface SpinnerBlock {
    kind: 'spinner';
    label?: string;
}

export interface ProgressBlock {
    kind: 'progress';
    /** In `[0, 1]`. Omit for indeterminate ("working, no fixed total"). */
    value?: number;
    label?: string;
}

export interface ButtonBlock {
    kind: 'button';
    label: string;
    id?: string;
    tabIndex?: number;
    tone?: ButtonTone;
    onClick?: Handler;
}

export interface InputBlock {
    kind: 'input';
    id: string;
    tabIndex?: number;
    value?: string;
    placeholder?: string;
    onChange?: Handler;
    onSubmit?: Handler;
}

export interface TextAreaBlock {
    kind: 'textarea';
    id: string;
    tabIndex?: number;
    rows?: number;
    value?: string;
    placeholder?: string;
    onChange?: Handler;
}

export interface ImageBlock {
    kind: 'image';
    /** `data:image/...;base64,...` URI. Remote URLs are post-v1. */
    src: string;
    fit?: ImageFit;
    alt?: string;
}

export interface RowBlock {
    kind: 'row';
    title: string;
    subtitle?: string;
    /** `data:image/...;base64,...` URI. */
    icon?: string;
    tabIndex?: number;
    onClick?: Handler;
}

/**
 * Standard Web `AbortSignal` shape. The host hands one to your `query()`
 * function and listens for it across `highbeam:http` calls. You can also
 * construct your own via `new AbortController()` for internal flows.
 */
export interface AbortSignal {
    readonly aborted: boolean;
    readonly reason?: unknown;
    addEventListener(type: 'abort', listener: () => void): void;
    removeEventListener(type: 'abort', listener: () => void): void;
    throwIfAborted(): void;
}

export interface AbortController {
    readonly signal: AbortSignal;
    abort(reason?: unknown): void;
}

/** Shape of the value returned from `http.get` / `http.post`. */
export interface HttpResponse {
    /** HTTP status code, e.g. 200. */
    status: number;
    /** Canonical reason phrase, e.g. `"OK"`. */
    statusText: string;
    /** Response headers, lower-cased keys. */
    headers: Record<string, string>;
    /** Response body as UTF-8 text. Binary is post-v1. */
    body: string;
    /** `status` is in 200..=299. */
    ok: boolean;
    /** Parse body as JSON. Throws on parse failure. */
    json(): unknown;
    /** Alias for `body`. */
    text(): string;
}

/** Options accepted by every `highbeam:http` call. */
export interface HttpOpts {
    headers?: Record<string, string>;
    signal?: AbortSignal;
    /** Per-request override of the default 30 s timeout. */
    timeoutMs?: number;
}

/** Shape `query()` must return: an async iterable of results. */
export type QueryFn = (
    input: string,
    signal: AbortSignal,
) => AsyncIterable<Result>;

/**
 * Shape of `manifest.json`. Required: `name`. Everything else is optional —
 * the host applies defaults at load time.
 *
 * `archiveUrl` + `manifestUrl` opt a plugin in to install-by-URL + update
 * checks; without them the plugin is local-only. See
 * docs/plugin-authoring.md for publication guidance.
 */
export interface Manifest {
    name: string;
    displayName?: string;
    version?: string;
    description?: string;
    entry?: string;
    debounceMs?: number;
    timeoutMs?: number;
    memoryMb?: number;
    capabilities?: readonly string[];
    platforms?: readonly ('macos' | 'linux')[];
    options?: readonly OptionDef[];
    /** Download URL for the plugin archive (`.tar.gz`, `.tgz`, `.tar`, `.zip`). */
    archiveUrl?: string;
    /** Canonical URL hosting *this* manifest. `update` re-fetches it. */
    manifestUrl?: string;
}

export type OptionDef =
    | { key: string; type: 'string'; label?: string; default?: string }
    | { key: string; type: 'bool'; label?: string; default?: boolean }
    | { key: string; type: 'int'; label?: string; default?: number; min?: number; max?: number }
    | { key: string; type: 'enum'; label?: string; default?: string; choices: readonly string[] };
