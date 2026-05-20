// `highbeam:actions` — builders for the `Result.action` field. Always
// available (gated only by the `actions` capability — declare it in
// `manifest.json`).
//
// These builders return plain objects matching the host's `Action` wire
// shape; the host deserialises them via serde when your `query()` yields.

import type { Action } from './types';

/**
 * Open a URL with the system handler (`/usr/bin/open` on macOS,
 * `xdg-open` on Linux).
 */
export function openUrl(url: string): Action;

/** Copy text to the clipboard. */
export function copy(text: string): Action;

/**
 * Spawn a subprocess. No stdout capture in v1 — see Stage 7's
 * `highbeam:system.exec` for the live-call variant.
 *
 * The `exec` capability is NOT required for this *action variant*; only the
 * Stage 7 live call requires `system.exec`.
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
