// Bitwarden bridge. Triggered by `<keyword> <query>` (default `bw <query>`).
//
// Behaviour:
//   - `bw <q>`              → list+search vault items via `bw list items`.
//   - Each item produces up to three rows:
//       * Copy password for <name>   → `bw get password <id>` → clipboard
//       * Copy username for <name>   → `bw get username <id>` → clipboard
//       * Open URL for <name>        → openUrl(item.login.uris[0].uri)
//   - Top ~3 items × 3 actions = at most 9 rows.
//
// Auth model: the plugin assumes the user already ran `bw login` + `bw
// unlock` in a terminal and exported `BW_SESSION` so the high-beam daemon
// inherits it. We never prompt for the master password. If `bw status` does
// not say "unlocked", we surface a single hint row.
//
// Caching: `bw list items` is the slow call (network on cold runs). We
// cache its JSON in `fs.cache` for `cache_seconds` (default 30) so back-to-
// back keystrokes don't re-walk the vault.

import { copy, openUrl } from "highbeam:actions";
import { readCache, writeCache } from "highbeam:fs";
import { fuzzy } from "highbeam:match";
import { getInt, getString } from "highbeam:settings";
import { exec } from "highbeam:system";
import { write as clipboardWrite } from "highbeam:clipboard";

const DEFAULT_KEYWORD = "bw";
const DEFAULT_CACHE_SECONDS = 30;
const MAX_ITEMS = 3;
const RESULT_LIMIT = MAX_ITEMS;
const FUZZY_THRESHOLD = 0.05;
const CACHE_NAME = "bitwarden-items.json";
const EXEC_TIMEOUT_MS = 2800; // a hair under manifest.timeoutMs = 3000
const INSTALL_DOCS_URL = "https://bitwarden.com/help/cli/";

// `bw` item types: 1 = login, 2 = secure note, 3 = card, 4 = identity.
// We only surface logins because the action variants assume passwords /
// usernames / URLs.
const ITEM_TYPE_LOGIN = 1;

// Module-level in-process cache for the parsed item list. The on-disk cache
// (`fs.cache`) survives daemon restarts; this one avoids re-reading the file
// on every keystroke within one session.
let memoryCache = null; // { fetchedAt: number, items: Item[] }

function readKeyword() {
    const raw = getString("keyword");
    if (typeof raw === "string" && raw.trim().length > 0) {
        return raw.trim();
    }
    return DEFAULT_KEYWORD;
}

function readCacheSeconds() {
    const raw = getInt("cache_seconds");
    if (typeof raw === "number" && Number.isFinite(raw) && raw >= 0) {
        return raw;
    }
    return DEFAULT_CACHE_SECONDS;
}

