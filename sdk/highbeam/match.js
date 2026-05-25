// `highbeam:match` stub. Simple case-insensitive subsequence matcher with
// prefix / consecutive-run bonuses — scores aren't bit-for-bit identical to
// nucleo's, but the ordering and highlight ranges agree on realistic input.

function matchOne(haystack, query) {
    if (query.length === 0) {
        return { score: 1, highlights: [] };
    }

    const hayLower = haystack.toLowerCase();
    const qLower = query.toLowerCase();
    const highlights = [];
    let hi = 0;
    let qi = 0;
    let runStart = -1;
    let runs = 0;
    let consecutive = 0;
    let maxConsecutive = 0;
    let startedAtZero = false;

    while (hi < hayLower.length && qi < qLower.length) {
        if (hayLower[hi] === qLower[qi]) {
            if (runStart === -1) {
                runStart = hi;
                if (hi === 0) startedAtZero = true;
                runs += 1;
            }
            consecutive += 1;
            if (consecutive > maxConsecutive) maxConsecutive = consecutive;
            qi += 1;
            hi += 1;
        } else {
            if (runStart !== -1) {
                highlights.push([runStart, hi]);
                runStart = -1;
            }
            consecutive = 0;
            hi += 1;
        }
    }
    if (qi < qLower.length) {
        return null;
    }

    if (runStart !== -1) {
        highlights.push([runStart, hi]);
    }

    const coverage = qLower.length / Math.max(hayLower.length, 1);
    const prefixBonus = startedAtZero ? 0.15 : 0;
    const runBonus = Math.min(0.25, (maxConsecutive - 1) * 0.05);
    const fragmentationPenalty = Math.min(0.2, (runs - 1) * 0.05);
    const score = Math.max(
        0,
        Math.min(1, coverage * 0.7 + prefixBonus + runBonus - fragmentationPenalty + 0.05),
    );

    return { score, highlights };
}

export function fuzzy(items, query, opts) {
    const key = opts?.key;

    if (typeof key !== 'function') {
        throw new TypeError('match.fuzzy: opts.key must be a function');
    }

    const threshold = opts?.threshold ?? 0;
    const limit = opts?.limit;
    const results = [];

    for (const item of items) {
        const haystack = key(item);
        const m = matchOne(haystack, query);

        if (m === null) continue;
        if (m.score < threshold) continue;

        results.push({ item, score: m.score, highlights: m.highlights });
    }

    results.sort((a, b) => b.score - a.score);

    if (typeof limit === 'number') {
        results.length = Math.min(results.length, limit);
    }

    return results;
}
