//! Web-spec `AbortController`/`AbortSignal` polyfill for plugins.
//!
//! Plugins receive a real `AbortSignal`-shaped JS object as the second arg
//! to `query(input, signal)`. [`Abort::cancel`] fires both the JS-side
//! listeners and the Rust [`CancellationToken`] that gates in-flight I/O.
//!
//! Pure-JS polyfill (see [`install_global_controller`]) avoids per-property
//! rquickjs binding overhead. The host wraps it with [`Abort`] which holds
//! a [`tokio_util::sync::CancellationToken`]. `highbeam:http` accepts a
//! signal from plugin code and turns it back into a token via
//! [`token_from_js_signal`].
//!
//! `AbortSignal.timeout(ms)` / `AbortSignal.any([…])` are post-v1.

use rquickjs::function::This;
use rquickjs::{CatchResultExt, Ctx, Error as JsError, Function, Object};
use tokio_util::sync::CancellationToken;

/// AbortController/AbortSignal polyfill. Idempotent.
const ABORT_CONTROLLER_JS: &str = include_str!("js/abort_controller.js");

const ABORT_CREATE_JS: &str = include_str!("js/abort_create.js");

const ABORT_FIRE_JS: &str = include_str!("js/abort_fire.js");

const ABORT_RELEASE_JS: &str = include_str!("js/abort_release.js");

/// Host handle for one signal. Owns a [`CancellationToken`] mirrored by the
/// JS controller's abort state plus an opaque controller id used by
/// [`Abort::cancel`] to flip the JS side from Rust.
///
/// The JS-side controller stays in the global registry until either
/// [`Abort::cancel`] (cancel path) or [`Abort::release`] (success path) is
/// called. Failing to call one of them leaves the registry entry alive for
/// the remainder of the plugin context's lifetime — a slow leak, not UB,
/// but worth avoiding.
pub struct Abort {
    token: CancellationToken,
    controller_id: i64,
}

impl Abort {
    /// Build a fresh controller+signal pair inside the active JS context.
    /// Returns the Rust handle and the JS-side `controller.signal` object.
    ///
    /// # Errors
    ///
    /// Propagates JS errors from the bootstrap script.
    pub fn create<'js>(ctx: &Ctx<'js>) -> Result<(Self, Object<'js>), JsError> {
        install_global_controller(ctx)?;

        let make: Function<'js> = ctx.eval(ABORT_CREATE_JS)?;
        let pair: Object<'js> = make.call(())?;
        let controller_id: i64 = pair.get("id")?;
        let signal: Object<'js> = pair.get("signal")?;
        let token = CancellationToken::new();

        Ok((Self { token, controller_id }, signal))
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

    /// Flip the abort flag and drop the JS registry entry. Fires JS-side
    /// listeners AND the Rust-side token. Idempotent. Must be called from
    /// inside an `async_with!` block.
    ///
    /// # Errors
    ///
    /// Propagates errors from invoking the JS-side controller's `abort()`.
    pub fn cancel(&self, ctx: &Ctx<'_>) -> Result<(), JsError> {
        if self.token.is_cancelled() {
            // Cancellation is idempotent, but `release` isn't — only call it
            // on the first cancel so a repeat call doesn't double-evaluate
            // the JS snippet.
            return Ok(());
        }
        self.token.cancel();
        let fire: Function<'_> = ctx.eval(ABORT_FIRE_JS)?;
        fire.call::<_, ()>((self.controller_id,))
            .catch(ctx)
            .map_err(|err| JsError::new_loading_message("abort", format!("fire listeners: {err}")))?;
        self.release(ctx)?;
        Ok(())
    }

    /// Drop the JS registry entry without firing listeners. The success-path
    /// counterpart to [`Self::cancel`]: when work completes naturally, the
    /// plugin doesn't need to observe an abort, but we still want the
    /// controller out of the registry so memory doesn't grow with query
    /// count over the plugin's lifetime.
    ///
    /// # Errors
    ///
    /// Propagates errors from the JS-side `delete`.
    pub fn release(&self, ctx: &Ctx<'_>) -> Result<(), JsError> {
        let release: Function<'_> = ctx.eval(ABORT_RELEASE_JS)?;
        release
            .call::<_, ()>((self.controller_id,))
            .catch(ctx)
            .map_err(|err| JsError::new_loading_message("abort", format!("release controller: {err}")))?;
        Ok(())
    }
}

/// Build the JS-side `AbortController` polyfill and the controller registry.
/// Idempotent.
///
/// # Errors
///
/// Propagates JS errors from the bootstrap eval.
pub fn install_global_controller(ctx: &Ctx<'_>) -> Result<(), JsError> {
    ctx.eval::<(), _>(ABORT_CONTROLLER_JS)?;
    Ok(())
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
    fn release_drops_controller_from_registry() {
        // Memory hygiene: every Abort::create registers a controller in the
        // JS-side global map. Without release (or cancel, which also
        // releases), the registry grows unboundedly over a plugin context's
        // lifetime. Build two abort handles, release one, assert the
        // registry contains only the other.
        let runtime = rt();
        runtime.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            async_with!(ctx => |ctx| {
                let (keep, _signal_keep) = Abort::create(&ctx).expect("create keep");
                let (drop_, _signal_drop) = Abort::create(&ctx).expect("create drop");
                let size_before: i32 = ctx.eval("globalThis.__highbeam_abort_registry.controllers.size").expect("eval");
                assert_eq!(size_before, 2);
                drop_.release(&ctx).expect("release");
                let size_after: i32 = ctx.eval("globalThis.__highbeam_abort_registry.controllers.size").expect("eval");
                assert_eq!(size_after, 1, "release should drop one entry");
                // `keep` is intentionally not released — without that, the
                // registry-shrink assertion above wouldn't discriminate
                // between "release worked" and "create never ran".
                drop(keep);
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
