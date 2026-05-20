// Starter vitest config for plugin authors. Copy this into your plugin
// directory and adjust the `replacement` path to point at the SDK stub
// package — relative to the plugin dir.
//
// The alias maps `highbeam:foo` imports (which the production host resolves
// natively) onto the stub `.js` files under `sdk/highbeam/` so vitest can
// run plugin tests in plain Node.

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
