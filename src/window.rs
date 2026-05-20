//! Slint window construction and lifecycle.
//!
//! `QueryWindow` markup is included via `slint::include_modules!`. Anything
//! the .slint file can't express — sizing, native center positioning,
//! blur-to-close, macOS app activation — lives here.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::QueryWindow;
use crate::theme::Theme;

/// Wire up the `QueryWindow` callbacks and native-window behaviour.
///
/// Caller is responsible for showing/hiding the window in response to hotkey
/// or IPC events; this only attaches handlers. The plugin-state-dependent
/// `on_query_edited` and `on_invoke_selected` callbacks are wired by
/// `crate::app::start`.
pub(crate) fn configure(window: &QueryWindow) {
    let weak_for_esc = window.as_weak();
    window.on_escape_pressed(move || {
        if let Some(w) = weak_for_esc.upgrade() {
            hide(&w);
        }
    });

    // winit Focused(true) is the earliest tick at which Slint's focus request
    // is guaranteed to land on the input. Calling `invoke_focus_input` from
    // `show()` is too early on macOS — the NSWindow isn't yet the key window
    // and the request gets dropped, leaving the user clicking into the field.
    let weak_for_focus = window.as_weak();
    window
        .window()
        .on_winit_window_event(move |_slint_win, event| {
            match event {
                winit::event::WindowEvent::Focused(true) => {
                    if let Some(w) = weak_for_focus.upgrade() {
                        w.invoke_focus_input();
                    }
                }
                winit::event::WindowEvent::Focused(false) => {
                    if let Some(w) = weak_for_focus.upgrade() {
                        hide(&w);
                    }
                }
                _ => {}
            }
            EventResult::Propagate
        });

    // TODO: hide macOS Dock/Cmd-Tab presence via
    // `NSApp.setActivationPolicy(NSApplicationActivationPolicyAccessory)`, or
    // bundle as `.app` with `LSUIElement=1` in Info.plist. Independent of the
    // activation logic below.
}

/// Push theme tokens into the window's `in-out` properties. Theme reload
/// is restart-only.
pub(crate) fn apply_theme(window: &QueryWindow, theme: &Theme) {
    window.set_background_color(theme.colors.background);
    window.set_foreground_color(theme.colors.foreground);
    window.set_muted_color(theme.colors.muted);
    window.set_highlight_color(theme.colors.highlight);
    window.set_selection_color(theme.colors.selection);
    window.set_border_color(theme.colors.border);
    window.set_font_family(theme.font.family.clone().into());
    window.set_font_size_query(theme.font.size_query);
    window.set_font_size_title(theme.font.size_title);
    window.set_font_size_subtitle(theme.font.size_subtitle);
    window.set_window_width(theme.window.width);
    window.set_window_border_radius(theme.window.border_radius);
}

/// Show the window, center it, and focus the input. Idempotent — calling
/// while already visible just re-centers and re-focuses ("focuses it if
/// already open").
///
/// On macOS, `window.show()` + `invoke_focus_input()` alone isn't enough:
/// our process isn't necessarily frontmost, the new `NSWindow` isn't yet
/// the key window, and Slint's focus request gets dropped. We replicate
/// the Spotlight/Alfred/Raycast pattern: activate the app process and make
/// the `NSWindow` key + frontmost ourselves before asking Slint for focus.
pub(crate) fn show(window: &QueryWindow) {
    if let Err(err) = window.show() {
        eprintln!("failed to show window: {err}");
        return;
    }
    center_on_focused_display(window);
    #[cfg(target_os = "macos")]
    macos::activate_and_make_key(window);
    window.invoke_focus_input();
}

/// Hide the window. Clears the input text so the next open starts fresh —
/// every close path (Esc, blur, programmatic) funnels through here.
pub(crate) fn hide(window: &QueryWindow) {
    window.invoke_clear_input();
    if let Err(err) = window.hide() {
        eprintln!("failed to hide window: {err}");
    }
}

