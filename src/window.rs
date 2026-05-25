//! Slint window construction and lifecycle.
//!
//! `QueryWindow` markup is included via `slint::include_modules!`. Anything
//! the .slint file can't express — sizing, native center positioning,
//! blur-to-close, macOS app activation — lives here.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::QueryWindow;
use crate::settings::WindowPosition;
use crate::settings_ui::SettingsController;
use crate::theme::ThemeVariant;

/// Process-wide single-shot flag. When true, every dismiss path also calls
/// `slint::quit_event_loop` so the launcher process exits on first dismiss
/// instead of going dormant for the next hotkey. Set once at daemon
/// startup via [`set_once_mode`]; reads are race-free because nothing
/// ever flips it back.
static ONCE_MODE: AtomicBool = AtomicBool::new(false);

/// Enable single-shot dismiss-quits behaviour. Called once from
/// `daemon::run` before any dismiss handler is registered.
pub fn set_once_mode(once: bool) {
    ONCE_MODE.store(once, Ordering::Relaxed);
}

fn quit_if_once() {
    if !ONCE_MODE.load(Ordering::Relaxed) {
        return;
    }

    // `slint::quit_event_loop` would do the graceful exit dance — return
    // from `run_event_loop_until_quit`, fall out of `daemon::run`, drop
    // the window. On Linux Wayland that drop crashes with SIGSEGV in
    // Slint 1.16's renderer because we never called `window.hide()` (the
    // documented `is_hidden` workaround — see `hide()` below). For a
    // single-shot process that's about to disappear anyway, the graceful
    // path buys us nothing: settings position is persisted by
    // `hide_and_persist_position` before this fires, frecency picks are
    // persisted at action-time, plugin shutdown is fire-and-forget. Match
    // the existing `Action::Quit` path and hard-exit.
    std::process::exit(0);
}

/// Grace window after `show()` during which a `Focused(false)` event is
/// treated as compositor noise rather than a user dismiss. On GNOME-Mutter
/// the activation handoff fires a spurious blur ~1s after we appear (the
/// launching terminal's prompt redraws, focus-stealing-prevention kicks
/// in). 1500ms is long enough to swallow that without making the launcher
/// feel sticky on a deliberate click-away.
#[cfg(target_os = "linux")]
const BLUR_GRACE_MS: u64 = 1500;

static EPOCH: OnceLock<Instant> = OnceLock::new();
static LAST_SHOW_MS: AtomicU64 = AtomicU64::new(0);

fn mark_show_time() {
    let start = EPOCH.get_or_init(Instant::now);
    let ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

    LAST_SHOW_MS.store(ms, Ordering::Relaxed);
}

#[cfg(target_os = "linux")]
fn ms_since_show() -> u64 {
    let Some(start) = EPOCH.get() else {
        return u64::MAX;
    };

    let now_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

    now_ms.saturating_sub(LAST_SHOW_MS.load(Ordering::Relaxed))
}

