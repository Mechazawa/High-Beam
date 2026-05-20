// Slow Echo — Stage 4 streaming + abort smoke-test plugin.
//
// Yields three result rows with a 300ms gap between each. Useful for
// confirming:
//   * progressive rendering: row 1 appears, then 2, then 3, with visible
//     gaps in between — not all-at-once.
//   * abort cascade: type, wait until the first row lands, then type again.
//     The in-flight stream should terminate quickly (the `signal.aborted`
//     check at the top of every iteration bails out).
//
// Each result copies a slightly different string when invoked, so you can
// also confirm that Enter on the highlighted row runs the correct action.

import { copy } from "highbeam:actions";

export async function* query(input, signal) {
    if (!input) return;
    for (let i = 0; i < 3; i++) {
        // Bail early if the host has cancelled. Plugins that don't check
        // this still get killed by the timeout interrupt; but checking is
        // the polite thing to do.
        if (signal && signal.aborted) return;
        await new Promise(r => setTimeout(r, 300));
        if (signal && signal.aborted) return;
        yield {
            key: `slow-${i}`,
            title: `slow ${i}: ${input}`,
            subtitle: `arrives at +${(i + 1) * 300}ms`,
            action: copy(`${input} (slow ${i})`),
        };
    }
}
