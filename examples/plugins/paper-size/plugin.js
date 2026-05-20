import { copy } from "highbeam:actions";

// Inlined from legacy/plugins/paper-sizes.json. Small + stable, so reading it
// at runtime via fs would be wasted overhead and a needless capability ask.
const PAPER_SIZES = {
    A0: { mm: "841 x 1189" },
    A1: { mm: "594 x 841" },
    A2: { mm: "420 x 594" },
    A3: { mm: "297 x 420" },
    A4: { mm: "210 x 297" },
    A5: { mm: "148 x 210" },
    A6: { mm: "105 x 148" },
    A7: { mm: "74 x 105" },
    A8: { mm: "52 x 74" },
    A9: { mm: "37 x 52" },
    A10: { mm: "26 x 37" },
    B0: { mm: "1000 x 1414" },
    B1: { mm: "707 x 1000" },
    B2: { mm: "500 x 707" },
    B3: { mm: "353 x 500" },
    B4: { mm: "250 x 353" },
    B5: { mm: "176 x 250" },
    B6: { mm: "125 x 176" },
    B7: { mm: "88 x 125" },
    B8: { mm: "62 x 88" },
    B9: { mm: "44 x 62" },
    B10: { mm: "31 x 44" },
    C0: { mm: "917 x 1297" },
    C1: { mm: "648 x 917" },
    C2: { mm: "458 x 648" },
    C3: { mm: "324 x 458" },
    C4: { mm: "229 x 324" },
    C5: { mm: "162 x 229" },
    C6: { mm: "114 x 162" },
    C7: { mm: "81 x 114" },
    C8: { mm: "57 x 81" },
    C9: { mm: "40 x 57" },
    C10: { mm: "28 x 40" },
    Letter: { mm: "215.9 x 279.4" },
    "Government-Letter": { mm: "203.2 x 226.7" },
    Legal: { mm: "215.9 x 355.6" },
    "Junior Legal": { mm: "203.2 x 127" },
    Ledger: { mm: "432 x 279" },
    Tabloid: { mm: "279 x 432" },
};

const PAPER_PREFIX = /^\s*paper\s+/i;

export async function* query(input, _signal) {
    if (!input) return;

    // Strip optional `paper ` prefix so `paper A4` behaves like `A4`.
    const raw = input.replace(PAPER_PREFIX, "").trim();
    if (!raw) return;

    const needle = raw.toLowerCase();

    for (const [name, info] of Object.entries(PAPER_SIZES)) {
        if (!name.toLowerCase().includes(needle)) continue;
        yield {
            key: name,
            title: name,
            subtitle: `${info.mm} mm`,
            weight: 100 * (needle.length / name.length),
            action: copy(info.mm),
        };
    }
}
