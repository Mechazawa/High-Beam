// Stub of `highbeam:icons` for vitest. Default is a 1x1 transparent PNG
// data URI — matches the Linux fallback from the Rust host so plugin
// snapshots stay stable across platforms.

import { vi } from 'vitest';

const TRANSPARENT_PNG =
    'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=';

export const forPath = vi.fn(async (_path, _opts) => TRANSPARENT_PNG);
