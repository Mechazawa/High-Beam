//! Host implementation of the `highbeam:icons` module.
//!
//! macOS: `sips` extracts the bundle icon as PNG (~50ms first call, cached
//! in-process).
//!
//! Linux: absolute paths load directly; bare names go through
//! `freedesktop-icons` XDG lookup walking the active GTK theme → parents →
//! hicolor → /usr/share/pixmaps. SVGs are passed through as
//! `image/svg+xml` data URIs (Slint already decodes those via its embedded
//! resvg); raster icons are resized to the requested size with the `image`
//! crate and re-encoded as PNG. A missing icon falls back to a 1×1
//! transparent PNG so the launcher never crashes on a stray bad entry.

use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(target_os = "macos")]
use crate::logging::LogErr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rquickjs::function::{Async, Opt};
use rquickjs::{Ctx, Function, Result as JsResult, Value, module::ModuleDef};

use crate::sdk::errors::{cap_error_thrower, throw_cap, throw_named};

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
        let f = match val.into_function() {
            Some(f) => f,
            None => cap_error_thrower(ctx, "icons")?,
        };
        exports.export("forPath", f)?;
        Ok(())
    }
}

/// Install the per-plugin `forPath` binding.
///
/// # Errors
///
/// Propagates JS errors from function construction or global assignment.
pub fn install<'js>(ctx: &Ctx<'js>, can_icons: bool) -> JsResult<()> {
    let for_path = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, path: String, opts: Opt<Value<'js>>| async move {
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
        }),
    )?;
    ctx.globals().set(FOR_PATH_GLOBAL, for_path)?;
    Ok(())
}

/// Two-level so a cache hit doesn't have to allocate a `String` just to
/// build the composite probe key — the outer map keys on `String` and
/// lookups borrow as `&str` directly. Inner map is keyed on the size,
/// which is already a small `Copy` integer.
fn cache() -> &'static Mutex<HashMap<String, HashMap<u32, String>>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, HashMap<u32, String>>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn resolve_icon(ctx: &Ctx<'_>, path: &str, size: u32) -> JsResult<String> {
    if let Some(hit) = cache()
        .lock()
        .expect("icon cache mutex")
        .get(path)
        .and_then(|by_size| by_size.get(&size))
        .cloned()
    {
        return Ok(hit);
    }

    let path_owned = path.to_owned();
    // A JoinError here means the blocking task panicked or was cancelled —
    // a real fault, not "no icon found". Surface it as IconError so plugins
    // can distinguish "extraction crashed" from "no usable icon" (which
    // falls back to a transparent PNG below).
    let (bytes, mime) = tokio::task::spawn_blocking(move || extract_icon_bytes(&path_owned, size))
        .await
        .map_err(|e| throw_io(ctx, &join_error_message(&e)))?
        .unwrap_or_else(|| (fallback_icon_bytes().to_vec(), "image/png"));

    let encoded = STANDARD.encode(&bytes);
    let data_uri = format!("data:{mime};base64,{encoded}");

    cache()
        .lock()
        .expect("icon cache mutex")
        .entry(path.to_owned())
        .or_default()
        .insert(size, data_uri.clone());

    Ok(data_uri)
}

/// Render a `tokio::task::JoinError` into the message we pass to JS-side
/// `IconError`. Pulled out so the message shape can be asserted without
/// having to manufacture a real `JoinError` (which has no public ctor).
fn join_error_message(err: &tokio::task::JoinError) -> String {
    format!("icon extraction crashed: {err}")
}

