// Shared types referenced from multiple `highbeam:*` modules. Each module
// re-exports the types it needs — this is the single source of truth.

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
    | { kind: 'reveal'; path: string };

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
