//! Host implementation of the `highbeam:system` module.
//!
//! `exec` (cap `system.exec`) captures stdout/stderr/code; capture is
//! truncated at the per-stream cap defined below. `AbortSignal` is honored
//! — the child is killed on abort via `kill_on_drop`.
//!
//! `applescript` (cap `system.applescript`) resolves with `null` on
//! non-macOS, never throws — plugins can call it without gating every site.

use std::process::Stdio;
use std::time::Duration;

#[cfg(target_os = "macos")]
use rquickjs::IntoJs;
use rquickjs::function::{Async, Opt};
use rquickjs::{Ctx, Function, Object, Result as JsResult, Value, module::ModuleDef};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::sdk::abort;
use crate::sdk::errors::{cap_error_thrower, throw_abort, throw_cap, throw_named};

const EXEC_GLOBAL: &str = "__highbeam_system_exec";
const APPLESCRIPT_GLOBAL: &str = "__highbeam_system_applescript";

/// Per-stream capture cap. Output beyond this is silently truncated — the
/// launcher use case is small CLI commands, not tailing a 10GB log.
const MAX_CAPTURE_BYTES: usize = 10 * 1024 * 1024;

pub struct SystemModule;

impl ModuleDef for SystemModule {
    fn declare(decl: &rquickjs::module::Declarations<'_>) -> JsResult<()> {
        decl.declare("exec")?;
        decl.declare("applescript")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> JsResult<()> {
        let globals = ctx.globals();
        let exec_val: Value<'js> = globals
            .get(EXEC_GLOBAL)
            .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
        let applescript_val: Value<'js> = globals
            .get(APPLESCRIPT_GLOBAL)
            .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
        let exec_fn = match exec_val.into_function() {
            Some(f) => f,
            None => cap_error_thrower(ctx, "system.exec")?,
        };
        let applescript_fn = match applescript_val.into_function() {
            Some(f) => f,
            None => cap_error_thrower(ctx, "system.applescript")?,
        };
        exports.export("exec", exec_fn)?;
        exports.export("applescript", applescript_fn)?;
        Ok(())
    }
}

/// Install per-plugin bindings.
///
/// # Errors
///
/// Propagates JS errors from function construction or global assignment.
// The subprocess capture stack (tokio::process + multi-branch select!) makes
// the generated future ~65KB; one-shot per call so the heap-vs-stack
// difference is irrelevant. Suppress across this fn vs scattering Box::pin.
#[allow(clippy::large_futures)]
pub fn install<'js>(ctx: &Ctx<'js>, can_exec: bool, can_applescript: bool) -> JsResult<()> {
    let exec = Function::new(
        ctx.clone(),
        Async(
            move |ctx: Ctx<'js>, cmd: String, args: Opt<Value<'js>>, opts: Opt<Value<'js>>| async move {
                if !can_exec {
                    return Err::<Object<'js>, _>(throw_cap(&ctx, "system.exec"));
                }
                let args_val = args.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                let opts_val = opts.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                let arg_strings = coerce_args(&args_val)?;
                let (token, timeout, cwd) = parse_exec_opts(&ctx, &opts_val)?;
                run_command(ctx, cmd, arg_strings, token, timeout, cwd).await
            },
        ),
    )?;

    let applescript = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, script: String, opts: Opt<Value<'js>>| async move {
            if !can_applescript {
                return Err::<Value<'js>, _>(throw_cap(&ctx, "system.applescript"));
            }
            #[cfg(target_os = "macos")]
            {
                let opts_val = opts.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                let (token, timeout, _cwd) = parse_exec_opts(&ctx, &opts_val)?;
                let out = run_command(
                    ctx.clone(),
                    "/usr/bin/osascript".into(),
                    vec!["-e".into(), script],
                    token,
                    timeout,
                    None,
                )
                .await?;
                let stdout: String = out.get("stdout")?;
                stdout.trim_end_matches('\n').to_owned().into_js(&ctx)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (script, opts);
                Ok(Value::new_null(ctx))
            }
        }),
    )?;

    ctx.globals().set(EXEC_GLOBAL, exec)?;
    ctx.globals().set(APPLESCRIPT_GLOBAL, applescript)?;
    Ok(())
}

