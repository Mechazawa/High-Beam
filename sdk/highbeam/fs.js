// Stub of `highbeam:fs` for vitest. All entries are `vi.fn()`s — fixtures
// belong in the plugin's test file, not here. `readDir` returns an async
// iterable; the default yields zero entries.

import { vi } from 'vitest';

async function* emptyAsyncIterable() {
    // Empty by design — see module header.
}

export const readDir = vi.fn((_path, _opts) => emptyAsyncIterable());
export const readFile = vi.fn(async (_path, _opts) => new Uint8Array());
export const readText = vi.fn(async (_path, _opts) => '');
export const readCache = vi.fn(async (_name) => null);
export const writeCache = vi.fn(async (_name, _data) => {});