// Strip the leading keyword token from `input`. Returns the trimmed remainder,
// or `null` when the input doesn't trigger the plugin.
function parseTrigger(input, keyword) {
    if (typeof input !== "string") return null;
    const trimmed = input.trimStart();
    const lowerKeyword = keyword.toLowerCase();
    const lower = trimmed.toLowerCase();
    if (!lower.startsWith(lowerKeyword)) return null;
    const rest = trimmed.slice(keyword.length);
    // Require a word boundary so `bwsomething` isn't treated as a trigger.
    if (rest.length > 0 && !/^\s/.test(rest)) return null;
    return rest.trim();
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

async function readOnDiskCache(ttlMs) {
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
        if (typeof parsed.fetchedAt !== "number") return null;
        if (!Array.isArray(parsed.items)) return null;
        // ttlMs of 0 disables the cache entirely — see manifest option
        // `cache_seconds` (min 0). Any positive TTL behaves the usual way.
        if (ttlMs <= 0) return null;
        if (Date.now() - parsed.fetchedAt > ttlMs) {
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
        // Cache writes are best-effort; the session-level cache still works.
    }
}

// Map a raw `bw list items` entry onto the leaner shape the plugin uses.
// We keep only the fields we render so the cache stays small.
function normaliseItem(raw) {
    if (!raw || typeof raw !== "object") return null;
    if (raw.type !== ITEM_TYPE_LOGIN) return null;
    if (typeof raw.id !== "string" || raw.id.length === 0) return null;
    if (typeof raw.name !== "string" || raw.name.length === 0) return null;
    const login = raw.login && typeof raw.login === "object" ? raw.login : {};
    const uris = Array.isArray(login.uris) ? login.uris : [];
    const firstUri = uris.find(
        (u) => u && typeof u.uri === "string" && u.uri.length > 0,
    );
    return {
        id: raw.id,
        name: raw.name,
        folderId: typeof raw.folderId === "string" ? raw.folderId : null,
        username:
            typeof login.username === "string" && login.username.length > 0
                ? login.username
                : null,
        url: firstUri ? firstUri.uri : null,
        hasPassword:
            typeof login.password === "string" && login.password.length > 0,
    };
}

function normaliseFolders(raw) {
    if (!Array.isArray(raw)) return {};
    const out = {};
    for (const f of raw) {
        if (!f || typeof f !== "object") continue;
        if (typeof f.id !== "string") continue;
        if (typeof f.name !== "string") continue;
        out[f.id] = f.name;
    }
    return out;
}

// Wrapper around `system.exec` that resolves with stdout on success and
// throws a structured Error on failure. We surface `notInstalled` / `locked`
// flags on the error so the caller can render the right hint row without
// scraping stderr a second time.
async function bwExec(args, signal) {
    let result;
    try {
        result = await exec("bw", args, {
            signal,
            timeoutMs: EXEC_TIMEOUT_MS,
        });
    } catch (err) {
        // Spawn failures land here (e.g. binary not on PATH, signal abort).
        const message = err && err.message ? String(err.message) : "";
        const e = new Error(message || "bw invocation failed");
        if (looksLikeNotInstalled(message)) {
            e.notInstalled = true;
        }
        throw e;
    }
    if (result.code === 0) {
        return result.stdout;
    }
    const stderr = result.stderr || "";
    const stdout = result.stdout || "";
    const combined = `${stderr}\n${stdout}`;
    const e = new Error(stderr.trim() || `bw exited with code ${result.code}`);
    if (looksLikeNotInstalled(combined)) {
        e.notInstalled = true;
    }
    if (looksLikeLocked(combined)) {
        e.locked = true;
    }
    throw e;
}

function looksLikeNotInstalled(text) {
    if (!text) return false;
    // Common spawn-failure messages across platforms. The host's
    // `system.exec` doesn't normalise these, so we match heuristically.
    return (
        /ENOENT/i.test(text) ||
        /not found/i.test(text) ||
        /no such file/i.test(text) ||
        /command not found/i.test(text)
    );
}

function looksLikeLocked(text) {
    if (!text) return false;
    return (
        /vault is locked/i.test(text) ||
        /not logged in/i.test(text) ||
        /you are not logged in/i.test(text) ||
        /session is invalid/i.test(text) ||
        /mac failed/i.test(text)
    );
}

// `bw status` prints a JSON object that includes `status: "unlocked" |
// "locked" | "unauthenticated"`. We treat anything that isn't strictly
// "unlocked" as locked — that's the only state where `bw get` works without
// re-prompting.
async function checkUnlocked(signal) {
    const stdout = await bwExec(["status"], signal);
    try {
        const parsed = JSON.parse(stdout);
        if (parsed && typeof parsed === "object" && typeof parsed.status === "string") {
            return parsed.status === "unlocked";
        }
    } catch {
        // Fall through to false — a malformed status reply is treated as locked.
    }
    return false;
}

async function fetchVault(signal) {
    const itemsJson = await bwExec(["list", "items"], signal);
    const foldersJson = await bwExec(["list", "folders"], signal);
    let rawItems;
    let rawFolders;
    try {
        rawItems = JSON.parse(itemsJson);
    } catch {
        throw new Error("could not parse `bw list items` JSON");
    }
    try {
        rawFolders = JSON.parse(foldersJson);
    } catch {
        rawFolders = [];
    }
    if (!Array.isArray(rawItems)) {
        throw new Error("unexpected `bw list items` shape");
    }
    const folders = normaliseFolders(rawFolders);
    const items = [];
    for (const raw of rawItems) {
        const item = normaliseItem(raw);
        if (item) items.push(item);
    }
    return { items, folders };
}

async function getVault(ttlMs, signal) {
    const now = Date.now();
    if (
        memoryCache &&
        ttlMs > 0 &&
        now - memoryCache.fetchedAt <= ttlMs
    ) {
        return memoryCache;
    }
    // ttlMs <= 0 means caching is disabled — always refetch.
    const disk = await readOnDiskCache(ttlMs);
    if (disk) {
        memoryCache = disk;
        return disk;
    }
    const { items, folders } = await fetchVault(signal);
    const payload = { fetchedAt: Date.now(), items, folders };
    memoryCache = payload;
    await writeOnDiskCache(payload);
    return payload;
}

function folderLabel(item, folders) {
    if (!item.folderId) return "No folder";
    return folders[item.folderId] || "No folder";
}

function buildResults(item, folders) {
    const folder = folderLabel(item, folders);
    const subtitle = item.username
        ? `${folder} — ${item.username}`
        : folder;
    const out = [];
    if (item.hasPassword) {
        out.push({
            key: `bw:${item.id}:password`,
            title: `Copy password for ${item.name}`,
            subtitle,
            action: {
                kind: "bw-copy",
                field: "password",
                id: item.id,
            },
        });
    }
    if (item.username) {
        out.push({
            key: `bw:${item.id}:username`,
            title: `Copy username for ${item.name}`,
            subtitle,
            action: {
                kind: "bw-copy",
                field: "username",
                id: item.id,
            },
        });
    }
    if (item.url) {
        out.push({
            key: `bw:${item.id}:url`,
            title: `Open URL for ${item.name}`,
            subtitle: `${folder} — ${item.url}`,
            action: openUrl(item.url),
        });
    }
    return out;
}

// `bw get <field> <id>` returns the value on stdout. We pull the value
// during `query()` (rather than emitting an `exec` action) because the
// clipboard host action only accepts a literal string — we don't know the
// password until we've called `bw`. Doing it eagerly per row is fine for the
// top-N items; the cost is N+M short `bw get` invocations on the keystroke
// that yields the rows, gated by the manifest's 3s timeout.
//
// The downside: we fetch passwords for items the user may never select. The
// alternative (an `exec`-style action that runs `bw get` on Enter and pipes
// stdout to the clipboard) would need shell composition we don't want to
// rely on cross-platform — `sh -c "bw get password <id> | pbcopy"` works on
// macOS but Linux clipboards vary (wl-copy / xclip / xsel). The current
// approach also lets us surface bw errors before the user commits.

async function resolveCopyAction(action, signal) {
    if (!action || action.kind !== "bw-copy") return action;
    try {
        const stdout = await bwExec(
            ["get", action.field, action.id],
            signal,
        );
        return copy(stdout.replace(/\n+$/, ""));
    } catch {
        // Surface a no-op-ish copy so Enter is still safe; the host will
        // log the bw failure separately on subsequent invocations.
        return copy("");
    }
}

// Build a deferred-action wrapper. We keep the action as a sentinel during
// listing so the host doesn't have to wait on `bw get` for items the user
// never selects. On Enter the host invokes the action; if a plugin needs to
// resolve it lazily, it has to do so during `query()` because there's no
// per-action hook in v1.
//
// For v1 we resolve eagerly during `query()`. See the comment above
// `resolveCopyAction` for the rationale.
function hintRow({ key, title, subtitle, action }) {
    return {
        key,
        title,
        subtitle,
        weight: 100,
        pinned: true,
        action,
    };
}

function notInstalledRow() {
    return hintRow({
        key: "bitwarden-not-installed",
        title: "Install the Bitwarden CLI",
        subtitle: "Press Enter to open the install docs (`bw` is not on PATH)",
        action: openUrl(INSTALL_DOCS_URL),
    });
}

function lockedRow() {
    return hintRow({
        key: "bitwarden-locked",
        title: "Unlock Bitwarden",
        subtitle:
            "Run `bw unlock` in a terminal and export `BW_SESSION` so high-beam inherits it",
        // Copy the unlock command so the user can paste it straight into a
        // terminal. Pinned + weight 100 keeps this on top.
        action: copy('export BW_SESSION="$(bw unlock --raw)"'),
    });
}

export async function* query(input, signal) {
    const keyword = readKeyword();
    const queryText = parseTrigger(input, keyword);
    if (queryText === null) return;
    if (queryText.length === 0) return;

    let unlocked;
    try {
        unlocked = await checkUnlocked(signal);
    } catch (err) {
        if (err && err.notInstalled) {
            yield notInstalledRow();
            return;
        }
        // Any other failure (locked, network, etc.) — treat as locked and
        // surface the unlock hint so the user has a clear next step.
        yield lockedRow();
        return;
    }
    if (signal?.aborted) return;
    if (!unlocked) {
        yield lockedRow();
        return;
    }

    const ttlMs = readCacheSeconds() * 1000;
    let vault;
    try {
        vault = await getVault(ttlMs, signal);
    } catch (err) {
        if (err && err.notInstalled) {
            yield notInstalledRow();
            return;
        }
        if (err && err.locked) {
            yield lockedRow();
            return;
        }
        // Unknown failure — yield nothing and let the host log via plugin.log.
        return;
    }
    if (signal?.aborted) return;

    const matches = fuzzy(vault.items, queryText, {
        key: (item) => item.name,
        threshold: FUZZY_THRESHOLD,
        limit: RESULT_LIMIT,
    });

    for (const match of matches) {
        const rows = buildResults(match.item, vault.folders);
        for (const row of rows) {
            const resolvedAction = await resolveCopyAction(row.action, signal);
            if (signal?.aborted) return;
            yield {
                ...row,
                weight: match.score * 100,
                action: resolvedAction,
            };
        }
    }
}

// Exposed for tests; not part of the host contract.
export const __test__ = {
    parseTrigger,
    normaliseItem,
    normaliseFolders,
    looksLikeNotInstalled,
    looksLikeLocked,
    resetMemoryCache() {
        memoryCache = null;
    },
    // clipboardWrite is imported so the capability check fires; some callers
    // may want imperative writes in the future.
    clipboardWrite,
};
