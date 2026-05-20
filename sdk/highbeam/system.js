// Stub of `highbeam:system` for vitest. Side-effectful in production —
// `vi.fn()` so plugin authors override per-test.

import { vi } from 'vitest';

export const exec = vi.fn(async (_cmd, _args, _opts) => ({
    stdout: '',
    stderr: '',
    code: 0,
}));

export const applescript = vi.fn(async (_script, _opts) => null);
