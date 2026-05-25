//! Host implementation of the `highbeam:view` module + the reactivity
//! runtime that lives inside each plugin's `QuickJS` context.
//!
//! Two layers:
//!
//! * The module def — pure block factories exported as `highbeam:view`.
//!   Plugins import them to build the tree their `render()` returns.
//! * The runtime install path (`install_runtime`) — evaluates the
//!   `view_runtime.js` shim and registers four host-callable globals
//!   the JS runtime depends on:
//!     - `__highbeam_paint_tree(handle, treeJson)` — push render to host.
//!     - `__highbeam_paint_error(handle, message, stack)` — push error.
//!     - `__highbeam_dispatch(actionJson)` — fire an Action to the host.
//!     - `__highbeam_close_view_request(handle)` — render returned null,
//!       host should pop the frame.
//!
//! Stage 3 wires the four globals as `tracing` stubs so the protocol is
//! observable in logs without yet being visible in the UI; Stage 4 swaps
//! them for the real Slint-paint / action-dispatch routing.

use std::sync::Arc;

use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{CatchResultExt, Ctx, Function, Object, Result as JsResult, Value};

const VIEW_RUNTIME_JS: &str = include_str!("js/view_runtime.js");

/// Wires the JS-side view runtime back to the host. Each plugin context
/// gets its own bridge — the closures capture the plugin name (for log
/// lines + frame ownership) plus whatever Slint-thread state each one
/// needs (the view stack, the host-message channel). Held behind an
/// `Arc` because rquickjs `Function` closures need `'static` ownership
/// and the same bridge is shared across the four install_* functions.
pub struct RuntimeBridge {
    /// Plugin that owns this bridge — captured for log lines and so the
    /// host can match `close_view_request` calls against the right
    /// frame.
    pub plugin_name: String,
    /// Called when the JS runtime produces a fresh rendered tree.
    /// Arguments: `(handle, tree_json)`. The closure parses + paints on
    /// the Slint thread.
    pub paint_tree: Box<dyn Fn(u64, String) + Send + Sync + 'static>,
    /// Called when a closure inside a view's `on*` handler (or the
    /// closure's eventual return value) yields an `Action`. Receives the
    /// JSON-serialised action; the host parses + routes it.
    pub dispatch: Box<dyn Fn(String) + Send + Sync + 'static>,
    /// Called when a view's `render()` returns `null` — the JS side has
    /// already torn down its own state; the host should pop the matching
    /// frame from the stack.
    pub close_request: Box<dyn Fn(u64) + Send + Sync + 'static>,
}

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

/// Install the reactivity runtime + host-callable bridge globals into a
/// plugin's `Ctx`. Idempotent — the JS shim guards on `__highbeam_views`
/// presence so re-eval is a no-op, and the bridge globals are re-set on
/// every call (cheap; lets the caller rotate the closures without
/// touching this code).
///
/// # Errors
///
/// Propagates JS errors from evaluating the runtime shim or setting the
/// bridge globals.
pub fn install_runtime(ctx: &Ctx<'_>, bridge: Arc<RuntimeBridge>) -> JsResult<()> {
    install_paint_tree(ctx, Arc::clone(&bridge))?;
    install_paint_error(ctx, bridge.plugin_name.clone())?;
    install_dispatch(ctx, Arc::clone(&bridge))?;
    install_close_request(ctx, bridge)?;

    ctx.eval::<(), _>(VIEW_RUNTIME_JS)?;
    Ok(())
}

fn install_paint_tree(ctx: &Ctx<'_>, bridge: Arc<RuntimeBridge>) -> JsResult<()> {
    let paint = Function::new(ctx.clone(), move |handle: u64, tree_json: String| {
        (bridge.paint_tree)(handle, tree_json);
        Ok::<_, rquickjs::Error>(())
    })?;
    ctx.globals().set("__highbeam_paint_tree", paint)?;
    Ok(())
}

fn install_paint_error(ctx: &Ctx<'_>, plugin_name: String) -> JsResult<()> {
    let paint = Function::new(ctx.clone(), move |handle: u64, message: String, stack: String| {
        tracing::error!(plugin = %plugin_name, handle, %message, %stack, "views: render error");
        Ok::<_, rquickjs::Error>(())
    })?;
    ctx.globals().set("__highbeam_paint_error", paint)?;
    Ok(())
}

fn install_dispatch(ctx: &Ctx<'_>, bridge: Arc<RuntimeBridge>) -> JsResult<()> {
    let dispatch = Function::new(ctx.clone(), move |action_json: String| {
        (bridge.dispatch)(action_json);
        Ok::<_, rquickjs::Error>(())
    })?;
    ctx.globals().set("__highbeam_dispatch", dispatch)?;
    Ok(())
}

