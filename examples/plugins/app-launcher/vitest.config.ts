import { defineConfig } from 'vitest/config';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const sdkDir = path.resolve(here, '../../../sdk/highbeam');

export default defineConfig({
    resolve: {
        alias: [
            {
                find: /^highbeam:(.*)$/,
                replacement: path.join(sdkDir, '$1.js'),
            },
        ],
    },
    test: {
        globals: true,
    },
});
