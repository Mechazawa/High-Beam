import { exec } from "highbeam:actions";
import { readDir } from "highbeam:fs";
import { forPath } from "highbeam:icons";
import { fuzzy } from "highbeam:match";
import os from "node:os";
const isMacOS = () => os.platform() === "darwin";

const PREF_PANE_DIRECTORIES = [
    "/System/Library/PreferencePanes",
    "/Library/PreferencePanes",
    // User-installed panes; absent on a default install, readDir swallows the miss.
    "~/Library/PreferencePanes",
];

const PANE_EXT = ".prefPane";
const RESULT_LIMIT = 8;
const SCORE_THRESHOLD = 0.2;

// Scanning three directories costs tens of ms even on SSD; results are
// static between OS updates so a single module-level cache pays for itself.
let panesCache = null;

function basenameWithoutExt(path, ext) {
    const slash = path.lastIndexOf("/");
    const name = slash >= 0 ? path.slice(slash + 1) : path;
    return name.endsWith(ext) ? name.slice(0, -ext.length) : name;
}

async function collectPanes() {
    const panes = [];
    for (const dir of PREF_PANE_DIRECTORIES) {
        try {
            for await (const entry of readDir(dir, { recursive: false })) {
                const name = entry.name ?? "";
                if (!name.endsWith(PANE_EXT)) continue;
                panes.push({
                    path: entry.path,
                    name: basenameWithoutExt(entry.path, PANE_EXT),
                });
            }
        } catch {
            // ~/Library/PreferencePanes is the common miss; nothing to recover.
        }
    }
    return panes;
}

async function getPanes() {
    if (panesCache === null) {
        panesCache = await collectPanes();
    }

    return panesCache;
}

export async function* query(input, _signal) {
    if (!isMacOS()) return;

    const trimmed = input?.trim();
    if (!trimmed) return;

    const panes = await getPanes();
    const matches = fuzzy(panes, trimmed, {
        key: (pane) => pane.name,
        threshold: SCORE_THRESHOLD,
        limit: RESULT_LIMIT,
    });

    for (const match of matches) {
        const pane = match.item;
        const icon = await forPath(pane.path);
        const result = {
            key: pane.path,
            title: pane.name,
            subtitle: pane.path,
            weight: match.score * 100,
            // `open <pane>` defers to launch services, which routes legacy
            // .prefPane bundles through System Settings on modern macOS.
            action: exec("open", [pane.path]),
        };
        if (icon) result.icon = icon;

        yield result;
    }
}