#[cfg(target_os = "macos")]
fn extract_icon_bytes(path: &str, size: u32) -> Option<(Vec<u8>, &'static str)> {
    use std::process::Command;

    let target = Path::new(path);

    if !target.exists() {
        return None;
    }

    // Read CFBundleIconFile via `defaults`, then `sips` converts the .icns
    // to a sized PNG. Works for plain files too (Quick Look thumbnail).
    let tmp = std::env::temp_dir().join(format!(
        "hb-icon-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let icon_src = resolve_macos_icon_source(target).unwrap_or_else(|| target.to_path_buf());

    let out = Command::new("/usr/bin/sips")
        .args(["-s", "format", "png", "-z", &size.to_string(), &size.to_string()])
        .arg(&icon_src)
        .arg("--out")
        .arg(&tmp)
        .output()
        .ok()?;

    if !out.status.success() {
        std::fs::remove_file(&tmp).log_debug("icons: cleanup tmp after failed sips");
        return None;
    }

    let bytes = std::fs::read(&tmp).ok();
    std::fs::remove_file(&tmp).log_debug("icons: cleanup tmp after sips extract");

    bytes.map(|b| (b, "image/png"))
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

#[cfg(target_os = "linux")]
fn extract_icon_bytes(spec: &str, size: u32) -> Option<(Vec<u8>, &'static str)> {
    let resolved = resolve_linux_icon_path(spec, size)?;
    load_linux_icon_file(&resolved, size)
}

#[cfg(target_os = "linux")]
fn resolve_linux_icon_path(spec: &str, size: u32) -> Option<PathBuf> {
    if spec.starts_with('/') {
        let p = PathBuf::from(spec);
        return p.is_file().then_some(p);
    }
    // freedesktop-icons's `with_size` is u16; clamp gracefully so silly large
    // requests still produce a result instead of overflowing.
    let size_u16 = u16::try_from(size).unwrap_or(u16::MAX);
    let mut builder = freedesktop_icons::lookup(spec).with_size(size_u16).with_cache();

    if let Some(theme) = gtk_theme_name() {
        builder = builder.with_theme(theme);
    }
    builder.find()
}

/// Cache the GTK icon theme name. `default_theme_gtk` shells out to read the
/// `GSettings` key on first call; the result is stable for the life of the
/// daemon and the lookup runs on the hot path for every keystroke.
#[cfg(target_os = "linux")]
fn gtk_theme_name() -> Option<&'static str> {
    static THEME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    THEME.get_or_init(freedesktop_icons::default_theme_gtk).as_deref()
}

#[cfg(target_os = "linux")]
fn load_linux_icon_file(path: &Path, size: u32) -> Option<(Vec<u8>, &'static str)> {
    let bytes = std::fs::read(path).ok()?;
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);

    // SVGs go through untouched — Slint's embedded resvg rasterises them
    // at render time, so we keep the vector source and skip a CPU-heavy
    // rasterise on every icon.
    if matches!(ext.as_deref(), Some("svg" | "svgz")) {
        return Some((bytes, "image/svg+xml"));
    }
    // Everything else (PNG, JPEG, XPM-via-image-feature, …): decode, fit
    // into the requested box with Lanczos3, and re-encode as PNG so the
    // UI always sees a uniform raster size regardless of what the theme
    // had on disk (themes often store at 256 / 512 etc.).
    let img = image::load_from_memory(&bytes).ok()?;
    let resized = img.resize(size, size, image::imageops::FilterType::Lanczos3);
    let mut out = Vec::new();
    resized
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some((out, "image/png"))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn extract_icon_bytes(_path: &str, _size: u32) -> Option<(Vec<u8>, &'static str)> {
    None
}

/// 1×1 transparent PNG — universal fallback for Linux / missing files.
fn fallback_icon_bytes() -> &'static [u8] {
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00,
        0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d,
        0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
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

    /// On Linux, an absolute path to a real PNG round-trips through the
    /// extractor: bytes come back, MIME is `image/png`, and the result is a
    /// valid PNG (the `image` crate re-encodes after resizing, so we don't
    /// just get the input bytes verbatim).
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_absolute_png_path_resolves_to_png_bytes() {
        // Generate a real 16×16 PNG with the same crate used in the
        // production resize path so we know the bytes round-trip cleanly.
        let img = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 0, 0, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode fixture PNG");

        let tmp = std::env::temp_dir().join(format!(
            "hb-icon-test-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::write(&tmp, &png).expect("write fixture PNG");
        let result = extract_icon_bytes(tmp.to_str().expect("utf-8 tmp path"), 64);
        let _ = std::fs::remove_file(&tmp);
        let (bytes, mime) = result.expect("absolute PNG path must resolve");
        assert_eq!(mime, "image/png");
        assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n", "must be a valid PNG header");
    }

    /// SVGs are passed through to the UI as-is (Slint rasterises them via
    /// its embedded resvg). We must not re-encode them as PNG.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_absolute_svg_path_passes_through_as_svg() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="red"/></svg>"#;
        let tmp = std::env::temp_dir().join(format!(
            "hb-icon-test-{}-{}.svg",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::write(&tmp, svg).expect("write fixture SVG");
        let result = extract_icon_bytes(tmp.to_str().expect("utf-8 tmp path"), 64);
        let _ = std::fs::remove_file(&tmp);
        let (bytes, mime) = result.expect("absolute SVG path must resolve");
        assert_eq!(mime, "image/svg+xml");
        assert_eq!(bytes.as_slice(), svg, "SVG bytes must pass through unchanged");
    }

    /// Absolute path that doesn't exist returns `None` so the caller can fall
    /// back to the transparent PNG — no panic, no `IconError` surfacing.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_missing_absolute_path_returns_none() {
        let missing = "/this/path/should/not/exist/anywhere-hb-icon-test.png";
        assert!(extract_icon_bytes(missing, 64).is_none());
    }

    /// Drive a real `JoinError` by panicking inside `spawn_blocking` and
    /// assert our message helper renders it as `IconError` text — a process
    /// killed mid-call must surface, not silently degrade to a placeholder.
    #[test]
    fn join_error_message_carries_panic_details() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt");
        let join_err = rt.block_on(async {
            tokio::task::spawn_blocking(|| panic!("simulated icon extractor crash"))
                .await
                .expect_err("the panicked task must produce a JoinError")
        });
        let msg = join_error_message(&join_err);
        assert!(
            msg.starts_with("icon extraction crashed: "),
            "expected the crash prefix, got: {msg}",
        );
    }
}
