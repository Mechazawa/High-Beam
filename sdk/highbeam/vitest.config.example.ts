// Vitest config that maps `highbeam:foo` imports onto the stub `.js` files
// in this directory, so plugin tests can run in plain Node.
//
// Plugin authors: either copy this file into your plugin and adjust the
// `replacement` path, or — if the SDK lives at a fixed relative location —
// re-export it directly:
//
//     import config from '<relpath>/sdk/highbeam/vitest.config.example';
//     export default config;

import { defineConfig } from 'vitest/config';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
    resolve: {
        alias: [
            {
                find: /^highbeam:(.*)$/,
                replacement: path.resolve(here, './$1.js'),
            },
        ],
    },
    test: {
        globals: true,
    },
});
