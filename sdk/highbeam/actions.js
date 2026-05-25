// `highbeam:actions` stub for vitest. The host returns the same plain
// objects, so this is the real impl — `expect(action).toEqual({...})` works.

export function openUrl(url) {
    return { kind: 'openUrl', url };
}

export function copy(text) {
    return { kind: 'copy', text };
}

export function exec(cmd, args) {
    return { kind: 'exec', cmd, args };
}

export function reveal(path) {
    return { kind: 'reveal', path };
}
