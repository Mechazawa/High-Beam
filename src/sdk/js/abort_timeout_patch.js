// Replace the native AbortSignal.timeout: cargo feature-unification forces
// llrt_abort's `sleep-timers` backend on (llrt_fetch depends on it with
// default features), and that backend reads llrt_timers' process-global
// runtime table — never initialised here, and unsound under one runtime
// per plugin anyway (unreachable_unchecked on lookup miss). This impl uses
// the host's own setTimeout, which is per-context and safe.
AbortSignal.timeout = (ms) => {
    const controller = new AbortController();
    setTimeout(
        () => controller.abort(new DOMException(`operation timed out after ${ms}ms`, "TimeoutError")),
        ms,
    );
    return controller.signal;
};
