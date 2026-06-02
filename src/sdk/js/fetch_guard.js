// llrt_fetch ships no request timeout; cancellation is signal-only. Wrap
// the global so every request races a default deadline — joined with the
// caller's own signal so explicit aborts still win.
(() => {
    const raw = globalThis.fetch;
    const DEFAULT_TIMEOUT_MS = 30_000;

    globalThis.fetch = (input, init) => {
        const opts = init ? { ...init } : {};
        const deadline = AbortSignal.timeout(DEFAULT_TIMEOUT_MS);
        opts.signal = opts.signal ? AbortSignal.any([opts.signal, deadline]) : deadline;
        return raw(input, opts);
    };
})();
