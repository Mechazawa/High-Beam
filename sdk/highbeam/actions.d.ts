// `highbeam:actions` — builders for `Result.action`. Requires the `actions`
// capability. Returns plain objects matching the host's wire shape.

import type { Action } from './types';

/**
 * Open a URL with the system handler (`/usr/bin/open` on macOS,
 * `xdg-open` on Linux).
 */
export function openUrl(url: string): Action;

/** Copy text to the clipboard. */
export function copy(text: string): Action;

/**
 * Spawn a subprocess fire-and-forget. No stdout capture — for that, use
 * `highbeam:system.exec` (which requires the `system.exec` capability;
 * this action variant does not).
 */
export function exec(cmd: string, args: readonly string[]): Action;

/**
 * Open the file's parent directory in the system file manager with the file
 * selected.
 *
 * - macOS: `open -R <path>` (Finder's "select this file" mode).
 * - Linux: best-effort `xdg-open <parent_dir>` — no selection.
 */
export function reveal(path: string): Action;
