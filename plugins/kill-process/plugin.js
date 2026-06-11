import { exec as execAction } from "highbeam:actions";
import { fuzzy } from "highbeam:match";
import { exec } from "highbeam:system";
import os from "node:os";

const isMacOS = () => os.platform() === "darwin";
const isLinux = () => os.platform() === "linux";

const TRIGGER = "kill";
const RESULT_LIMIT = 10;
const SCORE_THRESHOLD = 0.05;
// `ps` should be near-instant; bound it so a hung binary doesn't eat the
// plugin's whole timeoutMs budget.
const PS_TIMEOUT_MS = 400;

// Parse `ps -axo pid,comm` output. The header is the first line; every
// remaining line is `<whitespace><pid> <comm>`. `comm` may contain spaces
// or be a full path (macOS) — we treat anything after the first whitespace
// run as the full comm value and use its basename for matching.
function parsePsOutput(stdout) {
    const lines = stdout.split("\n");
    const out = [];
    // Skip the first non-empty line (the header). Any line that doesn't lead
    // with a numeric PID is silently dropped — handles wrapped warnings and
    // accidental stderr bleed.
    let seenHeader = false;

    for (const rawLine of lines) {
        const line = rawLine.trim();
        if (line.length === 0) continue;
        if (!seenHeader) {
            seenHeader = true;
            continue;
        }
        const space = line.search(/\s/);
        if (space < 0) continue;
        const pidText = line.slice(0, space);
        if (!/^[0-9]+$/.test(pidText)) continue;
        const comm = line.slice(space + 1).trim();
        if (comm.length === 0) continue;
        const pid = Number(pidText);
        if (!Number.isFinite(pid)) continue;
        out.push({ pid, comm, name: basename(comm) });
    }
    return out;
}

function basename(path) {
    const slash = path.lastIndexOf("/");
    return slash >= 0 ? path.slice(slash + 1) : path;
}

// Strip the leading `kill` token; return the trimmed query that follows, or
// `null` when the input doesn't trigger the plugin at all.
function parseTrigger(input) {
    if (typeof input !== "string") return null;
    const trimmed = input.trimStart();
    if (!trimmed.toLowerCase().startsWith(TRIGGER)) return null;
    const rest = trimmed.slice(TRIGGER.length);
    // `kill` with no separator and no further chars is fine (empty query);
    // `killer` is not — require a word boundary.
    if (rest.length > 0 && !/^\s/.test(rest)) return null;

    return rest.trim();
}

export async function* query(input, signal) {
    if (!isMacOS() && !isLinux()) return;

    const queryText = parseTrigger(input);
    if (queryText === null) return;
    if (queryText.length === 0) return;

    let result;
    try {
        result = await exec("ps", ["-axo", "pid,comm"], {
            signal,
            timeoutMs: PS_TIMEOUT_MS,
        });
    } catch {
        // ps failing (e.g. signal aborted) is non-actionable; drop silently.
        return;
    }

    if (signal?.aborted) return;
    if (result.code !== 0) return;

    const processes = parsePsOutput(result.stdout);
    const matches = fuzzy(processes, queryText, {
        key: (proc) => proc.name,
        threshold: SCORE_THRESHOLD,
        limit: RESULT_LIMIT,
    });

    for (const match of matches) {
        const proc = match.item;

        yield {
            key: String(proc.pid),
            title: proc.name,
            subtitle: `PID ${proc.pid} — Enter to send SIGTERM`,
            weight: match.score * 100,
            action: execAction("kill", [String(proc.pid)]),
        };
    }
}
