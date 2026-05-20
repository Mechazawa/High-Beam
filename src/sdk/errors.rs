//! Shared helpers for throwing structured JS errors from `highbeam:*`
//! modules. Plugin code branches on `err.name` (`'FsError'`,
//! `'CapabilityError'`, `'AbortError'`, etc.).

use rquickjs::{Ctx, Object};

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
