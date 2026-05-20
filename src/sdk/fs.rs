//! Host implementation of the `highbeam:fs` module.
//!
//! Surface:
//!
//! ```ts
//! import { readDir, readFile, readText, readCache, writeCache } from 'highbeam:fs';
//!
//! for await (const entry of readDir('/Applications', { recursive: true })) {
//!     if (entry.isDir && entry.name.endsWith('.app')) yield entry;
//! }
//! const bytes = await readFile('/tmp/x.bin');
//! const text = await readText('/tmp/x.txt');
//! await writeCache('apps.json', JSON.stringify(apps));
//! const cached = await readCache('apps.json');
//! ```
//!
//! Capabilities:
//!   * `fs.read` grants `readDir`, `readFile`, `readText`
//!   * `fs.cache` grants `readCache`, `writeCache` — scoped to the plugin's
//!     own cache dir; cross-plugin reads are impossible by construction
//!
//! Cache name sanitization rejects path-traversal attempts (`..`, leading `/`,
//! embedded separators). Cache files live one flat layer deep under the
//! plugin's own directory.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rquickjs::function::{Async, Opt};
use rquickjs::{Ctx, Function, Object, Result as JsResult, TypedArray, Value, module::ModuleDef};
use tokio_util::sync::CancellationToken;

use crate::sdk::abort;

const READ_DIR_GLOBAL: &str = "__highbeam_fs_read_dir";
const READ_FILE_GLOBAL: &str = "__highbeam_fs_read_file";
const READ_TEXT_GLOBAL: &str = "__highbeam_fs_read_text";
const READ_CACHE_GLOBAL: &str = "__highbeam_fs_read_cache";
const WRITE_CACHE_GLOBAL: &str = "__highbeam_fs_write_cache";

pub struct FsModule;

impl ModuleDef for FsModule {
    fn declare(decl: &rquickjs::module::Declarations<'_>) -> JsResult<()> {
        decl.declare("readDir")?;
        decl.declare("readFile")?;
        decl.declare("readText")?;
        decl.declare("readCache")?;
        decl.declare("writeCache")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> JsResult<()> {
        let globals = ctx.globals();
        for (export_name, global_name, cap) in [
            ("readDir", READ_DIR_GLOBAL, "fs.read"),
            ("readFile", READ_FILE_GLOBAL, "fs.read"),
            ("readText", READ_TEXT_GLOBAL, "fs.read"),
            ("readCache", READ_CACHE_GLOBAL, "fs.cache"),
            ("writeCache", WRITE_CACHE_GLOBAL, "fs.cache"),
        ] {
            let val: Value<'js> = globals
                .get(global_name)
                .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
            if let Some(f) = val.into_function() {
                exports.export(export_name, f)?;
            } else {
                exports.export(export_name, cap_error_thrower(ctx, cap)?)?;
            }
        }
        Ok(())
    }
}

fn cap_error_thrower<'js>(ctx: &Ctx<'js>, cap: &'static str) -> JsResult<Function<'js>> {
    Function::new(
        ctx.clone(),
        Async(
            move |ctx: Ctx<'js>, _args: rquickjs::function::Rest<Value<'js>>| async move {
                Err::<(), _>(throw_cap(&ctx, cap))
            },
        ),
    )
}

/// Per-plugin bindings, installed before the plugin's entry module evaluates.
///
/// # Errors
///
/// Propagates JS errors from function construction or global assignment.
pub fn install<'js>(
    ctx: &Ctx<'js>,
    can_read: bool,
    can_cache: bool,
    cache_dir: PathBuf,
) -> JsResult<()> {
    let cache_dir_for_read = cache_dir.clone();
    let cache_dir_for_write = cache_dir;

    let read_dir = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>, path: String, opts: Opt<Value<'js>>| -> JsResult<Object<'js>> {
            if !can_read {
                return Err(throw_cap(&ctx, "fs.read"));
            }
            let opts_val = opts.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
            build_dir_async_iterator(&ctx, &path, &opts_val)
        },
    )?;

    let read_file = Function::new(
        ctx.clone(),
        Async(
            move |ctx: Ctx<'js>, path: String, opts: Opt<Value<'js>>| async move {
                if !can_read {
                    return Err::<TypedArray<'js, u8>, _>(throw_cap(&ctx, "fs.read"));
                }
                let opts_val = opts.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                read_file_impl(ctx, path, opts_val).await
            },
        ),
    )?;

    let read_text = Function::new(
        ctx.clone(),
        Async(
            move |ctx: Ctx<'js>, path: String, opts: Opt<Value<'js>>| async move {
                if !can_read {
                    return Err::<String, _>(throw_cap(&ctx, "fs.read"));
                }
                let opts_val = opts.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                read_text_impl(ctx, path, opts_val).await
            },
        ),
    )?;

    let read_cache = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, name: String| {
            let dir = cache_dir_for_read.clone();
            async move {
                if !can_cache {
                    return Err::<Value<'js>, _>(throw_cap(&ctx, "fs.cache"));
                }
                read_cache_impl(ctx, name, dir).await
            }
        }),
    )?;

    let write_cache = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, name: String, data: Value<'js>| {
            let dir = cache_dir_for_write.clone();
            async move {
                if !can_cache {
                    return Err::<(), _>(throw_cap(&ctx, "fs.cache"));
                }
                write_cache_impl(ctx, name, data, dir).await
            }
        }),
    )?;

    let globals = ctx.globals();
    globals.set(READ_DIR_GLOBAL, read_dir)?;
    globals.set(READ_FILE_GLOBAL, read_file)?;
    globals.set(READ_TEXT_GLOBAL, read_text)?;
    globals.set(READ_CACHE_GLOBAL, read_cache)?;
    globals.set(WRITE_CACHE_GLOBAL, write_cache)?;
    Ok(())
}

