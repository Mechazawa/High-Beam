// `highbeam:platform` — real metadata from Node so tests can branch on
// `isMacOS()` without mocks.

import nodeOs from 'node:os';

function detectOs() {
    return process.platform === 'darwin' ? 'macos' : 'linux';
}

function detectVersion() {
    try {
        return nodeOs.release() || 'unknown';
    } catch {
        return 'unknown';
    }
}

export const os = detectOs();
export const arch = process.arch;
export const version = detectVersion();

export function isMacOS() {
    return os === 'macos';
}

export function isLinux() {
    return os === 'linux';
}
