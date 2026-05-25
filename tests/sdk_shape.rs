//! CI: assert each `highbeam:*` module's runtime exports match the
//! hand-written `.d.ts` under `sdk/highbeam/`. Drift surfaces as a failing
//! test rather than as cryptic plugin-author errors at runtime.

use rquickjs::loader::{Loader, Resolver};
use rquickjs::{AsyncContext, AsyncRuntime, Ctx, Error as JsError, Module, Object, async_with};

use high_beam::sdk::actions::ActionsModule;
use high_beam::sdk::clipboard::ClipboardModule;
use high_beam::sdk::fs::FsModule;
use high_beam::sdk::http::HttpModule;
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
        "highbeam:http" => &["get", "post", "put", "patch", "delete"],
        "highbeam:clipboard" => &["read", "write"],
        "highbeam:fs" => &["readDir", "readFile", "readText", "readCache", "writeCache"],
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
        other => {
            panic!("expected_for({other}): no expected list — keep this in sync with sdk/highbeam")
        }
    }
}

struct OneShotResolver(&'static str);

impl Resolver for OneShotResolver {
    fn resolve(&mut self, _ctx: &Ctx<'_>, _base: &str, name: &str) -> Result<String, JsError> {
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
    Http,
    Clipboard,
    Fs,
    Icons,
    Match,
    System,
    Platform,
    Settings,
    View,
}

impl Loader for OneShotLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> Result<Module<'js>, JsError> {
        match self {
            Self::Actions => Module::declare_def::<ActionsModule, _>(ctx.clone(), name),
            Self::Http => Module::declare_def::<HttpModule, _>(ctx.clone(), name),
            Self::Clipboard => Module::declare_def::<ClipboardModule, _>(ctx.clone(), name),
            Self::Fs => Module::declare_def::<FsModule, _>(ctx.clone(), name),
            Self::Icons => Module::declare_def::<IconsModule, _>(ctx.clone(), name),
            Self::Match => Module::declare_def::<MatchModule, _>(ctx.clone(), name),
            Self::System => Module::declare_def::<SystemModule, _>(ctx.clone(), name),
            Self::Platform => Module::declare_def::<PlatformModule, _>(ctx.clone(), name),
            Self::Settings => Module::declare_def::<SettingsModule, _>(ctx.clone(), name),
            Self::View => Module::declare_def::<ViewModule, _>(ctx.clone(), name),
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
        async_with!(ctx => |ctx| {
            // Tiny entry module imports the SDK module and stashes its
            // namespace on globalThis so we can introspect.
            let src = format!(
                r#"
                import * as ns from "{specifier}";
                globalThis.__ns = ns;
                "#
            );
            let declared = Module::declare(ctx.clone(), "shape:test", src.into_bytes())
                .expect("declare");
            let (_module, eval_promise) = declared.eval().expect("eval");
            eval_promise.into_future::<()>().await.expect("await eval");

            let ns: Object<'_> = ctx.globals().get("__ns").expect("read __ns");
            let mut found: Vec<String> = ns
                .keys::<String>()
                .filter_map(Result::ok)
                .collect();
            found.sort();
            let mut expected_sorted: Vec<String> =
                expected.iter().map(|s| (*s).to_string()).collect();
            expected_sorted.sort();

            assert_eq!(
                found, expected_sorted,
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
fn http_module_exports_match_dts() {
    assert_module_exports("highbeam:http", OneShotLoader::Http);
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
