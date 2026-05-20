// Frecency demo — yields three fixed rows at equal weight so picks bubble
// each one to the top as you choose it.

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
