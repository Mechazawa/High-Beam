// `highbeam:http` — async HTTP client. Requires the `http` capability.
//
// Built on a shared reqwest client. Default timeout is 30 seconds; override
// per-request via `opts.timeoutMs`. Body decoding is UTF-8 only for v1.
//
// Cancellation: pass an `AbortSignal` via `opts.signal`. The plugin's
// `query(input, signal)` signal works here — abort cascades from the host's
// "new keystroke" event right into the in-flight request future.

import type { HttpOpts, HttpResponse } from './types';

/** GET request. Resolves to an HttpResponse regardless of status. */
export function get(url: string, opts?: HttpOpts): Promise<HttpResponse>;

/**
 * POST request. `body` can be a string (sent verbatim) or an object
 * (JSON-stringified with `Content-Type: application/json`).
 */
export function post(
    url: string,
    body?: string | object,
    opts?: HttpOpts,
): Promise<HttpResponse>;
