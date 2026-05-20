// `highbeam:system` stub — `vi.fn()`s for per-test override.

import { vi } from 'vitest';

export const exec = vi.fn(async (_cmd, _args, _opts) => ({
    stdout: '',
    stderr: '',
    code: 0,
}));

export const applescript = vi.fn(async (_script, _opts) => null);