fn coerce_args<'js>(args: &Value<'js>) -> JsResult<Vec<String>> {
    if args.is_undefined() || args.is_null() {
        return Ok(Vec::new());
    }
    let Some(arr) = args.as_array() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());

    for v in arr.iter::<Value<'js>>() {
        let v = v?;

        if let Some(s) = v.into_string() {
            out.push(s.to_string()?);
        }
    }
    Ok(out)
}

fn parse_exec_opts<'js>(
    ctx: &Ctx<'js>,
    opts: &Value<'js>,
) -> JsResult<(CancellationToken, Option<Duration>, Option<String>)> {
    let Some(o) = opts.as_object() else {
        return Ok((CancellationToken::new(), None, None));
    };
    let signal = o.get::<_, Object<'js>>("signal").ok();
    let token = match signal {
        Some(sig) => abort::token_from_js_signal(ctx, &sig)?,
        None => CancellationToken::new(),
    };
    let timeout = o
        .get::<_, f64>("timeoutMs")
        .ok()
        .filter(|ms| ms.is_finite() && *ms > 0.0)
        .map(|ms| {
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let n = ms as u64;
            Duration::from_millis(n)
        });
    let cwd = o.get::<_, String>("cwd").ok();
    Ok((token, timeout, cwd))
}

async fn run_command(
    ctx: Ctx<'_>,
    cmd: String,
    args: Vec<String>,
    token: CancellationToken,
    timeout: Option<Duration>,
    cwd: Option<String>,
) -> JsResult<Object<'_>> {
    let mut command = Command::new(&cmd);
    command
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);

    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let mut child = command
        .spawn()
        .map_err(|e| throw_io(&ctx, &format!("spawn {cmd}: {e}")))?;

    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();

    let wait_fut = async {
        let (s, e, status) = tokio::join!(
            read_capped_opt(stdout.as_mut()),
            read_capped_opt(stderr.as_mut()),
            child.wait(),
        );
        Ok::<_, std::io::Error>((s?, e?, status?))
    };

    let result = tokio::select! {
        biased;
        () = token.cancelled() => {
            return Err(throw_abort(&ctx));
        }
        () = sleep_opt(timeout) => {
            return Err(throw_io(&ctx, &format!("exec {cmd}: timeout")));
        }
        r = wait_fut => r,
    };

    let (stdout_bytes, stderr_bytes, status) = result.map_err(|e| throw_io(&ctx, &format!("exec {cmd}: {e}")))?;

    let obj = Object::new(ctx.clone())?;
    obj.set("stdout", String::from_utf8_lossy(&stdout_bytes).into_owned())?;
    obj.set("stderr", String::from_utf8_lossy(&stderr_bytes).into_owned())?;

    match status.code() {
        Some(c) => obj.set("code", c)?,
        None => obj.set("code", Value::new_null(ctx))?,
    }
    Ok(obj)
}

/// Read `reader` into a capped `Vec<u8>` when present; return an empty
/// vec otherwise. Lets the join site drop the "is the pipe wired up?"
/// branch into a single line per stream.
async fn read_capped_opt<R: AsyncReadExt + Unpin>(reader: Option<&mut R>) -> std::io::Result<Vec<u8>> {
    match reader {
        Some(r) => read_capped(r).await,
        None => Ok(Vec::new()),
    }
}

async fn read_capped<R: AsyncReadExt + Unpin>(reader: &mut R) -> std::io::Result<Vec<u8>> {
    // 4 KiB buffers keep the `tokio::join!`'d pair of these futures under
    // clippy's `large_futures` 16 KiB threshold without needing a Box::pin
    // detour. Matches the default macOS / Linux pipe buffer size — bigger
    // chunks wouldn't help throughput against the pipe.
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];

    loop {
        if buf.len() >= MAX_CAPTURE_BYTES {
            // Drain to EOF so the child doesn't block on a full pipe.
            let mut sink = [0u8; 4096];

            loop {
                let n = reader.read(&mut sink).await?;

                if n == 0 {
                    break;
                }
            }

            break;
        }
        let n = reader.read(&mut chunk).await?;

        if n == 0 {
            break;
        }
        let remaining = MAX_CAPTURE_BYTES.saturating_sub(buf.len());
        buf.extend_from_slice(&chunk[..n.min(remaining)]);
    }
    Ok(buf)
}

async fn sleep_opt(timeout: Option<Duration>) {
    match timeout {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending::<()>().await,
    }
}

fn throw_io(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
    throw_named(ctx, "SystemError", message)
}
