// `highbeam:icons` — native icon resolution. Cap: `icons`.
//
// Returns a `data:image/png;base64,...` URI suitable for direct use in an
// `<img src>` or as a result row's icon. Cached in-process keyed by
// `(path, size)` so repeated lookups during one query stay cheap.
//
// macOS: extracted from the bundle's `CFBundleIconFile` via `sips`. Slow on
// the first call (~50ms), instant after.
//
// Linux: best-effort — returns a transparent fallback rather than throwing.

export interface IconOptions {
    /** Pixel size of the longest edge. Default 128. */
    size?: number;
}

/** Resolve a data-URI icon for the given filesystem path. */
export function forPath(path: string, opts?: IconOptions): Promise<string>;