fn install_close_request(ctx: &Ctx<'_>, bridge: Arc<RuntimeBridge>) -> JsResult<()> {
    let close = Function::new(ctx.clone(), move |handle: u64| {
        (bridge.close_request)(handle);
        Ok::<_, rquickjs::Error>(())
    })?;
    ctx.globals().set("__highbeam_close_view_request", close)?;
    Ok(())
}

/// Call the JS runtime's `init(handle, props)`. Triggers `setup` →
/// first render → `mounted` (mounted runs in a microtask scheduled by
/// the JS runtime itself).
///
/// `props_json` is a serialised JSON string; the runtime parses it on
/// the JS side via `JSON.parse` so we don't have to walk it on the Rust
/// side. Functions in props would already be stripped by serde at the
/// receive boundary; passing JSON keeps everything cleanly data-only.
///
/// # Errors
///
/// Propagates JS errors from the install path or from the `init` call
/// itself (including any uncaught throw from `setup` or the first
/// `render`).
pub fn invoke_init(ctx: &Ctx<'_>, handle: u64, props_json: &str) -> JsResult<()> {
    let views: Object<'_> = ctx
        .globals()
        .get("__highbeam_views")
        .catch(ctx)
        .map_err(|err| rquickjs::Error::new_loading_message("view", format!("__highbeam_views missing: {err}")))?;
    let init: Function<'_> = views
        .get("init")
        .catch(ctx)
        .map_err(|err| rquickjs::Error::new_loading_message("view", format!("init missing: {err}")))?;
    let json_parse: Function<'_> = ctx.eval("JSON.parse")?;
    let props_value: Value<'_> = json_parse.call((props_json.to_owned(),))?;
    init.call::<_, ()>((i64::try_from(handle).unwrap_or(i64::MAX), props_value))?;
    Ok(())
}

/// Call the JS runtime's `event(handle, callbackId, value)`. The plugin
/// runs the registered callback bound to the view's reactive proxy; any
/// state mutation re-renders via the microtask flush.
///
/// # Errors
///
/// Propagates JS errors from looking up the runtime or invoking
/// `event` (uncaught throws inside the callback bubble through the JS
/// runtime's own paint-error path, not as a Rust error).
pub fn invoke_event(ctx: &Ctx<'_>, handle: u64, callback_id: u64, value_json: &str) -> JsResult<()> {
    let views: Object<'_> = ctx.globals().get("__highbeam_views")?;
    let event: Function<'_> = views.get("event")?;
    let json_parse: Function<'_> = ctx.eval("JSON.parse")?;
    let value: Value<'_> = json_parse.call((value_json.to_owned(),))?;
    event.call::<_, ()>((
        i64::try_from(handle).unwrap_or(i64::MAX),
        i64::try_from(callback_id).unwrap_or(i64::MAX),
        value,
    ))?;
    Ok(())
}

