// `highbeam:icons` stub. Default is a 1×1 transparent PNG matching the Rust
// host's Linux fallback so snapshots stay stable cross-platform.

import { vi } from 'vitest';

const TRANSPARENT_PNG =
    'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=';

export const forPath = vi.fn(async (_path, _opts) => TRANSPARENT_PNG);
