// Stub of `highbeam:platform`. Pure metadata — derive from Node's `process`
// and `os` instead of stubbing; plugin tests want real values so they can
// branch on `isMacOS()` without setting up mocks.

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
