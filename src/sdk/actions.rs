//! Host implementation of the `highbeam:actions` module.
//!
//! Each builder returns a plain JS object matching
//! [`crate::plugins::result::Action`]'s wire shape so `serde_json` can
//! deserialise the yielded result directly.
//!
//! `exec` here is the action builder (no capability required). Live
//! subprocess execution lives in `highbeam:system.exec`.

use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{Ctx, Function, Object, Result as JsResult, Value};

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
            |ctx: Ctx<'js>, view: Value<'js>, props: Value<'js>, opts: Value<'js>| {
                build_show_view(&ctx, view, props, &opts)
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

/// Build a `showView` action object. `props` defaults to `{}` (matches the
/// vitest stub); `opts.reset` defaults to `false`. Later stages swap the
/// inlined `view` for a per-plugin handle id; Stage 1 keeps the view object
/// in-place so the wire shape is pinned without minting a registry yet.
fn build_show_view<'js>(
    ctx: &Ctx<'js>,
    view: Value<'js>,
    props: Value<'js>,
    opts: &Value<'js>,
) -> JsResult<Object<'js>> {
    let obj = Object::new(ctx.clone())?;
    obj.set("kind", "showView")?;
    obj.set("view", view)?;

    let props_value = if props.is_undefined() || props.is_null() {
        Object::new(ctx.clone())?.into_value()
    } else {
        props
    };
    obj.set("props", props_value)?;

    let reset = opts
        .as_object()
        .and_then(|o| o.get::<_, bool>("reset").ok())
        .unwrap_or(false);
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