/// Center horizontally and place vertically at ~1/3 from the top, Spotlight-style.
fn center_on_focused_display(window: &QueryWindow) {
    // Resolve the monitor under the current cursor — on macOS that's the
    // screen the user is actively looking at. Falls back to primary if winit
    // can't tell us. Works in physical pixels so scale factor is irrelevant.
    let slint_window = window.window();
    let Some((monitor_pos, monitor_size)) = slint_window
        .with_winit_window(|w: &winit::window::Window| {
            let monitor = w
                .current_monitor()
                .or_else(|| w.primary_monitor())
                .or_else(|| w.available_monitors().next())?;
            Some((monitor.position(), monitor.size()))
        })
        .flatten()
    else {
        return;
    };

    let window_size = slint_window.size();
    let monitor_w = i32::try_from(monitor_size.width).unwrap_or(i32::MAX);
    let monitor_h = i32::try_from(monitor_size.height).unwrap_or(i32::MAX);
    let win_w = i32::try_from(window_size.width).unwrap_or(i32::MAX);
    let win_h = i32::try_from(window_size.height).unwrap_or(i32::MAX);

    let x = monitor_pos.x + (monitor_w - win_w) / 2;
    // Top of the window at ~1/3 of screen height — that sits above center the
    // way Spotlight does, regardless of window height.
    let y = monitor_pos.y + (monitor_h / 3) - (win_h / 2);

    slint_window.set_position(slint::PhysicalPosition::new(x, y));
}

#[cfg(target_os = "macos")]
mod macos {
    //! macOS-specific app/window activation. Without this the launcher window
    //! opens behind whatever was previously frontmost and the `TextInput`
    //! receives no keystrokes — Slint's focus request is meaningless if the
    //! OS-level key window is still someone else's.

    use std::ptr::NonNull;

    use objc2_app_kit::{NSApplication, NSView, NSWindow};
    use objc2_foundation::MainThreadMarker;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::ComponentHandle;
    use slint::winit_030::{WinitWindowAccessor, winit};

    use crate::QueryWindow;

    /// Activate our app process and make our `NSWindow` key + frontmost.
    ///
    /// Order matters: `NSApp.activate(ignoringOtherApps: true)` first to
    /// flip the process to frontmost; without it `makeKeyAndOrderFront` on
    /// a background app's window may show the window but won't move keyboard
    /// focus. `MainThreadMarker::new()` (rather than `new_unchecked()`) makes
    /// a future off-thread caller fail loudly instead of being UB.
    pub fn activate_and_make_key(window: &QueryWindow) {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("activate_and_make_key called off the main thread");
            return;
        };

        let app = NSApplication::sharedApplication(mtm);
        // reason: `activateIgnoringOtherApps:` is deprecated in macOS 14 in
        // favour of cooperative `activate()`, but launchers explicitly do NOT
        // want to be cooperative — the whole point is being summoned over
        // whatever was frontmost. Spotlight/Alfred/Raycast use the same call.
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);

        // Walk winit -> raw-window-handle -> NSView -> NSWindow.
        let ns_window = window
            .window()
            .with_winit_window(|w: &winit::window::Window| ns_window_from_winit(w))
            .flatten();
        let Some(ns_window) = ns_window else {
            eprintln!("could not resolve NSWindow from winit window");
            return;
        };

        ns_window.makeKeyAndOrderFront(None);
    }

    fn ns_window_from_winit(
        winit_window: &winit::window::Window,
    ) -> Option<objc2::rc::Retained<NSWindow>> {
        // `raw-window-handle` only exposes the content view's `NSView` ptr;
        // walk up via `[NSView window]` per the raw-window-handle docs.
        let handle = winit_window.window_handle().ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        let ns_view_ptr: NonNull<NSView> = appkit.ns_view.cast();
        // SAFETY: winit owns the NSView and keeps it alive for the lifetime
        // of the winit window. The pointer it hands us via raw-window-handle
        // is guaranteed valid while `winit_window` (the closure argument)
        // exists. We only borrow it for the duration of `[NSView window]`.
        let ns_view: &NSView = unsafe { ns_view_ptr.as_ref() };
        ns_view.window()
    }
}

