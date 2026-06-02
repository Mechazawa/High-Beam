// `highbeam:fs` stub for vitest. `vi.fn()`s — plugin tests supply their
// own fixtures. `readDir` returns an empty async iterable by default.

import { vi } from 'vitest';

async function* emptyAsyncIterable() {
}

export const readDir = vi.fn((_path, _opts) => emptyAsyncIterable());
export const readFile = vi.fn(async (_path, _opts) => new Uint8Array());
export const readText = vi.fn(async (_path, _opts) => '');
export const readCache = vi.fn(async (_name) => null);
export const writeCache = vi.fn(async (_name, _data) => {});

// Pure helper — real impl, mirroring the host's semantics.
export const basename = (path) => {
    const trimmed = String(path).replace(/\/+$/, '');
    const idx = trimmed.lastIndexOf('/');
    return idx < 0 ? trimmed : trimmed.slice(idx + 1);
};