/// JS-side glue: wrap a host-driven `next()` callable into an async iterator
/// whose `Symbol.asyncIterator` returns itself. Plugin code does `for await
/// (const x of readDir(p))` and gets one entry at a time.
fn readdir_iterator_js() -> &'static str {
    static SRC: OnceLock<String> = OnceLock::new();
    SRC.get_or_init(|| {
        r"
        ((nextHost) => {
            return {
                async next() {
                    const r = await nextHost();
                    if (r === null) return { value: undefined, done: true };
                    return { value: r, done: false };
                },
                [Symbol.asyncIterator]() { return this; },
            };
        })
        "
        .trim()
        .to_owned()
    })
}

fn build_dir_async_iterator<'js>(
    ctx: &Ctx<'js>,
    path: &str,
    opts: &Value<'js>,
) -> JsResult<Object<'js>> {
    let recursive = opts
        .as_object()
        .and_then(|o| o.get::<_, bool>("recursive").ok())
        .unwrap_or(false);
    let signal: Option<Object<'js>> = opts.as_object().and_then(|o| o.get("signal").ok());
    let token = match &signal {
        Some(s) => abort::token_from_js_signal(ctx, s)?,
        None => CancellationToken::new(),
    };

    // The walker stack: each entry is a directory we still need to read.
    let walker = std::sync::Arc::new(std::sync::Mutex::new(DirWalker::new(
        PathBuf::from(path),
        recursive,
    )));

    let walker_for_next = std::sync::Arc::clone(&walker);
    let next_host = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>| {
            let walker = std::sync::Arc::clone(&walker_for_next);
            let token = token.clone();
            async move {
                if token.is_cancelled() {
                    return Err::<Value<'js>, _>(throw_abort(&ctx));
                }
                let entry = tokio::task::spawn_blocking(move || {
                    let mut w = walker.lock().expect("walker mutex poisoned");
                    w.next()
                })
                .await
                .map_err(|e| throw_io(&ctx, &e.to_string()))?;
                match entry {
                    Some(Ok(e)) => Ok(e.to_js(&ctx)?),
                    Some(Err(err)) => Err(throw_io(&ctx, &err.to_string())),
                    None => Ok(Value::new_null(ctx.clone())),
                }
            }
        }),
    )?;

    let make_iter: Function<'js> = ctx.clone().eval(readdir_iterator_js().as_bytes())?;
    make_iter.call((next_host,))
}

struct DirWalker {
    stack: Vec<PathBuf>,
    recursive: bool,
    current: Option<std::fs::ReadDir>,
}

impl DirWalker {
    fn new(root: PathBuf, recursive: bool) -> Self {
        Self {
            stack: vec![root],
            recursive,
            current: None,
        }
    }
}

