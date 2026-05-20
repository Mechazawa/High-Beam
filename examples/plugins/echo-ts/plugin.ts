// TypeScript variant of the echo plugin. Build with `tsc` — the host loads
// the sibling `plugin.js` (the compiler's output).

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
