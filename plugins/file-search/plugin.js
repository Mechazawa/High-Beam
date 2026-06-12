// File Search plugin — `find <query>` shells out to the platform's native
// indexer (Spotlight on macOS, locate's DB on Linux) and yields matching
// files. Trigger is case-insensitive with an optional space, so both
// `find report` and `findreport` work; the latter behaves the same as the
// former with no separator between trigger and query.

import { openUrl } from "highbeam:actions";
import { forPath } from "highbeam:icons";
import { exec } from "highbeam:system";
import os from "node:os";

// node:os.platform() returns "darwin"/"linux"; wrap so call sites stay put.
const isMacOS = () => os.platform() === "darwin";
const isLinux = () => os.platform() === "linux";

const RESULT_LIMIT = 20;
// Below pinned things (which sit at 100) but above zero-weight stragglers,
// so file matches don't fight calculator/dnd-style pinned plugins for the
// top spot.
const RESULT_WEIGHT = 50;

// Matches `find` followed by either whitespace or end-of-string; the body
// after the trigger is captured (trimmed by the caller). Case-insensitive
// so `Find foo` and `FIND foo` work too.
const TRIGGER_RE = /^find(?:\s+(.*))?$/i;

function basename(path) {
    const slash = path.lastIndexOf("/");
    return slash >= 0 ? path.slice(slash + 1) : path;
}

function parseQuery(input) {
    if (typeof input !== "string") return null;
    const match = TRIGGER_RE.exec(input.trim());
    if (!match) return null;
    const body = (match[1] ?? "").trim();
    if (!body) return null;

    return body;
}

// `mdfind -onlyin <home> "<query>"` scopes Spotlight to the user's home
// dir. exec spawns without a shell, so `~` would reach mdfind as a literal
// path and silently match nothing — pass the expanded home dir instead.
// Output is newline-separated absolute paths; trailing newline is normal.
// Empty stdout (no matches) is *not* an error — the exit code is still 0.
async function runMdfind(query, signal) {
    const result = await exec(
        "mdfind",
        ["-onlyin", os.homedir(), query],
        { signal },
    );
    if (result.code !== 0) return { paths: [], failed: true };

    return { paths: parsePaths(result.stdout), failed: false };
}

// `locate -i -n 20 "<query>"`: -i = case-insensitive, -n = result cap.
// Exit code is non-zero when `locate` isn't installed (ENOENT bubbles up
// to the host as code: null) or when no matches were found — we treat the
// "command missing" case as a friendly hint and silent-drop "no matches".
async function runLocate(query, signal) {
    const result = await exec(
        "locate",
        ["-i", "-n", String(RESULT_LIMIT), query],
        { signal },
    );
    // `locate` returns 1 for "no matches" too, but stdout is empty in that
    // case. A non-zero exit *with* output usually means a permission error
    // on the db file; we'd rather show what we got than nothing.
    const paths = parsePaths(result.stdout);
    // Only flag "missing" when stdout is empty AND exit is non-zero — that's
    // how ENOENT manifests via the host (code: null or 127).
    const missing = paths.length === 0 && result.code !== 0;

    return { paths, missing };
}

function parsePaths(stdout) {
    if (!stdout) return [];
    const lines = stdout.split("\n");
    const out = [];
    for (const line of lines) {
        const trimmed = line.trim();
        if (!trimmed) continue;
        out.push(trimmed);
        if (out.length >= RESULT_LIMIT) break;
    }
    return out;
}

async function resolveIcon(path) {
    // Icon extraction is best-effort: the host returns a 1×1 transparent
    // PNG when it can't read the file, but the call itself can also throw
    // (deleted file between mdfind and us, permission denied, …). Swallow.
    try {
        return await forPath(path);
    } catch {
        return undefined;
    }
}

export async function* query(input, signal) {
    if (!isMacOS() && !isLinux()) return;

    const body = parseQuery(input);
    if (!body) return;

    let paths;
    let missingLocate = false;

    if (isMacOS()) {
        const res = await runMdfind(body, signal);
        paths = res.paths;
    } else {
        const res = await runLocate(body, signal);
        paths = res.paths;
        missingLocate = res.missing;
    }

    if (signal?.aborted) return;

    if (paths.length === 0) {
        if (missingLocate) {
            // Surface a single informational row so the user knows *why*
            // they got nothing — silent failure on a missing index is
            // confusing. The action is a no-op `openUrl` of the man page
            // hint via a copy-friendly subtitle.
            yield {
                key: "file-search:missing-locate",
                title: "`locate` not available",
                subtitle: "Install mlocate or plocate to enable file search",
                weight: RESULT_WEIGHT,
                action: { kind: "noop" },
            };
        }
        return;
    }

    for (const path of paths) {
        if (signal?.aborted) return;
        const icon = await resolveIcon(path);
        // `openUrl(path)` works for both platforms: the host uses the `open`
        // crate, which delegates to `/usr/bin/open` on macOS and `xdg-open`
        // on Linux — both happily accept absolute file paths. Picked over
        // `exec("xdg-open", [path])` so the action shape stays uniform and
        // the host's existing OpenUrl handler does the right thing.
        const result = {
            key: path,
            title: basename(path),
            subtitle: path,
            weight: RESULT_WEIGHT,
            action: openUrl(path),
        };
        if (icon) result.icon = icon;

        yield result;
    }
}
