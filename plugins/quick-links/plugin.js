// Quick Links — open URL templates by prefix. Type `gh user/repo`, `npm pkg`,
// `rfc 7231`, etc., and the matching template yields one pinned result. The
// bundled `links.json` is loaded once and cached in-module.
//
// URL-encoding policy: the whole argument is fed through `encodeURIComponent`.
// That means `gh microsoft/vscode` produces `https://github.com/microsoft%2Fvscode`
// — GitHub redirects the percent-encoded form to the slash form, and encoding
// is the only choice that's safe across every template (search queries on MDN
// shouldn't see raw `&` or `#` either). If a future template wants raw slashes
// it can opt in with a `raw: true` flag; not needed for v1.
//
// TODO(post-v1): merge user overrides from
// `~/Library/Application Support/high-beam/plugins/quick-links/links.json`
// resolved via `highbeam:fs.readCache` so users can add/override prefixes
// without touching the bundle.

import { openUrl } from "highbeam:actions";
import { readText } from "highbeam:fs";

const DATA_PATH = "./links.json";

let linksPromise = null;

function loadLinks() {
    if (!linksPromise) {
        linksPromise = readText(DATA_PATH).then((text) => JSON.parse(text));
    }
    return linksPromise;
}

function buildUrl(template, arg) {
    return template.replace("{}", encodeURIComponent(arg));
}

export async function* query(input, _signal) {
    if (!input) return;
    // First whitespace run separates prefix from arg. Trim leading whitespace
    // only — trailing whitespace within the arg is the user's problem and a
    // bare prefix like `gh ` (no arg) shouldn't yield a row.
    const trimmed = input.replace(/^\s+/, "");
    const match = /^(\S+)\s+(.+)$/.exec(trimmed);
    if (!match) return;

    const [, prefix, arg] = match;
    const argTrimmed = arg.trim();
    if (!argTrimmed) return;

    const links = await loadLinks();
    for (const link of links) {
        if (link.prefix !== prefix) continue;
        yield {
            key: `${link.prefix}:${argTrimmed}`,
            title: `${link.prefix} ${argTrimmed}`,
            subtitle: link.description,
            weight: 90,
            pinned: true,
            action: openUrl(buildUrl(link.template, argTrimmed)),
        };
    }
}
