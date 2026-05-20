// Echo (TypeScript) — same behaviour as the JS `echo` plugin, but
// written in TypeScript against the `@high-beam/sdk` ambient types.
//
// Build with `tsc` (the host doesn't compile TypeScript itself; that's the
// plugin author's responsibility). The `plugin.js` next to this file is
// the compiled output the host actually loads.

import { copy } from 'highbeam:actions';
import type { AbortSignal, Result } from 'highbeam:actions';

export async function* query(
    input: string,
    _signal: AbortSignal,
): AsyncIterable<Result> {
    if (!input) return;
    yield {
        key: 'echo-ts',
        title: `echo (ts): ${input}`,
        subtitle: 'press Enter to copy',
        action: copy(input),
    };
}
