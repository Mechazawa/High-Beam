//! Tiny `setTimeout` polyfill — the bare minimum so plugins can write
//!
//! ```js
//! await new Promise(r => setTimeout(r, 250));
//! ```
//!
//! …the canonical "wait 250ms" idiom. Raw `QuickJS` doesn't ship
//! `setTimeout`/`setInterval` because they're browser/Node concepts; we
//! provide a host-bound version that drives `tokio::time::sleep`.
//!
//! `clearTimeout` / `clearInterval` are accepted as no-ops — we don't track
//! ids. Plugins are expected to `await` `setTimeout` directly rather than
//! juggle handles.

use rquickjs::function::Async;
use rquickjs::{Ctx, Error as JsError, Function};

/// Install the timer polyfills on the global object. Idempotent if the host
/// re-runs it.
///
/// # Errors
///
/// Propagates JS errors from constructing the host functions.
pub fn install<'js>(ctx: &Ctx<'js>) -> Result<(), JsError> {
    let set_timeout = Function::new(
        ctx.clone(),
        Async(|cb: Function<'js>, delay_ms: f64| async move {
            // f64 -> u64 with NaN/negatives clamped to zero. We deliberately
            // accept up to u64::MAX ms (well past sane usage) so we don't
            // mask a plugin bug with an obscure cap.
            let delay = if delay_ms.is_finite() && delay_ms > 0.0 {
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let ms = delay_ms as u64;
                std::time::Duration::from_millis(ms)
            } else {
                std::time::Duration::from_millis(0)
            };
            tokio::time::sleep(delay).await;
            // Best effort fire — swallow callback exceptions because we
            // can't surface them to a non-existent `try`/`catch` site.
            let _ = cb.call::<_, ()>(());
            Ok::<i32, JsError>(0)
        }),
    )?;
    ctx.globals().set("setTimeout", set_timeout)?;

    let noop = Function::new(ctx.clone(), || {})?;
    ctx.globals().set("clearTimeout", noop.clone())?;
    ctx.globals().set("clearInterval", noop)?;
    Ok(())
}
