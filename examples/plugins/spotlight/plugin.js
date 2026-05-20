import { openUrl } from "highbeam:actions";
import { readDir } from "highbeam:fs";
import { forPath } from "highbeam:icons";
import { fuzzy } from "highbeam:match";
import { isMacOS } from "highbeam:platform";

const APP_DIRECTORIES = [
    "/Applications",
    "/System/Applications",
    "/System/Library/CoreServices",
];

const APP_EXT = ".app";
const RESULT_LIMIT = 10;
const SCORE_THRESHOLD = 0.3;

// Module-level cache: scanning these directories takes ~hundreds of ms;
// the host re-imports the module rarely, so this survives across queries.
let appsCache = null;

function basenameWithoutExt(path, ext) {
    const slash = path.lastIndexOf("/");
    const name = slash >= 0 ? path.slice(slash + 1) : path;
    return name.endsWith(ext) ? name.slice(0, -ext.length) : name;
}

function highlight(name, highlights) {
    if (!highlights || highlights.length === 0) return name;
    let out = "";
    let cursor = 0;
    for (const [start, end] of highlights) {
        if (start > cursor) out += name.slice(cursor, start);
        out += `<b>${name.slice(start, end)}</b>`;
        cursor = end;
    }
    if (cursor < name.length) out += name.slice(cursor);
    return out;
}

async function collectApps() {
    const apps = [];
    for (const dir of APP_DIRECTORIES) {
        try {
            for await (const entry of readDir(dir, { recursive: true })) {
                // Filter on the basename so we catch `.app` bundles regardless
                // of how the host represents them (file vs dir flag varies by
                // platform and FS type).
                const name = entry.name ?? "";
                if (!name.endsWith(APP_EXT)) continue;
                apps.push({
                    path: entry.path,
                    appName: basenameWithoutExt(entry.path, APP_EXT),
                });
            }
        } catch {
            // A missing or unreadable dir shouldn't kill the whole scan —
            // /System/Applications doesn't exist on older macOS versions.
        }
    }
    return apps;
}

async function getApps() {
    if (appsCache === null) {
        appsCache = await collectApps();
    }
    return appsCache;
}

export async function* query(input, _signal) {
    if (!isMacOS()) return;
    const trimmed = input?.trim();
    if (!trimmed) return;

    const apps = await getApps();
    const matches = fuzzy(apps, trimmed, {
        key: (app) => app.appName,
        threshold: SCORE_THRESHOLD,
        limit: RESULT_LIMIT,
    });

    for (const match of matches) {
        const { path, appName } = match.item;
        const icon = await forPath(path);
        yield {
            key: path,
            title: highlight(appName, match.highlights),
            subtitle: path,
            weight: match.score * 100,
            icon,
            action: openUrl(path),
        };
    }
}
