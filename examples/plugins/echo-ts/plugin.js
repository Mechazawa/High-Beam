// Compiled output of plugin.ts. Hand-written so the example works without
// requiring a tsc invocation; in practice plugin authors would run
// `tsc` and ship the compiler output here.

import { copy } from "highbeam:actions";

export async function* query(input, _signal) {
    if (!input) return;
    yield {
        key: "echo-ts",
        title: `echo (ts): ${input}`,
        subtitle: "press Enter to copy",
        action: copy(input),
    };
}
