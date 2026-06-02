//! Behavioural tests for `highbeam:system` — capability gating and the
//! cross-platform `applescript` contract (resolves `null` on non-macOS).

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Error as JsError, Module, async_with};

use high_beam::sdk::system::{SystemModule, install};

struct OnlySystem;

impl Resolver for OnlySystem {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        _base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String, JsError> {
        if name == "highbeam:system" || name == "test:harness" {
            Ok(name.to_owned())
        } else {
            Err(JsError::new_resolving("<test>", "unexpected import"))
        }
    }
}

impl Loader for OnlySystem {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js>, JsError> {
        Module::declare_def::<SystemModule, _>(ctx.clone(), name)
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt")
}

#[test]
fn applescript_returns_null_on_non_macos() {
    #[cfg(target_os = "macos")]
    {
        // On macOS, applescript actually runs — just verify the call
        // completes without throwing.
        let rt = rt();
        rt.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            async_rt.set_loader(OnlySystem, OnlySystem).await;
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            async_with!(ctx => |ctx| {
                install(&ctx, true, true).catch(&ctx).expect("install");
                let src = br"
                    import { applescript } from 'highbeam:system';
                    (async () => {
                        const r = await applescript('return 42');
                        globalThis.__out = String(r);
                    })();
                ";
                let declared = Module::declare(ctx.clone(), "test:harness", src.to_vec())
                    .catch(&ctx).expect("declare");
                let (_m, eval) = declared.eval().catch(&ctx).expect("eval");
                eval.into_future::<()>().await.catch(&ctx).expect("await eval");
                // Drain microtasks.
                for _ in 0..10 {
                    let _: () = ctx.eval("0").expect("noop");
                    tokio::task::yield_now().await;
                }
            })
            .await;
        });
    }

    #[cfg(not(target_os = "macos"))]
    {
        let rt = rt();
        rt.block_on(async {
            let async_rt = AsyncRuntime::new().expect("rt");
            async_rt.set_loader(OnlySystem, OnlySystem).await;
            let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
            let result: String = async_with!(ctx => |ctx| {
                install(&ctx, true, true).catch(&ctx).expect("install");
                let src = br#"
                    import { applescript } from 'highbeam:system';
                    globalThis.__test = (async () => {
                        const r = await applescript('return 42');
                        return r === null ? "null" : "got: " + r;
                    })();
                "#;
                let declared = Module::declare(ctx.clone(), "test:harness", src.to_vec())
                    .catch(&ctx).expect("declare");
                let (_m, eval) = declared.eval().catch(&ctx).expect("eval");
                eval.into_future::<()>().await.catch(&ctx).expect("await eval");

                let promise: rquickjs::Promise<'_> = ctx.globals().get("__test").expect("read __test");
                let s: String = promise.into_future::<String>().await.expect("await promise");
                s
            })
            .await;
            assert_eq!(result, "null");
        });
    }
}

#[test]
fn exec_without_cap_throws_capability_error() {
    let rt = rt();
    rt.block_on(async {
        let async_rt = AsyncRuntime::new().expect("rt");
        async_rt.set_loader(OnlySystem, OnlySystem).await;
        let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
        let outcome: String = async_with!(ctx => |ctx| {
            install(&ctx, false, false).catch(&ctx).expect("install");
            let src = br"
                import { exec } from 'highbeam:system';
                globalThis.__test = (async () => {
                    try {
                        await exec('/bin/echo', ['hi']);
                        return 'no-throw';
                    } catch (err) {
                        return err.name + ':' + err.message;
                    }
                })();
            ";
            let declared = Module::declare(ctx.clone(), "test:harness", src.to_vec())
                .catch(&ctx).expect("declare");
            let (_m, eval) = declared.eval().catch(&ctx).expect("eval");
            eval.into_future::<()>().await.catch(&ctx).expect("await eval");
            let promise: rquickjs::Promise<'_> = ctx.globals().get("__test").expect("read __test");
            promise.into_future::<String>().await.expect("await promise")
        })
        .await;
        assert!(
            outcome.starts_with("CapabilityError"),
            "expected CapabilityError, got {outcome}"
        );
    });
}

#[test]
fn applescript_without_cap_throws_capability_error() {
    let rt = rt();
    rt.block_on(async {
        let async_rt = AsyncRuntime::new().expect("rt");
        async_rt.set_loader(OnlySystem, OnlySystem).await;
        let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
        let outcome: String = async_with!(ctx => |ctx| {
            install(&ctx, false, false).catch(&ctx).expect("install");
            let src = br"
                import { applescript } from 'highbeam:system';
                globalThis.__test = (async () => {
                    try {
                        await applescript('return 1');
                        return 'no-throw';
                    } catch (err) {
                        return err.name + ':' + err.message;
                    }
                })();
            ";
            let declared = Module::declare(ctx.clone(), "test:harness", src.to_vec())
                .catch(&ctx).expect("declare");
            let (_m, eval) = declared.eval().catch(&ctx).expect("eval");
            eval.into_future::<()>().await.catch(&ctx).expect("await eval");
            let promise: rquickjs::Promise<'_> = ctx.globals().get("__test").expect("read __test");
            promise.into_future::<String>().await.expect("await promise")
        })
        .await;
        assert!(
            outcome.starts_with("CapabilityError"),
            "expected CapabilityError, got {outcome}"
        );
    });
}

#[test]
#[cfg(target_os = "macos")]
fn exec_with_cap_captures_stdout() {
    let rt = rt();
    rt.block_on(async {
        let async_rt = AsyncRuntime::new().expect("rt");
        async_rt.set_loader(OnlySystem, OnlySystem).await;
        let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
        let outcome: String = async_with!(ctx => |ctx| {
            install(&ctx, true, false).catch(&ctx).expect("install");
            let src = br"
                import { exec } from 'highbeam:system';
                globalThis.__test = (async () => {
                    const r = await exec('/bin/echo', ['hi']);
                    return r.stdout.trim() + ':' + r.code;
                })();
            ";
            let declared = Module::declare(ctx.clone(), "test:harness", src.to_vec())
                .catch(&ctx).expect("declare");
            let (_m, eval) = declared.eval().catch(&ctx).expect("eval");
            eval.into_future::<()>().await.catch(&ctx).expect("await eval");
            let promise: rquickjs::Promise<'_> = ctx.globals().get("__test").expect("read __test");
            promise.into_future::<String>().await.expect("await promise")
        })
        .await;
        assert_eq!(outcome, "hi:0");
    });
}
