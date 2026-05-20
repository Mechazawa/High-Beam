//! Host implementation of the `highbeam:clipboard` module.
//!
//! Stage 4 surface:
//!
//! ```ts
//! import { read, write } from 'highbeam:clipboard';
//! await write('hi');
//! const text = await read();
//! ```
//!
//! Capability gating: the module loads if the plugin has *either*
//! `clipboard.read` or `clipboard.write`. Each function additionally guards
//! itself on its specific capability — calling `write()` from a plugin that
//! only declared `clipboard.read` throws a `CapabilityError`.
//!
//! The bound functions are constructed *outside* `ModuleDef::evaluate`
//! (which has no per-plugin state) and stashed on `globalThis` under reserved
//! names. `evaluate` re-exports them from the namespace. This lets the same
//! `ClipboardModule` symbol serve every plugin while still gating per-plugin
//! caps. The reserved global names are documented constants below.
//!
//! Each call constructs a fresh `arboard::Clipboard` — cheap on macOS, fine
//! on Linux. Avoiding a global keeps us from worrying about thread-affinity
//! issues on X11.

use rquickjs::function::Async;
use rquickjs::{Ctx, Function, Object, Result as JsResult, Value, module::ModuleDef};

/// Where [`install`] stashes the read/write callables so the module's
/// `evaluate()` can re-export them as `read` / `write`. Plugin code should
/// never reach for these directly.
const READ_GLOBAL: &str = "__highbeam_clipboard_read";
const WRITE_GLOBAL: &str = "__highbeam_clipboard_write";

/// Module definition registered against the `highbeam:clipboard` specifier.
///
/// State-less — per-plugin gating lives in the bound functions [`install`]
/// places on globalThis before module evaluation.
pub struct ClipboardModule;

impl ModuleDef for ClipboardModule {
    fn declare(decl: &rquickjs::module::Declarations<'_>) -> JsResult<()> {
        decl.declare("read")?;
        decl.declare("write")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> JsResult<()> {
        // Pull the bound functions off the global object. If the runtime
        // didn't install them, fall back to capability-error stubs so an
        // accidental import of the module without proper cap binding still
        // gives an actionable error rather than a `TypeError: undefined`.
        let globals = ctx.globals();
        let read_val: Value<'js> = globals
            .get(READ_GLOBAL)
            .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
        let write_val: Value<'js> = globals
            .get(WRITE_GLOBAL)
            .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));

        let read_fn = if let Some(f) = read_val.into_function() {
            f
        } else {
            // No host binding installed → return a cap-error thrower.
            Function::new(
                ctx.clone(),
                Async(
                    |ctx: Ctx<'js>| async move { Err::<(), _>(throw_cap(&ctx, "clipboard.read")) },
                ),
            )?
        };
        let write_fn = if let Some(f) = write_val.into_function() {
            f
        } else {
            Function::new(
                ctx.clone(),
                Async(|ctx: Ctx<'js>, _text: String| async move {
                    Err::<(), _>(throw_cap(&ctx, "clipboard.write"))
                }),
            )?
        };
        exports.export("read", read_fn)?;
        exports.export("write", write_fn)?;
        Ok(())
    }
}

/// Build the per-plugin bound `read` / `write` functions. Stashes them on
/// `globalThis` under [`READ_GLOBAL`] / [`WRITE_GLOBAL`] so the module's
/// `evaluate` can pick them up at import time.
///
/// Must be called *before* the plugin's entry module evaluates.
///
/// # Errors
///
/// Propagates JS errors from function construction or global assignment.
pub fn install<'js>(ctx: &Ctx<'js>, can_read: bool, can_write: bool) -> JsResult<()> {
    let read_fn = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>| async move {
            if !can_read {
                return Err(throw_cap(&ctx, "clipboard.read"));
            }
            let text = tokio::task::spawn_blocking(|| -> Result<String, String> {
                let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
                cb.get_text().map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| throw_io(&ctx, &e.to_string()))?
            .map_err(|e| throw_io(&ctx, &e))?;
            Ok::<String, rquickjs::Error>(text)
        }),
    )?;

    let write_fn = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, text: String| async move {
            if !can_write {
                return Err(throw_cap(&ctx, "clipboard.write"));
            }
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
                cb.set_text(text).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| throw_io(&ctx, &e.to_string()))?
            .map_err(|e| throw_io(&ctx, &e))?;
            Ok::<(), rquickjs::Error>(())
        }),
    )?;

    ctx.globals().set(READ_GLOBAL, read_fn)?;
    ctx.globals().set(WRITE_GLOBAL, write_fn)?;
    Ok(())
}

fn throw_cap(ctx: &Ctx<'_>, cap: &str) -> rquickjs::Error {
    let err = match Object::new(ctx.clone()) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let _ = err.set("name", "CapabilityError");
    let _ = err.set(
        "message",
        format!("missing capability: {cap} (declare it in manifest.json)"),
    );
    ctx.throw(err.into_value())
}

fn throw_io(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
    let err = match Object::new(ctx.clone()) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let _ = err.set("name", "ClipboardError");
    let _ = err.set("message", message.to_owned());
    ctx.throw(err.into_value())
}
