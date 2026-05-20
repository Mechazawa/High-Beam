// Used by `Abort::cancel` to fire the JS listeners for a registered controller.
((id) => {
    const c = globalThis.__highbeam_abort_registry.controllers.get(id);
    if (c) c.abort();
})
