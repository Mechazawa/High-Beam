// Dictionary (Linux) — `define <word>` or `dict <word>` looks the word up via
// (in priority order) WordNet's `wn`, then `dict`, then a last-resort grep
// against /usr/share/dict/words. All three are invoked via system.exec.
//
// Output strategy: yield up to 3 results, each carrying the FULL definition
// in its copy action while showing a truncated subtitle inline.

import { copy } from "highbeam:actions";
import { exec } from "highbeam:system";
import { isLinux } from "highbeam:platform";

const TRIGGERS = ["define ", "dict "];
const SUBTITLE_MAX = 80;
const MAX_RESULTS = 3;
// Defs come back small from `wn`/`dict`; cap stdout collection at 1 MiB so a
// pathological output can't drown the runtime.
const EXEC_TIMEOUT_MS = 1200;

// Tool detection cache — `which` is cheap but not free, and the host
// re-imports the module rarely so this survives across queries.
// `null` = not yet probed; `false` = probed and missing; `true` = available.
const toolCache = {
    wn: null,
    dict: null,
    grep: null,
};

async function which(tool, signal) {
    if (toolCache[tool] === null) {
        try {
            const { code } = await exec("which", [tool], {
                signal,
                timeoutMs: EXEC_TIMEOUT_MS,
            });
            toolCache[tool] = code === 0;
        } catch {
            toolCache[tool] = false;
        }
    }
    return toolCache[tool];
}

// `wn <word> -over` (the "overview" report) groups senses by part-of-speech
// using stanzas that look like:
//
//   Overview of noun rust
//
//   The noun rust has 4 senses (first 1 from tagged texts)
//
//   1. (5) rust, rusting -- (a red or brown oxide coating on iron...)
//   2. corrosion, rusting -- (a destructive process...)
//
//   Overview of verb rust
//   ...
//
// Each sense line starts with `<n>.` optionally followed by `(freq)`,
// then a comma-separated synonym list, then ` -- ` and `(definition)`
// (the definition may include `; "example"` suffixes inside the parens).
// We strip the outer parens and keep everything inside as the definition.
function parseWordNetOverview(stdout) {
    const senses = [];
    // Match across the whole text in case lines are wrapped — wn does wrap
    // long synonym lists. The non-greedy `[\s\S]*?` plus a lookahead handles
    // both end-of-line and the next sense / next overview header.
    const re = /^\s*\d+\.\s*(?:\(\d+\)\s*)?([\s\S]*?)\s--\s\(([\s\S]*?)\)\s*$/gm;
    let m;
    while ((m = re.exec(stdout)) !== null) {
        const synonyms = m[1].replace(/\s+/g, " ").trim();
        const defText = m[2].replace(/\s+/g, " ").trim();
        if (!defText) continue;
        // Keep the synonyms as a hint, but the definition is the headline.
        senses.push({ synonyms, definition: defText });
        if (senses.length >= MAX_RESULTS) break;
    }
    return senses;
}

// `dict <word>` output looks like:
//
//   3 definitions found
//
//   From The Collaborative International Dictionary of English v.0.48 [gcide]:
//
//     Rust \Rust\, n. ...
//        1. (Chem.) The reddish yellow coating ...
//        2. A minute fungus ...
//
//   From WordNet (r) 3.0 (2006) [wn]:
//
//     rust
//         n 1: a red or brown oxide coating ...
//
// Multi-database: we collect each `From ... [<short>]:` block's body and
// flatten the leading whitespace; the first non-empty block is the headline
// definition. We surface only the first block — the second is usually a
// near-duplicate (e.g. WordNet repeated under the dict umbrella).
function parseDictOutput(stdout) {
    const lines = stdout.split("\n");
    const blocks = [];
    let current = null;
    for (const raw of lines) {
        const line = raw.replace(/\s+$/, "");
        const header = line.match(/^From\s+(.+?)\s+\[(.+?)\]:\s*$/);
        if (header) {
            if (current && current.body.length > 0) blocks.push(current);
            current = { source: header[1], short: header[2], body: [] };
            continue;
        }
        if (current) {
            // Skip the `N definitions found` preamble.
            if (line.length === 0 && current.body.length === 0) continue;
            current.body.push(line);
        }
    }
    if (current && current.body.length > 0) blocks.push(current);

    if (blocks.length === 0) return [];

    // Take the first block's body — that's the headline definition. Strip
    // the common leading indent dict uses (2 spaces for the headword, 5 for
    // numbered defs) by left-trimming each non-empty line.
    const first = blocks[0];
    const body = first.body
        .map((l) => l.trim())
        .filter((l) => l.length > 0)
        .join(" ");
    if (!body) return [];
    return [{ synonyms: "", definition: body, source: first.source }];
}

