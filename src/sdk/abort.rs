//! Host-side handle over the native `AbortController`/`AbortSignal` classes
//! from `llrt_abort`.
//!
//! Plugins receive a spec-shaped `AbortSignal` as the second arg to
//! `query(input, signal)`. [`Abort::cancel`] fires the JS-side listeners
//! (via the native controller) and the Rust [`CancellationToken`] in one
//! call. `highbeam:fs` / `highbeam:system` accept a signal from plugin code
//! and turn it back into a token via [`token_from_js_signal`] — that works
//! against any spec-shaped signal because it attaches through
//! `addEventListener`, which the native class exposes via its
//! `EventTarget` prototype.
//!
//! Unlike the old JS polyfill there is no host-side controller registry:
//! the controller is a GC-managed class instance, so there is no `release`
//! step and nothing to leak on the natural-completion path.

use llrt_abort::AbortController;
use rquickjs::class::Class;
use rquickjs::function::{Opt, This};
use rquickjs::{Ctx, Error as JsError, Function, Object};
use tokio_util::sync::CancellationToken;

/// Host handle for one controller+signal pair. Lifetime-bound to the JS
/// context it was created in — create, cancel, and drop all happen inside
/// the same `async_with` scope.
pub struct Abort<'js> {
    controller: Class<'js, AbortController<'js>>,
    token: CancellationToken,
}

impl<'js> Abort<'js> {
    /// Build a fresh controller+signal pair inside the active JS context.
    /// Returns the Rust handle and the JS-side `controller.signal` object.
    ///
    /// # Errors
    ///
    /// Propagates JS errors from constructing the native controller.
    pub fn create(ctx: &Ctx<'js>) -> Result<(Self, Object<'js>), JsError> {
        let controller = Class::instance(ctx.clone(), AbortController::new(ctx.clone())?)?;
        let signal = controller.borrow().signal();
        let signal_obj: Object<'js> = signal.into_inner();

        Ok((
            Self {
                controller,
                token: CancellationToken::new(),
            },
            signal_obj,
        ))
    }

    /// The Rust-side token. Host I/O futures race this against their work.
    #[must_use]
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Abort the controller: fires JS-side listeners (with a spec
    /// `DOMException` `AbortError` reason) AND the Rust-side token.
    /// Idempotent. Must be called from inside an `async_with` block on the
    /// owning context.
    ///
    /// # Errors
    ///
    /// Propagates errors from the native controller's `abort()`.
    pub fn cancel(&self, ctx: &Ctx<'js>) -> Result<(), JsError> {
        if self.token.is_cancelled() {
            return Ok(());
        }
        self.token.cancel();
        AbortController::abort(ctx.clone(), This(self.controller.clone()), Opt(None))
    }
}

/// Replaces the native `AbortSignal.timeout` with a setTimeout-based impl —
/// see the comment in the script for why the native one is unsound here.
const ABORT_TIMEOUT_PATCH_JS: &str = include_str!("js/abort_timeout_patch.js");

/// Install the native `AbortController`/`AbortSignal` classes plus the
/// `DOMException` class their abort reasons are constructed from.
/// Idempotent. The host's `setTimeout` (see [`crate::sdk::timers`]) must be
/// installed in the same context for `AbortSignal.timeout` to work.
///
/// # Errors
///
/// Propagates JS errors from the class registration.
pub fn install(ctx: &Ctx<'_>) -> Result<(), JsError> {
    llrt_exceptions::init(ctx)?;
    llrt_abort::init(ctx)?;
    ctx.eval::<(), _>(ABORT_TIMEOUT_PATCH_JS)
}

/// Build a Rust [`CancellationToken`] that flips when the given JS-side
/// `AbortSignal` aborts. Pre-cancelled token if the signal is already aborted.
///
/// # Errors
///
/// Propagates JS errors from reading `aborted` or attaching the listener.
pub fn token_from_js_signal<'js>(ctx: &Ctx<'js>, signal: &Object<'js>) -> Result<CancellationToken, JsError> {
    let aborted: bool = signal.get::<_, bool>("aborted").unwrap_or(false);
    let token = CancellationToken::new();

    if aborted {
        token.cancel();
        return Ok(token);
    }

