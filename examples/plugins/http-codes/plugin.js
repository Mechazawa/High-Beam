// HTTP status code lookup — port of the v2 HttpCodePlugin.
//
// Pinned plugin: results always show regardless of the user's full query,
// because the trigger regex tolerates a stray space ("http", "http 4",
// "http404"). The bundled `http.json` is loaded once on first invocation
// and cached in-module to keep the per-keystroke budget tight.

import { openUrl } from "highbeam:actions";
import { readText } from "highbeam:fs";

const TRIGGER = /^http\s*(\d*)/i;
const MAX_RESULTS = 9;

// Resolved relative to the plugin's own directory by the host loader, so
// every install location works without hand-edited absolute paths.
const DATA_PATH = "./http.json";

let codesPromise = null;

function loadCodes() {
    if (!codesPromise) {
        codesPromise = readText(DATA_PATH).then((text) => JSON.parse(text));
    }
    return codesPromise;
}

export async function* query(input, _signal) {
    const match = TRIGGER.exec(input);
    if (!match) return;

    const prefix = match[1] ?? "";
    const codes = await loadCodes();
    // Clamp the weight so longer-than-3-digit prefixes (e.g. someone
    // typing "http 4040") don't push above the 100 ceiling the host
    // expects for pinned results.
    const weight = Math.min(100 * (prefix.length / 3), 100);

    let yielded = 0;
    for (const { key, title, description } of codes) {
        if (yielded >= MAX_RESULTS) return;
        const code = String(key);
        if (!code.startsWith(prefix)) continue;
        yielded += 1;
        yield {
            key: code,
            title: `${code} - ${title}`,
            subtitle: description,
            weight,
            pinned: true,
            action: openUrl(
                `https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/${code}`,
            ),
        };
    }
}