/// Call the JS runtime's `close(handle)`. Runs `unmounted` if defined,
/// aborts the mounted-signal, and clears the per-handle state.
///
/// # Errors
///
/// Propagates JS errors from looking up the runtime or invoking
/// `close`.
pub fn invoke_close(ctx: &Ctx<'_>, handle: u64) -> JsResult<()> {
    let views: Object<'_> = ctx.globals().get("__highbeam_views")?;
    let close: Function<'_> = views.get("close")?;
    close.call::<_, ()>((i64::try_from(handle).unwrap_or(i64::MAX),))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rquickjs::{AsyncContext, AsyncRuntime, async_with};

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt")
    }

    /// No-op bridge — the JS-side fallback path uses these closures when
    /// the host doesn't care about `paint_tree` / `dispatch` /
    /// `close_view_request` side effects.
    fn test_bridge(plugin: &str) -> Arc<RuntimeBridge> {
        Arc::new(RuntimeBridge {
            plugin_name: plugin.to_owned(),
            paint_tree: Box::new(|_handle, _tree| {}),
            dispatch: Box::new(|_action_json| {}),
            close_request: Box::new(|_handle| {}),
        })
    }

    #[test]
    fn install_runtime_is_idempotent() {
        let runtime = rt();
        runtime.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            async_with!(ctx => |ctx| {
                install_runtime(&ctx, test_bridge("test-plugin")).expect("install");
                install_runtime(&ctx, test_bridge("test-plugin")).expect("re-install");
                let has_views: bool = ctx
                    .eval("typeof globalThis.__highbeam_views === 'object'")
                    .expect("eval");
                assert!(has_views);
                let live: i32 = ctx.eval("__highbeam_views._liveCount()").expect("eval count");
                assert_eq!(live, 0);
            })
            .await;
        });
    }

    #[test]
    fn init_then_close_runs_setup_and_unmounted() {
        let runtime = rt();
        runtime.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            async_with!(ctx => |ctx| {
                // Bootstrap AbortController so the runtime's `new AbortController()`
                // in init() resolves.
                crate::sdk::abort::install_global_controller(&ctx).expect("abort");
                install_runtime(&ctx, test_bridge("t")).expect("install");

                // Plant a fake registry + view that records calls on globals
                // so the test can assert without an actual showView call path.
                ctx.eval::<(), _>(
                    r#"
                    globalThis.__test_calls = [];
                    globalThis.__highbeam_view_registry = {
                        nextId: 2,
                        byHandle: {
                            "1": {
                                setup(props) {
                                    __test_calls.push(['setup', props]);
                                    return { x: props.x };
                                },
                                render() {
                                    __test_calls.push(['render', this.x]);
                                    return { body: [{ kind: 'text', text: String(this.x) }] };
                                },
                                unmounted() {
                                    __test_calls.push(['unmounted', this.x]);
                                },
                            },
                        },
                    };
                    "#,
                )
                .expect("plant");

                invoke_init(&ctx, 1, r#"{"x":42}"#).expect("init");
                invoke_close(&ctx, 1).expect("close");

                let calls: String = ctx.eval("JSON.stringify(__test_calls)").expect("read calls");
                assert!(calls.contains("setup"), "setup not called: {calls}");
                assert!(calls.contains("render"), "render not called: {calls}");
                assert!(calls.contains("unmounted"), "unmounted not called: {calls}");

                let live: i32 = ctx.eval("__highbeam_views._liveCount()").expect("live");
                assert_eq!(live, 0, "instance not cleaned up");
            })
            .await;
        });
    }

    #[test]
    fn render_returning_null_calls_close_request_stub() {
        let runtime = rt();
        runtime.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            async_with!(ctx => |ctx| {
                crate::sdk::abort::install_global_controller(&ctx).expect("abort");
                install_runtime(&ctx, test_bridge("t")).expect("install");
                // Override the stub paint hooks with recorders so we can assert.
                ctx.eval::<(), _>(
                    r#"
                    globalThis.__close_calls = [];
                    globalThis.__highbeam_close_view_request = (h) => {
                        __close_calls.push(h);
                    };
                    globalThis.__highbeam_view_registry = {
                        nextId: 2,
                        byHandle: {
                            "5": {
                                setup() { return {}; },
                                render() { return null; },
                            },
                        },
                    };
                    "#,
                )
                .expect("plant");

                invoke_init(&ctx, 5, "{}").expect("init");

                let calls: String = ctx.eval("JSON.stringify(__close_calls)").expect("read");
                assert_eq!(calls, "[5]", "close-request was not fired: {calls}");
            })
            .await;
        });
    }

    #[test]
    fn event_invokes_callback_registered_in_previous_render() {
        // Verifies the callback table: a function embedded in the tree
        // gets a fresh id at render time, and `event(handle, id, value)`
        // routes back to that exact function with `this` bound to the
        // reactive proxy. We side-channel the click count into a global
        // so the assertion doesn't depend on the microtask flush — that
        // path is exercised end-to-end in Stage 4.
        let runtime = rt();
        runtime.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            async_with!(ctx => |ctx| {
                crate::sdk::abort::install_global_controller(&ctx).expect("abort");
                install_runtime(&ctx, test_bridge("t")).expect("install");

                ctx.eval::<(), _>(
                    r#"
                    globalThis.__paint_calls = [];
                    globalThis.__highbeam_paint_tree = (handle, treeJson) => {
                        __paint_calls.push([handle, treeJson]);
                    };
                    globalThis.__highbeam_view_registry = {
                        nextId: 2,
                        byHandle: {
                            "9": {
                                setup() { return { count: 0 }; },
                                render() {
                                    return {
                                        body: [{
                                            kind: 'button',
                                            label: 'tap',
                                            onClick: () => {
                                                this.count += 1;
                                                globalThis.__last_count = this.count;
                                            },
                                        }],
                                    };
                                },
                            },
                        },
                    };
                    "#,
                )
                .expect("plant");

                invoke_init(&ctx, 9, "{}").expect("init");

                let first_tree: String = ctx
                    .eval("__paint_calls[0][1]")
                    .expect("read tree");
                let callback_id: u64 = extract_first_callback_id(&first_tree);
                invoke_event(&ctx, 9, callback_id, "null").expect("event");

                let last: i32 = ctx.eval("__last_count").expect("read last_count");
                assert_eq!(last, 1, "callback did not mutate the proxied state");
            })
            .await;
        });
    }

    /// Grep a stringified tree for `"__callbackId":N` and return the
    /// first N as a u64. Test-only helper.
    fn extract_first_callback_id(tree_json: &str) -> u64 {
        const MARKER: &str = "\"__callbackId\":";
        let idx = tree_json.find(MARKER).expect("no __callbackId in tree");
        let after = &tree_json[idx + MARKER.len()..];
        let end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
        after[..end].parse().expect("parse callback id")
    }
}
