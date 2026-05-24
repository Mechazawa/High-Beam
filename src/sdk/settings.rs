//! Host implementation of the `highbeam:settings` module.
//!
//! Lets a plugin read its OWN option values — the host scopes per plugin
//! internally so calling `get('limit')` returns the calling plugin's value,
//! never another plugin's. No capability gate (reading your own metadata is
//! free); plugins that didn't declare options see `undefined` for everything.
//!
//! Per-plugin values live on `globalThis` under a reserved slot (resolved by
//! the runtime at load time), the same trick used for clipboard / fs / etc.
//! The exported functions are stateless lookups against that bag.

use std::collections::HashMap;
use std::hash::BuildHasher;

use rquickjs::{Ctx, Function, IntoJs, Object, Result as JsResult, Value, module::ModuleDef};
use serde_json::Value as JsonValue;

const OPTIONS_GLOBAL: &str = "__highbeam_settings_options";

pub struct SettingsModule;

impl ModuleDef for SettingsModule {
    fn declare(decl: &rquickjs::module::Declarations<'_>) -> JsResult<()> {
        decl.declare("get")?;
        decl.declare("getString")?;
        decl.declare("getBool")?;
        decl.declare("getInt")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> JsResult<()> {
        // All four readers share the same source — the global options bag the
        // runtime stashed at install time. Typed variants exist for ergonomic
        // call sites; the runtime contract is "return undefined when the key
        // is missing or the type doesn't match".
        let get = Function::new(ctx.clone(), |ctx: Ctx<'js>, key: String| {
            read_value(&ctx, &key, ValueKind::Any)
        })?;
        let get_string = Function::new(ctx.clone(), |ctx: Ctx<'js>, key: String| {
            read_value(&ctx, &key, ValueKind::String)
        })?;
        let get_bool = Function::new(ctx.clone(), |ctx: Ctx<'js>, key: String| {
            read_value(&ctx, &key, ValueKind::Bool)
        })?;
        let get_int = Function::new(ctx.clone(), |ctx: Ctx<'js>, key: String| {
            read_value(&ctx, &key, ValueKind::Int)
        })?;
        exports.export("get", get)?;
        exports.export("getString", get_string)?;
        exports.export("getBool", get_bool)?;
        exports.export("getInt", get_int)?;
        Ok(())
    }
}

/// Pre-populate this plugin's options bag on `globalThis`. Must run BEFORE
/// the plugin's entry module evaluates.
///
/// `merged` already folds the user's TOML overrides onto the manifest
/// defaults — the SDK reader just hands the matching value back.
///
/// # Errors
///
/// Propagates JS errors from object construction or global assignment.
pub fn install<S: BuildHasher>(ctx: &Ctx<'_>, merged: &HashMap<String, JsonValue, S>) -> JsResult<()> {
    let bag = Object::new(ctx.clone())?;
    for (key, value) in merged {
        bag.set(key.as_str(), json_to_js(ctx, value)?)?;
    }
    ctx.globals().set(OPTIONS_GLOBAL, bag)?;
    Ok(())
}

#[derive(Clone, Copy)]
enum ValueKind {
    Any,
    String,
    Bool,
    Int,
}

/// Look up `key` in the per-plugin options bag and project it to the requested
/// kind. Missing key / type mismatch / no bag at all → JS `undefined`.
fn read_value<'js>(ctx: &Ctx<'js>, key: &str, want: ValueKind) -> Value<'js> {
    let Ok(bag) = ctx.globals().get::<_, Object<'js>>(OPTIONS_GLOBAL) else {
        // No bag means install() never ran — every read is undefined.
        return Value::new_undefined(ctx.clone());
    };
    let Ok(value): JsResult<Value<'js>> = bag.get(key) else {
        return Value::new_undefined(ctx.clone());
    };
    if value.is_undefined() || value.is_null() {
        return Value::new_undefined(ctx.clone());
    }
    let matches = match want {
        ValueKind::Any => true,
        ValueKind::String => value.is_string(),
        ValueKind::Bool => value.is_bool(),
        // Treat any JS number as an "int" read — the manifest declares the
        // option as int and the persisted TOML uses integer; floats slipping
        // through here would be a host bug, not a plugin one.
        ValueKind::Int => value.is_number(),
    };
    if matches {
        value
    } else {
        Value::new_undefined(ctx.clone())
    }
}

/// Convert one `serde_json::Value` into the equivalent JS value. Nested
/// arrays/objects round-trip via `JSON.parse(JSON.stringify(...))`-style
/// recursion so the plugin's `get()` returns a tree it can index into.
fn json_to_js<'js>(ctx: &Ctx<'js>, value: &JsonValue) -> JsResult<Value<'js>> {
    match value {
        JsonValue::Null => Ok(Value::new_null(ctx.clone())),
        JsonValue::Bool(b) => b.into_js(ctx),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_js(ctx)
            } else if let Some(u) = n.as_u64() {
                // BigInts would need separate handling; downcast to f64 is
                // accurate for every realistic option-value range.
                #[allow(clippy::cast_precision_loss)]
                {
                    (u as f64).into_js(ctx)
                }
            } else if let Some(f) = n.as_f64() {
                f.into_js(ctx)
            } else {
                Ok(Value::new_null(ctx.clone()))
            }
        }
        JsonValue::String(s) => s.as_str().into_js(ctx),
        JsonValue::Array(arr) => {
            let out = rquickjs::Array::new(ctx.clone())?;
            for (idx, item) in arr.iter().enumerate() {
                out.set(idx, json_to_js(ctx, item)?)?;
            }
            Ok(out.into_value())
        }
        JsonValue::Object(obj) => {
            let out = Object::new(ctx.clone())?;
            for (k, v) in obj {
                out.set(k.as_str(), json_to_js(ctx, v)?)?;
            }
            Ok(out.into_value())
        }
    }
}

