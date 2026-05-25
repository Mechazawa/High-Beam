// Remove a registered controller from the abort registry. Called from the
// host once it no longer needs to address the controller — e.g. after a
// query's iterator drains naturally, or after a lifecycle hook resolves.
// Safe to call multiple times; the second .delete is a no-op.
((id) => {
    globalThis.__highbeam_abort_registry.controllers.delete(id);
})
