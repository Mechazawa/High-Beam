//! Host implementation of the `highbeam:actions` module.
//!
//! Two exports for Stage 3:
//!
//! ```js
//! import { openUrl, copy } from 'highbeam:actions';
//! openUrl('https://example.com'); // returns { kind: 'openUrl', url: '…' }
//! copy('hello');                  // returns { kind: 'copy',    text: '…' }
//! ```
//!
//! Both builders return a plain JS object matching [`crate::plugins::result::Action`]'s
//! wire shape, so the host can deserialize via `serde_json` without bespoke
//! glue (see `runtime.rs`).
//!
//! Stage 4 will add `exec` and `reveal`.

use rquickjs::{Ctx, Object, Result, module::ModuleDef};

/// Module definition registered against the `highbeam:actions` import specifier.
pub struct ActionsModule;

impl ModuleDef for ActionsModule {
    fn declare(decl: &rquickjs::module::Declarations<'_>) -> Result<()> {
        decl.declare("openUrl")?;
        decl.declare("copy")?;
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
        exports.export("openUrl", open_url)?;
        exports.export("copy", copy)?;
        Ok(())
    }
}
