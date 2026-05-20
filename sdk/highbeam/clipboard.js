// `highbeam:clipboard` stub for vitest. `vi.fn()`s so tests can spy /
// override via `vi.mocked(read).mockResolvedValueOnce(...)`.

import { vi } from 'vitest';

export const read = vi.fn(async () => '');
export const write = vi.fn(async (_text) => {});
