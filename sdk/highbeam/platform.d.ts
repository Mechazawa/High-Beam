// `highbeam:platform` — host metadata. Always importable, no capability
// required.

/** Operating system identifier. Matches `std::env::consts::OS` in the host. */
export const os: 'macos' | 'linux';

/** CPU architecture. Common values: `x86_64`, `aarch64`. */
export const arch: string;

/**
 * OS version string. Best-effort:
 *   - macOS: `sw_vers -productVersion` output (e.g. `14.4.1`)
 *   - Linux: `uname -r` output (kernel release)
 *
 * Returns `"unknown"` if detection fails — never throws.
 */
export const version: string;

export function isMacOS(): boolean;
export function isLinux(): boolean;
