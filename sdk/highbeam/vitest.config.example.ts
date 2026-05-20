// Maps `highbeam:foo` imports onto the stub `.js` files so plugin tests can
// run in plain Node. Copy into your plugin and adjust the `replacement`
// path, or re-export this file directly when the SDK is at a known path.

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
