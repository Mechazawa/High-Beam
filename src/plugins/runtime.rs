//! rquickjs `Context` per plugin and the `query()` driver.
//!
//! Per-plugin state held here:
//!   * `AsyncRuntime` + `AsyncContext` — own JS heap and global object
//!   * memory cap from `manifest.memoryMb`
//!   * shared `Arc<AtomicBool>` set by the wall-clock timer that the interrupt
//!     hook checks; tripping it makes `QuickJS` raise an uncatchable exception
//!     and return control to Rust
//!   * resolver/loader pair that whitelists `highbeam:*` specifiers and gates
//!     each module on the manifest's capability list
//!
//! Streaming + cancellation (Stage 4):
//!   * `run_query` builds a host [`Abort`] and gives the plugin its
//!     `AbortSignal` as the second argument to `query(input, signal)`.
//!   * It returns an `mpsc::Receiver<PluginResult>` that the dispatcher reads
//!     from as the plugin yields. Closing the dispatcher's end (via
//!     `cancel_handle.abort()` or just dropping the receiver) interrupts the
//!     in-flight query.
//!   * Per-iteration we race the next-step future against the abort token so
//!     we wake up promptly when a new keystroke arrives.

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
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::plugins::manifest::Manifest;
use crate::plugins::result::PluginResult;
use crate::sdk::abort::{Abort, install_global_controller};
use crate::sdk::actions::ActionsModule;
use crate::sdk::capability;
use crate::sdk::clipboard;
use crate::sdk::fs;
use crate::sdk::http::HttpModule;
use crate::sdk::icons;
use crate::sdk::r#match::MatchModule;
use crate::sdk::platform::PlatformModule;
use crate::sdk::system;
use crate::sdk::timers;

/// JS-side iterator normalizer (turns a value returned from `query()` into a
/// real async iterator). Loaded once per query at the top of
/// `stream_query`; tiny enough that re-eval cost is negligible.
const ITERATOR_NORMALIZE_JS: &str = include_str!("../sdk/js/iterator_normalize.js");

const HIGHBEAM_SCHEME: &str = "highbeam:";
const ACTIONS_MODULE: &str = "highbeam:actions";
const HTTP_MODULE: &str = "highbeam:http";
const CLIPBOARD_MODULE: &str = "highbeam:clipboard";
const FS_MODULE: &str = "highbeam:fs";
const ICONS_MODULE: &str = "highbeam:icons";
const SYSTEM_MODULE: &str = "highbeam:system";
const MATCH_MODULE: &str = "highbeam:match";
const PLATFORM_MODULE: &str = "highbeam:platform";
/// Slot on `globalThis` where we stash the plugin's `query` export at load
/// time. Re-importing the plugin module inside `run_query` would require a
/// `Module<'js>` reference that can't survive the `.await` on each iteration
/// step; `Persistent<Function<'static>>` would be cleaner but contains a raw
/// pointer (`!Send`), so it can't cross the `async_with!` boundary under
/// rquickjs's `parallel` feature. Keep the global name unusual so plugin
/// code can't accidentally collide.
const QUERY_GLOBAL: &str = "__highbeam_query";

/// A loaded, evaluated plugin ready to handle queries.
///
/// `AsyncContext` keeps the underlying `AsyncRuntime` alive via its own
/// internal `Arc`, so we only hold the context here.
pub struct LoadedPlugin {
    pub manifest: Manifest,
    context: AsyncContext,
    timeout: Duration,
    // Held to keep the interrupt flag alive for the runtime's lifetime; the
    // interrupt hook captures a clone of the same `Arc`.
    interrupt_flag: Arc<AtomicBool>,
}

/// Errors surfaced while loading or running a plugin.
///
/// Stage 4 logs these to stderr (Stage 9 routes them to per-plugin logfiles).
#[derive(Debug)]
pub enum PluginError {
    Io(std::io::Error),
    /// JS load/eval/runtime error. Pre-formatted (we drop the JS exception
    /// payload here because the `rquickjs::Error` borrow can't outlive its
    /// context).
    Js(String),
    /// `query()` exceeded `manifest.timeoutMs`.
    Timeout,
    /// Query was cancelled before it produced its full result set (host-
    /// initiated, not an error from the plugin's perspective).
    Cancelled,
    /// Whatever the plugin yielded did not match the [`PluginResult`] schema.
    InvalidResult(String),
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io: {err}"),
            Self::Js(msg) => write!(f, "js: {msg}"),
            Self::Timeout => f.write_str("plugin exceeded its timeoutMs budget"),
            Self::Cancelled => f.write_str("query cancelled"),
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
    /// `cache_dir` is the plugin's own slot under the host's cache directory.
    /// The host pre-creates the path; the SDK only writes into it.
    ///
    /// # Errors
    ///
    /// Fails if the entry file can't be read, the JS source has a syntax
    /// error, the module imports anything outside the `highbeam:` scheme,
    /// or the `query` export is missing / not a function.
    pub async fn load(plugin_dir: &Path, manifest: Manifest) -> Result<Self, PluginError> {
        let cache_dir = default_cache_dir(&manifest.name);
        Self::load_with_cache_dir(plugin_dir, manifest, cache_dir).await
    }

