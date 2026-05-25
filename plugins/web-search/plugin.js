// Web search plugin. Two modes:
//
//   1. Explicit engine prefix (`google rust`, `wiki Einstein`, `gh foo/bar`)
//      yields a pinned, high-weight row that opens the engine's search URL.
//   2. Anything else yields a single low-weight Google fallback row so the
//      launcher always has *something* to surface for unrecognised input.

import { openUrl } from "highbeam:actions";

// `prefixes` is the public surface; the same engine can be triggered by any
// of the listed prefixes (e.g. `wiki` and `wikipedia`). `template` is the
// URL stem — the URL-encoded query is appended verbatim.
const ENGINES = [
    {
        id: "google",
        label: "Google",
        prefixes: ["google"],
        template: "https://www.google.com/search?q=",
    },
    {
        id: "ddg",
        label: "DuckDuckGo",
        prefixes: ["ddg"],
        template: "https://duckduckgo.com/?q=",
    },
    {
        id: "bing",
        label: "Bing",
        prefixes: ["bing"],
        template: "https://www.bing.com/search?q=",
    },
    {
        id: "wikipedia",
        label: "Wikipedia",
        prefixes: ["wikipedia", "wiki"],
        template: "https://en.wikipedia.org/wiki/Special:Search?search=",
    },
    {
        id: "youtube",
        label: "YouTube",
        prefixes: ["youtube", "yt"],
        template: "https://www.youtube.com/results?search_query=",
    },
    {
        id: "github",
        label: "GitHub",
        prefixes: ["github", "gh"],
        template: "https://github.com/search?q=",
    },
    {
        id: "stackoverflow",
        label: "Stack Overflow",
        prefixes: ["stackoverflow", "so"],
        template: "https://stackoverflow.com/search?q=",
    },
];

const PREFIX_INDEX = new Map(
    ENGINES.flatMap((engine) => engine.prefixes.map((prefix) => [prefix, engine])),
);

const GOOGLE = ENGINES.find((engine) => engine.id === "google");

function buildUrl(engine, query) {
    return `${engine.template}${encodeURIComponent(query)}`;
}

// Splits the input into `[prefix, rest]` only when the prefix is a known
// engine and is followed by whitespace plus a non-empty query. Returns
// `null` otherwise — keeps the matching logic out of `query()`.
function matchEnginePrefix(input) {
    const match = /^(\S+)\s+(.+)$/.exec(input);
    if (!match) return null;

    const [, rawPrefix, rest] = match;
    const engine = PREFIX_INDEX.get(rawPrefix.toLowerCase());
    if (!engine) return null;

    const query = rest.trim();
    if (!query) return null;

    return { engine, query };
}

export async function* query(input, _signal) {
    if (!input) return;
    const trimmed = input.trim();
    if (!trimmed) return;

    const matched = matchEnginePrefix(input);
    if (matched) {
        const { engine, query: searchQuery } = matched;
        const url = buildUrl(engine, searchQuery);

        yield {
            key: `web-search:${engine.id}`,
            title: `Search ${engine.label} for "${searchQuery}"`,
            subtitle: url,
            weight: 80,
            pinned: true,
            action: openUrl(url),
        };
        return;
    }

    // A bare engine prefix (`google`, `gh`, trailing whitespace, …) is an
    // unfinished trigger, not a free-text search — stay silent until the
    // user types a query.
    if (PREFIX_INDEX.has(trimmed.toLowerCase())) return;

    // Fallback: low-weight Google search for the whole input. Sits below
    // pinned/high-weight results from sibling plugins so it only surfaces
    // when nothing else has a better answer.
    const url = buildUrl(GOOGLE, trimmed);

    yield {
        key: "web-search:fallback",
        title: `Search Google for "${trimmed}"`,
        subtitle: url,
        weight: 5,
        pinned: false,
        action: openUrl(url),
    };
}
