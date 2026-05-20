//! Host implementation of the `highbeam:icons` module.
//!
//! Surface:
//!
//! ```ts
//! import { forPath } from 'highbeam:icons';
//! const dataUri = await forPath('/Applications/Safari.app', { size: 128 });
//! ```
//!
//! Resolution strategy:
//!   * macOS: `sips` extracts the resource fork icon as PNG. Slow on first
//!     call (~50ms); cached in-process to make repeated lookups cheap.
//!   * Linux: best-effort — return a tiny generic icon data URI rather than
//!     throwing. The Spotlight plugin's main consumer never crashes for
//!     missing icons.
//!
//! Cap: `icons`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rquickjs::function::{Async, Opt, Rest};
use rquickjs::{Ctx, Function, Result as JsResult, Value, module::ModuleDef};

use crate::sdk::errors::{throw_cap, throw_named};

const FOR_PATH_GLOBAL: &str = "__highbeam_icons_for_path";

pub struct IconsModule;

impl ModuleDef for IconsModule {
    fn declare(decl: &rquickjs::module::Declarations<'_>) -> JsResult<()> {
        decl.declare("forPath")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> JsResult<()> {
        let val: Value<'js> = ctx
            .globals()
            .get(FOR_PATH_GLOBAL)
            .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
        let f = if let Some(f) = val.into_function() {
            f
        } else {
            Function::new(
                ctx.clone(),
                Async(|ctx: Ctx<'js>, _args: Rest<Value<'js>>| async move {
                    Err::<String, _>(throw_cap(&ctx, "icons"))
                }),
            )?
        };
        exports.export("forPath", f)?;
        Ok(())
    }
}

/// Build the per-plugin `forPath` binding.
///
/// # Errors
///
/// Propagates JS errors from function construction or global assignment.
pub fn install<'js>(ctx: &Ctx<'js>, can_icons: bool) -> JsResult<()> {
    let for_path = Function::new(
        ctx.clone(),
        Async(
            move |ctx: Ctx<'js>, path: String, opts: Opt<Value<'js>>| async move {
                if !can_icons {
                    return Err::<String, _>(throw_cap(&ctx, "icons"));
                }
                let opts_val = opts.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                let size = opts_val
                    .as_object()
                    .and_then(|o| o.get::<_, f64>("size").ok())
                    .filter(|s| s.is_finite() && *s > 0.0)
                    .map_or(128u32, |s| {
                        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                        let n = s as u32;
                        n
                    });
                resolve_icon(&ctx, &path, size).await
            },
        ),
    )?;
    ctx.globals().set(FOR_PATH_GLOBAL, for_path)?;
    Ok(())
}

fn cache() -> &'static Mutex<HashMap<(String, u32), String>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<(String, u32), String>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn resolve_icon(ctx: &Ctx<'_>, path: &str, size: u32) -> JsResult<String> {
    let key = (path.to_owned(), size);
    if let Some(hit) = cache().lock().expect("icon cache mutex").get(&key) {
        return Ok(hit.clone());
    }
    let path_owned = path.to_owned();
    let result = tokio::task::spawn_blocking(move || extract_icon_bytes(&path_owned, size))
        .await
        .map_err(|e| throw_io(ctx, &e.to_string()))?;
    let bytes = result.unwrap_or_else(|| fallback_icon_bytes().to_vec());
    let encoded = STANDARD.encode(&bytes);
    let data_uri = format!("data:image/png;base64,{encoded}");
    cache()
        .lock()
        .expect("icon cache mutex")
        .insert(key, data_uri.clone());
    Ok(data_uri)
}

#[cfg(target_os = "macos")]
fn extract_icon_bytes(path: &str, size: u32) -> Option<Vec<u8>> {
    use std::process::Command;

    let target = Path::new(path);
    if !target.exists() {
        return None;
    }

    // Strategy: read the bundle's CFBundleIconFile via `defaults`, then `sips`
    // converts the .icns to a sized PNG written to a temp file. This avoids
    // shelling out to `osascript` and works for both .app bundles and plain
    // files (sips will return the file's Quick Look thumbnail in those cases).
    let tmp = std::env::temp_dir().join(format!(
        "hb-icon-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let icon_src = resolve_macos_icon_source(target).unwrap_or_else(|| target.to_path_buf());

    let out = Command::new("/usr/bin/sips")
        .args([
            "-s",
            "format",
            "png",
            "-z",
            &size.to_string(),
            &size.to_string(),
        ])
        .arg(&icon_src)
        .arg("--out")
        .arg(&tmp)
        .output()
        .ok()?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    let bytes = std::fs::read(&tmp).ok();
    let _ = std::fs::remove_file(&tmp);
    bytes
}

#[cfg(target_os = "macos")]
fn resolve_macos_icon_source(target: &Path) -> Option<std::path::PathBuf> {
    let resources = target.join("Contents/Resources");
    if !resources.is_dir() {
        return None;
    }
    let info_plist = target.join("Contents/Info.plist");
    if let Ok(out) = std::process::Command::new("/usr/bin/defaults")
        .args(["read"])
        .arg(info_plist.with_extension(""))
        .arg("CFBundleIconFile")
        .output()
        && out.status.success()
    {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !name.is_empty() {
            let with_ext = if std::path::Path::new(&name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("icns"))
            {
                name
            } else {
                format!("{name}.icns")
            };
            let candidate = resources.join(with_ext);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn extract_icon_bytes(_path: &str, _size: u32) -> Option<Vec<u8>> {
    None
}

/// 1×1 transparent PNG. Used as the universal fallback so callers never
/// crash when icon extraction is unavailable (Linux, missing file, etc.).
fn fallback_icon_bytes() -> &'static [u8] {
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    PNG
}

fn throw_io(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
    throw_named(ctx, "IconError", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_is_a_valid_png_header() {
        let bytes = fallback_icon_bytes();
        assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn cache_is_reused_across_calls() {
        let c = cache();
        c.lock().unwrap().insert(("k".into(), 1), "v".into());
        let v = c.lock().unwrap().get(&("k".to_owned(), 1)).cloned();
        assert_eq!(v.as_deref(), Some("v"));
        // Cleanup so the test doesn't pollute other tests in the same process.
        c.lock().unwrap().remove(&("k".to_owned(), 1));
    }
}
