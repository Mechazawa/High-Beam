// View reactivity runtime — lives inside each plugin's QuickJS context.
//
// Installs a `globalThis.__highbeam_views` object exposing the three
// host-callable entry points: `init(handle, props)`, `event(handle,
// callbackId, value)`, `close(handle)`. Each manages a per-handle
// instance with:
//
//   * The original `ViewDef` (looked up in `__highbeam_view_registry`).
//   * The working state wrapped in a recursive `Proxy` so any property
//     `set` (anywhere in the tree) schedules a microtask-batched
//     re-render.
//   * A callback table mapping freshly-minted ids to the closures
//     embedded in the rendered tree; the host fires `event` with an id
//     and we look the function up here.
//   * A `mounted`/`unmounted` abort token for the host-injected
//     `AbortSignal` mounted's `{ signal }` argument carries.
//
// The runtime *pushes* trees back to the host by calling the
// host-installed `__highbeam_paint_tree(handle, treeJson)` /
// `__highbeam_paint_error(handle, message, stack)` globals.
// `__highbeam_dispatch(actionJson)` posts an Action to the host
// (closures that return an Action also get dispatched after the
// closure settles). See `docs/views.md` for the full contract.

(function () {
    if (globalThis.__highbeam_views) return;

    const instances = new Map();
    const proxyCache = new WeakMap();

    function reactive(target, onDirty) {
        if (target === null || typeof target !== 'object') return target;

        if (proxyCache.has(target)) return proxyCache.get(target);

        const proxy = new Proxy(target, {
            get(t, key, receiver) {
                const value = Reflect.get(t, key, receiver);
                // Lazy nested wrapping — only pay the Proxy cost when the
                // plugin actually reaches into a subtree.
                if (value !== null && typeof value === 'object') {
                    return reactive(value, onDirty);
                }
                return value;
            },
            set(t, key, value, receiver) {
                const ok = Reflect.set(t, key, value, receiver);
                onDirty();
                return ok;
            },
            deleteProperty(t, key) {
                const ok = Reflect.deleteProperty(t, key);
                onDirty();
                return ok;
            },
        });

        proxyCache.set(target, proxy);
        return proxy;
    }

    function scheduleFlush(handle) {
        const inst = instances.get(handle);
        if (!inst || inst.dirty || inst.closed) return;
        inst.dirty = true;

        Promise.resolve().then(() => {
            const live = instances.get(handle);
            if (!live || live.closed) return;
            live.dirty = false;
            renderAndPush(handle);
        });
    }

    function renderAndPush(handle) {
        const inst = instances.get(handle);
        if (!inst || inst.closed) return;

        let tree;
        try {
            tree = inst.view.render.call(inst.proxy);
        } catch (err) {
            paintError(handle, err);
            closeFrame(handle);
            return;
        }

        if (tree === null) {
            // `render → null` means: dismiss this frame.
            closeFrame(handle);
            __highbeam_close_view_request(handle);
            return;
        }

        const serialised = substituteCallbacks(tree, inst);
        try {
            __highbeam_paint_tree(handle, JSON.stringify(serialised));
        } catch (err) {
            paintError(handle, err);
        }
    }

    function substituteCallbacks(node, inst) {
        if (node === null || typeof node !== 'object') return node;
        if (Array.isArray(node)) {
            return node.map((n) => substituteCallbacks(n, inst));
        }

        const out = {};
        for (const key of Object.keys(node)) {
            const value = node[key];

            if (typeof value === 'function') {
                const id = inst.nextCallbackId;
                inst.nextCallbackId += 1;
                inst.callbacks.set(id, value);
                out[key] = { __callbackId: id };
            } else if (
                value !== null &&
                typeof value === 'object' &&
                typeof value.kind === 'string' &&
                !Array.isArray(value)
            ) {
                // Action object passed as an `on*` shorthand — wrap as a
                // closure-id that, when fired, dispatches the action.
                const id = inst.nextCallbackId;
                inst.nextCallbackId += 1;
                const action = value;
                inst.callbacks.set(id, () => action);
                out[key] = { __callbackId: id };
            } else if (value !== null && typeof value === 'object') {
                out[key] = substituteCallbacks(value, inst);
            } else {
                out[key] = value;
            }
        }
        return out;
    }

    function paintError(handle, err) {
        const message = err && err.message ? err.message : String(err);
        const stack = err && err.stack ? err.stack : '';
        try {
            __highbeam_paint_error(handle, message, stack);
        } catch (_e) {
            // Last-ditch — if even reporting fails, log to console so
            // the per-plugin log captures it.
            console.error(`view ${handle}: error reporting failed: ${_e?.message ?? _e}`);
        }
    }

    function init(handle, props) {
        const registry = globalThis.__highbeam_view_registry;
        if (!registry) {
            throw new Error(`view ${handle}: registry not initialised`);
        }
        const view = registry.byHandle[String(handle)];
        if (!view) {
            throw new Error(`view ${handle}: no view registered for handle`);
        }

        const inst = {
            view,
            props,
            callbacks: new Map(),
            nextCallbackId: 1,
            dirty: false,
            closed: false,
            abortController: new AbortController(),
        };
        instances.set(handle, inst);

        let state;
        try {
            state = view.setup(props);
        } catch (err) {
            paintError(handle, err);
            closeFrame(handle);
            return;
        }
        inst.proxy = reactive(state, () => scheduleFlush(handle));

        renderAndPush(handle);

        // Defer `mounted` to a microtask so the first paint reaches the
        // host before any mounted-driven HTTP starts.
        Promise.resolve().then(() => {
            const live = instances.get(handle);
            if (!live || live.closed || typeof live.view.mounted !== 'function') return;

            let outcome;
            try {
                outcome = live.view.mounted.call(live.proxy, { signal: live.abortController.signal });
            } catch (err) {
                paintError(handle, err);
                closeFrame(handle);
                return;
            }

            if (outcome && typeof outcome.then === 'function') {
                outcome.then(
                    () => {},
                    (err) => {
                        paintError(handle, err);
                        closeFrame(handle);
                    },
                );
            }
        });
    }

    function event(handle, callbackId, value) {
        const inst = instances.get(handle);
        if (!inst || inst.closed) return;
        const cb = inst.callbacks.get(callbackId);
        if (!cb) return;  // stale callback (from a previous render tree)

        let result;
        try {
            result = cb.call(inst.proxy, value);
        } catch (err) {
            paintError(handle, err);
            closeFrame(handle);
            return;
        }

        if (result && typeof result.then === 'function') {
            result.then(
                (resolved) => maybeDispatchReturn(handle, resolved),
                (err) => {
                    paintError(handle, err);
                    closeFrame(handle);
                },
            );
        } else {
            maybeDispatchReturn(handle, result);
        }
    }

    function maybeDispatchReturn(handle, value) {
        if (value && typeof value === 'object' && typeof value.kind === 'string') {
            try {
                __highbeam_dispatch(JSON.stringify(value));
            } catch (err) {
                paintError(handle, err);
            }
        }
    }

    function close(handle) {
        closeFrame(handle);
    }

    function closeFrame(handle) {
        const inst = instances.get(handle);
        if (!inst || inst.closed) return;
        inst.closed = true;

        // Fire mounted's signal before running unmounted so handlers
        // checking `signal.aborted` see the abort while they still have
        // a chance to bail.
        try { inst.abortController.abort(); } catch (_e) { /* swallow */ }

        if (typeof inst.view.unmounted === 'function') {
            try {
                inst.view.unmounted.call(inst.proxy);
            } catch (err) {
                paintError(handle, err);
            }
        }

        // Keep the registry entry alive even after close. A result row
        // the host cached for an earlier keystroke still references
        // the same handle — re-picking that row should re-open the
        // same view, which can only work if `byHandle[handle]` still
        // resolves to the original `ViewDef`. The bounded leak (one
        // ViewDef per showView yield) clears when the plugin context
        // is reloaded or the daemon restarts; in practice the count
        // stays low because the SDK only mints on showView calls,
        // not on every render.
        instances.delete(handle);
    }

    globalThis.__highbeam_views = {
        init,
        event,
        close,
        // Test-facing: number of live frames in this context. Not part
        // of the host protocol.
        _liveCount: () => instances.size,
    };
})();
