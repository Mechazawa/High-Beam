//! Host implementation of the `highbeam:actions` module.
//!
//! ```js
//! import { openUrl, copy, exec, reveal } from 'highbeam:actions';
//! openUrl('https://example.com');         // { kind: 'openUrl', url: '…' }
//! copy('hello');                          // { kind: 'copy',    text: '…' }
//! exec('/usr/bin/say', ['hello']);        // { kind: 'exec',    cmd, args }
//! reveal('/Users/me/Downloads/file.pdf'); // { kind: 'reveal',  path: '…' }
//! ```
//!
//! All builders return a plain JS object matching [`crate::plugins::result::Action`]'s
//! wire shape so `serde_json` can deserialise the yielded result directly.
//!
//! Note: `exec` here is the *action builder*. Live subprocess execution is
//! `highbeam:system.exec` (capability `system.exec`); returning an exec
//! action from `query()` does NOT require that capability.

use rquickjs::{Ctx, Object, Result, Value, module::ModuleDef};

/// Module definition registered against the `highbeam:actions` import specifier.
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
                // Coerce `args` to an array — if the plugin passed undefined,
                // treat it as an empty arg vector. Otherwise pass through and
                // let serde reject malformed shapes at deserialize time.
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
