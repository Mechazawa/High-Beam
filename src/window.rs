//! Slint window construction and lifecycle.
//!
//! The `QueryWindow` markup is included via the `slint::include_modules!` macro
//! in the crate root. Window-level behaviour that the .slint markup can't
//! express — sizing, native center positioning, blur-to-close — lives here.

use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

use crate::QueryWindow;

/// Wire up the `QueryWindow` callbacks and native-window behaviour.
///
/// Caller is responsible for showing/hiding the window in response to hotkey
/// or IPC events. This function only attaches handlers.
pub(crate) fn configure(window: &QueryWindow) {
    // Typing in the input prints to stdout. Stage 3 replaces this with the
    // dispatch into the plugin runtime.
    window.on_query_edited(|text| {
        println!("query: {text}");
    });

    // Esc closes the window. The daemon stays running.
    let weak_for_esc = window.as_weak();
    window.on_escape_pressed(move || {
        if let Some(w) = weak_for_esc.upgrade() {
            hide(&w);
        }
    });

    // Handle window-level focus events:
    //   * Focused(true)  — the OS just made our window the key window. Forward
    //     focus to the TextInput now. Doing this from `show()` directly is too
    //     early on macOS: even after `window.show()` returns, the NSWindow is
    //     not yet the key window, so Slint's focus request gets dropped. By the
    //     time we receive this event the window is actually mapped and active.
    //   * Focused(false) — blur-to-close. Daemon keeps running; window hides.
    // reason: winit Focused(true) is the earliest tick at which Slint's focus
    // request is guaranteed to land on the input. Tried calling
    // `invoke_focus_input` from `show()` — focus was lost before the window
    // became key, so the user had to click into the field.
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
    // `NSApp.setActivationPolicy(NSApplicationActivationPolicyAccessory)` (or,
    // cleaner, bundle the app with `LSUIElement=1` in Info.plist when we ship
    // a real .app target). Independent from the activation logic below: that
    // call only governs frontmost/key state, not whether we appear in
    // Cmd-Tab / Dock.
}

/// Show the window, center it, and focus the input. Idempotent — calling this
/// while the window is already visible just re-centers and re-focuses, which
/// matches v2's behaviour and the spec ("focuses it if already open").
///
/// On macOS, simply calling `window.show()` + `invoke_focus_input()` is not
/// enough: our daemon process isn't necessarily the frontmost app, and even
/// once Slint creates the `NSWindow` it isn't automatically the key window.
/// Slint will dutifully ask the `TextInput` to take focus, but the OS routes
/// keystrokes to whoever is actually key, so the field appears focused
/// visually (or not at all) and typing goes nowhere. We replicate what
/// Spotlight/Alfred/Raycast do: activate the app process and make the
/// `NSWindow` key + frontmost ourselves before asking Slint for focus.
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
/// covers every close path (Esc, blur, and any future programmatic close)
/// because they all funnel through here.
pub(crate) fn hide(window: &QueryWindow) {
    window.invoke_clear_input();
    if let Err(err) = window.hide() {
        eprintln!("failed to hide window: {err}");
    }
}

/// Center horizontally and place vertically at ~1/3 from the top, Spotlight-style.
fn center_on_focused_display(window: &QueryWindow) {
    // Resolve the monitor under the current cursor; on macOS that's the screen
    // the user is actively looking at, which matches what users expect from
    // Spotlight. Falls back to the primary monitor if winit can't tell us.
    // We work in physical pixels throughout, so the monitor's scale factor is
    // irrelevant here.
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
    // Monitor sizes from winit are u32. They're physical pixel counts of a
    // single display, which fits comfortably in i32 in any reasonable setup.
    let monitor_w = i32::try_from(monitor_size.width).unwrap_or(i32::MAX);
    let monitor_h = i32::try_from(monitor_size.height).unwrap_or(i32::MAX);
    let win_w = i32::try_from(window_size.width).unwrap_or(i32::MAX);
    let win_h = i32::try_from(window_size.height).unwrap_or(i32::MAX);

    let x = monitor_pos.x + (monitor_w - win_w) / 2;
    // Place the top of the window at roughly 1/3 of the screen height — that
    // sits above center the way Spotlight does, regardless of window height.
    let y = monitor_pos.y + (monitor_h / 3) - (win_h / 2);

    slint_window.set_position(slint::PhysicalPosition::new(x, y));
}

#[cfg(target_os = "macos")]
mod macos {
    //! macOS-specific app/window activation. Without this the launcher window
    //! opens behind whatever was previously frontmost and the `TextInput` never
    //! actually receives keystrokes — Slint's focus request is meaningless if
    //! the OS-level key window is still someone else's. See `show()` for the
    //! call site and rationale.

    use std::ptr::NonNull;

    use objc2_app_kit::{NSApplication, NSView, NSWindow};
    use objc2_foundation::MainThreadMarker;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::ComponentHandle;
    use slint::winit_030::{WinitWindowAccessor, winit};

    use crate::QueryWindow;

    /// Activate our app process and make our `NSWindow` key + frontmost.
    ///
    /// Order matters:
    ///   1. `NSApp.activate(ignoringOtherApps: true)` — flips the process to
    ///      frontmost. Without this, `makeKeyAndOrderFront` on a background
    ///      app's window may show the window but won't move keyboard focus.
    ///   2. `nsWindow.makeKeyAndOrderFront(nil)` — makes our specific window
    ///      the key window and brings it to the front of the app's stack.
    ///
    /// The Slint event loop guarantees we're on the main thread, so the
    /// `MainThreadMarker` ask is just paperwork. We do check with `::new()`
    /// rather than `::new_unchecked()` so a future off-thread caller fails
    /// loudly instead of being undefined behaviour.
    pub fn activate_and_make_key(window: &QueryWindow) {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("activate_and_make_key called off the main thread");
            return;
        };

        let app = NSApplication::sharedApplication(mtm);
        // reason: `activateIgnoringOtherApps:` is marked deprecated in macOS 14
        // in favour of the cooperative `activate()`, but launchers explicitly
        // *don't* want to be cooperative — the whole point is "I am being
        // summoned over whatever you were doing." Spotlight/Alfred/Raycast all
        // still use the ignoring-other-apps variant for the same reason.
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);

        // Walk winit -> raw-window-handle -> NSView -> NSWindow. The Slint
        // `with_winit_window` accessor returns `Some(T)` if the closure ran;
        // we additionally encode "did we find an NSWindow" in the inner
        // `Option`, then flatten so a single None means "no window to focus".
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
        // `raw-window-handle` only exposes the `NSView` pointer for the
        // window's content view; we walk up via `[NSView window]` to get the
        // `NSWindow` itself. This is the path the raw-window-handle docs
        // explicitly call out as the supported way to reach the NSWindow.
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