impl Iterator for DirWalker {
    type Item = std::io::Result<DirEntryOut>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current.is_none() {
                let dir = self.stack.pop()?;
                match std::fs::read_dir(&dir) {
                    Ok(rd) => self.current = Some(rd),
                    // Permission errors on system dirs (e.g. /Library/PreferencePanes
                    // under SIP) are normal on macOS; swallow and keep walking
                    // rather than terminating the iterator.
                    Err(_) => continue,
                }
            }
            let current = self.current.as_mut()?;
            match current.next() {
                None => {
                    self.current = None;
                }
                Some(Err(err)) => return Some(Err(err)),
                Some(Ok(entry)) => {
                    let path = entry.path();
                    let metadata = match entry.file_type() {
                        Ok(ft) => ft,
                        Err(err) => return Some(Err(err)),
                    };
                    let is_dir = metadata.is_dir();
                    let is_file = metadata.is_file();
                    if self.recursive && is_dir {
                        self.stack.push(path.clone());
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    return Some(Ok(DirEntryOut {
                        name,
                        path,
                        is_file,
                        is_dir,
                    }));
                }
            }
        }
    }
}

struct DirEntryOut {
    name: String,
    path: PathBuf,
    is_file: bool,
    is_dir: bool,
}

impl DirEntryOut {
    fn to_js<'js>(&self, ctx: &Ctx<'js>) -> JsResult<Value<'js>> {
        let obj = Object::new(ctx.clone())?;
        obj.set("name", self.name.clone())?;
        obj.set("path", self.path.to_string_lossy().into_owned())?;
        obj.set("isFile", self.is_file)?;
        obj.set("isDir", self.is_dir)?;
        Ok(obj.into_value())
    }
}

async fn read_file_impl<'js>(
    ctx: Ctx<'js>,
    path: String,
    opts: Value<'js>,
) -> JsResult<TypedArray<'js, u8>> {
    let token = signal_to_token(&ctx, &opts)?;
    let bytes = tokio::select! {
        biased;
        () = token.cancelled() => return Err(throw_abort(&ctx)),
        r = tokio::task::spawn_blocking(move || std::fs::read(path)) => {
            r.map_err(|e| throw_io(&ctx, &e.to_string()))?
                .map_err(|e| throw_io(&ctx, &e.to_string()))?
        }
    };
    TypedArray::new(ctx, bytes)
}

async fn read_text_impl<'js>(ctx: Ctx<'js>, path: String, opts: Value<'js>) -> JsResult<String> {
    let token = signal_to_token(&ctx, &opts)?;
    let s = tokio::select! {
        biased;
        () = token.cancelled() => return Err(throw_abort(&ctx)),
        r = tokio::task::spawn_blocking(move || std::fs::read_to_string(path)) => {
            r.map_err(|e| throw_io(&ctx, &e.to_string()))?
                .map_err(|e| throw_io(&ctx, &e.to_string()))?
        }
    };
    Ok(s)
}

async fn read_cache_impl(ctx: Ctx<'_>, name: String, cache_dir: PathBuf) -> JsResult<Value<'_>> {
    let path = match resolve_cache_path(&cache_dir, &name) {
        Ok(p) => p,
        Err(err) => return Err(throw_io(&ctx, err)),
    };
    let result = tokio::task::spawn_blocking(move || std::fs::read(&path))
        .await
        .map_err(|e| throw_io(&ctx, &e.to_string()))?;
    match result {
        Ok(bytes) => Ok(TypedArray::new(ctx, bytes)?.into_value()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Value::new_null(ctx)),
        Err(err) => Err(throw_io(&ctx, &err.to_string())),
    }
}

async fn write_cache_impl<'js>(
    ctx: Ctx<'js>,
    name: String,
    data: Value<'js>,
    cache_dir: PathBuf,
) -> JsResult<()> {
    let path = match resolve_cache_path(&cache_dir, &name) {
        Ok(p) => p,
        Err(err) => return Err(throw_io(&ctx, err)),
    };
    let bytes = coerce_data_to_bytes(&ctx, data)?;
    tokio::task::spawn_blocking(move || {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)
    })
    .await
    .map_err(|e| throw_io(&ctx, &e.to_string()))?
    .map_err(|e| throw_io(&ctx, &e.to_string()))?;
    Ok(())
}

