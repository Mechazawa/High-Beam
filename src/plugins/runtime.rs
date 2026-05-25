//! rquickjs `Context` per plugin and the `query()` driver.
//!
//! Per plugin: own `AsyncRuntime` + `AsyncContext` with a memory cap from
//! `manifest.memoryMb`, an interrupt hook backed by a shared `AtomicBool`
//! that the wall-clock timer flips, and a resolver/loader pair that
//! whitelists `highbeam:*` specifiers + gates each module on capabilities.
//!
//! `run_query_stream` returns an `mpsc::Receiver<PluginResult>` the
//! dispatcher reads as the plugin yields. Cancelling the supplied token
//! aborts the in-flight query both JS-side (`signal.aborted`) and Rust-side
//! (interrupt hook for blocking loops).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value as JsonValue;

use rquickjs::function::This;
use rquickjs::loader::{Loader, Resolver};
use rquickjs::module::Evaluated;
use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Error as JsError, Function, IntoJs, Module, Object, Promise,
    Value, async_with,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::logging::LogErr;
use crate::plugins::log::{LogLevel, PluginLog};
use crate::plugins::manifest::Manifest;
use crate::plugins::result::PluginResult;
use crate::sdk::abort::{Abort, install_global_controller};
use crate::sdk::actions::ActionsModule;
use crate::sdk::capability;
use crate::sdk::clipboard;
use crate::sdk::console;
use crate::sdk::fs;
use crate::sdk::http::HttpModule;
use crate::sdk::icons;
use crate::sdk::r#match::MatchModule;
use crate::sdk::platform::PlatformModule;
use crate::sdk::settings::{self, SettingsModule};
use crate::sdk::system;
use crate::sdk::timers;
use crate::sdk::view::ViewModule;

/// JS-side normalizer: turns whatever `query()` returns into a real async
/// iterator. Re-eval cost per query is negligible.
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
const SETTINGS_MODULE: &str = "highbeam:settings";
const VIEW_MODULE: &str = "highbeam:view";
/// Slot on `globalThis` for the plugin's `query` export. We can't carry a
/// `Module<'js>` across iterator `.await` points, and
/// `Persistent<Function<'static>>` holds a raw `!Send` pointer that can't
/// cross `async_with!` under rquickjs's `parallel` feature.
const QUERY_GLOBAL: &str = "__highbeam_query";
/// Slots for the optional lifecycle hooks. Same reasoning as `QUERY_GLOBAL` —
/// the JS Function handle can't escape its evaluating context.
const ON_ENABLE_GLOBAL: &str = "__highbeam_on_enable";
const ON_DISABLE_GLOBAL: &str = "__highbeam_on_disable";

/// Which lifecycle hook to dispatch.
#[derive(Debug, Clone, Copy)]
pub enum HookKind {
    Enable,
    Disable,
}

impl HookKind {
    fn js_global(self) -> &'static str {
        match self {
            Self::Enable => ON_ENABLE_GLOBAL,
            Self::Disable => ON_DISABLE_GLOBAL,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Enable => "onEnable",
            Self::Disable => "onDisable",
        }
    }
}

/// Why a lifecycle hook is firing. Forwarded to the JS hook as the first
/// argument; the string form is `camelCase` to match the existing
/// plugin-result serialisation style.
#[derive(Debug, Clone, Copy)]
pub enum LifecycleReason {
    /// First time the host has seen this plugin (no recorded
    /// `last_loaded_version`).
    Install,
    /// Manifest `version` changed since the last load.
    Update,
    /// Manual `reload` verb (dev iteration).
    Reload,
}

impl LifecycleReason {
    fn as_js_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Reload => "reload",
        }
    }
}

/// A loaded, evaluated plugin ready to handle queries.
///
/// `AsyncContext` keeps the underlying `AsyncRuntime` alive via its own
/// internal `Arc`.
pub struct LoadedPlugin {
    pub manifest: Manifest,
    /// Directory the plugin was loaded from — preserved so the registry's
    /// reload path can re-read the manifest without reconstructing the
    /// search.
    pub plugin_dir: std::path::PathBuf,
    context: AsyncContext,
    timeout: Duration,
    // Mirrors what the interrupt hook captures; we keep the Arc alive here.
    interrupt_flag: Arc<AtomicBool>,
    log: Arc<PluginLog>,
    /// Whether the plugin exported an `onEnable` function.
    has_on_enable: bool,
    /// Whether the plugin exported an `onDisable` function.
    has_on_disable: bool,
    /// Fires when this plugin is being torn down (e.g. registry swap, daemon
    /// shutdown). Lifecycle hook tasks observe this and forward it to the
    /// JS-side `AbortSignal` they hand to the plugin.
    shutdown: CancellationToken,
}

