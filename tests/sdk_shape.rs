//! CI: assert each `highbeam:*` module's runtime exports match the
//! hand-written `.d.ts` under `sdk/highbeam/`. Drift surfaces as a failing
//! test rather than as cryptic plugin-author errors at runtime.

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::{AsyncContext, AsyncRuntime, Ctx, Error as JsError, Module, Object};

use high_beam::sdk::actions::ActionsModule;
use high_beam::sdk::clipboard::ClipboardModule;
use high_beam::sdk::fs::FsModule;
use high_beam::sdk::icons::IconsModule;
use high_beam::sdk::r#match::MatchModule;
use high_beam::sdk::platform::PlatformModule;
use high_beam::sdk::settings::SettingsModule;
use high_beam::sdk::system::SystemModule;
use high_beam::sdk::view::ViewModule;

/// Mirrors `sdk/highbeam/<name>.d.ts`.
fn expected_for(name: &str) -> &'static [&'static str] {
    match name {
        "highbeam:actions" => &["openUrl", "copy", "exec", "reveal", "showView", "closeView"],
        "highbeam:clipboard" => &["read", "write"],
        "highbeam:fs" => &["readDir", "readFile", "readText", "readCache", "writeCache", "basename"],
        "highbeam:icons" => &["forPath"],
        "highbeam:match" => &["fuzzy"],
        "highbeam:system" => &["exec", "applescript"],
        "highbeam:platform" => &["os", "arch", "version", "isMacOS", "isLinux"],
        "highbeam:settings" => &["get", "getString", "getBool", "getInt"],
        "highbeam:view" => &[
            "Stack",
            "Divider",
            "Heading",
            "Text",
            "Spinner",
            "ProgressBar",
            "Button",
            "Input",
            "TextArea",
            "Image",
            "Row",
        ],
        other => node_expected_for(other),
    }
}

/// `node:*` module export lists, split out so `expected_for` stays under
/// clippy's function-length bar. The fs family lives here; the rest in
/// [`node_misc_expected_for`].
fn node_expected_for(name: &str) -> &'static [&'static str] {
    match name {
        "node:fs" => &[
            "default",
            "promises",
            "accessSync",
            "mkdirSync",
            "mkdtempSync",
            "readdirSync",
            "readFileSync",
            "rmdirSync",
            "rmSync",
            "statSync",
            "lstatSync",
            "writeFileSync",
            "constants",
            "chmodSync",
            "renameSync",
            "symlinkSync",
        ],
        "node:fs/promises" => &[
            "default",
            "access",
            "open",
            "readFile",
            "writeFile",
            "rename",
            "readdir",
            "mkdir",
            "mkdtemp",
            "rm",
            "rmdir",
            "stat",
            "lstat",
            "constants",
            "chmod",
            "symlink",
        ],
        other => node_misc_expected_for(other),
    }
}

/// `node:path`, `node:os`, `node:zlib`, `node:string_decoder` exports.
fn node_misc_expected_for(name: &str) -> &'static [&'static str] {
    match name {
        "node:path" => &[
            "default",
            "basename",
            "dirname",
            "extname",
            "format",
            "parse",
            "join",
            "resolve",
            "relative",
            "normalize",
            "isAbsolute",
            "delimiter",
            "sep",
        ],
        "node:os" => &[
            "default",
            "arch",
            "availableParallelism",
            "devNull",
            "endianness",
            "EOL",
            "getPriority",
            "homedir",
            "platform",
            "release",
            "setPriority",
            "tmpdir",
            "type",
            "userInfo",
            "version",
            "networkInterfaces",
            "cpus",
            "freemem",
            "totalmem",
            "hostname",
            "loadavg",
            "machine",
            "uptime",
        ],
        "node:string_decoder" => &["default", "StringDecoder"],
        "node:zlib" => &[
            "default",
            "deflate",
            "deflateSync",
            "deflateRaw",
            "deflateRawSync",
            "gzip",
            "gzipSync",
            "inflate",
            "inflateSync",
            "inflateRaw",
            "inflateRawSync",
            "gunzip",
            "gunzipSync",
            "brotliCompress",
            "brotliCompressSync",
            "brotliDecompress",
            "brotliDecompressSync",
            "zstdCompress",
            "zstdCompressSync",
            "zstdDecompress",
            "zstdDecompressSync",
        ],
        other => {
            panic!("expected_for({other}): no expected list — keep this in sync with sdk/highbeam")
        }
    }
}

struct OneShotResolver(&'static str);

impl Resolver for OneShotResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        _base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String, JsError> {
        if name == self.0 || name == "shape:test" {
            Ok(name.to_owned())
        } else {
            Err(JsError::new_resolving(
                "<shape-test>",
                format!("unexpected import: {name}"),
            ))
        }
    }
}

enum OneShotLoader {
    Actions,
    Clipboard,
    Fs,
    Icons,
    Match,
    System,
    Platform,
    Settings,
    View,
    NodePath,
    NodeFs,
    NodeFsPromises,
    NodeOs,
    NodeStringDecoder,
    NodeZlib,
}