    let token_for_cb = token.clone();
    let cb = Function::new(ctx.clone(), move || {
        token_for_cb.cancel();
    })?;
    let add: Function<'js> = signal.get("addEventListener")?;
    add.call::<_, ()>((This(signal.clone()), "abort", cb))?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rquickjs::{AsyncContext, AsyncRuntime};

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt")
    }

    #[test]
    fn install_is_idempotent() {
        let runtime = rt();
        runtime.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            ctx.async_with(async move |ctx| {
                install(&ctx).expect("first");
                install(&ctx).expect("second");
                let has: bool = ctx.eval("typeof AbortController === 'function'").expect("eval");
                assert!(has);
            })
            .await;
        });
    }

    #[test]
    fn js_side_controller_aborts_listener() {
        let runtime = rt();
        runtime.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            ctx.async_with(async move |ctx| {
                install(&ctx).expect("install");
                let fired: bool = ctx
                    .eval(
                        r"
                    (() => {
                        const c = new AbortController();
                        let fired = false;
                        c.signal.addEventListener('abort', () => { fired = true; });
                        c.abort();
                        return fired && c.signal.aborted;
                    })()
                ",
                    )
                    .expect("eval");
                assert!(fired);
            })
            .await;
        });
    }

    #[test]
    fn host_abort_cancels_token_and_fires_js_listener() {
        let runtime = rt();
        runtime.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            ctx.async_with(async move |ctx| {
                install(&ctx).expect("install");
                let (abort, signal) = Abort::create(&ctx).expect("create");
                let attach: Function<'_> = ctx
                    .eval(
                        r"((sig) => {
                    sig._observed = false;
                    sig.addEventListener('abort', () => { sig._observed = true; });
                })",
                    )
                    .expect("eval attach");
                attach.call::<_, ()>((signal.clone(),)).expect("attach call");
                assert!(!abort.is_aborted());
                abort.cancel(&ctx).expect("cancel");
                assert!(abort.is_aborted());
                let observed: bool = signal.get("_observed").expect("read observed");
                assert!(observed, "JS listener did not fire");
                let aborted: bool = signal.get("aborted").expect("read aborted");
                assert!(aborted, "signal.aborted is false after abort()");
            })
            .await;
        });
    }

    #[test]
    fn host_abort_reason_is_dom_exception_abort_error() {
        // The native classes give plugins spec-correct reasons — pin that
        // (the old polyfill had no `reason` at all).
        let runtime = rt();
        runtime.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            ctx.async_with(async move |ctx| {
                install(&ctx).expect("install");
                let (abort, signal) = Abort::create(&ctx).expect("create");
                abort.cancel(&ctx).expect("cancel");
                let probe: Function<'_> = ctx
                    .eval("((sig) => sig.reason instanceof DOMException && sig.reason.name)")
                    .expect("eval probe");
                let name: String = probe.call((signal,)).expect("probe call");
                assert_eq!(name, "AbortError");
            })
            .await;
        });
    }

    #[test]
    fn token_from_js_signal_flips_on_js_abort() {
        let runtime = rt();
        runtime.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            ctx.async_with(async move |ctx| {
                install(&ctx).expect("install");
                let pair: Object<'_> = ctx
                    .eval(
                        r"(() => {
                    const c = new AbortController();
                    return { c, s: c.signal };
                })()",
                    )
                    .expect("eval pair");
                let signal: Object<'_> = pair.get("s").expect("get s");
                let token = token_from_js_signal(&ctx, &signal).expect("token");
                assert!(!token.is_cancelled());
                let controller: Object<'_> = pair.get("c").expect("get c");
                let abort_fn: Function<'_> = controller.get("abort").expect("abort fn");
                abort_fn.call::<_, ()>((This(controller.clone()),)).expect("call abort");
                assert!(token.is_cancelled(), "token should flip when JS calls abort()");
            })
            .await;
        });
    }

    #[test]
    fn signal_timeout_static_exists() {
        // AbortSignal.timeout comes from the sleep-tokio feature; a missing
        // sleep backend would compile but leave the static absent.
        let runtime = rt();
        runtime.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            ctx.async_with(async move |ctx| {
                install(&ctx).expect("install");
                let has: bool = ctx
                    .eval("typeof AbortSignal.timeout === 'function' && typeof AbortSignal.any === 'function'")
                    .expect("eval");
                assert!(has, "AbortSignal.timeout / .any statics missing");
            })
            .await;
        });
    }
}