    /// Variant of [`Self::load`] with an explicit cache directory — used by
    /// the test harness to keep cache writes inside a tmpdir.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::load`].
    pub async fn load_with_cache_dir(
        plugin_dir: &Path,
        manifest: Manifest,
        cache_dir: std::path::PathBuf,
    ) -> Result<Self, PluginError> {
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
        let loader = HighbeamLoader::new(manifest.capabilities.clone());
        runtime.set_loader(resolver, loader).await;

        let context = AsyncContext::full(&runtime)
            .await
            .map_err(|err| PluginError::Js(err.to_string()))?;

        // Evaluate the entry module. We name it `plugin:main` so error
        // backtraces print something sensible. The declare + eval split lets
        // us await the eval-promise so top-level imports (e.g.
        // `import 'highbeam:actions'`) finish before we look up `query`.
        let entry_path_str = entry_path.display().to_string();
        let source_bytes = source.into_bytes();
        let plugin_caps = manifest.capabilities.clone();
        let plugin_dir_owned = plugin_dir.to_path_buf();
        async_with!(context => |ctx| {
            // Install the JS-side AbortController polyfill so plugins can do
            // `new AbortController()` for their own cancellation flows.
            install_global_controller(&ctx)
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("install AbortController: {err}")))?;

            // `setTimeout`/`clearTimeout` polyfill so plugins can `await
            // new Promise(r => setTimeout(r, ms))`. Strictly speaking
            // optional, but every JS author expects it; without it streaming
            // demos like our slow-echo plugin can't write the obvious code.
            timers::install(&ctx)
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("install setTimeout: {err}")))?;

            // Pre-install the per-plugin clipboard bindings so the
            // `highbeam:clipboard` module can pick them up at evaluate-time.
            // Only relevant if the plugin declared at least one clipboard cap;
            // we install unconditionally because the inert no-cap stubs are
            // cheap and keep the module's evaluate path branch-free.
            let can_read = plugin_caps.iter().any(|c| c == "clipboard.read");
            let can_write = plugin_caps.iter().any(|c| c == "clipboard.write");
            clipboard::install(&ctx, can_read, can_write)
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("install clipboard: {err}")))?;

            let can_fs_read = plugin_caps.iter().any(|c| c == "fs.read");
            let can_fs_cache = plugin_caps.iter().any(|c| c == "fs.cache");
            fs::install(
                &ctx,
                can_fs_read,
                can_fs_cache,
                cache_dir.clone(),
                plugin_dir_owned.clone(),
            )
            .catch(&ctx)
            .map_err(|err| PluginError::Js(format!("install fs: {err}")))?;

            let can_icons = plugin_caps.iter().any(|c| c == "icons");
            icons::install(&ctx, can_icons)
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("install icons: {err}")))?;

            let can_system_exec = plugin_caps.iter().any(|c| c == "system.exec");
            let can_system_applescript = plugin_caps.iter().any(|c| c == "system.applescript");
            system::install(&ctx, can_system_exec, can_system_applescript)
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("install system: {err}")))?;

            let declared = Module::declare(ctx.clone(), "plugin:main", source_bytes)
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("declare {entry_path_str}: {err}")))?;
            let (module, eval_promise) = declared
                .eval()
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("eval {entry_path_str}: {err}")))?;
            eval_promise
                .into_future::<()>()
                .await
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("await eval {entry_path_str}: {err}")))?;

            let query: Function<'_> = module
                .get("query")
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("missing `query` export: {err}")))?;
            ctx.globals()
                .set(QUERY_GLOBAL, query)
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("stash query global: {err}")))?;
            Ok::<_, PluginError>(())
        })
        .await?;

        let timeout = Duration::from_millis(manifest.timeout_ms);
        Ok(Self {
            manifest,
            context,
            timeout,
            interrupt_flag,
        })
    }

    /// Stream results from `query(input, signal)` over an `mpsc` channel.
    ///
    /// Returns:
    ///   * `receiver` — yields one [`PluginResult`] per `yield` from the plugin.
    ///     Closes when the plugin's iterator drains, the timeout fires, the
    ///     plugin throws, or the caller cancels via `cancel.cancel()`.
    ///   * `cancel` — host handle to cancel mid-flight. The dispatcher flips
    ///     this when a newer keystroke arrives.
    ///
    /// # Errors
    ///
    /// Channel send is fallible (receiver dropped) but the worker treats that
    /// as a cancellation, not an error. Per-iteration JS errors close the
    /// channel and are reported via the receiver returning `None`. (Stage 9
    /// will route those errors to the plugin's logfile.)
    #[must_use]
    pub fn run_query_stream(
        &self,
        input: &str,
        cancel: CancellationToken,
    ) -> mpsc::UnboundedReceiver<PluginResult> {
        let (tx, rx) = mpsc::unbounded_channel();
        // Reset and arm the interrupt flag for this call.
        self.interrupt_flag.store(false, Ordering::Relaxed);
        let flag_for_timer = Arc::clone(&self.interrupt_flag);
        let timeout = self.timeout;

        let cancel_for_timer = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = tokio::time::sleep(timeout) => {
                    flag_for_timer.store(true, Ordering::Relaxed);
                    cancel_for_timer.cancel();
                }
                () = cancel_for_timer.cancelled() => {
                    // Caller-initiated cancel — we still flip the interrupt
                    // flag so blocking CPU loops inside QuickJS wake up.
                    flag_for_timer.store(true, Ordering::Relaxed);
                }
            }
        });

        let input_owned = input.to_owned();
        let context = self.context.clone();
        let plugin_name = self.manifest.name.clone();
        tokio::spawn(async move {
            let outcome: Result<(), PluginError> = async_with!(context => |ctx| {
                stream_query(ctx, &input_owned, &tx, &cancel).await
            })
            .await;
            if let Err(err) = outcome
                && !matches!(err, PluginError::Cancelled)
            {
                eprintln!("plugins: {plugin_name}: query: {err}");
            }
        });

        rx
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        // Best effort: clear the interrupt flag and drop the runtime. The
        // AsyncRuntime/Context handle their own cleanup; no extra teardown
        // needed for Stage 4.
        self.interrupt_flag.store(false, Ordering::Relaxed);
    }
}

