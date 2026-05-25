// Slow Echo — streaming + abort smoke test. Three rows with a 300ms gap.

import { copy } from "highbeam:actions";

export async function* query(input, signal) {
    if (!input) return;

    for (let i = 0; i < 3; i++) {
        // The timeout interrupt would kill us eventually; checking
        // `signal.aborted` is the cooperative path.
        if (signal?.aborted) return;
        await new Promise(r => setTimeout(r, 300));
        if (signal?.aborted) return;

        yield {
            key: `slow-${i}`,
            title: `slow ${i}: ${input}`,
            subtitle: `arrives at +${(i + 1) * 300}ms`,
            action: copy(`${input} (slow ${i})`),
        };
    }
}
