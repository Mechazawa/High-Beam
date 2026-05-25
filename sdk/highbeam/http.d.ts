// `highbeam:http` — async HTTP client. Requires the `http` capability.
//
// Built on a shared reqwest client. Default timeout is 30 seconds; override
// per-request via `opts.timeoutMs`. Body decoding is UTF-8 only for v1.
//
// Cancellation: pass an `AbortSignal` via `opts.signal`. The plugin's
// `query(input, signal)` signal works here — abort cascades from the host's
// "new keystroke" event right into the in-flight request future.
//
// `delete` is a JS reserved word. Import with a rename or via the
// namespace form:
//   `import { delete as del } from 'highbeam:http';`
//   `import * as http from 'highbeam:http'; http.delete(url);`

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

/** PUT request. Same body coercion as POST. */
export function put(
    url: string,
    body?: string | object,
    opts?: HttpOpts,
): Promise<HttpResponse>;

/** PATCH request. Same body coercion as POST. */
export function patch(
    url: string,
    body?: string | object,
    opts?: HttpOpts,
): Promise<HttpResponse>;

/**
 * DELETE request. `body` is optional but allowed — some APIs accept a
 * payload with DELETE. Same coercion as POST when provided.
 */
declare function deleteRequest(
    url: string,
    body?: string | object,
    opts?: HttpOpts,
): Promise<HttpResponse>;
export { deleteRequest as delete };