/// Iterate the plugin's async iterator, sending each yielded result through
/// `tx` as it arrives. Returns early on cancel/timeout.
async fn stream_query<'js>(
    ctx: Ctx<'js>,
    input: &str,
    tx: &mpsc::UnboundedSender<PluginResult>,
    cancel: &CancellationToken,
) -> Result<(), PluginError> {
    let query: Function<'js> = ctx
        .globals()
        .get(QUERY_GLOBAL)
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("`query` is not callable: {err}")))?;

    // Resolve `JSON.stringify` once and reuse for every yielded result.
    let stringify: Function<'js> = ctx
        .globals()
        .get::<_, Object<'js>>("JSON")
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("resolve JSON global: {err}")))?
        .get("stringify")
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("resolve JSON.stringify: {err}")))?;

    let (abort, signal) = Abort::create(&ctx)
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("build signal: {err}")))?;

    // Drive the host abort into the JS controller when the dispatcher cancels.
    // We do this inline via a select! in the iteration loop rather than
    // spawning a side task because the side task would need its own
    // `async_with!` context for the JS-side cancel call — single-threaded
    // ergonomics get ugly. Instead the next-step race fires the JS-side
    // abort once the token flips.

    let input_js = input
        .into_js(&ctx)
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("convert input: {err}")))?;
    let iter_or_iterable: Value<'js> = query
        .call((input_js, signal.clone()))
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("call query(): {err}")))?;

    let iter_obj = normalize_async_iterator(&ctx, iter_or_iterable)?;

    let next: Function<'js> = iter_obj
        .get("next")
        .catch(&ctx)
        .map_err(|err| PluginError::Js(format!("iterator.next missing: {err}")))?;

    loop {
        if cancel.is_cancelled() {
            // Best effort fire JS-side listeners so plugin code can clean up.
            let _ = abort.cancel(&ctx);
            return Err(PluginError::Cancelled);
        }
        let step_promise: Promise<'js> = next
            .call((This(iter_obj.clone()),))
            .catch(&ctx)
            .map_err(|err| PluginError::Js(format!("iterator.next() threw: {err}")))?;
        let next_fut = step_promise.into_future::<Object<'js>>();

        let step: Object<'js> = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let _ = abort.cancel(&ctx);
                return Err(PluginError::Cancelled);
            }
            result = next_fut => result
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("await iterator.next(): {err}")))?,
        };

        let done: bool = step
            .get("done")
            .catch(&ctx)
            .map_err(|err| PluginError::Js(format!("read step.done: {err}")))?;
        if done {
            return Ok(());
        }
        let value: Value<'js> = step
            .get("value")
            .catch(&ctx)
            .map_err(|err| PluginError::Js(format!("read step.value: {err}")))?;
        let json_str: String = stringify
            .call((value,))
            .catch(&ctx)
            .map_err(|err| PluginError::Js(format!("JSON.stringify yielded value: {err}")))?;
        let parsed: PluginResult = serde_json::from_str(&json_str)
            .map_err(|err| PluginError::InvalidResult(format!("{err}: {json_str}")))?;
        if tx.send(parsed).is_err() {
            // Receiver dropped — treat as a cancel.
            let _ = abort.cancel(&ctx);
            return Err(PluginError::Cancelled);
        }
    }
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
    // One-shot helper: returns either the iterator object directly (if `next`
    // exists) or the result of calling `Symbol.asyncIterator`.
    let pick: Function<'js> = ctx
        .clone()
        .eval::<Function<'js>, _>(ITERATOR_NORMALIZE_JS)
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("build iterator probe: {err}")))?;
    let iter: Object<'js> = pick
        .call((obj,))
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("normalize async iterator: {err}")))?;
    Ok(iter)
}

