// Stub of `highbeam:clipboard` for vitest. Pure side effect in production,
// so the test stub returns `vi.fn()`s plugin authors can spy on and
// override via `vi.mocked(read).mockResolvedValueOnce(...)`.

import { vi } from 'vitest';

export const read = vi.fn(async () => '');
export const write = vi.fn(async (_text) => {});