fn coerce_data_to_bytes<'js>(ctx: &Ctx<'js>, data: Value<'js>) -> JsResult<Vec<u8>> {
    if let Some(s) = data.clone().into_string() {
        return Ok(s.to_string()?.into_bytes());
    }
    if let Ok(ta) = TypedArray::<u8>::from_value(data) {
        let bytes: &[u8] = ta.as_ref();
        return Ok(bytes.to_vec());
    }
    Err(throw_io(
        ctx,
        "writeCache: data must be a string or a Uint8Array",
    ))
}

fn signal_to_token<'js>(ctx: &Ctx<'js>, opts: &Value<'js>) -> JsResult<CancellationToken> {
    let Some(o) = opts.as_object() else {
        return Ok(CancellationToken::new());
    };
    let Ok(sig) = o.get::<_, Object<'js>>("signal") else {
        return Ok(CancellationToken::new());
    };
    abort::token_from_js_signal(ctx, &sig)
}

/// Resolve a cache name to an absolute path inside `cache_dir`, rejecting
/// anything that could escape (path separators, parent refs, hidden files,
/// empty/over-long names).
fn resolve_cache_path(cache_dir: &Path, name: &str) -> Result<PathBuf, &'static str> {
    if name.is_empty() {
        return Err("cache name must not be empty");
    }
    if name.len() > 255 {
        return Err("cache name must be 255 chars or fewer");
    }
    if name.starts_with('.') {
        return Err("cache name must not start with '.'");
    }
    if name.contains('/') || name.contains('\\') {
        return Err("cache name must not contain path separators");
    }
    if name == ".." || name.contains("..") {
        return Err("cache name must not contain '..'");
    }
    for c in name.chars() {
        if c == '\0' || c.is_control() {
            return Err("cache name must not contain control characters");
        }
    }
    Ok(cache_dir.join(name))
}

fn throw_cap(ctx: &Ctx<'_>, cap: &str) -> rquickjs::Error {
    let err = match Object::new(ctx.clone()) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let _ = err.set("name", "CapabilityError");
    let _ = err.set(
        "message",
        format!("missing capability: {cap} (declare it in manifest.json)"),
    );
    ctx.throw(err.into_value())
}

fn throw_io(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
    let err = match Object::new(ctx.clone()) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let _ = err.set("name", "FsError");
    let _ = err.set("message", message.to_owned());
    ctx.throw(err.into_value())
}

fn throw_abort(ctx: &Ctx<'_>) -> rquickjs::Error {
    let err = match Object::new(ctx.clone()) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let _ = err.set("name", "AbortError");
    let _ = err.set("message", "operation aborted");
    ctx.throw(err.into_value())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_cache_path_accepts_simple_name() {
        let p = resolve_cache_path(Path::new("/tmp/cache"), "apps.json").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/cache/apps.json"));
    }

    #[test]
    fn resolve_cache_path_rejects_traversal() {
        assert!(resolve_cache_path(Path::new("/tmp"), "../etc/passwd").is_err());
        assert!(resolve_cache_path(Path::new("/tmp"), "..").is_err());
        assert!(resolve_cache_path(Path::new("/tmp"), "a/b").is_err());
        assert!(resolve_cache_path(Path::new("/tmp"), "/abs").is_err());
        assert!(resolve_cache_path(Path::new("/tmp"), ".hidden").is_err());
        assert!(resolve_cache_path(Path::new("/tmp"), "").is_err());
    }

    #[test]
    fn dir_walker_non_recursive_yields_immediate_children() {
        let dir = std::env::temp_dir().join(format!(
            "hb-fs-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested/b.txt"), "b").unwrap();

        let names: Vec<String> = DirWalker::new(dir.clone(), false)
            .filter_map(Result::ok)
            .map(|e| e.name)
            .collect();
        assert!(names.contains(&"a.txt".to_owned()));
        assert!(names.contains(&"nested".to_owned()));
        assert!(!names.contains(&"b.txt".to_owned()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dir_walker_recursive_descends_into_subdirs() {
        let dir = std::env::temp_dir().join(format!(
            "hb-fs-test-rec-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested/b.txt"), "b").unwrap();

        let names: Vec<String> = DirWalker::new(dir.clone(), true)
            .filter_map(Result::ok)
            .map(|e| e.name)
            .collect();
        assert!(names.contains(&"b.txt".to_owned()));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
