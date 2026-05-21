import { exec, openUrl } from "highbeam:actions";
import { readDir, readText } from "highbeam:fs";
import { forPath } from "highbeam:icons";
import { fuzzy } from "highbeam:match";
import { isLinux, isMacOS } from "highbeam:platform";

const MAC_APP_DIRECTORIES = [
    "/Applications",
    "/System/Applications",
    "/System/Library/CoreServices",
    // QuickJS doesn't expose `$HOME`; the SDK silently skips unreadable dirs,
    // so listing this unconditionally is cheap and safe.
    "~/Applications",
];

const LINUX_DESKTOP_DIRECTORIES = [
    "/usr/share/applications",
    "/usr/local/share/applications",
    "~/.local/share/applications",
];

const APP_EXT = ".app";
const DESKTOP_EXT = ".desktop";
const RESULT_LIMIT = 10;
const SCORE_THRESHOLD = 0.05;

// Module-level cache — scanning these dirs is hundreds of ms; the host
// re-imports rarely, so this survives across queries.
let appsCache = null;

function basenameWithoutExt(path, ext) {
    const slash = path.lastIndexOf("/");
    const name = slash >= 0 ? path.slice(slash + 1) : path;
    return name.endsWith(ext) ? name.slice(0, -ext.length) : name;
}


async function collectMacApps() {
    const apps = [];
    for (const dir of MAC_APP_DIRECTORIES) {
        try {
            for await (const entry of readDir(dir, { recursive: false })) {
                // Filter on basename — `.app` bundles flip between file vs
                // dir depending on filesystem.
                const name = entry.name ?? "";
                if (!name.endsWith(APP_EXT)) continue;
                apps.push({
                    kind: "mac",
                    path: entry.path,
                    appName: basenameWithoutExt(entry.path, APP_EXT),
                });
            }
        } catch {
            // Missing dirs are normal (~/Applications, older macOS layouts).
        }
    }
    return apps;
}

// Strip freedesktop field codes (%f %F %u %U %d %D %n %N %i %c %k %v %m).
function stripExecPlaceholders(execLine) {
    return execLine.replace(/%[fFuUdDnNickvm]/g, "").replace(/\s+/g, " ").trim();
}

function parseDesktopFile(text) {
    const entry = {};
    let inDesktopEntry = false;
    for (const rawLine of text.split("\n")) {
        const line = rawLine.trim();
        if (line.length === 0) continue;
        if (line.startsWith("#")) continue;
        if (line.startsWith("[")) {
            // Ignore action / localized sub-groups (`[Desktop Action New]`).
            inDesktopEntry = line === "[Desktop Entry]";
            continue;
        }
        if (!inDesktopEntry) continue;
        const eq = line.indexOf("=");
        if (eq < 0) continue;
        const key = line.slice(0, eq).trim();
        // Skip localized keys (`Name[de]`).
        if (key.includes("[")) continue;
        const value = line.slice(eq + 1).trim();
        entry[key] = value;
    }
    return entry;
}

async function collectLinuxApps() {
    const apps = [];
    for (const dir of LINUX_DESKTOP_DIRECTORIES) {
        try {
            for await (const entry of readDir(dir, { recursive: false })) {
                const name = entry.name ?? "";
                if (!name.endsWith(DESKTOP_EXT)) continue;
                let text;
                try {
                    text = await readText(entry.path);
                } catch {
                    continue;
                }
                const fields = parseDesktopFile(text);
                if (fields.Type !== "Application") continue;
                if (fields.NoDisplay === "true") continue;
                if (!fields.Name) continue;
                if (!fields.Exec) continue;
                const command = stripExecPlaceholders(fields.Exec);
                if (!command) continue;
                // XDG icon-theme lookup is post-v1; only absolute paths work.
                const iconPath =
                    fields.Icon && fields.Icon.startsWith("/")
                        ? fields.Icon
                        : null;
                apps.push({
                    kind: "linux",
                    path: entry.path,
                    appName: fields.Name,
                    command,
                    iconPath,
                });
            }
        } catch {
            // Missing dir is normal — most distros populate one or two.
        }
    }
    return apps;
}

async function collectApps() {
    if (isMacOS()) return collectMacApps();
    if (isLinux()) return collectLinuxApps();
    return [];
}

async function getApps() {
    if (appsCache === null) {
        appsCache = await collectApps();
    }
    return appsCache;
}

async function resolveIcon(app) {
    if (app.kind === "mac") {
        return forPath(app.path);
    }
    if (app.kind === "linux" && app.iconPath) {
        return forPath(app.iconPath);
    }
    return undefined;
}

function actionFor(app) {
    if (app.kind === "mac") {
        return openUrl(app.path);
    }
    // `sh -c` preserves whatever quoting/pipes the Exec line had — JS-side
    // tokenisation would drop shell metacharacters.
    return exec("sh", ["-c", app.command]);
}

export async function* query(input, _signal) {
    if (!isMacOS() && !isLinux()) return;
    const trimmed = input?.trim();
    if (!trimmed) return;

    const apps = await getApps();
    const matches = fuzzy(apps, trimmed, {
        key: (app) => app.appName,
        threshold: SCORE_THRESHOLD,
        limit: RESULT_LIMIT,
    });

    for (const match of matches) {
        const app = match.item;
        const icon = await resolveIcon(app);
        const result = {
            key: app.path,
            title: app.appName,
            subtitle: app.path,
            weight: match.score * 100,
            action: actionFor(app),
        };
        if (icon) result.icon = icon;
        yield result;
    }
}