/// Decode a plugin-supplied icon spec into a `slint::Image`.
///
/// Anything that isn't a `data:<mime>;base64,...` URI returns
/// `Image::default()` (paints blank, placeholder row styling kicks in).
/// Bare filesystem paths are deliberately not loaded — plugins resolve via
/// `highbeam:icons.forPath(...)`; touching the filesystem from the render
/// path would couple the UI thread to disk latency.
pub(crate) fn decode_icon(spec: Option<&str>) -> Image {
    let Some(spec) = spec else {
        return Image::default();
    };
    let Some((mime, b64)) = parse_data_uri(spec) else {
        return Image::default();
    };
    let Ok(bytes) = STANDARD.decode(b64) else {
        return Image::default();
    };
    decode_bytes(mime, &bytes).unwrap_or_default()
}

fn parse_data_uri(spec: &str) -> Option<(&str, &str)> {
    let rest = spec.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let meta = meta.strip_suffix(";base64")?;
    Some((meta, payload))
}

fn decode_bytes(mime: &str, bytes: &[u8]) -> Option<Image> {
    if mime.eq_ignore_ascii_case("image/svg+xml") {
        return Image::load_from_svg_data(bytes).ok();
    }
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba.as_raw(), width, height);
    Some(Image::from_rgba8(buffer))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1×1 green PNG, hand-encoded so the test stays self-contained.
    const PNG_1X1_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

    // 1×1 JPEG — rare in launcher icons but the decode path needs coverage.
    const TINY_JPEG_B64: &str = "/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQH/2wBDAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQH/wAARCAABAAEDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwD9/KKKKAP/2Q==";

    #[test]
    fn decode_icon_none_returns_default() {
        let img = decode_icon(None);
        assert_eq!(img.size().width, 0);
    }

    #[test]
    fn decode_icon_empty_string_returns_default() {
        let img = decode_icon(Some(""));
        assert_eq!(img.size().width, 0);
    }

    #[test]
    fn decode_icon_non_data_uri_returns_default() {
        // Bare filesystem path: plugin was supposed to pre-resolve via
        // `highbeam:icons.forPath(...)`; do NOT touch the disk here.
        let img = decode_icon(Some(
            "/Applications/Safari.app/Contents/Resources/AppIcon.icns",
        ));
        assert_eq!(img.size().width, 0);
    }

    #[test]
    fn decode_icon_http_url_returns_default() {
        let img = decode_icon(Some("https://example.com/icon.png"));
        assert_eq!(img.size().width, 0);
    }

    #[test]
    fn decode_icon_valid_png_data_uri_returns_image() {
        let uri = format!("data:image/png;base64,{PNG_1X1_B64}");
        let img = decode_icon(Some(&uri));
        assert_eq!(img.size().width, 1);
        assert_eq!(img.size().height, 1);
    }

    #[test]
    fn decode_icon_valid_jpeg_data_uri_returns_image() {
        let uri = format!("data:image/jpeg;base64,{TINY_JPEG_B64}");
        let img = decode_icon(Some(&uri));
        assert_eq!(img.size().width, 1);
        assert_eq!(img.size().height, 1);
    }

    #[test]
    fn decode_icon_invalid_base64_returns_default() {
        let img = decode_icon(Some("data:image/png;base64,not!valid!base64!@#$"));
        assert_eq!(img.size().width, 0);
    }

    #[test]
    fn decode_icon_unsupported_mime_returns_default() {
        let img = decode_icon(Some("data:text/plain;base64,aGVsbG8="));
        assert_eq!(img.size().width, 0);
    }

    #[test]
    fn decode_icon_missing_base64_marker_returns_default() {
        // No `;base64` suffix — we only support base64 payloads.
        let img = decode_icon(Some("data:image/png,abc"));
        assert_eq!(img.size().width, 0);
    }
}
