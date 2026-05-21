// Obsidian vault search. Walks the configured vault for `.md` notes, fuzzy-
// matches against filenames, and opens the chosen note via Obsidian's URL
// scheme (`obsidian://open?vault=...&file=...`). The host's OpenUrl handler
// dispatches to `open` / `xdg-open` for us, so a single `openUrl(...)` works
// across both platforms.
//
// The vault walk is done with non-recursive `readDir` calls in a manual BFS
// loop so we can skip `.obsidian/`, `_archive/`, and other dotted directories
// before descending. The host's `recursive: true` mode walks everything
// unconditionally — wrong for a 10k-note vault with a large `.obsidian/`
// plugin tree.

import { openUrl } from "highbeam:actions";
import { readDir, readCache, writeCache } from "highbeam:fs";
import { fuzzy } from "highbeam:match";
import { getBool, getInt, getString } from "highbeam:settings";

const DEFAULT_KEYWORD = "obs";
const DEFAULT_CACHE_SECONDS = 60;
const RESULT_LIMIT = 9;
const FUZZY_THRESHOLD = 0.05;
const CACHE_NAME = "vault-index.json";
const MD_EXT = ".md";

// Module-level memo: avoids re-reading the on-disk cache between keystrokes
// in the same session. Keyed on vault_path so changing the option re-builds.
let memoryCache = null;

function readKeyword() {
    const raw = getString("keyword");
    if (typeof raw === "string" && raw.trim().length > 0) {
        return raw.trim();
    }
    return DEFAULT_KEYWORD;
}

function readAlwaysOn() {
    const raw = getBool("always_on");
    return raw === true;
}

function readCacheSeconds() {
    const raw = getInt("cache_seconds");
    if (typeof raw === "number" && Number.isFinite(raw) && raw >= 0) {
        return raw;
    }
    return DEFAULT_CACHE_SECONDS;
}

function readVaultPath() {
    const raw = getString("vault_path");
    if (typeof raw !== "string") return "";
    return raw.trim();
}

function readVaultName(vaultPath) {
    const raw = getString("vault_name");
    if (typeof raw === "string" && raw.trim().length > 0) {
        return raw.trim();
    }
    return basename(stripTrailingSlash(vaultPath));
}

function stripTrailingSlash(path) {
    if (path.length > 1 && path.endsWith("/")) {
        return path.slice(0, -1);
    }
    return path;
}

function basename(path) {
    const slash = path.lastIndexOf("/");
    return slash >= 0 ? path.slice(slash + 1) : path;
}

// Strip the leading keyword token. Mirrors bitwarden's parser — requires a
// word boundary after the keyword so `obsfoo` isn't treated as a trigger.
function parseTrigger(input, keyword) {
    if (typeof input !== "string") return null;
    const trimmed = input.trimStart();
    const lowerKeyword = keyword.toLowerCase();
    const lower = trimmed.toLowerCase();
    if (!lower.startsWith(lowerKeyword)) return null;
    const rest = trimmed.slice(keyword.length);
    if (rest.length > 0 && !/^\s/.test(rest)) return null;
    return rest.trim();
}

function shouldSkipDir(name) {
    if (name.length === 0) return true;
    // Obsidian's own config tree, anything dotted (`.git`, `.trash`, …), and
    // the conventional archive directory.
    if (name.startsWith(".")) return true;
    if (name === "_archive") return true;
    return false;
}

// BFS over the vault, yielding only `.md` files. `readDir` errors on a single
// directory are non-fatal — permission errors and disappearing dirs shouldn't
// abort the whole walk.
async function collectNotes(vaultPath, signal) {
    const root = stripTrailingSlash(vaultPath);
    const notes = [];
    const queue = [root];
    while (queue.length > 0) {
        if (signal?.aborted) return notes;
        const dir = queue.shift();
        try {
            for await (const entry of readDir(dir, { recursive: false, signal })) {
                if (signal?.aborted) return notes;
                const name = entry?.name ?? "";
                const path = entry?.path ?? "";
                if (!name || !path) continue;
                if (entry.isDir) {
                    if (shouldSkipDir(name)) continue;
                    queue.push(path);
                    continue;
                }
                if (!entry.isFile) continue;
                if (!name.endsWith(MD_EXT)) continue;
                notes.push({
                    name: name.slice(0, -MD_EXT.length),
                    path,
                    relPath: relativePath(root, path),
                });
            }
        } catch {
            // Unreadable subdir — skip and keep walking.
        }
    }
    return notes;
}

function relativePath(root, path) {
    if (path.startsWith(`${root}/`)) {
        return path.slice(root.length + 1);
    }
    return path;
}

