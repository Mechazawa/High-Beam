// D&D 5e spell search. Triggers on `spell <q>` or `5e spell <q>` — the
// keyword gate avoids fuzzy-ranking 300KB of spells on every keystroke.

import { openUrl } from "highbeam:actions";
import { fuzzy } from "highbeam:match";
import { readText } from "highbeam:fs";

const TRIGGER = /^\s*(?:5e\s+)?spells?(?:\s+(.*))?$/i;

// Cached on first match — JSON is bulky and most queries never need it.
let spellsPromise = null;

function loadSpells() {
    if (!spellsPromise) {
        spellsPromise = readText("./5eSpells.json").then((text) => JSON.parse(text));
    }
    return spellsPromise;
}

function capitalize(s) {
    return s.length === 0 ? s : s[0].toUpperCase() + s.slice(1);
}

function subtitleFor(spell) {
    const levelPart = spell.level === "cantrip"
        ? "Cantrip"
        : `Level ${spell.level}`;
    const school = capitalize(spell.school);
    return [levelPart, school, spell.classes, spell.range, spell["casting time"]]
        .filter(Boolean)
        .join(" · ");
}

export async function* query(input, _signal) {
    const match = TRIGGER.exec(input);
    if (!match) return;
    const q = (match[1] ?? "").trim();
    if (!q) return;

    const spells = await loadSpells();
    const ranked = fuzzy(spells, q, {
        key: (s) => s.name,
        threshold: 0.05,
        limit: 10,
    });

    for (const { item, score } of ranked) {
        yield {
            key: item.href || item.name,
            title: item.name,
            subtitle: subtitleFor(item),
            weight: score * 100,
            action: openUrl(item.href),
        };
    }
}