impl Loader for OneShotLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js>, JsError> {
        match self {
            Self::Actions => Module::declare_def::<ActionsModule, _>(ctx.clone(), name),
            Self::Clipboard => Module::declare_def::<ClipboardModule, _>(ctx.clone(), name),
            Self::Fs => Module::declare_def::<FsModule, _>(ctx.clone(), name),
            Self::Icons => Module::declare_def::<IconsModule, _>(ctx.clone(), name),
            Self::Match => Module::declare_def::<MatchModule, _>(ctx.clone(), name),
            Self::System => Module::declare_def::<SystemModule, _>(ctx.clone(), name),
            Self::Platform => Module::declare_def::<PlatformModule, _>(ctx.clone(), name),
            Self::Settings => Module::declare_def::<SettingsModule, _>(ctx.clone(), name),
            Self::View => Module::declare_def::<ViewModule, _>(ctx.clone(), name),
            Self::NodePath => Module::declare_def::<llrt_path::PathModule, _>(ctx.clone(), name),
            Self::NodeFs => Module::declare_def::<llrt_fs::FsModule, _>(ctx.clone(), name),
            Self::NodeFsPromises => Module::declare_def::<llrt_fs::FsPromisesModule, _>(ctx.clone(), name),
            Self::NodeOs => Module::declare_def::<llrt_os::OsModule, _>(ctx.clone(), name),
            Self::NodeStringDecoder => {
                Module::declare_def::<llrt_string_decoder::StringDecoderModule, _>(ctx.clone(), name)
            }
            Self::NodeZlib => Module::declare_def::<llrt_zlib::ZlibModule, _>(ctx.clone(), name),
        }
    }
}

fn assert_module_exports(specifier: &'static str, loader: OneShotLoader) {
    let expected = expected_for(specifier);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt");
    rt.block_on(async {
        let async_rt = AsyncRuntime::new().expect("rt");
        async_rt.set_loader(OneShotResolver(specifier), loader).await;
        let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
        ctx.async_with(async move |ctx| {
            // Tiny entry module imports the SDK module and stashes its
            // namespace on globalThis so we can introspect.
            let src = format!(
                r#"
                import * as ns from "{specifier}";
                globalThis.__ns = ns;
                "#
            );
            let declared = Module::declare(ctx.clone(), "shape:test", src.into_bytes()).expect("declare");
            let (_module, eval_promise) = declared.eval().expect("eval");
            eval_promise.into_future::<()>().await.expect("await eval");

            let ns: Object<'_> = ctx.globals().get("__ns").expect("read __ns");
            let mut found: Vec<String> = ns.keys::<String>().filter_map(Result::ok).collect();
            found.sort();
            let mut expected_sorted: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
            expected_sorted.sort();

            assert_eq!(
                found,
                expected_sorted,
                "{specifier}: runtime exports do not match the .d.ts list.\n\
                 expected: {expected_sorted:?}\n\
                 got:      {found:?}\n\
                 (update either the Rust ModuleDef or sdk/highbeam/{name}.d.ts)",
                name = specifier.trim_start_matches("highbeam:"),
            );
        })
        .await;
    });
}

#[test]
fn actions_module_exports_match_dts() {
    assert_module_exports("highbeam:actions", OneShotLoader::Actions);
}

#[test]
fn node_os_module_exports_match_docs() {
    assert_module_exports("node:os", OneShotLoader::NodeOs);
}

#[test]
fn node_string_decoder_module_exports_match_docs() {
    assert_module_exports("node:string_decoder", OneShotLoader::NodeStringDecoder);
}

#[test]
fn node_zlib_module_exports_match_docs() {
    assert_module_exports("node:zlib", OneShotLoader::NodeZlib);
}

#[test]
fn clipboard_module_exports_match_dts() {
    assert_module_exports("highbeam:clipboard", OneShotLoader::Clipboard);
}

#[test]
fn fs_module_exports_match_dts() {
    assert_module_exports("highbeam:fs", OneShotLoader::Fs);
}

#[test]
fn icons_module_exports_match_dts() {
    assert_module_exports("highbeam:icons", OneShotLoader::Icons);
}

#[test]
fn match_module_exports_match_dts() {
    assert_module_exports("highbeam:match", OneShotLoader::Match);
}

#[test]
fn system_module_exports_match_dts() {
    assert_module_exports("highbeam:system", OneShotLoader::System);
}

#[test]
fn platform_module_exports_match_dts() {
    assert_module_exports("highbeam:platform", OneShotLoader::Platform);
}

#[test]
fn settings_module_exports_match_dts() {
    assert_module_exports("highbeam:settings", OneShotLoader::Settings);
}

#[test]
fn view_module_exports_match_dts() {
    assert_module_exports("highbeam:view", OneShotLoader::View);
}

#[test]
fn node_path_module_exports_match_docs() {
    assert_module_exports("node:path", OneShotLoader::NodePath);
}

#[test]
fn node_fs_module_exports_match_docs() {
    assert_module_exports("node:fs", OneShotLoader::NodeFs);
}

#[test]
fn node_fs_promises_module_exports_match_docs() {
    assert_module_exports("node:fs/promises", OneShotLoader::NodeFsPromises);
}
