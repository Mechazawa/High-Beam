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
pub fn configure(window: &QueryWindow) {
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

    // Hide the macOS Dock/Cmd-Tab presence by setting the app activation
    // policy to "accessory". This must be done before any window is shown.
    // TODO: revisit — the truly clean fix is bundling with `LSUIElement` in
    // Info.plist when we ship a .app, but this works for `cargo run`.
    #[cfg(target_os = "macos")]
    set_macos_accessory_policy();
}

/// Show the window, center it, and focus the input. Idempotent — calling this
/// while the window is already visible just re-centers and re-focuses, which
/// matches v2's behaviour and the spec ("focuses it if already open").
///
/// Focus on first show is driven by the `Focused(true)` winit event in
/// `configure()` — that's the earliest point where the `NSWindow` is actually
/// key on macOS. We also call `invoke_focus_input` here so a re-show while the
/// window is already visible (and thus won't get a fresh Focused(true)) still
/// pulls the caret back into the input.
pub fn show(window: &QueryWindow) {
    if let Err(err) = window.show() {
        eprintln!("failed to show window: {err}");
        return;
    }
    center_on_focused_display(window);
    window.invoke_focus_input();
}

/// Hide the window. Clears the input text so the next open starts fresh —
/// covers every close path (Esc, blur, and any future programmatic close)
/// because they all funnel through here.
pub fn hide(window: &QueryWindow) {
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
    let slint_window = window.window();
    let Some((monitor_pos, monitor_size, scale)) = slint_window
        .with_winit_window(|w: &winit::window::Window| {
            let monitor = w
                .current_monitor()
                .or_else(|| w.primary_monitor())
                .or_else(|| w.available_monitors().next())?;
            Some((monitor.position(), monitor.size(), monitor.scale_factor()))
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

    let _ = scale; // physical coords; we don't need to scale further here.
    slint_window.set_position(slint::PhysicalPosition::new(x, y));
}

#[cfg(target_os = "macos")]
fn set_macos_accessory_policy() {
    // The cleanest way to set NSApplicationActivationPolicyAccessory without
    // pulling in `objc2`/`cocoa` is to bundle the app with LSUIElement=1 in
    // its Info.plist. For `cargo run` we have no bundle, so the binary shows
    // up in Cmd-Tab and bounces in the Dock. Living with that for Stage 2;
    // revisit when we add a real .app bundle target.
    //
    // TODO: switch to a small `objc2` call (`NSApp.setActivationPolicy(...)`)
    // or ship an Info.plist via cargo-bundle.
}
