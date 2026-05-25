// `highbeam:match` — host-side fuzzy matcher. No capability required.
//
// Backed by `nucleo-matcher` — Smith-Waterman with filename-style bonus
// heuristics. Scores are normalised to [0, 1] so plugins can threshold
// against `score` directly.
//
// `highlights` is an array of `[startByte, endByte)` byte ranges into the
// matched key string — use them to render bolded matches without re-running
// the matcher.

export interface FuzzyOptions<T> {
    /** Extract the haystack string from an item. */
    key: (item: T) => string;
    /** Drop matches with score < threshold. Default 0 (keep all). */
    threshold?: number;
    /** Cap the number of returned matches. */
    limit?: number;
}

export interface Match<T> {
    item: T;
    /** Score in [0, 1]; higher is better. */
    score: number;
    /** `[start, end)` byte ranges into the matched key. */
    highlights: [number, number][];
}

/** Rank `items` by fuzzy match against `query`. */
export function fuzzy<T>(
    items: readonly T[],
    query: string,
    opts: FuzzyOptions<T>,
): Match<T>[];
