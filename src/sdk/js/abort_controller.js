// AbortController / AbortSignal polyfill. Idempotent — re-eval is a no-op.
// The registry shape is consumed by `src/sdk/abort.rs`; keep both sides in sync.
(function () {
    if (globalThis.AbortController && globalThis.__highbeam_abort_registry) return;

    class AbortSignalImpl {
        constructor() {
            this._aborted = false;
            this._reason = undefined;
            this._listeners = [];
        }
        get aborted() { return this._aborted; }
        get reason() { return this._reason; }
        addEventListener(type, listener) {
            if (type !== 'abort') return;
            if (this._aborted) {
                try { listener.call(this); } catch (_e) { /* swallow */ }
                return;
            }
            this._listeners.push(listener);
        }
        removeEventListener(type, listener) {
            if (type !== 'abort') return;
            this._listeners = this._listeners.filter(l => l !== listener);
        }
        throwIfAborted() {
            if (this._aborted) {
                const e = new Error('operation aborted');
                e.name = 'AbortError';
                throw e;
            }
        }
    }

    class AbortControllerImpl {
        constructor() { this.signal = new AbortSignalImpl(); }
        abort(reason) {
            const s = this.signal;
            if (s._aborted) return;
            s._aborted = true;
            s._reason = reason ?? Object.assign(new Error('operation aborted'), { name: 'AbortError' });
            const ls = s._listeners;
            s._listeners = [];
            for (const l of ls) {
                try { l.call(s); } catch (_e) { /* swallow */ }
            }
        }
    }

    globalThis.AbortController = AbortControllerImpl;
    globalThis.AbortSignal = AbortSignalImpl;
    globalThis.__highbeam_abort_registry = {
        next_id: 0,
        controllers: new Map(),
    };
})();
