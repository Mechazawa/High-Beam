// xkcd comic lookup. Triggered by `xkcd <arg>`:
//   - `xkcd latest`     → newest comic
//   - `xkcd random`     → uniformly random comic from 1..latest
//   - `xkcd <number>`   → that comic by number (404 → no results)
//   - `xkcd <text>`     → fuzzy title search against a cached index
//
// The title index is cached to `fs.cache` as `xkcd-index.json`. The
// `onEnable` hook rebuilds the full archive on install / update; in the
// meantime the first text search bootstraps the latest 500 comics (~10s)
// and subsequent loads refresh incrementally.

import { openUrl } from "highbeam:actions";
import { get } from "highbeam:http";
import { readCache, writeCache } from "highbeam:fs";
import { fuzzy } from "highbeam:match";

const TRIGGER = /^xkcd(?:\s+(.+))?$/i;
const LATEST_URL = "https://xkcd.com/info.0.json";
const COMIC_URL = (n) => `https://xkcd.com/${n}/info.0.json`;
const COMIC_PAGE = (n) => `https://xkcd.com/${n}/`;
const EXPLAIN_PAGE = (n) => `https://www.explainxkcd.com/wiki/index.php/${n}`;

const CACHE_NAME = "xkcd-index.json";
// 24h freshness window. New comics land Mon/Wed/Fri so a daily refresh is
// plenty without hammering the CDN.
const CACHE_TTL_MS = 24 * 60 * 60 * 1000;
// Pragmatic cap: only index the latest N comics. xkcd's CDN is generous but
// we shouldn't fan out 3000+ requests just to find a title.
const INDEX_SIZE = 500;
// Be polite — at most this many concurrent fetches when populating the index.
const INDEX_CONCURRENCY = 50;
// Comic #404 famously 404s. Don't treat it as an error during indexing.
const KNOWN_MISSING = new Set([404]);

let cachedIndex = null;

async function fetchLatest(signal) {
    const res = await get(LATEST_URL, { signal });
    if (!res.ok) {
        throw new Error(`xkcd latest fetched HTTP ${res.status}`);
    }
    return res.json();
}

async function fetchComic(num, signal) {
    const res = await get(COMIC_URL(num), { signal });
    if (res.status === 404) return null;
    if (!res.ok) {
        throw new Error(`xkcd ${num} fetched HTTP ${res.status}`);
    }
    return res.json();
}

function resultFor(comic) {
    return {
        key: `xkcd-${comic.num}`,
        title: `${comic.num}: ${comic.title}`,
        subtitle: comic.alt ?? "",
        weight: 80,
        pinned: false,
        action: openUrl(COMIC_PAGE(comic.num)),
        // Alt opens the explainxkcd page — separate plugin would be heavier
        // than just wiring the secondary verb here.
        altAction: openUrl(EXPLAIN_PAGE(comic.num)),
        altSubtitle: "→ explainxkcd.com",
    };
}

async function readCachedIndex() {
    if (cachedIndex) return cachedIndex;
    const raw = await readCache(CACHE_NAME);
    if (!raw) return null;
    try {
        const text = typeof raw === "string"
            ? raw
            : new TextDecoder().decode(raw);
        const parsed = JSON.parse(text);
        if (!parsed || !Array.isArray(parsed.comics)) return null;
        cachedIndex = parsed;
        return parsed;
    } catch {
        return null;
    }
}

function indexIsFresh(index) {
    if (!index || typeof index.last_updated !== "number") return false;
    return Date.now() - index.last_updated < CACHE_TTL_MS;
}

