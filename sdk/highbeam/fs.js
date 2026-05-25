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