/// Errors surfaced while loading or running a plugin.
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
    /// tests to keep cache writes inside a tmpdir.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::load`].
    pub async fn load_with_cache_dir(
        plugin_dir: &Path,
        manifest: Manifest,
        cache_dir: std::path::PathBuf,
    ) -> Result<Self, PluginError> {
        let log = PluginLog::for_plugin_dir(plugin_dir);
        Self::load_with_log(plugin_dir, manifest, cache_dir, log, HashMap::new()).await
    }

    /// Variant accepting an explicit [`PluginLog`] handle and the merged
    /// per-plugin options bag — used by the loader to pre-populate
    /// `highbeam:settings` and by tests that need to point the logfile at a
    /// tmpdir.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::load`].
    pub async fn load_with_log(
        plugin_dir: &Path,
        manifest: Manifest,
        cache_dir: std::path::PathBuf,
        log: Arc<PluginLog>,
        merged_options: HashMap<String, JsonValue>,
    ) -> Result<Self, PluginError> {
        let entry_path = manifest.entry_path(plugin_dir);
        let read_path = entry_path.clone();
        let source = tokio::task::spawn_blocking(move || std::fs::read_to_string(&read_path))
            .await
            .map_err(|join_err| {
                PluginError::Io(std::io::Error::other(format!(
                    "spawn_blocking join failed reading {}: {join_err}",
                    entry_path.display()
                )))
            })?
            .map_err(|err| {
                PluginError::Io(std::io::Error::new(
                    err.kind(),
                    format!("read {}: {err}", entry_path.display()),
                ))
            })?;

        let runtime = AsyncRuntime::new().map_err(|err| PluginError::Js(err.to_string()))?;

        // memoryMb = 0 means "leave the engine default" — never "unlimited".
        if manifest.memory_mb > 0 {
            let bytes = usize::try_from(manifest.memory_mb)
                .unwrap_or(usize::MAX)
                .saturating_mul(1024 * 1024);
            runtime.set_memory_limit(bytes).await;
        }

        // Wall-clock timer flips this; hook returns `true` whenever set,
        // causing QuickJS to raise an uncatchable exception and return.
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

        // Module is named `plugin:main` so backtraces print something
        // sensible. `declare → eval → await` lets top-level imports
        // (e.g. `import 'highbeam:actions'`) finish before we look up
        // `query`.
        let entry_path_str = entry_path.display().to_string();
        let source_bytes = source.into_bytes();
        let plugin_caps = manifest.capabilities.clone();
        let plugin_dir_owned = plugin_dir.to_path_buf();
        let log_for_ctx = Arc::clone(&log);
        let exports = async_with!(context => |ctx| {
            install_host_globals(
                &ctx,
                &log_for_ctx,
                &plugin_caps,
                cache_dir.clone(),
                plugin_dir_owned.clone(),
                &merged_options,
            )?;

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

            // onEnable / onDisable are optional — a `module.get` for a
            // missing export raises, so swallow that into a `None` rather
            // than treating it as a hard error.
            let has_on_enable = stash_optional_hook(&ctx, &module, "onEnable", ON_ENABLE_GLOBAL)?;
            let has_on_disable = stash_optional_hook(&ctx, &module, "onDisable", ON_DISABLE_GLOBAL)?;

            Ok::<_, PluginError>(ExportFlags { has_on_enable, has_on_disable })
        })
        .await?;

        let timeout = Duration::from_millis(manifest.timeout_ms);
        Ok(Self {
            manifest,
            plugin_dir: plugin_dir.to_path_buf(),
            context,
            timeout,
            interrupt_flag,
            log,
            has_on_enable: exports.has_on_enable,
            has_on_disable: exports.has_on_disable,
            shutdown: CancellationToken::new(),
        })
    }

    /// The per-plugin log writer. Exposed for the loader (which logs load-time
    /// failures into the same file) and tests.
    #[must_use]
    pub fn log(&self) -> Arc<PluginLog> {
        Arc::clone(&self.log)
    }

    /// Stream results from `query(input, signal)` over an `mpsc` channel.
    /// The receiver closes when the plugin's iterator drains, the timeout
    /// fires, the plugin throws, or the caller cancels.
    #[must_use]
    pub fn run_query_stream(&self, input: &str, cancel: CancellationToken) -> mpsc::UnboundedReceiver<PluginResult> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.interrupt_flag.store(false, Ordering::Relaxed);
        let flag_for_timer = Arc::clone(&self.interrupt_flag);
        let timeout = self.timeout;
        // Distinguishes "timer tripped" from "caller cancelled" so the
        // post-run logger can report budget exhaustion specifically.
        let timed_out = Arc::new(AtomicBool::new(false));
        let timed_out_for_timer = Arc::clone(&timed_out);

        let cancel_for_timer = cancel.clone();
        // Interrupting tight JS loops requires a thread off the executor: if
        // the watchdog rode the tokio scheduler, a plugin spinning on
        // `while(true){}` would starve it forever. `spawn_blocking` parks on
        // the dedicated blocking pool so the deadline always fires.
        tokio::task::spawn_blocking(move || {
            const TICK: Duration = Duration::from_millis(10);
            let start = std::time::Instant::now();

            while start.elapsed() < timeout {
                if cancel_for_timer.is_cancelled() {
                    flag_for_timer.store(true, Ordering::Relaxed);
                    return;
                }
                let remaining = timeout.saturating_sub(start.elapsed());
                std::thread::sleep(remaining.min(TICK));
            }
            timed_out_for_timer.store(true, Ordering::Relaxed);
            flag_for_timer.store(true, Ordering::Relaxed);
            cancel_for_timer.cancel();
        });

        let input_owned = input.to_owned();
        let context = self.context.clone();
        let log_for_task = Arc::clone(&self.log);
        let timeout_ms = self.manifest.timeout_ms;
        let memory_mb = self.manifest.memory_mb;
        tokio::spawn(async move {
            let input_for_stream = input_owned.clone();
            let outcome: Result<(), PluginError> = async_with!(context => |ctx| {
                stream_query(ctx, &input_for_stream, &tx, &cancel).await
            })
            .await;
            log_query_outcome(
                &log_for_task,
                &outcome,
                &input_owned,
                timed_out.load(Ordering::Relaxed),
                timeout_ms,
                memory_mb,
            );
        });

        rx
    }

    /// Run the JS view runtime's `init(handle, props)` for a freshly-pushed
    /// view frame. Lazily installs the runtime + bridge globals on first
    /// use. Triggers the plugin's `setup → first render → mounted`
    /// sequence; the rendered tree comes back to the host via the
    /// `__highbeam_paint_tree` bridge.
    ///
    /// `bridge` carries the dispatch + close-request closures the JS
    /// runtime calls from its `on*` handlers and `render → null` path.
    /// The host owns its construction so the closures can capture
    /// Slint-thread state without leaking that surface into the
    /// per-plugin runtime layer.
    ///
    /// # Errors
    ///
    /// Propagates JS errors from installing the runtime or invoking
    /// `init` (including any uncaught throw from the plugin's `setup`
    /// or first `render`).
    pub async fn view_init(
        &self,
        handle: u64,
        props: &JsonValue,
        bridge: Arc<crate::sdk::view::RuntimeBridge>,
    ) -> Result<(), PluginError> {
        let props_json = props.to_string();
        async_with!(self.context => |ctx| {
            crate::sdk::view::install_runtime(&ctx, bridge)
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("install view runtime: {err}")))?;
            crate::sdk::view::invoke_init(&ctx, handle, &props_json)
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("view init: {err}")))?;
            Ok::<_, PluginError>(())
        })
        .await
    }

    /// Tear down a view frame on the JS side — runs `unmounted`, fires
    /// the mounted-signal's abort, and drops the instance + its registry
    /// entry so the view object itself becomes GC-eligible.
    ///
    /// # Errors
    ///
    /// Propagates JS errors from invoking `close`.
    pub async fn view_close(&self, handle: u64) -> Result<(), PluginError> {
        async_with!(self.context => |ctx| {
            crate::sdk::view::invoke_close(&ctx, handle)
                .catch(&ctx)
                .map_err(|err| PluginError::Js(format!("view close: {err}")))?;
            Ok::<_, PluginError>(())
        })
        .await
    }

    /// Whether this plugin exported the requested hook. Callers can skip
    /// scheduling work for plugins that won't react.
    #[must_use]
    pub fn has_hook(&self, kind: HookKind) -> bool {
        match kind {
            HookKind::Enable => self.has_on_enable,
            HookKind::Disable => self.has_on_disable,
        }
    }

    /// Fire one lifecycle hook in the background.
    ///
    /// Returns immediately with a `JoinHandle` the caller may drop — the
    /// task runs to completion (or until the plugin's `shutdown` token
    /// fires, signalling the runtime is being torn down). Outcome is
    /// recorded in `plugin.log`; nothing surfaces to the caller. No-op
    /// if the plugin didn't export the requested hook.
    pub fn run_lifecycle_hook(&self, kind: HookKind, reason: LifecycleReason) -> tokio::task::JoinHandle<()> {
        if !self.has_hook(kind) {
            return tokio::spawn(async {});
        }
        let context = self.context.clone();
        let log = Arc::clone(&self.log);
        let shutdown = self.shutdown.clone();
        let plugin_name = self.manifest.name.clone();
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let outcome: Result<(), PluginError> = async_with!(context => |ctx| {
                run_hook(&ctx, kind, reason, &shutdown).await
            })
            .await;
            log_hook_outcome(&log, &plugin_name, kind, reason, &outcome, started.elapsed());
        })
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        // Invariant: `interrupt_flag` is owned by exactly one plugin
        // generation. The registry serialises reloads (no parallel
        // installs/reloads of the same plugin), so the Arc isn't shared
        // with the next-generation `LoadedPlugin`, and clearing the flag
        // here can never race with a fresh plugin's watchdog. If reloads
        // ever go parallel, this reset must move into the constructor
        // (where the new plugin owns a fresh AtomicBool) instead.
        self.interrupt_flag.store(false, Ordering::Relaxed);
        // Tells any still-running lifecycle hook task it's time to wind
        // down — the JS-side AbortSignal gets fired by the task itself.
        self.shutdown.cancel();
    }
}

