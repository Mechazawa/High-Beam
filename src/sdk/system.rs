//! Host implementation of the `highbeam:system` module.
//!
//! Surface:
//!
//! ```ts
//! import { exec, applescript } from 'highbeam:system';
//! const { stdout, stderr, code } = await exec('/usr/bin/uptime', []);
//! const macOnly = await applescript('tell application "Finder" to get name');
//! ```
//!
//! Capabilities:
//!   * `system.exec` — gates `exec`
//!   * `system.applescript` — gates `applescript`
//!
//! `applescript` is a no-op on non-macOS — it resolves with `null`, never
//! throws, so plugins can call it without platform-gating every call site.
//!
//! `exec` captures stdout/stderr (truncated at `MAX_CAPTURE_BYTES`) and the
//! process exit code. `AbortSignal` honored — the subprocess is killed on
//! abort.

use std::process::Stdio;
use std::time::Duration;

use rquickjs::function::{Async, Opt, Rest};
use rquickjs::{Ctx, Function, IntoJs, Object, Result as JsResult, Value, module::ModuleDef};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::sdk::abort;
use crate::sdk::errors::{throw_abort, throw_cap, throw_named};

const EXEC_GLOBAL: &str = "__highbeam_system_exec";
const APPLESCRIPT_GLOBAL: &str = "__highbeam_system_applescript";

/// Capture cap per stream. Subprocesses that exceed this have their output
/// silently truncated — the launcher use case is "small CLI commands", not
/// "tail a 10GB log file".
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
        let exec_fn = if let Some(f) = exec_val.into_function() {
            f
        } else {
            Function::new(
                ctx.clone(),
                Async(|ctx: Ctx<'js>, _args: Rest<Value<'js>>| async move {
                    Err::<Value<'js>, _>(throw_cap(&ctx, "system.exec"))
                }),
            )?
        };
        let applescript_val: Value<'js> = globals
            .get(APPLESCRIPT_GLOBAL)
            .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
        let applescript_fn = if let Some(f) = applescript_val.into_function() {
            f
        } else {
            Function::new(
                ctx.clone(),
                Async(|ctx: Ctx<'js>, _args: Rest<Value<'js>>| async move {
                    Err::<Value<'js>, _>(throw_cap(&ctx, "system.applescript"))
                }),
            )?
        };
        exports.export("exec", exec_fn)?;
        exports.export("applescript", applescript_fn)?;
        Ok(())
    }
}

/// Build per-plugin bindings.
///
/// # Errors
///
/// Propagates JS errors from function construction or global assignment.
// The subprocess capture stack (tokio::process + select! over multiple
// branches) bumps the generated future to ~65KB; spawning is a one-shot per
// plugin call so the heap-vs-stack difference is irrelevant. Suppress the
// warning across this fn rather than scattering Box::pin around.
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
        Async(
            move |ctx: Ctx<'js>, script: String, opts: Opt<Value<'js>>| async move {
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
            },
        ),
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

    let read_stdout = async {
        if let Some(s) = stdout.as_mut() {
            Box::pin(read_capped(s)).await
        } else {
            Ok(Vec::new())
        }
    };
    let read_stderr = async {
        if let Some(s) = stderr.as_mut() {
            Box::pin(read_capped(s)).await
        } else {
            Ok(Vec::new())
        }
    };

    let wait_fut = async {
        let (s, e, status) = tokio::join!(read_stdout, read_stderr, child.wait());
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

    let (stdout_bytes, stderr_bytes, status) =
        result.map_err(|e| throw_io(&ctx, &format!("exec {cmd}: {e}")))?;

    let obj = Object::new(ctx.clone())?;
    obj.set(
        "stdout",
        String::from_utf8_lossy(&stdout_bytes).into_owned(),
    )?;
    obj.set(
        "stderr",
        String::from_utf8_lossy(&stderr_bytes).into_owned(),
    )?;
    match status.code() {
        Some(c) => obj.set("code", c)?,
        None => obj.set("code", Value::new_null(ctx))?,
    }
    Ok(obj)
}

async fn read_capped<R: AsyncReadExt + Unpin>(reader: &mut R) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if buf.len() >= MAX_CAPTURE_BYTES {
            // Drain to EOF so the child doesn't block on a full pipe.
            let mut sink = [0u8; 8192];
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
