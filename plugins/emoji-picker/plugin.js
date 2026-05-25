// Emoji picker. Keyword-gated (configurable trigger, default `emoji`) so the
// 1.8k-row dataset isn't fuzzy-ranked on every keystroke. Enter copies the
// character.

import { copy } from "highbeam:actions";
import { fuzzy } from "highbeam:match";
import { readText } from "highbeam:fs";
import { getString, getBool } from "highbeam:settings";

const DATA_PATH = "./emoji-data.json";
const MAX_RESULTS = 9;
const DEFAULT_TRIGGER = "emoji";

// Field weights: an exact-ish name hit should always beat a tag-only hit.
const NAME_WEIGHT = 1.0;
const ALIAS_WEIGHT = 0.85;
const TAG_WEIGHT = 0.7;
const MATCH_THRESHOLD = 0.1;

// Fitzpatrick skin-tone modifiers (U+1F3FB..U+1F3FF) and a label per code.
const SKIN_TONES = [
    { mod: "\u{1F3FB}", label: "light" },
    { mod: "\u{1F3FC}", label: "medium-light" },
    { mod: "\u{1F3FD}", label: "medium" },
    { mod: "\u{1F3FE}", label: "medium-dark" },
    { mod: "\u{1F3FF}", label: "dark" },
];

let emojiPromise = null;

function loadEmoji() {
    if (!emojiPromise) {
        emojiPromise = readText(DATA_PATH).then((text) => JSON.parse(text));
    }
    return emojiPromise;
}

function buildTrigger() {
    const raw = (getString("trigger") ?? DEFAULT_TRIGGER).trim();
    const keyword = raw.length === 0 ? DEFAULT_TRIGGER : raw;
    // Escape regex metacharacters so a user-supplied keyword like `e:` works.
    const escaped = keyword.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return new RegExp(`^\\s*${escaped}(?:\\s+(.*))?$`, "i");
}

function subtitleFor(item) {
    const parts = [];
    const aliases = item.a ?? [];
    const tags = item.t ?? [];
    if (aliases.length) parts.push(`:${aliases.join(": :")}:`);
    if (tags.length) parts.push(tags.join(", "));
    return parts.join(" - ");
}

// Run fuzzy against a single string field per item, returning a map of
// item-index -> best score for that field.
function scoreField(items, q, project, threshold) {
    // Materialise as { idx, hay } so we can map back to the original item.
    const haystack = items
        .map((item, i) => ({ idx: i, hay: project(item) }))
        .filter(({ hay }) => hay);
    const ranked = fuzzy(haystack, q, {
        key: (h) => h.hay,
        threshold,
    });

    const out = new Map();
    for (const { item, score } of ranked) {
        const prev = out.get(item.idx);
        if (prev === undefined || score > prev) out.set(item.idx, score);
    }

    return out;
}

function bestAliasHay(item) {
    const aliases = item.a ?? [];
    if (aliases.length === 0) return "";
    return aliases.join(" ");
}

function bestTagHay(item) {
    const tags = item.t ?? [];
    if (tags.length === 0) return "";
    return tags.join(" ");
}

function rank(items, q) {
    const nameScores = scoreField(items, q, (i) => i.n, MATCH_THRESHOLD);
    const aliasScores = scoreField(items, q, bestAliasHay, MATCH_THRESHOLD);
    const tagScores = scoreField(items, q, bestTagHay, MATCH_THRESHOLD);

    const seen = new Set([
        ...nameScores.keys(),
        ...aliasScores.keys(),
        ...tagScores.keys(),
    ]);

    const ranked = [...seen]
        .map((idx) => {
            const nScore = (nameScores.get(idx) ?? 0) * NAME_WEIGHT;
            const aScore = (aliasScores.get(idx) ?? 0) * ALIAS_WEIGHT;
            const tScore = (tagScores.get(idx) ?? 0) * TAG_WEIGHT;
            return { item: items[idx], score: Math.max(nScore, aScore, tScore) };
        })
        .filter(({ score }) => score > 0);

    ranked.sort((a, b) => b.score - a.score);
    return ranked;
}

export async function* query(input, _signal) {
    const trigger = buildTrigger();
    const match = trigger.exec(input);
    if (!match) return;

    const q = (match[1] ?? "").trim();
    if (!q) return;

    const emoji = await loadEmoji();
    const ranked = rank(emoji, q);
    const includeSkin = getBool("skin_tones") === true;

    let yielded = 0;
    for (const { item, score } of ranked) {
        if (yielded >= MAX_RESULTS) return;

        const subtitle = subtitleFor(item);
        const baseKey = item.a?.[0] || item.n;
        yield {
            key: `emoji:${baseKey}`,
            title: `${item.c}  ${item.n}`,
            subtitle: subtitle || undefined,
            weight: Math.round(score * 100),
            action: copy(item.c),
        };
        yielded += 1;

        if (!includeSkin || !item.s) continue;
        // Expand into the five Fitzpatrick variants. Each gets a fractionally
        // lower weight so the base form stays on top.
        for (const [i, { mod, label }] of SKIN_TONES.entries()) {
            if (yielded >= MAX_RESULTS) return;

            const variant = item.c + mod;
            yield {
                key: `emoji:${baseKey}:${label}`,
                title: `${variant}  ${item.n} (${label} skin)`,
                subtitle: subtitle || undefined,
                weight: Math.max(0, Math.round(score * 100) - (i + 1)),
                action: copy(variant),
            };
            yielded += 1;
        }
    }
}
