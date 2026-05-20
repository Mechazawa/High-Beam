// Dictionary (macOS) — `define <word>` or `dict <word>` opens the word in
// Dictionary.app via the `dict://` URL scheme handled by LaunchServices.
//
// Multi-word phrases are URL-encoded (encodeURIComponent) so the resulting
// URL is always well-formed; Dictionary.app accepts the percent-encoded form
// and decodes it before performing the lookup.

import { openUrl } from "highbeam:actions";

const TRIGGER = /^(?:define|dict)\s+(.+?)\s*$/i;

export async function* query(input, _signal) {
    const match = TRIGGER.exec(input);
    if (!match) return;

    const word = match[1];
    if (!word) return;

    yield {
        key: "lookup",
        title: `Define "${word}"`,
        subtitle: "Open in Dictionary.app",
        weight: 80,
        pinned: true,
        action: openUrl(`dict://${encodeURIComponent(word)}`),
    };
}
