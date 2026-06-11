//! Behavioural tests for `highbeam:fs` — capability gating, recursive
//! readDir traversal, and cache scoping (path-traversal names are rejected).

use std::path::PathBuf;

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Error as JsError, Module};

use high_beam::sdk::fs::{FsModule, install};

mod common;
use common::fresh_tmp;

struct OnlyFs;

impl Resolver for OnlyFs {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        _base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String, JsError> {
        if name == "highbeam:fs" || name == "test:harness" {
            Ok(name.to_owned())
        } else {
            Err(JsError::new_resolving("<test>", "unexpected import"))
        }
    }
}

impl Loader for OnlyFs {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js>, JsError> {
        Module::declare_def::<FsModule, _>(ctx.clone(), name)
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt")
}

async fn run_script(can_read: bool, can_cache: bool, cache_dir: PathBuf, src: Vec<u8>) -> String {
    let async_rt = AsyncRuntime::new().expect("rt");
    async_rt.set_loader(OnlyFs, OnlyFs).await;
    let ctx = AsyncContext::full(&async_rt).await.expect("ctx");
    let plugin_dir = std::env::temp_dir();
    ctx.async_with(async move |ctx| {
        install(&ctx, can_read, can_cache, cache_dir, plugin_dir)
            .catch(&ctx)
            .expect("install");
        let declared = Module::declare(ctx.clone(), "test:harness", src)
            .catch(&ctx)
            .expect("declare");
        let (_m, eval) = declared.eval().catch(&ctx).expect("eval");
        eval.into_future::<()>().await.catch(&ctx).expect("await eval");
        let promise: rquickjs::Promise<'_> = ctx.globals().get("__test").expect("read __test");
        match promise.into_future::<String>().await.catch(&ctx) {
            Ok(s) => s,
            Err(e) => panic!("await promise: {e}"),
        }
    })
    .await
}

#[test]
fn basename_strips_directories_and_trailing_slashes() {
    let dir = fresh_tmp("basename");
    let cache = dir.join("cache");
    let rt = rt();
    // Both caps false — basename is a pure helper and must work ungated.
    let outcome = rt.block_on(run_script(
        false,
        false,
        cache,
        br"
            import { basename } from 'highbeam:fs';
            globalThis.__test = Promise.resolve([
                basename('/foo/bar.txt'),
                basename('/foo/bar/'),
                basename('relative.md'),
                basename('/'),
                basename(''),
                basename('..'),
            ].join('|'));
        "
        .to_vec(),
    ));
    assert_eq!(outcome, "bar.txt|bar|relative.md|||..");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_text_without_cap_throws() {
    let dir = fresh_tmp("no-cap");
    let cache = dir.join("cache");
    let rt = rt();
    let outcome = rt.block_on(run_script(
        false,
        false,
        cache,
        br"
            import { readText } from 'highbeam:fs';
            globalThis.__test = (async () => {
                try { await readText('/etc/hosts'); return 'no-throw'; }
                catch (err) { return err.name + ':' + err.message; }
            })();
        "
        .to_vec(),
    ));
    assert!(
        outcome.starts_with("CapabilityError"),
        "expected CapabilityError, got {outcome}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_text_with_cap_returns_file_contents() {
    let dir = fresh_tmp("read-text");
    let file = dir.join("hello.txt");
    std::fs::write(&file, "hello world").unwrap();
    let path_str = file.to_string_lossy().into_owned();
    let src = format!(
        r"
            import {{ readText }} from 'highbeam:fs';
            globalThis.__test = readText({path_str:?});
        ",
    )
    .into_bytes();
    let rt = rt();
    let outcome = rt.block_on(run_script(true, false, dir.join("cache"), src));
    assert_eq!(outcome, "hello world");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_dir_recursive_yields_nested_entries() {
    let dir = fresh_tmp("readdir-rec");
    std::fs::create_dir_all(dir.join("nested")).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    std::fs::write(dir.join("nested/b.txt"), "b").unwrap();
    let path_str = dir.to_string_lossy().into_owned();
    let src = format!(
        r"
            import {{ readDir }} from 'highbeam:fs';
            globalThis.__test = (async () => {{
                const names = [];
                for await (const e of readDir({path_str:?}, {{ recursive: true }})) {{
                    names.push(e.name);
                }}
                names.sort();
                return names.join(',');
            }})();
        ",
    )
    .into_bytes();
    let rt = rt();
    let outcome = rt.block_on(run_script(true, false, dir.join("cache"), src));
    assert!(outcome.contains("a.txt"), "got {outcome}");
    assert!(outcome.contains("b.txt"), "got {outcome}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_cache_then_read_cache_roundtrips() {
    let dir = fresh_tmp("cache-rt");
    let cache = dir.join("cache");
    let src = br"
        import { writeCache, readCache } from 'highbeam:fs';
        globalThis.__test = (async () => {
            await writeCache('blob.txt', 'hello');
            const bytes = await readCache('blob.txt');
            let s = '';
            for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
            return s;
        })();
    "
    .to_vec();
    let rt = rt();
    let outcome = rt.block_on(run_script(false, true, cache, src));
    assert_eq!(outcome, "hello");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_cache_returns_null_for_missing() {
    let dir = fresh_tmp("cache-miss");
    let cache = dir.join("cache");
    let src = br"
        import { readCache } from 'highbeam:fs';
        globalThis.__test = (async () => {
            const r = await readCache('nope.txt');
            return r === null ? 'null' : 'got-bytes';
        })();
    "
    .to_vec();
    let rt = rt();
    let outcome = rt.block_on(run_script(false, true, cache, src));
    assert_eq!(outcome, "null");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_cache_rejects_path_traversal() {
    let dir = fresh_tmp("cache-traversal");
    let cache = dir.join("cache");
    let src = br"
        import { writeCache } from 'highbeam:fs';
        globalThis.__test = (async () => {
            try {
                await writeCache('../escape.txt', 'oops');
                return 'no-throw';
            } catch (err) {
                return err.name + ':' + err.message;
            }
        })();
    "
    .to_vec();
    let rt = rt();
    let outcome = rt.block_on(run_script(false, true, cache, src));
    assert!(outcome.starts_with("FsError"), "expected FsError, got {outcome}");
    let _ = std::fs::remove_dir_all(&dir);
}
