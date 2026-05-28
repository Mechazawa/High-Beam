//! Host implementation of the `highbeam:actions` module.
//!
//! Each builder returns a plain JS object matching
//! [`crate::plugins::result::Action`]'s wire shape so `serde_json` can
//! deserialise the yielded result directly.
//!
//! `exec` here is the action builder (no capability required). Live
//! subprocess execution lives in `highbeam:system.exec`.

use rquickjs::function::Opt;
use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{Ctx, Function, Object, Result as JsResult, Value};

/// `globalThis` slot the SDK stashes the view registry on. Per `QuickJS`
/// context (so per plugin), so handles are unique within a plugin and the
/// host pairs `(plugin_name, handle)` to look a frame back up.
pub(crate) const VIEW_REGISTRY_GLOBAL: &str = "__highbeam_view_registry";

pub struct ActionsModule;

impl ModuleDef for ActionsModule {
    fn declare(decl: &Declarations<'_>) -> JsResult<()> {
        decl.declare("openUrl")?;
        decl.declare("copy")?;
        decl.declare("exec")?;
        decl.declare("reveal")?;
        decl.declare("showView")?;
        decl.declare("closeView")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> JsResult<()> {
        let open_url = Function::new(ctx.clone(), |ctx: Ctx<'js>, url: String| {
            let obj = Object::new(ctx)?;
            obj.set("kind", "openUrl")?;
            obj.set("url", url)?;
            Ok::<_, rquickjs::Error>(obj)
        })?;
        let copy = Function::new(ctx.clone(), |ctx: Ctx<'js>, text: String| {
            let obj = Object::new(ctx)?;
            obj.set("kind", "copy")?;
            obj.set("text", text)?;
            Ok::<_, rquickjs::Error>(obj)
        })?;
        let exec = Function::new(ctx.clone(), |ctx: Ctx<'js>, cmd: String, args: Value<'js>| {
            let obj = Object::new(ctx.clone())?;
            obj.set("kind", "exec")?;
            obj.set("cmd", cmd)?;

            // Treat undefined as `[]`; pass arrays through and let serde
            // reject malformed shapes at deserialize time.
            if args.is_undefined() || args.is_null() {
                let empty = rquickjs::Array::new(ctx)?;
                obj.set("args", empty)?;
            } else {
                obj.set("args", args)?;
            }
            Ok::<_, rquickjs::Error>(obj)
        })?;
        let reveal = Function::new(ctx.clone(), |ctx: Ctx<'js>, path: String| {
            let obj = Object::new(ctx)?;
            obj.set("kind", "reveal")?;
            obj.set("path", path)?;
            Ok::<_, rquickjs::Error>(obj)
        })?;
        let show_view = Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, view: Value<'js>, props: Opt<Value<'js>>, opts: Opt<Value<'js>>| {
                let reset = opts
                    .0
                    .as_ref()
                    .and_then(rquickjs::Value::as_object)
                    .and_then(|o| o.get::<_, bool>("reset").ok())
                    .unwrap_or(false);
                build_show_view(&ctx, view, props.0, reset)
            },
        )?;
        let close_view = build_close_view(ctx)?;

        exports.export("openUrl", open_url)?;
        exports.export("copy", copy)?;
        exports.export("exec", exec)?;
        exports.export("reveal", reveal)?;
        exports.export("showView", show_view)?;
        exports.export("closeView", close_view)?;
        Ok(())
    }
}

/// Build a `showView` action object. The view literal is stashed in a
/// per-context registry on `globalThis` and the action carries only the
/// freshly-minted `handle` — opaque to the host but enough to ask the
/// plugin's `QuickJS` context to render or close that specific view.
///
/// `props` defaults to `{}` when undefined; `opts.reset` defaults to
/// `false`.
fn build_show_view<'js>(
    ctx: &Ctx<'js>,
    view: Value<'js>,
    props: Option<Value<'js>>,
    reset: bool,
) -> JsResult<Object<'js>> {
    let handle = register_view(ctx, view)?;

    let obj = Object::new(ctx.clone())?;
    obj.set("kind", "showView")?;
    obj.set("handle", handle)?;

    let props_value = match props {
        Some(p) if !p.is_undefined() && !p.is_null() => p,
        _ => Object::new(ctx.clone())?.into_value(),
    };
    obj.set("props", props_value)?;
    obj.set("reset", reset)?;

    Ok(obj)
}

/// `closeView` is exported as a frozen constant — `onClick: closeView`
/// (no call) is the documented usage, so the action object can be shared.
fn build_close_view<'js>(ctx: &Ctx<'js>) -> JsResult<Object<'js>> {
    let obj = Object::new(ctx.clone())?;
    obj.set("kind", "closeView")?;
    Ok(obj)
}

/// Mint a fresh handle for `view`, stash the view on the per-context
/// registry, and return the handle. Handles start at `1` (zero is
/// reserved as a sentinel for "not a view" should the host ever need to
/// distinguish). Returns the handle as a `u32`-safe number — well within
/// the JS Number safe-integer range and matches the host enum's `u64`
/// field on the receive side.
fn register_view<'js>(ctx: &Ctx<'js>, view: Value<'js>) -> JsResult<i64> {
    let registry = ensure_registry(ctx)?;
    let next: i64 = registry.get("nextId").unwrap_or(1);
    registry.set("nextId", next + 1)?;

    let by_handle: Object<'js> = registry.get("byHandle")?;
    by_handle.set(next.to_string(), view)?;

    Ok(next)
}

/// Lazily create the per-context view registry on `globalThis`. Shape:
/// `{ nextId: <i64>, byHandle: { [handle: string]: ViewDef } }`. Handles
/// are stringified for the `byHandle` lookup because JS object keys are
/// strings anyway — coercing them once at insert-time keeps both sides
/// (host-driven lookups in later stages, debug inspection from the JS
/// console) symmetrical.
fn ensure_registry<'js>(ctx: &Ctx<'js>) -> JsResult<Object<'js>> {
    let globals = ctx.globals();

    if let Ok(existing) = globals.get::<_, Object<'js>>(VIEW_REGISTRY_GLOBAL) {
        return Ok(existing);
    }
    let registry = Object::new(ctx.clone())?;
    registry.set("nextId", 1i64)?;
    registry.set("byHandle", Object::new(ctx.clone())?)?;
    globals.set(VIEW_REGISTRY_GLOBAL, registry.clone())?;
    Ok(registry)
}