/// Set of hook exports we found on the plugin's entry module.
struct ExportFlags {
    has_on_enable: bool,
    has_on_disable: bool,
}

/// Try to read an optional `Function` export from `module` and stash it on
/// the JS globals under `global_name`. Returns `true` when the export was
/// present.
fn stash_optional_hook<'js>(
    ctx: &Ctx<'js>,
    module: &Module<'js, Evaluated>,
    export_name: &str,
    global_name: &str,
) -> Result<bool, PluginError> {
    let Ok(func) = module.get::<_, Function<'js>>(export_name) else {
        return Ok(false);
    };
    ctx.globals()
        .set(global_name, func)
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("stash {export_name} global: {err}")))?;
    Ok(true)
}

/// Install every host-side global the plugin's `query` body might touch
/// before the entry module evaluates. Order matters only for `console` (so
/// later installs can route through it once we move logging into JS); the
/// rest are independent.
fn install_host_globals<S: std::hash::BuildHasher>(
    ctx: &Ctx<'_>,
    log: &Arc<PluginLog>,
    plugin_caps: &[String],
    cache_dir: std::path::PathBuf,
    plugin_dir: std::path::PathBuf,
    merged_options: &HashMap<String, JsonValue, S>,
) -> Result<(), PluginError> {
    console::install(ctx, log)
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("install console: {err}")))?;

    install_global_controller(ctx)
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("install AbortController: {err}")))?;

    timers::install(ctx)
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("install setTimeout: {err}")))?;

    // Per-plugin options bag; populated even when the plugin declared no
    // options so `get('anything')` is always callable and returns undefined.
    settings::install(ctx, merged_options)
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("install settings: {err}")))?;

    // Per-plugin SDK bindings get installed unconditionally; the inert
    // no-cap stubs are cheap and keep the module evaluate path branch-free.
    let can_read = plugin_caps.iter().any(|c| c == "clipboard.read");
    let can_write = plugin_caps.iter().any(|c| c == "clipboard.write");
    clipboard::install(ctx, can_read, can_write)
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("install clipboard: {err}")))?;

    let can_fs_read = plugin_caps.iter().any(|c| c == "fs.read");
    let can_fs_cache = plugin_caps.iter().any(|c| c == "fs.cache");
    fs::install(ctx, can_fs_read, can_fs_cache, cache_dir, plugin_dir)
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("install fs: {err}")))?;

    let can_icons = plugin_caps.iter().any(|c| c == "icons");
    icons::install(ctx, can_icons)
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("install icons: {err}")))?;

    let can_system_exec = plugin_caps.iter().any(|c| c == "system.exec");
    let can_system_applescript = plugin_caps.iter().any(|c| c == "system.applescript");
    system::install(ctx, can_system_exec, can_system_applescript)
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("install system: {err}")))?;

    Ok(())
}

