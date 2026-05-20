//! rquickjs `Context` per plugin and the `query()` driver.
//!
//! Per-plugin state held here:
//!   * `AsyncRuntime` + `AsyncContext` — own JS heap and global object
//!   * memory cap from `manifest.memoryMb`
//!   * shared `Arc<AtomicBool>` set by the wall-clock timer that the interrupt
//!     hook checks; tripping it makes `QuickJS` raise an uncatchable exception
//!     and return control to Rust
//!   * resolver/loader pair that whitelists `highbeam:*` specifiers and
//!     resolves `highbeam:actions` against our in-host module
//!
//! `AsyncIterable` consumption: `query(input, signal)` returns an async iterator
//! protocol object. We `await iter.next()` repeatedly, deserialize each
//! `{ value, done }` step into [`PluginResult`], and stop when `done` is true.
//! Streaming arrives in Stage 4.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rquickjs::function::This;
use rquickjs::loader::{Loader, Resolver};
use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Error as JsError, Function, IntoJs, Module,
    Object, Promise, Value, async_with,
};

use crate::plugins::manifest::Manifest;
use crate::plugins::result::PluginResult;
use crate::sdk::actions::ActionsModule;

const HIGHBEAM_SCHEME: &str = "highbeam:";
const ACTIONS_MODULE: &str = "highbeam:actions";

/// A loaded, evaluated plugin ready to handle queries.
pub struct LoadedPlugin {
    pub manifest: Manifest,
    // Stored to anchor the runtime's lifetime to the plugin's. `AsyncContext`
    // holds an internal `Arc` to its runtime so this field isn't strictly
    // required for correctness, but keeping it explicit reads better and
    // future-proofs against rquickjs internals changing.
    _runtime: AsyncRuntime,
    context: AsyncContext,
    timeout: Duration,
    // Held to keep the interrupt flag alive for the runtime's lifetime; the
    // interrupt hook captures a clone of the same `Arc`.
    interrupt_flag: Arc<AtomicBool>,
}

/// Errors surfaced while loading or running a plugin.
///
/// Stage 3 logs these to stderr (Stage 9 routes them to per-plugin logfiles).
#[derive(Debug)]
pub enum PluginError {
    Io(std::io::Error),
    /// JS load/eval/runtime error. Pre-formatted (we drop the JS exception
    /// payload here because the `rquickjs::Error` borrow can't outlive its
    /// context).
    Js(String),
    /// `query()` exceeded `manifest.timeoutMs`.
    Timeout,
    /// Whatever the plugin yielded did not match the [`PluginResult`] schema.
    InvalidResult(String),
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io: {err}"),
            Self::Js(msg) => write!(f, "js: {msg}"),
            Self::Timeout => f.write_str("plugin exceeded its timeoutMs budget"),
            Self::InvalidResult(msg) => write!(f, "invalid result: {msg}"),
        }
    }
}

impl std::error::Error for PluginError {}

