// Compiled output of plugin.ts (hand-written so the example works without
// running `tsc`). Plugin authors normally compile via `tsc` and ship that.

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