// Folder string shown as the subtitle. `Projects/2026/note.md` →
// `Projects/2026/`; a top-level note → `/`.
function folderOf(relPath) {
    const slash = relPath.lastIndexOf("/");
    if (slash < 0) return "/";
    return `${relPath.slice(0, slash)}/`;
}

function decodeCacheBlob(raw) {
    if (raw === null || raw === undefined) return null;
    if (typeof raw === "string") return raw;
    try {
        return new TextDecoder().decode(raw);
    } catch {
        return null;
    }
}

async function readOnDiskCache(vaultPath, ttlMs) {
    let raw;
    try {
        raw = await readCache(CACHE_NAME);
    } catch {
        return null;
    }
    const text = decodeCacheBlob(raw);
    if (!text) return null;
    try {
        const parsed = JSON.parse(text);
        if (!parsed || typeof parsed !== "object") return null;
        if (parsed.vaultPath !== vaultPath) return null;
        if (typeof parsed.fetchedAt !== "number") return null;
        if (!Array.isArray(parsed.notes)) return null;
        if (ttlMs > 0 && Date.now() - parsed.fetchedAt > ttlMs) {
            return null;
        }
        return parsed;
    } catch {
        return null;
    }
}

async function writeOnDiskCache(payload) {
    try {
        await writeCache(CACHE_NAME, JSON.stringify(payload));
    } catch {
        // Cache writes are best-effort.
    }
}

async function getNotes(vaultPath, ttlMs, signal) {
    const now = Date.now();
    if (
        memoryCache &&
        memoryCache.vaultPath === vaultPath &&
        ttlMs > 0 &&
        now - memoryCache.fetchedAt <= ttlMs
    ) {
        return memoryCache.notes;
    }
    const disk = await readOnDiskCache(vaultPath, ttlMs);
    if (disk) {
        memoryCache = disk;
        return disk.notes;
    }
    const notes = await collectNotes(vaultPath, signal);
    const payload = { vaultPath, fetchedAt: Date.now(), notes };
    memoryCache = payload;
    await writeOnDiskCache(payload);
    return notes;
}

// `obsidian://open` accepts vault by name OR by absolute path; name is the
// shorter form and what Obsidian itself emits, so we prefer it. The note
// path is the vault-relative path WITHOUT the `.md` suffix.
function buildObsidianUrl(vaultName, vaultPath, relPath) {
    const file = relPath.endsWith(MD_EXT)
        ? relPath.slice(0, -MD_EXT.length)
        : relPath;
    const params = new URLSearchParams();
    if (vaultName) {
        params.set("vault", vaultName);
    } else {
        params.set("path", vaultPath);
    }
    params.set("file", file);
    return `obsidian://open?${params.toString()}`;
}

function missingVaultPathRow() {
    return {
        key: "obsidian:missing-vault-path",
        title: "Set the Obsidian vault path",
        subtitle:
            "Open high-beam settings and set the Obsidian plugin's `vault_path` to your vault's absolute path",
        weight: 100,
        pinned: true,
        action: { kind: "noop" },
    };
}

export async function* query(input, signal) {
    if (typeof input !== "string") return;

    const vaultPath = readVaultPath();
    const alwaysOn = readAlwaysOn();
    const keyword = readKeyword();

    let queryText;
    if (alwaysOn) {
        queryText = input.trim();
    } else {
        const parsed = parseTrigger(input, keyword);
        if (parsed === null) return;
        queryText = parsed;
    }
    if (queryText.length === 0) return;

    if (vaultPath.length === 0) {
        yield missingVaultPathRow();
        return;
    }

    const ttlMs = readCacheSeconds() * 1000;
    const notes = await getNotes(vaultPath, ttlMs, signal);
    if (signal?.aborted) return;

    const ranked = fuzzy(notes, queryText, {
        key: (note) => note.name,
        threshold: FUZZY_THRESHOLD,
        limit: RESULT_LIMIT,
    });

    const vaultName = readVaultName(vaultPath);
    for (const { item, score } of ranked) {
        if (signal?.aborted) return;
        yield {
            key: `obsidian:${item.path}`,
            title: item.name,
            subtitle: folderOf(item.relPath),
            weight: score * 100,
            action: openUrl(buildObsidianUrl(vaultName, vaultPath, item.relPath)),
        };
    }
}

// Exposed for tests; not part of the host contract.
export const __test__ = {
    parseTrigger,
    folderOf,
    buildObsidianUrl,
    shouldSkipDir,
    resetMemoryCache() {
        memoryCache = null;
    },
};
