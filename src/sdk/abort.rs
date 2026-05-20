//! Web-spec `AbortController` / `AbortSignal` polyfill for plugins.
//!
//! Plugins receive a real `AbortSignal`-shaped JS object as the second arg to
//! `query(input, signal)`. The host can call [`Abort::cancel`] from Rust to
//! cascade cancellation through host I/O (`http.get(url, { signal })`) and
//! through any JS-side `signal.aborted` / `addEventListener('abort', …)`
//! listeners the plugin registered.
//!
//! Implementation strategy: the JS-side polyfill is pure JS (see
//! [`install_global_controller`]) so we don't pay rquickjs binding costs for
//! every property access. The host wraps that with a tiny Rust handle
//! ([`Abort`]) that owns a [`tokio_util::sync::CancellationToken`]; calling
//! [`Abort::cancel`] from Rust runs the JS-side `controller.abort()` so JS
//! listeners fire, and flips the token so in-flight reqwest futures wake up.
//!
//! The cap-gated `highbeam:http` module accepts a signal from the plugin and
//! turns it back into a token via [`token_from_js_signal`] — it registers a
//! JS `addEventListener('abort', …)` that flips a brand-new token, then races
//! the request future against that token.
//!
//! `AbortSignal.timeout(ms)` and `AbortSignal.any([signals])` are explicitly
//! *not* in v1 — plugin authors who want those write the JS themselves.

use rquickjs::function::This;
use rquickjs::{CatchResultExt, Ctx, Error as JsError, Function, Object};
use tokio_util::sync::CancellationToken;

/// AbortController/AbortSignal polyfill. Idempotent; safe to evaluate twice
/// in the same context.
const ABORT_CONTROLLER_JS: &str = include_str!("js/abort_controller.js");

/// Expression-shaped helper: `(() => { … })`. Used by [`Abort::create`].
const ABORT_CREATE_JS: &str = include_str!("js/abort_create.js");

/// Expression-shaped helper: `((id) => { … })`. Used by [`Abort::cancel`].
const ABORT_FIRE_JS: &str = include_str!("js/abort_fire.js");

/// Host handle for one signal the host gives a plugin.
///
/// The handle owns:
///   * a [`CancellationToken`] mirrored by the JS controller's abort state
///   * an opaque JS-side identifier so [`Abort::cancel`] can flip the matching
///     controller's `abort()` from Rust
///
/// The controller (and its `.signal`) are returned by [`Abort::create`]; pass
/// the signal object as a JS argument to whatever plugin code needs it.
pub struct Abort {
    token: CancellationToken,
    controller_id: i64,
}

impl Abort {
    /// Build a fresh controller+signal pair inside the active JS context.
    ///
    /// Returns the Rust handle and the JS-side `controller.signal` object.
    /// Side effect: registers the controller in the global
    /// `__highbeam_abort_registry` so [`Abort::cancel`] can look it up later.
    ///
    /// # Errors
    ///
    /// Propagates JS errors from the bootstrap script.
    pub fn create<'js>(ctx: &Ctx<'js>) -> Result<(Self, Object<'js>), JsError> {
        // Lazy-install the polyfill the first time we need it. Multiple calls
        // are idempotent because the JS bootstrap guards on `globalThis.AbortController`.
        install_global_controller(ctx)?;

        // Hand the registry a fresh controller, get back the id + signal.
        let make: Function<'js> = ctx.eval(ABORT_CREATE_JS)?;
        let pair: Object<'js> = make.call(())?;
        let controller_id: i64 = pair.get("id")?;
        let signal: Object<'js> = pair.get("signal")?;
        let token = CancellationToken::new();

        Ok((
            Self {
                token,
                controller_id,
            },
            signal,
        ))
    }

    /// The Rust-side token. Host I/O futures race this against their work.
    #[must_use]
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// Has this signal been aborted?
    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Flip the abort flag. Fires JS-side listeners *and* the Rust-side token.
    ///
    /// Idempotent. Must be called from inside an `async_with!` block so the
    /// JS controller can run its listeners.
    ///
    /// # Errors
    ///
    /// Propagates errors from invoking the JS-side controller's `abort()`.
    pub fn cancel(&self, ctx: &Ctx<'_>) -> Result<(), JsError> {
        if self.token.is_cancelled() {
            return Ok(());
        }
        self.token.cancel();
        let fire: Function<'_> = ctx.eval(ABORT_FIRE_JS)?;
        fire.call::<_, ()>((self.controller_id,))
            .catch(ctx)
            .map_err(|err| {
                JsError::new_loading_message("abort", format!("fire listeners: {err}"))
            })?;
        Ok(())
    }
}

/// Build the JS-side `AbortController` polyfill and the controller registry.
/// Idempotent — multiple calls within the same context are no-ops.
///
/// # Errors
///
/// Propagates JS errors from the bootstrap eval.
pub fn install_global_controller(ctx: &Ctx<'_>) -> Result<(), JsError> {
    ctx.eval::<(), _>(ABORT_CONTROLLER_JS)?;
    Ok(())
}

/// Build a Rust [`CancellationToken`] that flips when the given JS-side
/// `AbortSignal` aborts. Used by host I/O modules (`highbeam:http`) to weave a
/// plugin-supplied signal into their cancellation logic.
///
/// If the signal is already aborted, returns a pre-cancelled token.
///
/// # Errors
///
/// Propagates JS errors from reading the `aborted` property or attaching the
/// listener.
pub fn token_from_js_signal<'js>(
    ctx: &Ctx<'js>,
    signal: &Object<'js>,
) -> Result<CancellationToken, JsError> {
    let aborted: bool = signal.get::<_, bool>("aborted").unwrap_or(false);
    let token = CancellationToken::new();
    if aborted {
        token.cancel();
        return Ok(token);
    }

    // Make a tiny zero-arg function that flips the token. The token clone is
    // captured by the function for as long as JS holds the listener.
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
    use rquickjs::{AsyncContext, AsyncRuntime, async_with};

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
            async_with!(ctx => |ctx| {
                install_global_controller(&ctx).expect("first");
                install_global_controller(&ctx).expect("second");
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
            async_with!(ctx => |ctx| {
                install_global_controller(&ctx).expect("install");
                let fired: bool = ctx.eval(r"
                    (() => {
                        const c = new AbortController();
                        let fired = false;
                        c.signal.addEventListener('abort', () => { fired = true; });
                        c.abort();
                        return fired && c.signal.aborted;
                    })()
                ").expect("eval");
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
            async_with!(ctx => |ctx| {
                let (abort, signal) = Abort::create(&ctx).expect("create");
                // Attach a JS listener on the signal.
                let attach: Function<'_> = ctx.eval(r"((sig) => {
                    sig._observed = false;
                    sig.addEventListener('abort', () => { sig._observed = true; });
                })").expect("eval attach");
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
    fn token_from_js_signal_flips_on_js_abort() {
        let runtime = rt();
        runtime.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            async_with!(ctx => |ctx| {
                install_global_controller(&ctx).expect("install");
                let pair: Object<'_> = ctx.eval(r"(() => {
                    const c = new AbortController();
                    return { c, s: c.signal };
                })()").expect("eval pair");
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
}