impl From<std::io::Error> for PluginError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl LoadedPlugin {
    /// Build a runtime for the plugin and evaluate its entry module.
    ///
    /// # Errors
    ///
    /// Fails if the entry file can't be read, the JS source has a syntax
    /// error, the module imports anything outside the `highbeam:` scheme,
    /// or the `query` export is missing / not a function.
    pub async fn load(plugin_dir: &Path, manifest: Manifest) -> Result<Self, PluginError> {
        let entry_path = manifest.entry_path(plugin_dir);
        let source = std::fs::read_to_string(&entry_path).map_err(|err| {
            PluginError::Io(std::io::Error::new(
                err.kind(),
                format!("read {}: {err}", entry_path.display()),
            ))
        })?;

        let runtime = AsyncRuntime::new().map_err(|err| PluginError::Js(err.to_string()))?;

        // Memory cap: docs say "0 = unlimited"; we treat 0 as "leave default
        // alone" so a hand-crafted manifest can't accidentally remove the cap.
        if manifest.memory_mb > 0 {
            let bytes = usize::try_from(manifest.memory_mb)
                .unwrap_or(usize::MAX)
                .saturating_mul(1024 * 1024);
            runtime.set_memory_limit(bytes).await;
        }

        // Interrupt flag: the wall-clock timer flips this; the hook returns
        // `true` whenever it's set, which causes QuickJS to raise an
        // uncatchable exception and return control to Rust.
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let hook_flag = Arc::clone(&interrupt_flag);
        runtime
            .set_interrupt_handler(Some(Box::new(move || hook_flag.load(Ordering::Relaxed))))
            .await;

        let resolver = HighbeamResolver;
        let loader = HighbeamLoader::new(manifest.has_capability("actions"));
        runtime.set_loader(resolver, loader).await;

        let context = AsyncContext::full(&runtime)
            .await
            .map_err(|err| PluginError::Js(err.to_string()))?;

        // Evaluate the entry module. We name it `plugin:main` so error
        // backtraces print something sensible. The `evaluate_main_module`
        // pattern (declare + eval) lets us await the eval-promise so
        // top-level imports finish before we look up `query`.
        let entry_path_str = entry_path.display().to_string();
        let source_bytes = source.into_bytes();
        let outcome: Result<(), PluginError> = async_with!(context => |ctx| {
            let declared = Module::declare(ctx.clone(), "plugin:main", source_bytes)
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("declare {entry_path_str}: {err}")))?;
            let (module, eval_promise) = declared
                .eval()
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("eval {entry_path_str}: {err}")))?;
            // Await the module's evaluation promise so top-level awaits and
            // host module loads (e.g. `import 'highbeam:actions'`) complete
            // before we look up `query`.
            eval_promise
                .into_future::<()>()
                .await
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("await eval {entry_path_str}: {err}")))?;
            // Hoist the `query` export onto `globalThis` so `run_query` can
            // grab it without re-importing (re-importing across awaits fights
            // the borrow checker on the rquickjs `Module` lifetime). The name
            // is intentionally weird to avoid clashing with anything user
            // code might do on its own globals.
            let query: Function<'_> = module
                .get("query")
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("missing `query` export: {err}")))?;
            ctx.globals()
                .set("__highbeam_query", query)
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("stash query global: {err}")))?;
            Ok(())
        })
        .await;
        outcome?;

        let timeout = Duration::from_millis(manifest.timeout_ms);
        Ok(Self {
            manifest,
            _runtime: runtime,
            context,
            timeout,
            interrupt_flag,
        })
    }

    /// Call `query(input, signal)` and collect its `AsyncIterable` into a Vec.
    ///
    /// Stage 3 hard-walls the entire call (both the call itself and the
    /// iteration loop) under `manifest.timeoutMs`. The `signal` argument is
    /// a stub `AbortSignal`-shaped object — Stage 4 wires it through host I/O.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Timeout`] if the budget is exceeded,
    /// [`PluginError::Js`] for any thrown JS exception, or
    /// [`PluginError::InvalidResult`] if a yielded value doesn't deserialize
    /// into [`PluginResult`].
    pub async fn run_query(&self, input: &str) -> Result<Vec<PluginResult>, PluginError> {
        // Reset and arm the interrupt flag for this call.
        self.interrupt_flag.store(false, Ordering::Relaxed);
        let flag_for_timer = Arc::clone(&self.interrupt_flag);
        let timeout = self.timeout;
        let timer_handle = tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            flag_for_timer.store(true, Ordering::Relaxed);
        });

        let input_owned = input.to_owned();
        let outcome: Result<Vec<PluginResult>, PluginError> = async_with!(self.context => |ctx| {
            collect_query_results(ctx, &input_owned).await
        })
        .await;

        timer_handle.abort();

        // If the interrupt flag tripped, surface that as Timeout rather than
        // the cryptic "InternalError: interrupted" JS-side message.
        if self.interrupt_flag.load(Ordering::Relaxed) {
            return Err(PluginError::Timeout);
        }
        outcome
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        // Best effort: clear the interrupt flag and drop the runtime. The
        // AsyncRuntime/Context handle their own cleanup; no extra teardown
        // needed for Stage 3.
        self.interrupt_flag.store(false, Ordering::Relaxed);
    }
}

async fn collect_query_results<'js>(
    ctx: Ctx<'js>,
    input: &str,
) -> Result<Vec<PluginResult>, PluginError> {
    // Grab the `query` function we hoisted onto globalThis during load. We
    // can't re-import the plugin module inside this closure cleanly because
    // the `Module<'js>` borrow can't survive the `.await` on the next-step
    // promise; the global stash side-steps that.
    let query: Function<'js> = ctx
        .globals()
        .get("__highbeam_query")
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("`query` is not callable: {err}")))?;

    // Build a stub `AbortSignal` so plugin authors writing for Stage 4+ don't
    // crash on Stage 3. Just enough surface to read `aborted` and call
    // `addEventListener(…)` without exploding.
    let signal = Object::new(ctx.clone())
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("build signal stub: {err}")))?;
    signal
        .set("aborted", false)
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("set signal.aborted: {err}")))?;
    let noop = Function::new(ctx.clone(), || {})
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("build signal noop: {err}")))?;
    signal
        .set("addEventListener", noop.clone())
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("set signal.addEventListener: {err}")))?;
    signal
        .set("removeEventListener", noop)
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("set signal.removeEventListener: {err}")))?;

    let input_js = input
        .into_js(&ctx)
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("convert input: {err}")))?;
    let signal_value: Value<'js> = signal.into_value();
    let iter_or_iterable: Value<'js> = query
        .call((input_js, signal_value))
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("call query(): {err}")))?;

    // `query` may be `async function*` (returns an async iterator directly) or
    // an `async function` returning an AsyncIterable (has `[Symbol.asyncIterator]`).
    // Normalize to an async iterator.
    let iter_obj = normalize_async_iterator(&ctx, iter_or_iterable)?;

    let next: Function<'js> = iter_obj
        .get("next")
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("iterator.next missing: {err}")))?;

    let mut results = Vec::new();
    loop {
        // `next` is bound to the iterator object via `This(iter_obj)` so
        // QuickJS treats this as `iter.next()` rather than a free function
        // call with `this === undefined` (which AsyncGenerator's internal
        // `next` rejects with "not an AsyncGenerator object").
        let step_promise: Promise<'js> = next
            .call((This(iter_obj.clone()),))
            .catch(&ctx)
            .map_err(|err| PluginError::Js(format!("iterator.next() threw: {err}")))?;
        let step: Object<'js> = step_promise
            .into_future::<Object<'js>>()
            .await
            .catch(&ctx)
            .map_err(|err| PluginError::Js(format!("await iterator.next(): {err}")))?;
        let done: bool = step
            .get("done")
            .catch(&ctx)
            .map_err(|err| PluginError::Js(format!("read step.done: {err}")))?;
        if done {
            break;
        }
        let value: Value<'js> = step
            .get("value")
            .catch(&ctx)
            .map_err(|err| PluginError::Js(format!("read step.value: {err}")))?;
        let json_str = value_to_json(&ctx, &value)?;
        let parsed: PluginResult = serde_json::from_str(&json_str)
            .map_err(|err| PluginError::InvalidResult(format!("{err}: {json_str}")))?;
        results.push(parsed);
    }
    Ok(results)
}