/// Resolve the per-plugin cache directory under the host's cache root.
///
/// `~/Library/Caches/high-beam/plugins/<name>/` on macOS,
/// `$XDG_CACHE_HOME/high-beam/plugins/<name>/` on Linux. Falls back to a
/// temp-dir-rooted path if `ProjectDirs` can't resolve (CI, exotic env).
fn default_cache_dir(plugin_name: &str) -> std::path::PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("", "", "high-beam") {
        dirs.cache_dir().join("plugins").join(plugin_name)
    } else {
        std::env::temp_dir()
            .join("high-beam")
            .join("plugins")
            .join(plugin_name)
    }
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
///
/// The capability table lives in [`crate::sdk::capability`] — adding a module
/// is a one-line table edit plus the loader arm below.
struct HighbeamLoader {
    capabilities: Vec<String>,
}

impl HighbeamLoader {
    fn new(capabilities: Vec<String>) -> Self {
        Self { capabilities }
    }
}

impl Loader for HighbeamLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> Result<Module<'js>, JsError> {
        if !name.starts_with(HIGHBEAM_SCHEME) {
            return Err(JsError::new_loading_message(
                name,
                format!("`{name}` is not a recognised highbeam module"),
            ));
        }

        // Capability gate: lookup the module in the central table, check that
        // at least one of its required caps is in the plugin's set. Modules
        // marked uncapped (match, platform) skip this gate entirely.
        if !capability::is_uncapped_module(name) {
            let Some(module_cap) = capability::for_module(name) else {
                return Err(JsError::new_loading_message(
                    name,
                    format!("`{name}` is not a recognised highbeam module"),
                ));
            };
            if !capability::grants_any(&self.capabilities, module_cap.any_of) {
                return Err(JsError::new_loading_message(
                    name,
                    format!(
                        "missing capability for `{name}`; declare one of {:?} in manifest.json",
                        module_cap.any_of
                    ),
                ));
            }
        }

        match name {
            ACTIONS_MODULE => Module::declare_def::<ActionsModule, _>(ctx.clone(), name),
            HTTP_MODULE => Module::declare_def::<HttpModule, _>(ctx.clone(), name),
            CLIPBOARD_MODULE => {
                Module::declare_def::<clipboard::ClipboardModule, _>(ctx.clone(), name)
            }
            FS_MODULE => Module::declare_def::<fs::FsModule, _>(ctx.clone(), name),
            ICONS_MODULE => Module::declare_def::<icons::IconsModule, _>(ctx.clone(), name),
            SYSTEM_MODULE => Module::declare_def::<system::SystemModule, _>(ctx.clone(), name),
            MATCH_MODULE => Module::declare_def::<MatchModule, _>(ctx.clone(), name),
            PLATFORM_MODULE => Module::declare_def::<PlatformModule, _>(ctx.clone(), name),
            other => Err(JsError::new_loading_message(
                name,
                format!("`{other}` is registered in the capability table but not in the loader"),
            )),
        }
    }
}
