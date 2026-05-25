// Used by `Abort::create`. Registers a fresh controller and returns its id + signal.
(() => {
    const id = ++globalThis.__highbeam_abort_registry.next_id;
    const c = new AbortController();
    globalThis.__highbeam_abort_registry.controllers.set(id, c);
    return { id, signal: c.signal };
})