/// If the value is already an async iterator (has `next`), return it as-is.
/// Otherwise call `Symbol.asyncIterator` on it to obtain one.
fn normalize_async_iterator<'js>(
    ctx: &Ctx<'js>,
    value: Value<'js>,
) -> Result<Object<'js>, PluginError> {
    let obj: Object<'js> = value
        .try_into_object()
        .map_err(|_| PluginError::Js("query() did not return an object".into()))?;
    let has_next: bool = ctx
        .clone()
        .eval::<Function<'js>, _>("(o) => typeof o?.next === 'function'")
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("build next-probe: {err}")))?
        .call((obj.clone(),))
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("invoke next-probe: {err}")))?;
    if has_next {
        return Ok(obj);
    }
    // Fall back to `obj[Symbol.asyncIterator]()`.
    let factory: Function<'js> = ctx
        .clone()
        .eval::<Function<'js>, _>("(o) => o[Symbol.asyncIterator]()")
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("build asyncIterator probe: {err}")))?;
    let iter: Object<'js> = factory
        .call((obj,))
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("invoke [Symbol.asyncIterator](): {err}")))?;
    Ok(iter)
}

fn value_to_json<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String, PluginError> {
    // Round-trip through the JS-side `JSON.stringify` instead of writing a
    // bespoke Value -> serde_json::Value walker. QuickJS's stringify already
    // handles every edge case (nested objects, escaping, etc.).
    let stringify: Function<'js> = ctx
        .clone()
        .eval::<Function<'js>, _>("JSON.stringify")
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("resolve JSON.stringify: {err}")))?;
    let s: String = stringify
        .call((value.clone(),))
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("JSON.stringify: {err}")))?;
    Ok(s)
}

/// Resolves `highbeam:*` specifiers; rejects everything else.
struct HighbeamResolver;

impl Resolver for HighbeamResolver {
    fn resolve(&mut self, _ctx: &Ctx<'_>, _base: &str, name: &str) -> Result<String, JsError> {
        if name.starts_with(HIGHBEAM_SCHEME) || name == "plugin:main" {
            Ok(name.to_owned())
        } else {
            Err(JsError::new_resolving(
                "<plugin>",
                format!(
                    "high-beam plugins may only import from `highbeam:*` (got {name:?}); see docs/02-plugin-sdk.md"
                ),
            ))
        }
    }
}

/// Loads exactly the `highbeam:*` modules the plugin's capabilities permit.
struct HighbeamLoader {
    actions_enabled: bool,
}

impl HighbeamLoader {
    const fn new(actions_enabled: bool) -> Self {
        Self { actions_enabled }
    }
}

impl Loader for HighbeamLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> Result<Module<'js>, JsError> {
        match name {
            ACTIONS_MODULE if self.actions_enabled => {
                Module::declare_def::<ActionsModule, _>(ctx.clone(), name)
            }
            ACTIONS_MODULE => Err(JsError::new_loading_message(
                name,
                "plugin lacks the `actions` capability",
            )),
            other if other.starts_with(HIGHBEAM_SCHEME) => Err(JsError::new_loading_message(
                name,
                format!("`{other}` is not implemented in Stage 3"),
            )),
            other => Err(JsError::new_loading_message(
                name,
                format!("`{other}` is not a recognised highbeam module"),
            )),
        }
    }
}