// Refresh the title index. Three modes:
//   - bootstrap   (no cache yet): fetch the latest INDEX_SIZE comics
//   - incremental (cache exists): fetch only comics newer than the
//                                 highest `num` already cached — usually
//                                 zero to a handful per day
//   - full        (`xkcd index`): fetch every comic from #1 to latest,
//                                 ignoring the existing cache
// Returns the index object that was persisted.
async function buildIndex(signal, { full = false } = {}) {
    const latest = await fetchLatest(signal);
    const max = latest.num;
    const existing = full ? null : await readCachedIndex();
    const existingMap = new Map(
        (existing?.comics ?? []).map((c) => [c.num, c]),
    );
    const haveLast = existing?.comics?.length
        ? existing.comics[existing.comics.length - 1].num
        : 0;

    let start;
    if (full) {
        start = 1;
    } else if (haveLast > 0) {
        start = haveLast + 1;
    } else {
        start = Math.max(1, max - INDEX_SIZE + 1);
    }

    if (start > max && existing) {
        // Already up-to-date — bump last_updated so the TTL resets and we
        // don't re-check the latest endpoint on every keystroke.
        const refreshed = { ...existing, last_updated: Date.now() };
        try {
            await writeCache(CACHE_NAME, JSON.stringify(refreshed));
        } catch {
            // Non-fatal — see below.
        }
        cachedIndex = refreshed;
        return refreshed;
    }

    const nums = [];
    for (let n = start; n <= max; n++) {
        if (!KNOWN_MISSING.has(n)) nums.push(n);
    }

    const comics = full ? [] : Array.from(existingMap.values());
    // Always include the latest comic from the metadata we already fetched —
    // saves one round-trip and is the most likely target of a fresh search.
    if (!existingMap.has(latest.num)) {
        comics.push({
            num: latest.num,
            title: latest.title,
            alt: latest.alt ?? "",
        });
    }
    const skip = new Set([latest.num, ...existingMap.keys()]);

    let cursor = 0;
    async function worker() {
        while (cursor < nums.length) {
            const i = cursor++;
            const n = nums[i];
            if (skip.has(n)) continue;
            if (signal?.aborted) return;
            try {
                const comic = await fetchComic(n, signal);
                if (comic) {
                    comics.push({
                        num: comic.num,
                        title: comic.title,
                        alt: comic.alt ?? "",
                    });
                }
            } catch {
                // Single-comic fetch failures don't block index building —
                // we just drop the entry and continue.
            }
        }
    }

    const workers = [];
    const concurrency = Math.min(INDEX_CONCURRENCY, nums.length);
    for (let i = 0; i < concurrency; i++) workers.push(worker());
    await Promise.all(workers);

    comics.sort((a, b) => a.num - b.num);
    const index = { last_updated: Date.now(), comics };
    try {
        await writeCache(CACHE_NAME, JSON.stringify(index));
    } catch {
        // Cache write failures aren't fatal — we still return a usable index
        // for this session.
    }
    cachedIndex = index;
    return index;
}

async function getIndex(signal) {
    const existing = await readCachedIndex();
    if (existing && indexIsFresh(existing)) return existing;
    return buildIndex(signal);
}

export async function* query(input, signal) {
    const match = TRIGGER.exec(input);
    if (!match) return;
    const arg = (match[1] ?? "").trim();
    if (!arg) return;

    if (/^latest$/i.test(arg)) {
        const comic = await fetchLatest(signal);
        yield resultFor(comic);
        return;
    }

    if (/^random$/i.test(arg)) {
        const latest = await fetchLatest(signal);
        const n = 1 + Math.floor(Math.random() * latest.num);
        const comic = n === latest.num ? latest : await fetchComic(n, signal);
        if (comic) yield resultFor(comic);
        return;
    }

    if (/^\d+$/.test(arg)) {
        const n = Number(arg);
        if (!Number.isFinite(n) || n < 1) return;
        const comic = await fetchComic(n, signal);
        if (comic) yield resultFor(comic);
        return;
    }

    const index = await getIndex(signal);
    if (signal?.aborted) return;
    const ranked = fuzzy(index.comics, arg, {
        key: (c) => c.title,
        threshold: 0.05,
        limit: 10,
    });
    for (const { item } of ranked) {
        yield {
            key: `xkcd-${item.num}`,
            title: `${item.num}: ${item.title}`,
            subtitle: item.alt ?? "",
            weight: 80,
            pinned: false,
            action: openUrl(COMIC_PAGE(item.num)),
            altAction: openUrl(EXPLAIN_PAGE(item.num)),
            altSubtitle: "→ explainxkcd.com",
        };
    }
}

// Build the full title index whenever the plugin lights up — first install
// or after an update. The hook runs in the background and aborts if the
// daemon tears the runtime down; queries fall back to the on-demand bootstrap
// while the rebuild is in flight.
export async function onEnable(_reason, signal) {
    await buildIndex(signal, { full: true });
}
