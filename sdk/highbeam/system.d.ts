// `highbeam:system` — gated escape hatches for subprocess execution and
// AppleScript. Two separate capabilities so plugins can declare only what
// they need.

export interface ExecOptions {
    signal?: AbortSignal;
    /** Hard wall-clock cap; the child is killed on timeout. */
    timeoutMs?: number;
    /** Working directory for the child. */
    cwd?: string;
}

export interface ExecResult {
    /** Captured stdout (truncated at 10 MB). */
    stdout: string;
    /** Captured stderr (truncated at 10 MB). */
    stderr: string;
    /** Exit code, or `null` if the process was killed by signal. */
    code: number | null;
}

/**
 * Run a subprocess and collect its output. Cap: `system.exec`.
 *
 * Both stdout and stderr are captured; output exceeding 10 MB is silently
 * truncated. Aborting the signal kills the child.
 */
export function exec(
    cmd: string,
    args: readonly string[],
    opts?: ExecOptions,
): Promise<ExecResult>;

export interface AppleScriptOptions {
    signal?: AbortSignal;
    timeoutMs?: number;
}

/**
 * Run an AppleScript snippet. Cap: `system.applescript`.
 *
 * On macOS: executes via `osascript -e <script>` and resolves with the script's
 * stdout (trailing newline trimmed). On every other platform: resolves with
 * `null` immediately — plugins don't have to gate every call site.
 */
export function applescript(
    script: string,
    opts?: AppleScriptOptions,
): Promise<string | null>;
