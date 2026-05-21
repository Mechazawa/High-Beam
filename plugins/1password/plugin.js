// 1Password bridge — search the local vault via the `op` CLI.
//
// The plugin assumes the user has already installed the 1Password CLI and
// signed in (`op signin`). It never prompts for credentials. Sensitive
// fields (passwords) are fetched at *action* time via a shell pipe so the
// secret only leaves 1Password when the user explicitly presses Enter.
// Non-sensitive metadata (item list, URLs) is fetched at query time and
// cached for `cache_seconds` via `fs.cache`.
//
// Trigger:  `<keyword> <query>`     (default keyword: `op`; also accepts `1p`)
// Rows:    up to 3 items × 3 actions (Copy password / Copy username / Open URL)
//          = 9 rows max.

import { copy, exec as execAction, openUrl } from "highbeam:actions";
import { readCache, writeCache } from "highbeam:fs";
import { fuzzy } from "highbeam:match";
import { isLinux, isMacOS } from "highbeam:platform";
import { getInt, getString } from "highbeam:settings";
import { exec } from "highbeam:system";

const DEFAULT_KEYWORD = "op";
// `1p` is accepted as an alias regardless of the configured keyword — short,
// unambiguous, matches the marketing shorthand.
const ALIAS_TRIGGERS = new Set(["1p"]);

const DEFAULT_CACHE_SECONDS = 30;
const MAX_CACHE_SECONDS = 600;

const ITEM_LIMIT = 3;
const FUZZY_THRESHOLD = 0.05;

const CACHE_NAME = "items.json";

// Per-process exec timeout. The plugin's manifest `timeoutMs` is 3000ms; we
// give each individual `op` call most of that since worst-case we issue a
// list followed by up to 3 parallel `get` calls.
const OP_TIMEOUT_MS = 2500;

// In-memory cache of full item records (for URL extraction). Items don't
// usually change URLs between vault edits — cache lifetime piggybacks on the
// list cache to keep the model simple.
let itemDetailsCache = new Map();
let itemDetailsCacheStamp = 0;

// ---------------------------------------------------------------------------
// Option helpers
// ---------------------------------------------------------------------------

function configuredKeyword() {
    const value = getString("keyword");
    if (typeof value !== "string") return DEFAULT_KEYWORD;
    const trimmed = value.trim().toLowerCase();
    return trimmed || DEFAULT_KEYWORD;
}

function configuredAccount() {
    const value = getString("account");
    if (typeof value !== "string") return "";
    return value.trim();
}

function configuredVault() {
    const value = getString("vault");
    if (typeof value !== "string") return "";
    return value.trim();
}

function configuredCacheSeconds() {
    const value = getInt("cache_seconds");
    if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
        return DEFAULT_CACHE_SECONDS;
    }
    const floored = Math.floor(value);
    if (floored > MAX_CACHE_SECONDS) return MAX_CACHE_SECONDS;
    return floored;
}

// ---------------------------------------------------------------------------
// Trigger parsing
// ---------------------------------------------------------------------------

// Returns the trimmed query string when `input` triggers the plugin, or
// `null` otherwise. Trigger words match case-insensitively and must be
// followed by whitespace (or end-of-string) so `optimist` doesn't trip the
// `op` keyword.
function parseTrigger(input, keyword) {
    if (typeof input !== "string") return null;
    const trimmed = input.trimStart();
    if (trimmed.length === 0) return null;
    const lower = trimmed.toLowerCase();
    const firstSpace = trimmed.search(/\s/);
    const head = firstSpace === -1 ? lower : lower.slice(0, firstSpace);
    if (head !== keyword && !ALIAS_TRIGGERS.has(head)) return null;
    const rest = firstSpace === -1 ? "" : trimmed.slice(firstSpace + 1);
    return rest.trim();
}

// ---------------------------------------------------------------------------
// `op` invocation
// ---------------------------------------------------------------------------

function baseOpArgs() {
    const args = [];
    const account = configuredAccount();
    if (account) args.push("--account", account);
    return args;
}

