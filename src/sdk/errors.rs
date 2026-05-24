//! Shared helpers for throwing structured JS errors from `highbeam:*`
//! modules. Plugin code branches on `err.name` (`'FsError'`,
//! `'CapabilityError'`, `'AbortError'`, etc.).

use rquickjs::function::{Async, Rest};
use rquickjs::{Ctx, Function, Object, Result as JsResult, Value};

const CAPABILITY_ERROR: &str = "CapabilityError";
const ABORT_ERROR: &str = "AbortError";

pub(crate) fn throw_cap(ctx: &Ctx<'_>, cap: &str) -> rquickjs::Error {
    throw_named(
        ctx,
        CAPABILITY_ERROR,
        &format!("missing capability: {cap} (declare it in manifest.json)"),
    )
}

pub(crate) fn throw_abort(ctx: &Ctx<'_>) -> rquickjs::Error {
    throw_named(ctx, ABORT_ERROR, "operation aborted")
}

pub(crate) fn throw_named(ctx: &Ctx<'_>, name: &'static str, message: &str) -> rquickjs::Error {
    let err = match Object::new(ctx.clone()) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let _ = err.set("name", name);
    let _ = err.set("message", message.to_owned());
    ctx.throw(err.into_value())
}

/// Build a JS function that throws a `CapabilityError` for `cap` on every
/// call. Used by `ModuleDef::evaluate` fallbacks when the runtime didn't
/// install a per-plugin binding (no host install ran, or the cap wasn't
/// declared). Accepts any arg list via `Rest<Value<'js>>` so the same stub
/// covers every export shape — the function never returns Ok, so the
/// declared return type is irrelevant once it throws.
pub(crate) fn cap_error_thrower<'js>(ctx: &Ctx<'js>, cap: &'static str) -> JsResult<Function<'js>> {
    Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, _args: Rest<Value<'js>>| async move { Err::<(), _>(throw_cap(&ctx, cap)) }),
    )
}