/// Map a `query()` outcome to a line in the plugin's logfile.
///
/// Distinguishes timeout (WARN), out-of-memory (ERROR), and other exceptions
/// (ERROR); `Cancelled` writes no line — the host abandoned the query.
fn log_query_outcome(
    log: &PluginLog,
    outcome: &Result<(), PluginError>,
    input: &str,
    timed_out: bool,
    timeout_ms: u64,
    memory_mb: u32,
) {
    let Err(err) = outcome else {
        return;
    };

    // Timeout wins over Cancelled — the host-driven cancel that flips on
    // a timeout is the cause the user wants to see.
    if timed_out || matches!(err, PluginError::Timeout) {
        log.write(
            LogLevel::Warn,
            &format!("query timed out after {timeout_ms}ms (manifest budget: {timeout_ms}ms); input: {input:?}"),
        );

        return;
    }

    if matches!(err, PluginError::Cancelled) {
        return;
    }

    let msg = err.to_string();

    if msg.to_ascii_lowercase().contains("out of memory") {
        log.write(
            LogLevel::Error,
            &format!("out of memory; memory limit {memory_mb}mb exceeded; input: {input:?}"),
        );

        return;
    }

    log.write(LogLevel::Error, &format!("query threw: {msg}; input: {input:?}"));
}