// Run `op item list --format=json` (optionally scoped to a vault) and parse
// the JSON. Throws a tagged error on failure so the caller can branch on
// "op not found" vs "op failed" (typically signed-out).
async function runOpItemList(signal) {
    const args = ["item", "list", "--format=json", ...baseOpArgs()];
    const vault = configuredVault();
    if (vault) args.push("--vault", vault);

    let result;
    try {
        result = await exec("op", args, {
            signal,
            timeoutMs: OP_TIMEOUT_MS,
        });
    } catch (err) {
        // Distinguish "binary not found" from "binary aborted / OS error".
        // Node `ENOENT` rides in via the error message in our host impl;
        // we look for both the structured tag and the textual fallback so
        // the missing-`op` UX is reliable across host versions.
        const tag = errorTag(err);
        throw new OpError("not-found", tag);
    }
    if (result.code !== 0) {
        // Non-zero typically means "not signed in" (exit 1, stderr explains).
        // We surface this as `signed-out` so the UI can prompt accordingly;
        // any non-zero is treated the same way.
        throw new OpError("signed-out", result.stderr || result.stdout || "");
    }
    try {
        const parsed = JSON.parse(result.stdout);
        if (!Array.isArray(parsed)) {
            throw new OpError("parse", "op item list did not return an array");
        }
        return parsed;
    } catch (err) {
        throw new OpError("parse", err && err.message ? err.message : String(err));
    }
}

// Try to detect when `op` itself is missing on PATH. The host's `system.exec`
// surfaces spawn failures as a thrown error with a free-form message; we
// look for known substrings rather than inspecting Node's `err.code` since
// QuickJS may not expose it.
function errorTag(err) {
    if (!err) return "";
    if (typeof err === "string") return err;
    if (err.message) return err.message;
    return String(err);
}

function looksLikeNotFound(message) {
    if (!message) return false;
    const lower = String(message).toLowerCase();
    return (
        lower.includes("enoent") ||
        lower.includes("not found") ||
        lower.includes("no such file") ||
        lower.includes("cannot find")
    );
}

class OpError extends Error {
    constructor(kind, detail) {
        super(`op error (${kind}): ${detail}`);
        this.name = "OpError";
        this.kind = kind;
        this.detail = detail;
    }
}

// ---------------------------------------------------------------------------
// List caching (fs.cache)
// ---------------------------------------------------------------------------

async function readItemListCache(cacheSeconds) {
    if (cacheSeconds <= 0) return null;
    let raw;
    try {
        raw = await readCache(CACHE_NAME);
    } catch {
        return null;
    }
    if (!raw) return null;
    let text;
    try {
        text = typeof raw === "string" ? raw : new TextDecoder().decode(raw);
    } catch {
        return null;
    }
    let parsed;
    try {
        parsed = JSON.parse(text);
    } catch {
        return null;
    }
    if (
        !parsed ||
        typeof parsed.stamp !== "number" ||
        !Array.isArray(parsed.items)
    ) {
        return null;
    }
    if (Date.now() - parsed.stamp > cacheSeconds * 1000) return null;
    return parsed;
}

async function writeItemListCache(items) {
    try {
        await writeCache(
            CACHE_NAME,
            JSON.stringify({ stamp: Date.now(), items }),
        );
    } catch {
        // Cache write failures are non-fatal — the next query simply re-runs
        // `op item list`. We don't want to surface an error row for this.
    }
}

async function invalidateItemListCache() {
    try {
        // Writing an explicitly-stale stamp guarantees the next read treats
        // it as a miss without us needing a separate delete API.
        await writeCache(
            CACHE_NAME,
            JSON.stringify({ stamp: 0, items: [] }),
        );
    } catch {
        // Same: non-fatal.
    }
}

// ---------------------------------------------------------------------------
// Item details (URL extraction)
// ---------------------------------------------------------------------------

