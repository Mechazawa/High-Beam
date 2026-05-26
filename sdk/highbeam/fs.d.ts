// `highbeam:fs` — read files and walk directories; cap-gated by `fs.read`.
// The cache helpers are scoped to the plugin's own cache directory and are
// cap-gated by `fs.cache`. Plugins cannot see each other's cache files —
// the directory is determined by the plugin's manifest `name`.

/** One entry yielded by `readDir`. */
export interface DirEntry {
    /** Filename only (no leading path). */
    name: string;
    /** Absolute path to the entry. */
    path: string;
    isFile: boolean;
    isDir: boolean;
}

/** Options accepted by `readDir`. */
export interface ReadDirOptions {
    /** Walk into subdirectories. Default: false. */
    recursive?: boolean;
    /** Abort the walk mid-iteration. */
    signal?: AbortSignal;
}

/** Common options for the file readers. */
export interface ReadFileOptions {
    signal?: AbortSignal;
}

/**
 * Walk a directory, yielding one [`DirEntry`] per entry. With
 * `{ recursive: true }` it also yields entries in nested subdirectories
 * (depth-first; order within a directory is OS-dependent / unspecified).
 *
 * Cap: `fs.read`.
 */
export function readDir(
    path: string,
    opts?: ReadDirOptions,
): AsyncIterable<DirEntry>;

/** Read a file as a Uint8Array. Cap: `fs.read`. */
export function readFile(
    path: string,
    opts?: ReadFileOptions,
): Promise<Uint8Array>;

/** Read a file as a UTF-8 string. Cap: `fs.read`. */
export function readText(
    path: string,
    opts?: ReadFileOptions,
): Promise<string>;

/**
 * Read a previously-cached blob by name. Returns `null` if the entry doesn't
 * exist. The name is scoped to the plugin's own cache dir — path separators
 * and traversal patterns are rejected.
 *
 * Cap: `fs.cache`.
 */
export function readCache(name: string): Promise<Uint8Array | null>;

/**
 * Write a blob to the plugin's cache by name. Creates the cache directory if
 * missing. The name must be a single path component (no slashes, no `..`).
 *
 * Cap: `fs.cache`.
 */
export function writeCache(
    name: string,
    data: Uint8Array | string,
): Promise<void>;
