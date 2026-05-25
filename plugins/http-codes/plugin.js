// HTTP status code lookup. Pinned results — the trigger tolerates "http",
// "http 4", "http404". `http.json` is loaded once and cached in-module.

import { openUrl } from "highbeam:actions";
import { readText } from "highbeam:fs";

const TRIGGER = /^http\s*(\d*)/i;
const MAX_RESULTS = 9;

// Resolved relative to the plugin dir by the host loader.
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
    // Clamp at 100 — the host caps pinned weight at 100, and the user can
    // type more than 3 digits.
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
