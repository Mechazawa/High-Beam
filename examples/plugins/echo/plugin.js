// Echo plugin — Stage 3 smoke-test fixture.
//
// Yields one result per keystroke; pressing Enter copies the current input
// to the system clipboard via the `copy` action. Useful for confirming that:
//   * the plugin loader picks up `manifest.json` + `plugin.js`
//   * the `highbeam:actions` import resolves and the `copy` builder works
//   * the host renders result rows, highlights row 0, and routes Enter back
//     into the action executor.
//
// Anything more interesting waits for Stage 4 (http, fs, debounced
// generators) and Stage 6 (real ported plugins).

import { copy } from "highbeam:actions";

export async function* query(input, _signal) {
    // Skip empty input — no point echoing nothing.
    if (!input) return;
    yield {
        key: "echo",
        title: `echo: ${input}`,
        subtitle: "press Enter to copy to clipboard",
        action: copy(input),
    };
}
