// `highbeam:settings` vitest stub. Plugins usually want to mock these via
// `vi.fn()` per-test rather than wire up a real options bag, so the default
// implementations all return undefined.
//
// Example mock in a test:
//   import * as settings from 'highbeam:settings';
//   vi.spyOn(settings, 'getString').mockReturnValue('alice');

import { vi } from 'vitest';

export const get = vi.fn((_key) => undefined);
export const getString = vi.fn((_key) => undefined);
export const getBool = vi.fn((_key) => undefined);
export const getInt = vi.fn((_key) => undefined);