/// Wire up the `QueryWindow` callbacks and native-window behaviour.
///
/// Caller is responsible for showing/hiding the window in response to hotkey
/// or IPC events; this only attaches handlers. The plugin-state-dependent
/// `on_query_edited` and `on_invoke_selected` callbacks are wired by
/// `crate::app::start`.
///
/// `settings` is the shared [`SettingsController`] — used here to read the
/// last user-chosen window position on show and to persist a new one when
/// the window hides after a drag.
pub(crate) fn configure(window: &QueryWindow, settings: SettingsController) {
    let weak_for_esc = window.as_weak();
    let settings_for_esc = settings.clone();

    window.on_escape_pressed(move || {
        if let Some(w) = weak_for_esc.upgrade() {
            hide_and_persist_position(&w, &settings_for_esc);
        }
    });

    let weak_for_drag = window.as_weak();

    window.on_request_window_drag(move || {
        if let Some(w) = weak_for_drag.upgrade() {
            start_native_drag(&w);
        }
    });

    let weak_for_recenter = window.as_weak();
    let settings_for_recenter = settings.clone();

    window.on_recenter_window(move || {
        settings_for_recenter.clear_launcher_position();

        if let Some(w) = weak_for_recenter.upgrade() {
            center_on_focused_display(&w);
        }
    });

    window.on_open_config_dir(|| match crate::paths::config_dir() {
        Ok(path) => {
            if let Err(err) = open::that(&path) {
                tracing::warn!(path = %path.display(), %err, "settings: open config dir failed");
            }
        }
        Err(err) => tracing::warn!(%err, "settings: could not resolve config dir"),
    });

    // winit Focused(true) is the earliest tick at which Slint's focus request
    // is guaranteed to land on the input. Calling `invoke_focus_input` from
    // `show()` is too early on macOS — the NSWindow isn't yet the key window
    // and the request gets dropped, leaving the user clicking into the field.
    //
    // Focused(false) → hide-on-blur runs on both macOS and Linux. The Linux
    // path is debounced by `BLUR_GRACE_MS` to swallow the spurious blur that
    // GNOME-Mutter fires ~1s after we appear (during the activation handoff:
    // the launching terminal's prompt redraws, focus-stealing-prevention
    // kicks in). The old accesskit-panic concern that kept this disabled is
    // moot now that `hide()` on Linux routes through `set_is_hidden(true)`
    // instead of Slint's broken Wayland `hide()`.
    let weak_for_focus = window.as_weak();
    let settings_for_focus = settings;

    window.window().on_winit_window_event(move |_slint_win, event| {
        match event {
            winit::event::WindowEvent::Focused(true) => {
                if let Some(w) = weak_for_focus.upgrade() {
                    // Only land focus on the launcher input when the launcher
                    // is the visible view — otherwise we'd yank focus off the
                    // settings rail / Done FocusScope every time the window
                    // regains focus (drag end, click-back, compositor reshuffle),
                    // and the launcher's invisible TextInput would eat Esc.
                    if w.get_current_view() == 0 {
                        w.invoke_focus_input();
                    }
                }
            }
            winit::event::WindowEvent::Focused(false) => {
                #[cfg(target_os = "linux")]
                if ms_since_show() < BLUR_GRACE_MS {
                    return EventResult::Propagate;
                }

                if let Some(w) = weak_for_focus.upgrade() {
                    // Settings and confirm views own modal-ish work — a
                    // native drag transiently steals focus on every platform
                    // and would auto-dismiss what the user is mid-editing.
                    // VIEW-QUERY (0) keeps blur-to-dismiss; anything else
                    // requires Esc.
                    if w.get_current_view() != 0 {
                        return EventResult::Propagate;
                    }

                    hide_and_persist_position(&w, &settings_for_focus);
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

/// Push theme tokens into the window's `in-out` properties. Re-callable
/// — the dark-mode watcher set up in [`crate::daemon::run`] bounces back
/// here through the Slint event loop when the OS appearance flips, so
/// every call must overwrite the full property surface rather than
/// patching individual fields.
pub(crate) fn apply_theme(window: &QueryWindow, theme: &ThemeVariant) {
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

/// Show the window, position it (saved or centered), and focus the input.
/// Idempotent — calling while already visible re-applies position and
/// re-focuses ("focuses it if already open").
///
/// Focus-grab is platform-specific:
/// * macOS uses `NSApp.activate(ignoringOtherApps:)` +
///   `makeKeyAndOrderFront`; no token plumbing.
/// * Wayland uses `xdg_activation_v1.activate(token, surface)` — the
///   `activation_token` is forwarded from the invoking process (terminal
///   keybind, IPC `--open`, etc.). Without it the compositor will not
///   raise our surface above whatever is currently focused, and on
///   GNOME-Mutter the launcher silently opens behind the active window.
/// * X11 / other paths ignore the token; winit handles focus there.
pub(crate) fn show(window: &QueryWindow, settings: &SettingsController, activation_token: Option<&str>) {
    // On Linux the previous dismiss may have left us in the `is-hidden`
    // collapsed-1×1 state instead of going through Slint's hide (which is
    // broken on Wayland — see `window_wayland`). Flip back BEFORE the show
    // call so Slint sees the real size when it re-applies layout.
    #[cfg(target_os = "linux")]
    window.set_is_hidden(false);

    if let Err(err) = window.show() {
        tracing::error!(%err, "failed to show window");
        return;
    }

    mark_show_time();
    apply_saved_or_centered_position(window, settings);
    #[cfg(target_os = "macos")]
    {
        let _ = activation_token; // unused on macOS; NSApp.activate covers it
        macos::activate_and_make_key(window);
    }

    #[cfg(target_os = "linux")]
    if let Some(token) = activation_token {
        crate::window_wayland::activate_with_token(window, token);
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = activation_token;
    window.invoke_focus_input();
}

/// Hide the window. Clears the input text so the next open starts fresh —
/// every close path (Esc, blur, programmatic) funnels through here.
/// Does *not* persist position; use [`hide_and_persist_position`] for the
/// user-driven close paths.
///
/// On Linux this DELIBERATELY does NOT call `Window::hide()`. Slint 1.16's
/// Wayland hide path destroys the underlying winit window via `suspend()`,
/// and when the destroy fails (because Slint's renderer holds extra
/// `Arc<winit::Window>` refs we can't drop from app code) the state still
/// flips to "None" — so the *next* `show()` won't re-attach the surface
/// and the launcher silently never opens again. Instead we set an
/// `is-hidden` flag in the .slint file that collapses the visible content
/// to 1×1 transparent, keeping Slint's `shown` state intact so subsequent
/// activations work.
pub(crate) fn hide(window: &QueryWindow) {
    window.invoke_clear_input();
    #[cfg(target_os = "linux")]
    {
        window.set_is_hidden(true);
    }

    #[cfg(not(target_os = "linux"))]
    if let Err(err) = window.hide() {
        tracing::error!(%err, "failed to hide window");
    }

    quit_if_once();
}

/// Snapshot the current window position, hand it to a background thread
/// for fire-and-forget persistence, then hide. The disk write must not
/// block the UI thread — same pattern the frecency picks use — so the
/// hide is observably instant regardless of disk latency.
pub(crate) fn hide_and_persist_position(window: &QueryWindow, settings: &SettingsController) {
    // Persist-dismiss fires BEFORE the hide so subscribers see the input
    // while it's still live — `clear-input` zeroes the text on hide.
    window.invoke_persist_dismiss(window.get_query_text());

    if let Some(pos) = capture_outer_position(window) {
        if ONCE_MODE.load(Ordering::Relaxed) {
            // In single-shot we're about to `quit_event_loop` and exit
            // the process — spawning a fire-and-forget thread races the
            // process death, so do the disk write inline. The user already
            // committed (Esc/Enter); the extra few ms is invisible.
            settings.set_launcher_position(pos);
        } else {
            let settings = settings.clone();

            if let Err(err) = thread::Builder::new()
                .name("highbeam-settings-position".into())
                .spawn(move || {
                    settings.set_launcher_position(pos);
                })
            {
                tracing::warn!(%err, "settings: position-persist thread spawn failed");
            }
        }
    }

    hide(window);
}

/// Apply the persisted launcher position, falling back to the centered
/// default when no position is saved or the saved one is no longer on any
/// connected display. The off-screen check matters when the user unplugs
/// the monitor the window was last positioned on — without it, the next
/// show would put the window in the void.
fn apply_saved_or_centered_position(window: &QueryWindow, settings: &SettingsController) {
    let Some(saved) = settings.launcher_position() else {
        center_on_focused_display(window);
        return;
    };

    let rects = monitor_rects(window);

    if !position_visible_on_any_monitor(saved, &rects) {
        tracing::info!(
            x = saved.x,
            y = saved.y,
            "settings: saved launcher position is off-screen, recentering",
        );
        center_on_focused_display(window);

        return;
    }

    set_outer_position(window, saved);
}

/// Read the OS-reported outer position. Returns `None` on platforms or
/// states where winit can't tell us (e.g. headless tests).
fn capture_outer_position(window: &QueryWindow) -> Option<WindowPosition> {
    window
        .window()
        .with_winit_window(|w: &winit::window::Window| {
            w.outer_position().ok().map(|p| WindowPosition { x: p.x, y: p.y })
        })
        .flatten()
}

/// Push the window's outer position via winit directly — `slint::Window::
/// set_position` writes the inner/content position which on macOS differs
/// from the outer `NSWindow` frame by the title bar height. We use the outer
/// rect so a restored position lines up byte-identical with where the user
/// dropped it.
fn set_outer_position(window: &QueryWindow, pos: WindowPosition) {
    let _ = window.window().with_winit_window(|w: &winit::window::Window| {
        w.set_outer_position(winit::dpi::PhysicalPosition::new(pos.x, pos.y));
    });
}

/// Kick off a native OS-driven window drag. winit's `drag_window()` blocks
/// inside the OS-level move loop until the user releases the mouse —
/// position is updated by the WM, not by us, so there's nothing to do on
/// the Rust side after this call.
fn start_native_drag(window: &QueryWindow) {
    let _ = window.window().with_winit_window(|w: &winit::window::Window| {
        if let Err(err) = w.drag_window() {
            tracing::warn!(%err, "winit drag_window failed");
        }
    });
}

/// Collect virtual-screen rectangles for every connected monitor. Empty
/// when winit can't introspect (headless test, missing backend) — callers
/// then treat any saved position as off-screen and recenter.
fn monitor_rects(window: &QueryWindow) -> Vec<MonitorRect> {
    window
        .window()
        .with_winit_window(|w: &winit::window::Window| {
            w.available_monitors()
                .map(|m| {
                    let pos = m.position();
                    let size = m.size();

                    MonitorRect {
                        x: pos.x,
                        y: pos.y,
                        width: i32::try_from(size.width).unwrap_or(i32::MAX),
                        height: i32::try_from(size.height).unwrap_or(i32::MAX),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Virtual-desktop rectangle, in physical pixels, used by
/// [`position_visible_on_any_monitor`]. Kept as a plain struct (rather than
/// re-using winit's `MonitorHandle`) so the visibility check is unit-
/// testable without a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Whether a candidate window origin would land somewhere a user could
/// actually grab it. The check is on the top-left corner only — "any
/// pixel overlap" was too permissive (a 1px sliver on a remembered
/// monitor that's now disconnected leaves the window unreachable), so we
/// require the origin itself to land inside some monitor.
pub(crate) fn position_visible_on_any_monitor(pos: WindowPosition, monitors: &[MonitorRect]) -> bool {
    if monitors.is_empty() {
        return false;
    }

    monitors.iter().any(|m| {
        // Inclusive on the leading edge, exclusive on the trailing — a
        // window at exactly the bottom-right pixel of a monitor has its
        // origin one row past the visible area.
        pos.x >= m.x && pos.y >= m.y && pos.x < m.x.saturating_add(m.width) && pos.y < m.y.saturating_add(m.height)
    })
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
            tracing::error!("activate_and_make_key called off the main thread");
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
            tracing::error!("could not resolve NSWindow from winit window");
            return;
        };

        ns_window.makeKeyAndOrderFront(None);
    }

    fn ns_window_from_winit(winit_window: &winit::window::Window) -> Option<objc2::rc::Retained<NSWindow>> {
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
    const PNG_1X1_B64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

    // 1×1 JPEG — rare in launcher icons but the decode path needs coverage.
    const TINY_JPEG_B64: &str = "/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQH/2wBDAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQH/wAARCAABAAEDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwD9/KKKKAP/2Q==";

    #[test]
    fn decode_icon_non_data_uri_returns_default() {
        // Bare filesystem path: plugin was supposed to pre-resolve via
        // `highbeam:icons.forPath(...)`; do NOT touch the disk here.
        let img = decode_icon(Some("/Applications/Safari.app/Contents/Resources/AppIcon.icns"));
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

    /// Single primary display at (0, 0) of size 1920x1080 — the common
    /// laptop-only case. Used as the baseline rect set for the visibility
    /// tests below.
    fn primary_only() -> Vec<MonitorRect> {
        vec![MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }]
    }

    /// Primary + a secondary display placed to the right of the primary —
    /// origin at (1920, 0). Mirrors a common dual-monitor desktop layout.
    fn primary_and_secondary_right() -> Vec<MonitorRect> {
        vec![
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            MonitorRect {
                x: 1920,
                y: 0,
                width: 2560,
                height: 1440,
            },
        ]
    }

    #[test]
    fn position_visible_in_primary_display() {
        let pos = WindowPosition { x: 100, y: 200 };
        assert!(position_visible_on_any_monitor(pos, &primary_only(),));
    }

    #[test]
    fn position_visible_in_secondary_display() {
        // 2000 > primary's right edge but inside the secondary that starts
        // at x = 1920. Without secondary-aware logic this would be reported
        // as off-screen and recenter unnecessarily.
        let pos = WindowPosition { x: 2000, y: 100 };
        assert!(position_visible_on_any_monitor(pos, &primary_and_secondary_right()));
    }

    #[test]
    fn position_off_all_displays_is_rejected() {
        // y = 5000 is past the bottom of every monitor. This is the
        // "user moved the launcher to a display they later disconnected"
        // case — the host should recenter rather than place the window in
        // the void.
        let pos = WindowPosition { x: 100, y: 5000 };
        assert!(!position_visible_on_any_monitor(pos, &primary_and_secondary_right()));
    }

    #[test]
    fn position_at_top_left_boundary_is_visible() {
        // The leading edge is inclusive — (0, 0) is the first visible pixel
        // of a monitor whose origin is (0, 0).
        let pos = WindowPosition { x: 0, y: 0 };
        assert!(position_visible_on_any_monitor(pos, &primary_only(),));
    }

    #[test]
    fn position_at_bottom_right_boundary_is_off_screen() {
        // The trailing edge is exclusive — a window whose origin lands
        // exactly at the pixel past the last visible column/row is
        // technically off-screen and we treat it as such to keep the
        // recenter-on-unreachable contract crisp.
        let pos = WindowPosition { x: 1920, y: 1080 };
        assert!(!position_visible_on_any_monitor(pos, &primary_only(),));
    }

    #[test]
    fn position_visible_requires_at_least_one_monitor() {
        // No monitors known to winit (headless tests, backend not
        // initialised) means we can't validate; the safe default is "treat
        // as off-screen" so the host falls back to the centered path.
        let pos = WindowPosition { x: 100, y: 100 };
        assert!(!position_visible_on_any_monitor(pos, &[]));
    }
}
