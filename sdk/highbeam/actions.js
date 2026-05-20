// Stub implementation of `highbeam:actions` for vitest. The host's Rust
// version returns the same plain objects; treating this as a real impl
// keeps test assertions simple — `expect(action).toEqual({...})`.

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