function detailsCacheValid(cacheSeconds) {
    if (cacheSeconds <= 0) return false;
    return Date.now() - itemDetailsCacheStamp < cacheSeconds * 1000;
}

function resetDetailsCache() {
    itemDetailsCache = new Map();
    itemDetailsCacheStamp = Date.now();
}

async function fetchItemDetails(id, signal) {
    const args = ["item", "get", id, "--format=json", ...baseOpArgs()];
    let result;
    try {
        result = await exec("op", args, {
            signal,
            timeoutMs: OP_TIMEOUT_MS,
        });
    } catch {
        return null;
    }
    if (result.code !== 0) return null;
    try {
        return JSON.parse(result.stdout);
    } catch {
        return null;
    }
}

function urlFromItem(detail) {
    if (!detail || typeof detail !== "object") return null;
    if (Array.isArray(detail.urls)) {
        for (const entry of detail.urls) {
            if (entry && typeof entry.href === "string" && entry.href) {
                return entry.href;
            }
        }
    }
    return null;
}

// Fetch URL details for the given items in parallel, populating the cache.
// Returns the same array shape with a `_url` field on each item (or null).
async function attachUrls(items, signal, cacheSeconds) {
    if (!detailsCacheValid(cacheSeconds)) resetDetailsCache();

    const needFetch = [];
    for (const item of items) {
        if (itemDetailsCache.has(item.id)) continue;
        needFetch.push(item);
    }

    if (needFetch.length > 0) {
        await Promise.all(
            needFetch.map(async (item) => {
                const detail = await fetchItemDetails(item.id, signal);
                itemDetailsCache.set(item.id, urlFromItem(detail));
            }),
        );
    }

    return items.map((item) => ({
        ...item,
        _url: itemDetailsCache.has(item.id)
            ? itemDetailsCache.get(item.id)
            : null,
    }));
}

// ---------------------------------------------------------------------------
// Clipboard pipe — picks the right OS-native clipboard CLI.
// ---------------------------------------------------------------------------

function clipboardPipeCommand() {
    if (isMacOS()) return "pbcopy";
    // Linux: prefer wayland (wl-copy) when WAYLAND_DISPLAY exists at action
    // time, fall back to xclip. We can't see env vars from JS — `sh` does the
    // detection so it works on both X11 and Wayland sessions without the
    // plugin caring.
    return "if [ -n \"$WAYLAND_DISPLAY\" ] && command -v wl-copy >/dev/null 2>&1; then wl-copy; elif command -v xclip >/dev/null 2>&1; then xclip -selection clipboard; else xsel --clipboard --input; fi";
}

// Shell-escape a single token for safe interpolation into a `sh -c` string.
// `op` item IDs are alphanumeric (UUID-ish) in practice, but we don't trust
// that — quoting is cheap insurance.
function shellQuote(value) {
    return `'${String(value).replace(/'/g, "'\\''")}'`;
}

// Build the `sh -c` script that fetches a field via `op` and pipes it to the
// OS clipboard CLI. The `--reveal` flag is required for password fields.
function copyFieldScript(itemId, field) {
    const account = configuredAccount();
    const opCmd = [
        "op",
        "item",
        "get",
        shellQuote(itemId),
        "--field",
        shellQuote(field),
        "--reveal",
    ];
    if (account) {
        opCmd.push("--account", shellQuote(account));
    }
    // `tr -d '\\n'` strips the trailing newline `op` prints — pasting a
    // password into a login field with a trailing LF can submit prematurely.
    return `${opCmd.join(" ")} | tr -d '\\n' | ${clipboardPipeCommand()}`;
}

function copyFieldAction(itemId, field) {
    return execAction("sh", ["-c", copyFieldScript(itemId, field)]);
}

// ---------------------------------------------------------------------------
// Error-row builders
// ---------------------------------------------------------------------------

function notFoundRow() {
    return {
        key: "op:install",
        title: "1Password CLI not found",
        subtitle: "Install the `op` CLI from 1Password to use this plugin",
        weight: 100,
        pinned: true,
        action: openUrl(
            "https://developer.1password.com/docs/cli/get-started/",
        ),
    };
}

