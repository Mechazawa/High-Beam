//! Host `console` global installed on each plugin's `QuickJS` context.
//!
//! Each call writes one line to the plugin's [`PluginLog`]. Primitives go
//! through `String(x)`, objects through `JSON.stringify` (circular structures
//! degrade to `[unserializable]`); args join with single spaces.
//!
//! `log`/`info` → INFO, `warn` → WARN, `error` → ERROR, `debug` → DEBUG.

use std::sync::Arc;

use rquickjs::function::Rest;
use rquickjs::{Ctx, Function, Object, Result as JsResult, Value};

use crate::plugins::log::{LogLevel, PluginLog};

/// Install `console.{log,info,warn,error,debug}` on the context's global.
///
/// # Errors
///
/// Propagates JS errors from constructing the host functions or assigning to
/// the global.
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

fn format_args<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> String {
    let Some(stringify) = resolve_stringify(ctx) else {
        return String::from("[console: JSON.stringify unavailable]");
    };

    args.iter()
        .map(|value| render_one(ctx, value, &stringify))
        .collect::<Vec<_>>()
        .join(" ")
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
        return stringify
            .call::<_, String>((value.clone(),))
            .unwrap_or_else(|_| String::from("[unserializable]"));
    }
    // Primitives via `String(value)` so a misbehaving Symbol/BigInt fails
    // back to a debug rendering rather than killing the log line.
    let Ok(string_ctor) = ctx.globals().get::<_, Function<'js>>("String") else {
        return format!("{value:?}");
    };
    string_ctor
        .call::<_, String>((value.clone(),))
        .unwrap_or_else(|_| format!("{value:?}"))
}
