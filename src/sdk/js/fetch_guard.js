// llrt_fetch ships no request timeout; cancellation is signal-only. Wrap
// the global so every request races a default deadline — joined with the
// caller's signals so explicit aborts still win (sooner than the deadline;
// they cannot extend past it).
(() => {
    const raw = globalThis.fetch;
    const DEFAULT_TIMEOUT_MS = 30_000;

    globalThis.fetch = (input, init) => {
        const opts = init ? { ...init } : {};
        const signals = [];
        if (opts.signal) signals.push(opts.signal);
        // A Request instance can carry its own signal. llrt gives the init
        // arg precedence, so once we set opts.signal the Request's would be
        // silently dropped — fold it in instead.
        if (input instanceof Request && input.signal) signals.push(input.signal);

        // Spec behavior llrt skips: a pre-aborted signal rejects before any
        // I/O. Has to happen here — llrt's fetch only subscribes to future
        // abort sends, and AbortSignal.any's already-aborted early-return
        // never sends, so a pre-aborted signal would otherwise hang the
        // request until the transport gives up.
        for (const signal of signals) {
            if (signal.aborted) {
                return Promise.reject(
                    signal.reason ?? new DOMException("This operation was aborted", "AbortError"),
                );
            }
        }

        signals.push(AbortSignal.timeout(DEFAULT_TIMEOUT_MS));
        opts.signal = signals.length === 1 ? signals[0] : AbortSignal.any(signals);
        return raw(input, opts);
    };
})();