/// Call one lifecycle hook and await its returned Promise.
///
/// Builds a fresh `AbortSignal` whose cancellation tracks `shutdown` so the
/// plugin's JS code can observe daemon-shutdown / plugin-tear-down via the
/// same `signal.aborted` shape it sees in `query`.
async fn run_hook<'js>(
    ctx: &Ctx<'js>,
    kind: HookKind,
    reason: LifecycleReason,
    shutdown: &CancellationToken,
) -> Result<(), PluginError> {
    let hook: Function<'js> = ctx
        .globals()
        .get(kind.js_global())
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("{label} not callable: {err}", label = kind.label())))?;

    let (abort, signal) = Abort::create(ctx)
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("build signal: {err}")))?;

    let reason_js = reason
        .as_js_str()
        .into_js(ctx)
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("convert reason: {err}")))?;
    let promise: Promise<'js> = hook
        .call((reason_js, signal.clone()))
        .catch(ctx)
        .map_err(|err| PluginError::Js(format!("call {label}(): {err}", label = kind.label())))?;
    let fut = promise.into_future::<()>();

    let result = tokio::select! {
        biased;
        () = shutdown.cancelled() => {
            abort.cancel(ctx).log_debug("plugin hook: fire abort listeners on shutdown");
            Err(PluginError::Cancelled)
        }
        res = fut => res
            .catch(ctx)
            .map_err(|err| PluginError::Js(format!("await {label}(): {err}", label = kind.label()))),
    };

    // Cancel paths already disposed via `abort.cancel`; release only on
    // the natural-completion arm. Either way the JS controller is now out
    // of the registry.
    if result.is_ok() {
        abort
            .release(ctx)
            .log_debug("plugin hook: release abort controller after success");
    }
    result
}

