// TypeScript variant of the echo plugin. Build with `npm run build` — the
// host loads the sibling `plugin.js` (the compiler's output).
import { copy } from 'highbeam:actions';
export async function* query(input, _signal) {
    if (!input)
        return;
    yield {
        key: 'echo-ts',
        title: `echo (ts): ${input}`,
        subtitle: 'press Enter to copy',
        action: copy(input),
    };
}
