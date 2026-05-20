// Stub of `highbeam:http` for vitest. Default returns a 200/OK empty body;
// plugin tests override per-call with `vi.mocked(get).mockResolvedValueOnce`.

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