/// Map a hook outcome to a `plugin.log` line.
fn log_hook_outcome(
    log: &PluginLog,
    plugin_name: &str,
    kind: HookKind,
    reason: LifecycleReason,
    outcome: &Result<(), PluginError>,
    elapsed: Duration,
) {
    let label = kind.label();
    let reason_str = reason.as_js_str();

    match outcome {
        Ok(()) => log.write(
            LogLevel::Info,
            &format!("hook {label}({reason_str}) completed in {:.1}s", elapsed.as_secs_f32()),
        ),
        Err(PluginError::Cancelled) => log.write(LogLevel::Info, &format!("hook {label}({reason_str}) cancelled")),
        Err(err) => {
            // tracing line too so `tail -f plugins/*/plugin.log` isn't the
            // only way to notice a misbehaving hook in dev.
            tracing::warn!(plugin = %plugin_name, hook = label, reason = reason_str, %err, "plugin lifecycle hook failed");
            log.write(LogLevel::Error, &format!("hook {label}({reason_str}) failed: {err}"));
        }
    }
}

/// Iterate the plugin's async iterator, sending each yielded result through
/// `tx`. Returns early on cancel/timeout.
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

    // The JS-side abort is fired inline from the select! below — spawning a
    // side task would need its own `async_with!` context to call back into
    // JS, which gets awkward under single-threaded rquickjs.

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
            // Fire JS-side listeners so plugin code can clean up.
            abort
                .cancel(&ctx)
                .log_debug("query stream: fire abort listeners on pre-step cancel");

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
                abort.cancel(&ctx).log_debug("query stream: fire abort listeners on mid-step cancel");
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
            // Natural completion: dispose the JS-side controller so the
            // registry doesn't accumulate one entry per query over the
            // plugin context's lifetime.
            abort
                .release(&ctx)
                .log_debug("query stream: release abort controller on natural completion");

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
        let parsed: PluginResult =
            serde_json::from_str(&json_str).map_err(|err| PluginError::InvalidResult(format!("{err}: {json_str}")))?;

        if tx.send(parsed).is_err() {
            // Receiver dropped — treat as a cancel.
            abort
                .cancel(&ctx)
                .log_debug("query stream: fire abort listeners after receiver dropped");

            return Err(PluginError::Cancelled);
        }
    }
}

/// If the value already has `next`, return it as-is; otherwise call its
/// `Symbol.asyncIterator`.
fn normalize_async_iterator<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Object<'js>, PluginError> {
    let obj: Object<'js> = value
        .try_into_object()
        .map_err(|_| PluginError::Js("query() did not return an object".into()))?;
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
/// Falls back to a temp-dir-rooted path when the platform cache dir can't
/// be resolved (e.g. headless test envs without `$HOME`).
pub(crate) fn default_cache_dir(plugin_name: &str) -> std::path::PathBuf {
    crate::paths::cache_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("high-beam"))
        .join("plugins")
        .join(plugin_name)
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
                format!("high-beam plugins may only import from `highbeam:*` (got {name:?})"),
            ))
        }
    }
}

/// Loads exactly the `highbeam:*` modules the plugin's capabilities permit.
/// Adding a module is a one-line table edit in [`crate::sdk::capability`]
/// plus a loader arm below.
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

        // Modules marked uncapped (match, platform) skip the gate.
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
            CLIPBOARD_MODULE => Module::declare_def::<clipboard::ClipboardModule, _>(ctx.clone(), name),
            FS_MODULE => Module::declare_def::<fs::FsModule, _>(ctx.clone(), name),
            ICONS_MODULE => Module::declare_def::<icons::IconsModule, _>(ctx.clone(), name),
            SYSTEM_MODULE => Module::declare_def::<system::SystemModule, _>(ctx.clone(), name),
            MATCH_MODULE => Module::declare_def::<MatchModule, _>(ctx.clone(), name),
            PLATFORM_MODULE => Module::declare_def::<PlatformModule, _>(ctx.clone(), name),
            SETTINGS_MODULE => Module::declare_def::<SettingsModule, _>(ctx.clone(), name),
            VIEW_MODULE => Module::declare_def::<ViewModule, _>(ctx.clone(), name),
            other => Err(JsError::new_loading_message(
                name,
                format!("`{other}` is registered in the capability table but not in the loader"),
            )),
        }
    }
}
