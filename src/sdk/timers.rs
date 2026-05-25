//! Tiny `setTimeout` polyfill — bare minimum so plugins can write
//! `await new Promise(r => setTimeout(r, 250))`. Raw `QuickJS` doesn't ship
//! this; we drive it via `tokio::time::sleep`.
//!
//! `clearTimeout`/`clearInterval` are no-ops — we don't track ids; plugins
//! are expected to `await setTimeout` directly.

use rquickjs::function::Async;
use rquickjs::{Ctx, Error as JsError, Function};

use crate::logging::LogErr;

/// Install the timer polyfills on the global object. Idempotent.
///
/// # Errors
///
/// Propagates JS errors from constructing the host functions.
pub fn install<'js>(ctx: &Ctx<'js>) -> Result<(), JsError> {
    let set_timeout = Function::new(
        ctx.clone(),
        Async(|cb: Function<'js>, delay_ms: f64| async move {
            // NaN/negatives clamp to 0; valid f64 up to u64::MAX ms passes
            // through — we don't mask plugin bugs with an obscure cap.
            let delay = if delay_ms.is_finite() && delay_ms > 0.0 {
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let ms = delay_ms as u64;
                std::time::Duration::from_millis(ms)
            } else {
                std::time::Duration::from_millis(0)
            };
            tokio::time::sleep(delay).await;
            // Callback exceptions have no try/catch site to surface to —
            // we route them through tracing at WARN so plugin bugs aren't
            // completely invisible.
            cb.call::<_, ()>(()).log_warn("setTimeout: callback threw");
            Ok::<i32, JsError>(0)
        }),
    )?;
    ctx.globals().set("setTimeout", set_timeout)?;

    let noop = Function::new(ctx.clone(), || {})?;
    ctx.globals().set("clearTimeout", noop.clone())?;
    ctx.globals().set("clearInterval", noop)?;
    Ok(())
}