function truncate(text, max) {
    if (text.length <= max) return text;
    // Trim to the previous word boundary if possible, then append an ellipsis.
    const slice = text.slice(0, max - 1);
    const lastSpace = slice.lastIndexOf(" ");
    const base = lastSpace > max * 0.6 ? slice.slice(0, lastSpace) : slice;
    return base + "…";
}

function parseTrigger(input) {
    if (typeof input !== "string") return null;
    const lower = input.toLowerCase();
    for (const prefix of TRIGGERS) {
        if (lower.startsWith(prefix)) {
            const word = input.slice(prefix.length).trim();
            if (!word) return null;
            return word;
        }
    }
    return null;
}

async function runWordNet(word, signal) {
    try {
        const { stdout, code } = await exec("wn", [word, "-over"], {
            signal,
            timeoutMs: EXEC_TIMEOUT_MS,
        });
        if (code !== 0) return [];
        return parseWordNetOverview(stdout);
    } catch {
        return [];
    }
}

async function runDict(word, signal) {
    try {
        const { stdout, code } = await exec("dict", [word], {
            signal,
            timeoutMs: EXEC_TIMEOUT_MS,
        });
        if (code !== 0) return [];
        return parseDictOutput(stdout);
    } catch {
        return [];
    }
}

async function runGrep(word, signal) {
    // `^word$` anchored, `-i` case-insensitive. Existence-only.
    try {
        const { code } = await exec(
            "grep",
            ["-i", `^${word}$`, "/usr/share/dict/words"],
            { signal, timeoutMs: EXEC_TIMEOUT_MS },
        );
        return code === 0;
    } catch {
        return false;
    }
}

function buildResult(word, definition, index) {
    return {
        key: `dictionary-linux:${word}:${index}`,
        title: word,
        subtitle: truncate(definition, SUBTITLE_MAX),
        weight: 80,
        pinned: true,
        action: copy(definition),
    };
}

function notFoundResult(word) {
    return {
        key: `dictionary-linux:${word}:not-found`,
        title: word,
        subtitle:
            "No definition found — try installing 'wn' (WordNet) or 'dictd'",
        weight: 80,
        pinned: true,
        action: copy(word),
    };
}

function existsOnlyResult(word) {
    const msg =
        "Word exists in /usr/share/dict/words; no definition available — install wn (WordNet) or dictd for definitions.";
    return {
        key: `dictionary-linux:${word}:exists`,
        title: word,
        subtitle: truncate(msg, SUBTITLE_MAX),
        weight: 80,
        pinned: true,
        action: copy(msg),
    };
}

export async function* query(input, signal) {
    if (!isLinux()) return;
    const word = parseTrigger(input);
    if (!word) return;
    if (signal?.aborted) return;

    if (await which("wn", signal)) {
        const senses = await runWordNet(word, signal);
        if (signal?.aborted) return;
        if (senses.length > 0) {
            for (let i = 0; i < Math.min(senses.length, MAX_RESULTS); i++) {
                yield buildResult(word, senses[i].definition, i);
            }
            return;
        }
        // wn present but returned nothing — fall through to dict/grep below.
    }

    if (await which("dict", signal)) {
        const defs = await runDict(word, signal);
        if (signal?.aborted) return;
        if (defs.length > 0) {
            for (let i = 0; i < Math.min(defs.length, MAX_RESULTS); i++) {
                yield buildResult(word, defs[i].definition, i);
            }
            return;
        }
    }

    if (await which("grep", signal)) {
        const exists = await runGrep(word, signal);
        if (signal?.aborted) return;
        if (exists) {
            yield existsOnlyResult(word);
            return;
        }
    }

    yield notFoundResult(word);
}

// Test-only hook: lets the test suite reset module-level state between
// scenarios without `vi.resetModules()` (which would also reset the
// `highbeam:*` stub vi.fn() identities and force re-binding).
export function __resetForTests() {
    toolCache.wn = null;
    toolCache.dict = null;
    toolCache.grep = null;
}
