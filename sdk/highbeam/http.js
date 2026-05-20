// `highbeam:http` stub for vitest. Default returns 200/OK empty body —
// override per call via `vi.mocked(get).mockResolvedValueOnce(...)`.

import { vi } from 'vitest';

function emptyResponse() {
    return {
        status: 200,
        statusText: 'OK',
        headers: {},
        body: '',
        ok: true,
        json() {
            return undefined;
        },
        text() {
            return '';
        },
    };
}

export const get = vi.fn(async (_url, _opts) => emptyResponse());
export const post = vi.fn(async (_url, _body, _opts) => emptyResponse());
