//! Host `console` global installed on each plugin's `QuickJS` context.
//!
//! ```js
//! console.log('hello', { count: 3 });
//! console.warn('about to retry');
//! ```
//!
//! Each call writes one line to the plugin's [`PluginLog`]. Arguments are
//! stringified the way every JS author already expects: primitives go through
//! `String(x)`, objects through `JSON.stringify` (with a `try`/`catch` so
//! circular structures degrade to `[unserializable]` instead of crashing the
//! plugin). Arguments are joined by single spaces.
//!
//! `console.{log,info}` map to `INFO`, `warn` → `WARN`, `error` → `ERROR`,
//! `debug` → `DEBUG`. Other `console.*` methods plugins may reach for are not
//! shimmed — adding them is a one-line table edit if a port needs it.

use std::sync::Arc;

use rquickjs::function::Rest;
use rquickjs::{Ctx, Function, Object, Result as JsResult, Value};

use crate::plugins::log::{LogLevel, PluginLog};

/// Install `console.{log,info,warn,error,debug}` on the context's global
/// object. Each method writes one [`PluginLog`] line.
///
/// # Errors
///
/// Propagates JS errors from constructing the host functions or assigning to
/// the global object.
pub fn install<'js>(ctx: &Ctx<'js>, log: &Arc<PluginLog>) -> JsResult<()> {
    let console = Object::new(ctx.clone())?;
    for (method, level) in [
        ("log", LogLevel::Info),
        ("info", LogLevel::Info),
        ("warn", LogLevel::Warn),
        ("error", LogLevel::Error),
        ("debug", LogLevel::Debug),
    ] {
        let log_for_fn = Arc::clone(log);
        let func = Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
            let message = format_args(&ctx, &args.0);
            log_for_fn.write(level, &message);
        })?;
        console.set(method, func)?;
    }
    ctx.globals().set("console", console)?;
    Ok(())
}

/// Format a list of console args, single-space-joined.
///
/// Each value is rendered the way a developer expects from a browser console:
///   * `null` / `undefined` print literally
///   * strings + numbers + booleans go through their JS coercion (`String(v)`)
///   * arrays + objects go through `JSON.stringify`; circular references
///     degrade to `[unserializable]` instead of crashing the plugin
fn format_args<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> String {
    let Some(stringify) = resolve_stringify(ctx) else {
        return String::from("[console: JSON.stringify unavailable]");
    };
    let mut parts = Vec::with_capacity(args.len());
    for value in args {
        parts.push(render_one(ctx, value, &stringify));
    }
    parts.join(" ")
}

fn resolve_stringify<'js>(ctx: &Ctx<'js>) -> Option<Function<'js>> {
    let json: Object<'js> = ctx.globals().get("JSON").ok()?;
    json.get::<_, Function<'js>>("stringify").ok()
}

fn render_one<'js>(ctx: &Ctx<'js>, value: &Value<'js>, stringify: &Function<'js>) -> String {
    if value.is_null() {
        return String::from("null");
    }
    if value.is_undefined() {
        return String::from("undefined");
    }
    if value.is_object() || value.is_array() {
        match stringify.call::<_, String>((value.clone(),)) {
            Ok(s) => return s,
            Err(_) => return String::from("[unserializable]"),
        }
    }
    // Primitives: coerce through the JS `String` constructor so we get the
    // same output `console.log` does (`String(42) === '42'`, `String(true)
    // === 'true'`, etc.). Falling back to a debug rendering on failure keeps
    // a misbehaving Symbol or BigInt from killing the log line.
    let string_ctor: Function<'js> = match ctx.globals().get("String") {
        Ok(f) => f,
        Err(_) => return format!("{value:?}"),
    };
    string_ctor
        .call::<_, String>((value.clone(),))
        .unwrap_or_else(|_| format!("{value:?}"))
}
