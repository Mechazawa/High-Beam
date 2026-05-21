// Clipboard history. On every keystroke the host invokes query(), which gives
// us a passive sampling moment: we read the system clipboard and prepend it
// to history.json if it differs from the most-recent entry. There is no
// background watcher in the v1 SDK — capture only happens while the launcher
// is being typed into.

import { copy } from "highbeam:actions";
import { read as readClipboard } from "highbeam:clipboard";
import { readCache, writeCache } from "highbeam:fs";
import { fuzzy } from "highbeam:match";
import { getInt, getString } from "highbeam:settings";

const CACHE_NAME = "history.json";

const DEFAULT_KEYWORD = "clip";
const DEFAULT_MAX_HISTORY = 50;
const DEFAULT_MAX_ENTRY_BYTES = 10 * 1024;
// `clipboard` and `history` are accepted as aliases regardless of the
// configured keyword — they're short, unambiguous, and avoid the user
// forgetting what they picked.
const ALIAS_TRIGGERS = new Set(["clipboard", "history"]);

let cachedHistory = null;

function configuredKeyword() {
    const value = getString("keyword");
    if (typeof value !== "string") return DEFAULT_KEYWORD;
    const trimmed = value.trim().toLowerCase();
    return trimmed || DEFAULT_KEYWORD;
}

function configuredMaxHistory() {
    const value = getInt("max_history");
    if (typeof value !== "number" || !Number.isFinite(value) || value < 1) {
        return DEFAULT_MAX_HISTORY;
    }
    return Math.floor(value);
}

function configuredMaxEntryBytes() {
    const value = getInt("max_entry_bytes");
    if (typeof value !== "number" || !Number.isFinite(value) || value < 1) {
        return DEFAULT_MAX_ENTRY_BYTES;
    }
    return Math.floor(value);
}

// JS strings are UTF-16; the size cap is meant to track actual storage cost.
function utf8ByteLength(text) {
    return new TextEncoder().encode(text).byteLength;
}

async function loadHistory() {
    if (cachedHistory) return cachedHistory;
    let raw;
    try {
        raw = await readCache(CACHE_NAME);
    } catch {
        cachedHistory = [];
        return cachedHistory;
    }
    if (!raw) {
        cachedHistory = [];
        return cachedHistory;
    }
    try {
        const text = typeof raw === "string"
            ? raw
            : new TextDecoder().decode(raw);
        const parsed = JSON.parse(text);
        if (!Array.isArray(parsed)) {
            cachedHistory = [];
            return cachedHistory;
        }
        // Defensive: drop anything that doesn't look like a history entry.
        // A corrupt or partially-rewritten file shouldn't crash the plugin.
        cachedHistory = parsed.filter(
            (e) =>
                e &&
                typeof e.text === "string" &&
                typeof e.copiedAt === "number",
        );
        return cachedHistory;
    } catch {
        cachedHistory = [];
        return cachedHistory;
    }
}

async function persistHistory(entries) {
    cachedHistory = entries;
    try {
        await writeCache(CACHE_NAME, JSON.stringify(entries));
    } catch {
        // Cache write failures are non-fatal — we still serve the in-memory
        // history for this session. Next successful write will catch up.
    }
}

async function captureClipboard(maxHistory, maxEntryBytes) {
    let current;
    try {
        current = await readClipboard();
    } catch {
        return;
    }
    if (typeof current !== "string") return;
    if (current.trim().length === 0) return;
    if (utf8ByteLength(current) > maxEntryBytes) return;

    const history = await loadHistory();
    if (history.length > 0 && history[0].text === current) return;

    const next = [{ text: current, copiedAt: Date.now() }, ...history];
    if (next.length > maxHistory) next.length = maxHistory;
    await persistHistory(next);
}

function relativeTime(copiedAt) {
    const diffMs = Date.now() - copiedAt;
    if (diffMs < 0) return "just now";
    const sec = Math.floor(diffMs / 1000);
    if (sec < 5) return "just now";
    if (sec < 60) return `${sec}s ago`;
    const min = Math.floor(sec / 60);
    if (min < 60) return `${min}m ago`;
    const hr = Math.floor(min / 60);
    if (hr < 24) return `${hr}h ago`;
    const day = Math.floor(hr / 24);
    if (day === 1) return "yesterday";
    if (day < 7) return `${day}d ago`;
    const wk = Math.floor(day / 7);
    if (wk < 5) return `${wk}w ago`;
    const mo = Math.floor(day / 30);
    if (mo < 12) return `${mo}mo ago`;
    const yr = Math.floor(day / 365);
    return `${yr}y ago`;
}

function previewTitle(text) {
    // Collapse whitespace so a multi-line paste still reads as a single row.
    const collapsed = text.replace(/\s+/g, " ").trim();
    if (collapsed.length <= 80) return collapsed;
    return `${collapsed.slice(0, 77)}...`;
}

function entryKey(entry) {
    // Frecency wants a stable identity. copiedAt is unique enough per entry
    // and doesn't change between sessions (the file is the source of truth).
    return `clip:${entry.copiedAt}`;
}

function resultFor(entry, weight) {
    return {
        key: entryKey(entry),
        title: previewTitle(entry.text),
        subtitle: relativeTime(entry.copiedAt),
        weight,
        action: copy(entry.text),
    };
}

function parseTrigger(input, keyword) {
    if (!input) return null;
    const trimmed = input.trim();
    if (!trimmed) return null;
    const lower = trimmed.toLowerCase();
    const firstSpace = trimmed.indexOf(" ");
    const head = (firstSpace === -1 ? lower : lower.slice(0, firstSpace));
    if (head !== keyword && !ALIAS_TRIGGERS.has(head)) return null;
    const rest = firstSpace === -1 ? "" : trimmed.slice(firstSpace + 1).trim();
    return { rest };
}

export async function* query(input, signal) {
    const keyword = configuredKeyword();
    const maxHistory = configuredMaxHistory();
    const maxEntryBytes = configuredMaxEntryBytes();

    // Capture runs on every keystroke regardless of trigger — that's the
    // whole point of a passive history. Errors inside are swallowed so they
    // can't break the trigger flow.
    await captureClipboard(maxHistory, maxEntryBytes);
    if (signal?.aborted) return;

    const trigger = parseTrigger(input, keyword);
    if (!trigger) return;

    const history = await loadHistory();
    if (history.length === 0) return;

    if (!trigger.rest) {
        // Bare trigger: surface history newest-first. Weights step down so
        // ordering is preserved across plugins that yield around the same
        // weight; the host stable-sorts ties.
        const max = Math.min(history.length, maxHistory);
        for (let i = 0; i < max; i += 1) {
            if (signal?.aborted) return;
            const weight = Math.max(10, 90 - i);
            yield resultFor(history[i], weight);
        }
        return;
    }

    const ranked = fuzzy(history, trigger.rest, {
        key: (e) => e.text,
        threshold: 0.05,
        limit: maxHistory,
    });
    for (const { item, score } of ranked) {
        if (signal?.aborted) return;
        yield resultFor(item, Math.round(score * 100));
    }
}
