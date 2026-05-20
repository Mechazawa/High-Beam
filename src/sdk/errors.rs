//! Shared host-side helpers for throwing structured JS errors out of the
//! `highbeam:*` modules.
//!
//! Each module's error variants are distinguished by the `name` field on the
//! JS error object so plugin code can branch on it (`err.name === 'FsError'`).
//! `CapabilityError` and `AbortError` are shared across modules; per-module
//! errors (`FsError`, `IconError`, etc.) go through [`throw_named`].

use rquickjs::{Ctx, Object};

const CAPABILITY_ERROR: &str = "CapabilityError";
const ABORT_ERROR: &str = "AbortError";

/// Throw a `CapabilityError` describing the missing capability declaration.
pub(crate) fn throw_cap(ctx: &Ctx<'_>, cap: &str) -> rquickjs::Error {
    throw_named(
        ctx,
        CAPABILITY_ERROR,
        &format!("missing capability: {cap} (declare it in manifest.json)"),
    )
}

/// Throw an `AbortError` — used when an `AbortSignal` fires mid-operation.
pub(crate) fn throw_abort(ctx: &Ctx<'_>) -> rquickjs::Error {
    throw_named(ctx, ABORT_ERROR, "operation aborted")
}

/// Throw a module-specific error (`FsError`, `IconError`, `HttpError`, etc.).
pub(crate) fn throw_named(ctx: &Ctx<'_>, name: &'static str, message: &str) -> rquickjs::Error {
    let err = match Object::new(ctx.clone()) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let _ = err.set("name", name);
    let _ = err.set("message", message.to_owned());
    ctx.throw(err.into_value())
}
