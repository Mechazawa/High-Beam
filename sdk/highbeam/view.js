// `highbeam:view` stub for vitest. Each factory returns a plain
// `{ kind, ...props }` object — the same shape the host emits in
// production. Tests can assert on the returned tree directly.

export function Stack(opts) {
    return { kind: 'stack', ...opts };
}

export function Divider() {
    return { kind: 'divider' };
}

export function Heading(opts) {
    return { kind: 'heading', ...opts };
}

export function Text(opts) {
    return { kind: 'text', ...opts };
}

export function Spinner(opts) {
    return { kind: 'spinner', ...(opts ?? {}) };
}

export function ProgressBar(opts) {
    return { kind: 'progress', ...(opts ?? {}) };
}

export function Button(opts) {
    return { kind: 'button', ...opts };
}

export function Input(opts) {
    return { kind: 'input', ...opts };
}

export function TextArea(opts) {
    return { kind: 'textarea', ...opts };
}

export function Image(opts) {
    return { kind: 'image', ...opts };
}

export function Row(opts) {
    return { kind: 'row', ...opts };
}
