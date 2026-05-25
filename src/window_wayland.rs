//! Wayland focus-grab via `xdg_activation_v1`.
//!
//! macOS gets focus by calling `NSApp.activate(ignoringOtherApps:)` +
//! `NSWindow.makeKeyAndOrderFront`. The Wayland analog is the
//! `xdg_activation_v1` protocol: the client that wants focus passes an
//! activation token (typically obtained as the `XDG_ACTIVATION_TOKEN` env
//! var by whatever just launched it) to
//! `xdg_activation_v1.activate(token, surface)`. The compositor honors
//! that request if it considers the token recent enough — without it,
//! GNOME-Mutter silently raises the launcher behind whatever has focus.
//!
//! winit 0.30 only consumes activation tokens at window *creation* time
//! (via `WindowAttributes::with_activation_token`); there's no public API
//! to re-activate an existing window. So we drop down to wayland-client
//! and share winit's `wl_display` via `Backend::from_foreign_display` — same
//! pattern softbuffer and smithay-clipboard use to interop with winit.
//!
//! The connection runs in "guest" mode so dropping our backend doesn't
//! close winit's socket. We talk to a private `EventQueue` so the registry
//! roundtrip doesn't disturb winit's own dispatch.

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use slint::ComponentHandle;
use slint::winit_030::{WinitWindowAccessor, winit};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

use crate::QueryWindow;
use crate::logging::LogErr;

/// Activate our Wayland surface with the supplied `XDG_ACTIVATION_TOKEN`.
///
/// Errors are logged at warn level rather than propagated: a failed
/// activation degrades to "window opens unfocused" — annoying, not fatal.
/// We never want a token typo or a compositor that doesn't bind
/// `xdg_activation_v1` to break the launcher entirely.
pub fn activate_with_token(window: &QueryWindow, token: &str) {
    if let Err(err) = try_activate(window, token) {
        tracing::warn!(%err, "wayland: xdg_activation_v1.activate failed");
    }
}

fn try_activate(window: &QueryWindow, token: &str) -> Result<(), Box<dyn std::error::Error>> {
    // All Wayland work happens inside `with_winit_window` so the raw
    // wl_display / wl_surface ptrs we extract via raw-window-handle stay
    // valid until the closure returns. winit owns them and the window is
    // alive while `window` is.
    let result =
        window
            .window()
            .with_winit_window(|w: &winit::window::Window| -> Result<(), Box<dyn std::error::Error>> {
                let display_handle = w.display_handle()?;
                let window_handle = w.window_handle()?;
                let (display_ptr, surface_ptr) = match (display_handle.as_raw(), window_handle.as_raw()) {
                    (RawDisplayHandle::Wayland(d), RawWindowHandle::Wayland(s)) => {
                        (d.display.as_ptr(), s.surface.as_ptr())
                    }
                    _ => {
                        // X11 / non-Wayland: nothing for us to do here.
                        // winit's regular activation handling covers X11.
                        return Ok(());
                    }
                };

                // SAFETY: `display_ptr` is winit's live wl_display for the
                // lifetime of this closure. `from_foreign_display` runs the
                // backend in "guest" mode — dropping it at the end of this
                // function does NOT close winit's connection.
                let backend = unsafe { Backend::from_foreign_display(display_ptr.cast()) };
                let conn = Connection::from_backend(backend);

                // Discover and bind `xdg_activation_v1`. This roundtrips to
                // the compositor on a private event queue, so winit's queue
                // is undisturbed.
                let (globals, mut event_queue) = registry_queue_init::<ActivateState>(&conn)?;
                let qh = event_queue.handle();
                let activation: XdgActivationV1 = globals.bind(&qh, 1..=1, ())?;

                // Reconstruct the WlSurface proxy from the raw ptr. SAFETY:
                // `surface_ptr` is winit's wl_surface for *this* window — it
                // outlives the closure, and the interface matches.
                let surface_id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface_ptr.cast())? };
                let surface = WlSurface::from_id(&conn, surface_id)?;

                // Fire-and-forget activation request. The compositor decides
                // whether to honor it; we never see a response.
                activation.activate(token.to_owned(), &surface);
                conn.flush()?;
                // Drain anything the registry roundtrip left pending so we
                // drop the queue cleanly.
                event_queue
                    .dispatch_pending(&mut ActivateState)
                    .log_warn("wayland: drain pending events after activation");
                Ok(())
            });

    match result {
        Some(Ok(())) => Ok(()),
        Some(Err(err)) => Err(err),
        None => Err("with_winit_window returned None (no winit backend)".into()),
    }
}

/// Empty Dispatch sink — we only do one registry roundtrip and send one
/// `activate` request; no events we receive matter.
struct ActivateState;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ActivateState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgActivationV1, ()> for ActivateState {
    fn event(
        _: &mut Self,
        _: &XdgActivationV1,
        _: <XdgActivationV1 as Proxy>::Event,
        &(): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSurface, ()> for ActivateState {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: <WlSurface as Proxy>::Event,
        &(): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
