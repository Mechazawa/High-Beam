//! Host implementation of the `highbeam:actions` module.
//!
//! Each builder returns a plain JS object matching
//! [`crate::plugins::result::Action`]'s wire shape so `serde_json` can
//! deserialise the yielded result directly.
//!
//! `exec` here is the action builder (no capability required). Live
//! subprocess execution lives in `highbeam:system.exec`.

use rquickjs::{Ctx, Object, Result, Value, module::ModuleDef};

pub struct ActionsModule;

impl ModuleDef for ActionsModule {
    fn declare(decl: &rquickjs::module::Declarations<'_>) -> Result<()> {
        decl.declare("openUrl")?;
        decl.declare("copy")?;
        decl.declare("exec")?;
        decl.declare("reveal")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> Result<()> {
        let open_url = rquickjs::Function::new(ctx.clone(), |ctx: Ctx<'js>, url: String| {
            let obj = Object::new(ctx)?;
            obj.set("kind", "openUrl")?;
            obj.set("url", url)?;
            Ok::<_, rquickjs::Error>(obj)
        })?;
        let copy = rquickjs::Function::new(ctx.clone(), |ctx: Ctx<'js>, text: String| {
            let obj = Object::new(ctx)?;
            obj.set("kind", "copy")?;
            obj.set("text", text)?;
            Ok::<_, rquickjs::Error>(obj)
        })?;
        let exec = rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, cmd: String, args: Value<'js>| {
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
            },
        )?;
        let reveal = rquickjs::Function::new(ctx.clone(), |ctx: Ctx<'js>, path: String| {
            let obj = Object::new(ctx)?;
            obj.set("kind", "reveal")?;
            obj.set("path", path)?;
            Ok::<_, rquickjs::Error>(obj)
        })?;
        exports.export("openUrl", open_url)?;
        exports.export("copy", copy)?;
        exports.export("exec", exec)?;
        exports.export("reveal", reveal)?;
        Ok(())
    }
}
