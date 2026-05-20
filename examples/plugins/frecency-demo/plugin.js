// Frecency demo plugin — Stage 5 visual smoke test.
//
// Yields three fixed rows regardless of input. All three have the same
// `weight`, so without frecency they sort by insertion order:
//
//   Alpha, Beta, Gamma
//
// After picking Gamma once, the next query should show Gamma first
// (1 fresh pick ≈ 1.10× modifier, the others stay at 1.0× ⇒ 55 vs 50).
//
// Picking Beta after that should reshuffle to Beta, Gamma, Alpha —
// most-recent pick wins the tie at equal pick count thanks to the
// decay term shaving a tiny bit off Gamma's modifier between bumps.

import { copy } from "highbeam:actions";

export async function* query(_input, _signal) {
    yield {
        key: "alpha",
        title: "Alpha",
        subtitle: "frecency-demo: equal weight, distinct key",
        weight: 50,
        action: copy("alpha"),
    };
    yield {
        key: "beta",
        title: "Beta",
        subtitle: "frecency-demo: equal weight, distinct key",
        weight: 50,
        action: copy("beta"),
    };
    yield {
        key: "gamma",
        title: "Gamma",
        subtitle: "frecency-demo: equal weight, distinct key",
        weight: 50,
        action: copy("gamma"),
    };
}
