//! Behavioural tests for `highbeam:icons`.
//!
//! Covers capability gating and the in-process cache (a second `forPath` call
//! returns the same data URI without re-extracting).

use rquickjs::loader::{Loader, Resolver};
use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Error as JsError, Module, async_with,
};

use high_beam::sdk::icons::{IconsModule, install};

struct OnlyIcons;

impl Resolver for OnlyIcons {
    fn resolve(&mut self, _ctx: &Ctx<'_>, _base: &str, name: &str) -> Result<String, JsError> {
        if name == "highbeam:icons" || name == "test:harness" {
            Ok(name.to_owned())
        } else {
            Err(JsError::new_resolving("<test>", "unexpected import"))
        }
    }
}

impl Loader for OnlyIcons {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> Result<Module<'js>, JsError> {
        Module::declare_def::<IconsModule, _>(ctx.clone(), name)
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt")
}

#[test]
fn for_path_without_cap_throws() {
    let rt = rt();
    let outcome: String = rt.block_on(async {
        let async_rt = AsyncRuntime::new().expect("rt");
        async_rt.set_loader(OnlyIcons, OnlyIcons).await;
        let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
        async_with!(ctx => |ctx| {
            install(&ctx, false).catch(&ctx).expect("install");
            let src = br"
                import { forPath } from 'highbeam:icons';
                globalThis.__test = (async () => {
                    try { await forPath('/tmp/nope'); return 'no-throw'; }
                    catch (err) { return err.name + ':' + err.message; }
                })();
            ";
            let declared = Module::declare(ctx.clone(), "test:harness", src.to_vec())
                .catch(&ctx).expect("declare");
            let (_m, eval) = declared.eval().catch(&ctx).expect("eval");
            eval.into_future::<()>().await.catch(&ctx).expect("await eval");
            let promise: rquickjs::Promise<'_> = ctx.globals().get("__test").expect("read __test");
            promise.into_future::<String>().await.expect("await promise")
        })
        .await
    });
    assert!(
        outcome.starts_with("CapabilityError"),
        "expected CapabilityError, got {outcome}"
    );
}

#[test]
fn for_path_with_cap_returns_data_uri_even_for_missing_path() {
    // The contract is "best-effort, never throw" — a missing file path must
    // resolve to the fallback data URI rather than blowing up.
    let rt = rt();
    let outcome: String = rt.block_on(async {
        let async_rt = AsyncRuntime::new().expect("rt");
        async_rt.set_loader(OnlyIcons, OnlyIcons).await;
        let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
        async_with!(ctx => |ctx| {
            install(&ctx, true).catch(&ctx).expect("install");
            let src = br"
                import { forPath } from 'highbeam:icons';
                globalThis.__test = (async () => {
                    const uri = await forPath('/path/that/does/not/exist/xyzzy');
                    return uri.startsWith('data:image/png;base64,') ? 'ok' : 'bad:' + uri.slice(0, 40);
                })();
            ";
            let declared = Module::declare(ctx.clone(), "test:harness", src.to_vec())
                .catch(&ctx).expect("declare");
            let (_m, eval) = declared.eval().catch(&ctx).expect("eval");
            eval.into_future::<()>().await.catch(&ctx).expect("await eval");
            let promise: rquickjs::Promise<'_> = ctx.globals().get("__test").expect("read __test");
            promise.into_future::<String>().await.expect("await promise")
        }).await
    });
    assert_eq!(outcome, "ok");
}

#[test]
fn for_path_returns_same_uri_on_cache_hit() {
    let rt = rt();
    let outcome: String = rt.block_on(async {
        let async_rt = AsyncRuntime::new().expect("rt");
        async_rt.set_loader(OnlyIcons, OnlyIcons).await;
        let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
        async_with!(ctx => |ctx| {
            install(&ctx, true).catch(&ctx).expect("install");
            let src = br"
                import { forPath } from 'highbeam:icons';
                globalThis.__test = (async () => {
                    const a = await forPath('/cache-test-path');
                    const b = await forPath('/cache-test-path');
                    return a === b ? 'same' : 'differ';
                })();
            ";
            let declared = Module::declare(ctx.clone(), "test:harness", src.to_vec())
                .catch(&ctx).expect("declare");
            let (_m, eval) = declared.eval().catch(&ctx).expect("eval");
            eval.into_future::<()>().await.catch(&ctx).expect("await eval");
            let promise: rquickjs::Promise<'_> = ctx.globals().get("__test").expect("read __test");
            promise.into_future::<String>().await.expect("await promise")
        })
        .await
    });
    assert_eq!(outcome, "same");
}
