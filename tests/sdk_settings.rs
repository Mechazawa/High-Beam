//! Integration tests for `highbeam:settings`. Per-plugin scoping is enforced
//! by the runtime calling `install()` with a plugin-specific bag — these tests
//! drive that contract end-to-end without going through the loader.

use std::collections::HashMap;
use std::sync::Arc;

use rquickjs::loader::{Loader, Resolver};
use rquickjs::{AsyncContext, AsyncRuntime, Ctx, Error as JsError, Module, async_with};
use serde_json::Value as JsonValue;
use serde_json::json;

use high_beam::sdk::settings::{self, SettingsModule};

struct OnlySettings;

impl Resolver for OnlySettings {
    fn resolve(&mut self, _ctx: &Ctx<'_>, _base: &str, name: &str) -> Result<String, JsError> {
        if name == "highbeam:settings" || name == "settings:test" {
            Ok(name.to_owned())
        } else {
            Err(JsError::new_resolving(
                "<settings-test>",
                format!("unexpected import: {name}"),
            ))
        }
    }
}

struct SettingsLoader;

impl Loader for SettingsLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> Result<Module<'js>, JsError> {
        Module::declare_def::<SettingsModule, _>(ctx.clone(), name)
    }
}

async fn run_with_options(options: HashMap<String, JsonValue>, script: &str) -> String {
    let rt = AsyncRuntime::new().expect("rt");
    rt.set_loader(OnlySettings, SettingsLoader).await;
    let ctx = AsyncContext::full(&rt).await.expect("ctx");
    let captured = Arc::new(std::sync::Mutex::new(String::new()));
    let captured_for_async = Arc::clone(&captured);
    let script_owned = script.to_string();
    async_with!(ctx => |ctx| {
        settings::install(&ctx, &options).expect("install");
        // Plugin under test stashes its observation on globalThis.__out so the
        // host can read it back as a single string.
        let src = format!(r#"
            import {{ get, getString, getBool, getInt }} from "highbeam:settings";
            {script_owned}
        "#);
        let declared = Module::declare(ctx.clone(), "settings:test", src.into_bytes())
            .expect("declare");
        let (_module, eval_promise) = declared.eval().expect("eval");
        eval_promise.into_future::<()>().await.expect("await eval");
        let out: String = ctx
            .globals()
            .get::<_, String>("__out")
            .unwrap_or_default();
        *captured_for_async.lock().unwrap() = out;
    })
    .await;
    captured.lock().unwrap().clone()
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt")
}

#[test]
fn get_returns_user_value_string() {
    let mut opts = HashMap::new();
    opts.insert("name".into(), json!("alice"));
    let out = rt().block_on(run_with_options(opts, "globalThis.__out = String(get('name'));"));
    assert_eq!(out, "alice");
}

#[test]
fn get_returns_undefined_for_missing_key() {
    let out = rt().block_on(run_with_options(
        HashMap::new(),
        "globalThis.__out = String(typeof get('does-not-exist'));",
    ));
    assert_eq!(out, "undefined");
}

#[test]
fn get_string_returns_undefined_for_non_string() {
    let mut opts = HashMap::new();
    opts.insert("count".into(), json!(7));
    let out = rt().block_on(run_with_options(
        opts,
        "globalThis.__out = String(typeof getString('count'));",
    ));
    assert_eq!(out, "undefined", "wrong type → undefined");
}

#[test]
fn get_bool_returns_user_value() {
    let mut opts = HashMap::new();
    opts.insert("live".into(), json!(true));
    let out = rt().block_on(run_with_options(opts, "globalThis.__out = String(getBool('live'));"));
    assert_eq!(out, "true");
}

#[test]
fn get_int_returns_user_value() {
    let mut opts = HashMap::new();
    opts.insert("limit".into(), json!(42));
    let out = rt().block_on(run_with_options(opts, "globalThis.__out = String(getInt('limit'));"));
    assert_eq!(out, "42");
}

#[test]
fn empty_bag_returns_undefined_for_every_key() {
    let out = rt().block_on(run_with_options(
        HashMap::new(),
        "globalThis.__out = [get('a'), getString('a'), getBool('a'), getInt('a')].map(v => typeof v).join(',');",
    ));
    assert_eq!(out, "undefined,undefined,undefined,undefined");
}