function signedOutRow() {
    return {
        key: "op:signin",
        title: "1Password CLI is signed out",
        subtitle: "Run `op signin` in a terminal, then try again",
        weight: 100,
        pinned: true,
        action: openUrl(
            "https://developer.1password.com/docs/cli/sign-in-manually/",
        ),
    };
}

// ---------------------------------------------------------------------------
// Item list loader (cache-aware)
// ---------------------------------------------------------------------------

async function loadItems(signal, cacheSeconds) {
    const cached = await readItemListCache(cacheSeconds);
    if (cached) return { items: cached.items, error: null };
    try {
        const items = await runOpItemList(signal);
        if (cacheSeconds > 0) await writeItemListCache(items);
        return { items, error: null };
    } catch (err) {
        await invalidateItemListCache();
        if (err instanceof OpError) {
            if (err.kind === "not-found" || looksLikeNotFound(err.detail)) {
                return { items: [], error: "not-found" };
            }
            return { items: [], error: "signed-out" };
        }
        return { items: [], error: "signed-out" };
    }
}

// ---------------------------------------------------------------------------
// Result builders
// ---------------------------------------------------------------------------

function vaultName(item) {
    if (item && item.vault && typeof item.vault.name === "string") {
        return item.vault.name;
    }
    return "";
}

function buildRowsForItem(item, score) {
    const rows = [];
    const title = typeof item.title === "string" ? item.title : "(untitled)";
    const subtitleSuffix = vaultName(item) ? ` — ${vaultName(item)}` : "";
    const baseWeight = Math.round((score ?? 1) * 100);

    rows.push({
        key: `op:${item.id}:password`,
        title: `Copy password for ${title}`,
        subtitle: `1Password${subtitleSuffix}`,
        weight: baseWeight,
        action: copyFieldAction(item.id, "password"),
    });

    rows.push({
        key: `op:${item.id}:username`,
        // Step weight down so the password row beats the username row when
        // scores tie. Users overwhelmingly want the password.
        title: `Copy username for ${title}`,
        subtitle: `1Password${subtitleSuffix}`,
        weight: Math.max(0, baseWeight - 1),
        action: copyFieldAction(item.id, "username"),
    });

    if (item._url) {
        rows.push({
            key: `op:${item.id}:url`,
            title: `Open URL for ${title}`,
            subtitle: item._url,
            weight: Math.max(0, baseWeight - 2),
            action: openUrl(item._url),
        });
    }

    return rows;
}

// ---------------------------------------------------------------------------
// query()
// ---------------------------------------------------------------------------

export async function* query(input, signal) {
    if (!isMacOS() && !isLinux()) return;

    const keyword = configuredKeyword();
    const trigger = parseTrigger(input, keyword);
    if (trigger === null) return;
    // Bare trigger yields nothing — searching the whole vault would dump
    // every entry through the ranker on every keystroke for no real benefit.
    if (trigger.length === 0) return;

    const cacheSeconds = configuredCacheSeconds();
    const { items, error } = await loadItems(signal, cacheSeconds);
    if (signal?.aborted) return;

    if (error === "not-found") {
        yield notFoundRow();
        return;
    }
    if (error === "signed-out") {
        yield signedOutRow();
        return;
    }

    if (items.length === 0) return;

    const ranked = fuzzy(items, trigger, {
        key: (item) => typeof item.title === "string" ? item.title : "",
        threshold: FUZZY_THRESHOLD,
        limit: ITEM_LIMIT,
    });
    if (ranked.length === 0) return;
    if (signal?.aborted) return;

    const topItems = ranked.map((m) => m.item);
    const withUrls = await attachUrls(topItems, signal, cacheSeconds);
    if (signal?.aborted) return;

    for (let i = 0; i < ranked.length; i += 1) {
        const match = ranked[i];
        const item = withUrls[i];
        for (const row of buildRowsForItem(item, match.score)) {
            yield row;
        }
    }
}
