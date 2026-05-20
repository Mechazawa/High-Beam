//! Host implementation of the `highbeam:clipboard` module.
//!
//! Module loads if the plugin has either `clipboard.read` or
//! `clipboard.write`; each function guards itself on its specific cap.
//!
//! Per-plugin bindings live on `globalThis` under reserved names because
//! `ModuleDef::evaluate` has no per-plugin state — one `ClipboardModule`
//! symbol serves every plugin via late re-export. A fresh
//! `arboard::Clipboard` per call sidesteps X11 thread-affinity issues.

use rquickjs::function::Async;
use rquickjs::{Ctx, Function, Result as JsResult, Value, module::ModuleDef};

use crate::sdk::errors::{throw_cap, throw_named};

const READ_GLOBAL: &str = "__highbeam_clipboard_read";
const WRITE_GLOBAL: &str = "__highbeam_clipboard_write";

/// Stateless module — per-plugin gating lives in the bound functions
/// [`install`] places on globalThis before module evaluation.
pub struct ClipboardModule;

impl ModuleDef for ClipboardModule {
    fn declare(decl: &rquickjs::module::Declarations<'_>) -> JsResult<()> {
        decl.declare("read")?;
        decl.declare("write")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> JsResult<()> {
        // If the runtime didn't install bound functions, fall back to
        // CapabilityError stubs so plugin authors get an actionable error
        // rather than `TypeError: undefined`.
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

/// Build per-plugin `read`/`write` and stash them on globalThis under
/// [`READ_GLOBAL`]/[`WRITE_GLOBAL`]. Must run BEFORE the plugin's entry
/// module evaluates.
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

fn throw_io(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
    throw_named(ctx, "ClipboardError", message)
}
