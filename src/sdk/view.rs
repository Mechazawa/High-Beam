//! Host implementation of the `highbeam:view` module.
//!
//! Each factory returns a plain JS object matching the wire shape
//! [`crate::ui`] (later stages) renders. The factories are pure data
//! constructors: every field on the opts object is copied through, with
//! a `kind` discriminator tacked on so `serde` can deserialise the tree.
//!
//! Stage 1 ships only the shape. Reactivity, event dispatch, and the
//! render pipeline arrive in later stages — see `docs/views.md` for the
//! full design.

use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{Ctx, Function, Object, Result as JsResult, Value};

pub struct ViewModule;

impl ModuleDef for ViewModule {
    fn declare(decl: &Declarations<'_>) -> JsResult<()> {
        decl.declare("Stack")?;
        decl.declare("Divider")?;
        decl.declare("Heading")?;
        decl.declare("Text")?;
        decl.declare("Spinner")?;
        decl.declare("ProgressBar")?;
        decl.declare("Button")?;
        decl.declare("Input")?;
        decl.declare("TextArea")?;
        decl.declare("Image")?;
        decl.declare("Row")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> JsResult<()> {
        exports.export("Stack", block_factory(ctx, "stack")?)?;
        exports.export("Divider", block_factory(ctx, "divider")?)?;
        exports.export("Heading", block_factory(ctx, "heading")?)?;
        exports.export("Text", block_factory(ctx, "text")?)?;
        exports.export("Spinner", block_factory(ctx, "spinner")?)?;
        exports.export("ProgressBar", block_factory(ctx, "progress")?)?;
        exports.export("Button", block_factory(ctx, "button")?)?;
        exports.export("Input", block_factory(ctx, "input")?)?;
        exports.export("TextArea", block_factory(ctx, "textarea")?)?;
        exports.export("Image", block_factory(ctx, "image")?)?;
        exports.export("Row", block_factory(ctx, "row")?)?;
        Ok(())
    }
}

/// Build a single block factory bound to `kind`. Each factory accepts an
/// optional opts object, copies its fields into a fresh object, and tags
/// the result with `kind` — set last so it overrides any caller-supplied
/// `kind` field, which would otherwise let a malformed call masquerade as
/// a different block type.
fn block_factory<'js>(ctx: &Ctx<'js>, kind: &'static str) -> JsResult<Function<'js>> {
    Function::new(ctx.clone(), move |ctx: Ctx<'js>, opts: Value<'js>| {
        make_block(&ctx, kind, opts)
    })
}

fn make_block<'js>(ctx: &Ctx<'js>, kind: &str, opts: Value<'js>) -> JsResult<Object<'js>> {
    let obj = Object::new(ctx.clone())?;

    if let Some(opts) = opts.into_object() {
        for entry in opts.props::<String, Value<'js>>() {
            let (key, value) = entry?;
            obj.set(key.as_str(), value)?;
        }
    }

    obj.set("kind", kind)?;
    Ok(obj)
}
