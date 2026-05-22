//! Daemon entry point — owns the Slint event loop, the global hotkey, and the
//! IPC socket. Cross-thread work routes through `slint::invoke_from_event_loop`
//! so UI work stays on the main thread.

use std::io;
use std::path::{Path, PathBuf};
use std::thread;

use slint::ComponentHandle;

use crate::QueryWindow;
use crate::app;
use crate::bundle_install;
use crate::ipc::{Command, Server};
use crate::logging;
use crate::plugins::loader::{self, LoaderOptions};
use crate::settings::Settings;
use crate::settings_ui::SettingsController;
use crate::theme::Theme;
use crate::window;

pub struct Options {
    /// Open the window immediately after the daemon starts.
    pub open_on_start: bool,
    /// Path to the unix socket we'll bind for single-instance.
    pub socket_path: PathBuf,
    /// Override for the plugins directory. `None` uses the default search
    /// order in [`crate::plugins::loader::LoaderOptions::resolve`].
    pub plugins_dir: Option<PathBuf>,
}

/// Run the daemon. Blocks until the Slint event loop exits.
///
/// # Errors
///
/// Returns an error if the Slint backend fails to initialize, the window
/// fails to construct, the unix socket can't be bound (e.g. another daemon
/// is already running), or the event loop reports a runtime error.
// reason: `Options` is a config struct created by the caller and consumed
// here; by-value is more ergonomic than forcing the caller to keep it alive.
#[allow(clippy::needless_pass_by_value)]
pub fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    logging::try_init();

    // Run before the plugin loader picks a directory: when launched from
    // `HighBeam.app` we want bundled defaults landing in the user's plugin
    // dir before [`crate::plugins::loader::LoaderOptions::resolve`] scans
    // it. A `--plugins-dir` override bypasses the platform default and
    // therefore the bundle install path too — that's intentional, devs
    // pointing at an arbitrary checkout don't want their workspace seeded.
    if options.plugins_dir.is_none() {
        bundle_install::install_default_plugins_if_needed();
    }

    // Pin the winit backend explicitly — we rely on it for monitor enumeration
    // and focus events; a default-backend swap would otherwise fail opaquely.
    slint::BackendSelector::new().backend_name("winit".into()).select()?;

    let window = QueryWindow::new()?;
    window::apply_theme(&window, &Theme::load_or_default());

    // Wire the settings view. We scan manifests synchronously here (cheap —
    // just reads `manifest.json` from each plugin dir) so the controller can
    // render rows for every plugin including disabled ones; the runtime
    // thread re-scans through `loader::load_all` for the JS load path.
    let loader_opts = LoaderOptions::resolve(options.plugins_dir.clone());
    let manifests = loader::scan_manifests(&loader_opts);
    let settings_for_ui = Settings::load_or_default();
    // Snapshot the hotkey before handing the settings into the controller —
    // hot-reload is out of scope for v1, so the daemon-startup value is what
    // we register with the OS.
    let hotkey_spec = settings_for_ui.global().hotkey.clone();
    let settings_controller = SettingsController::new(manifests, settings_for_ui);

    settings_controller.wire(&window);

    // Configure the window only after the settings controller exists —
    // `configure` wires the drag/recenter callbacks against the live
    // controller so dragged positions flow into the same on-disk file the
    // settings UI is writing.
    window::configure(&window, settings_controller.clone());

    // Keep the host alive for the daemon's lifetime; `Drop` sends Shutdown.
    let _plugin_host = app::start(&window, options.plugins_dir.clone(), settings_controller.clone())?;

    spawn_ipc_listener(&options.socket_path, window.as_weak(), settings_controller.clone())?;

    #[cfg(target_os = "macos")]
    let _hotkey_guard = {
        let reg = spawn_hotkey_listener(window.as_weak(), &hotkey_spec, settings_controller.clone());
        if let Some(reg) = reg {
            let shared = std::sync::Arc::new(reg);
            settings_controller.attach_hotkey_registration(std::sync::Arc::clone(&shared));
            Some(shared)
        } else {
            None
        }
    };
    // Suppress unused-variable warning on Linux where we don't register a
    // global hotkey (the WM handles it via `highbeam --open`).
    #[cfg(not(target_os = "macos"))]
    let _ = hotkey_spec;

    if options.open_on_start {
        window::show(&window, &settings_controller);
    }

    // `run_event_loop_until_quit` (not `window.run()`) — the daemon must
    // survive window hide/show; `ComponentHandle::run()` ends the loop when
    // the last window closes, which would kill the daemon on the first Esc.
    slint::run_event_loop_until_quit()?;
    Ok(())
}

fn spawn_ipc_listener(
    socket_path: &Path,
    weak: slint::Weak<QueryWindow>,
    settings: SettingsController,
) -> io::Result<()> {
    let server = Server::bind(socket_path)?;
    thread::Builder::new().name("highbeam-ipc".into()).spawn(move || {
        let result = server.run(move |cmd| match cmd {
            Command::Open => {
                let weak = weak.clone();
                let settings = settings.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = weak.upgrade() {
                        window::show(&w, &settings);
                    }
                });
            }
        });
        if let Err(err) = result {
            tracing::error!(%err, "ipc server exited");
        }
    })?;
    Ok(())
}

/// Parse a user-supplied accelerator string, falling back to the schema
/// default on any error. Daemon startup must never refuse to launch because
/// the user typo'd `Shftt+Space`.
#[cfg(target_os = "macos")]
fn parse_hotkey_or_default(spec: &str) -> global_hotkey::hotkey::HotKey {
    use std::str::FromStr;

    use global_hotkey::hotkey::HotKey;

    match HotKey::from_str(spec) {
        Ok(hk) => hk,
        Err(err) => {
            tracing::warn!(
                bad_value = %spec,
                fallback = crate::settings::DEFAULT_HOTKEY,
                %err,
                "settings: hotkey string did not parse; falling back to default",
            );

            HotKey::from_str(crate::settings::DEFAULT_HOTKEY).expect("DEFAULT_HOTKEY is a constant the parser knows")
        }
    }
}

/// Owned handle for the live OS-level hotkey registration. Wraps the
/// `GlobalHotKeyManager` together with the currently-registered `HotKey`,
/// so the settings layer can swap the binding without a daemon restart.
///
/// Linux builds get a no-op variant: the WM owns the hotkey there via
/// `highbeam --open`, so there's no OS handle for us to manage.
#[cfg(target_os = "macos")]
pub struct HotkeyRegistration {
    manager: global_hotkey::GlobalHotKeyManager,
    current: std::sync::Arc<std::sync::Mutex<global_hotkey::hotkey::HotKey>>,
}

#[cfg(not(target_os = "macos"))]
pub struct HotkeyRegistration;

#[cfg(target_os = "macos")]
impl HotkeyRegistration {
    /// Replace the live hotkey with the one parsed from `spec`. Bad specs
    /// fall back to the daemon default. On register failure, the previous
    /// binding is restored so the user is never left without a hotkey.
    pub fn reregister(&self, spec: &str) {
        let Ok(mut current) = self.current.lock() else {
            tracing::error!("hotkey: registration mutex poisoned");
            return;
        };
        let new = parse_hotkey_or_default(spec);
        let _ = self.manager.unregister(*current);

        if let Err(err) = self.manager.register(new) {
            tracing::error!(%err, %spec, "hotkey: re-register failed; restoring previous");
            let _ = self.manager.register(*current);
            return;
        }
        *current = new;
    }
}

#[cfg(not(target_os = "macos"))]
impl HotkeyRegistration {
    pub fn reregister(&self, _spec: &str) {
        // No OS-level hotkey on Linux; settings still persist for next launch.
    }
}

#[cfg(target_os = "macos")]
fn spawn_hotkey_listener(
    weak: slint::Weak<QueryWindow>,
    hotkey_spec: &str,
    settings: SettingsController,
) -> Option<HotkeyRegistration> {
    use std::sync::{Arc, Mutex};

    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    // GlobalHotKeyManager has to be created and kept alive on the same thread
    // for the duration we want hotkey events; the caller holds it for the
    // life of the daemon.
    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => m,
        Err(err) => {
            tracing::error!(%err, "failed to create global hotkey manager");
            return None;
        }
    };

    let hotkey = parse_hotkey_or_default(hotkey_spec);

    if let Err(err) = manager.register(hotkey) {
        tracing::error!(%err, spec = %hotkey_spec, "failed to register global hotkey");
        return None;
    }

    let current = Arc::new(Mutex::new(hotkey));
    let current_for_thread = Arc::clone(&current);

    if let Err(err) = thread::Builder::new().name("highbeam-hotkey".into()).spawn(move || {
        let receiver = GlobalHotKeyEvent::receiver();
        while let Ok(event) = receiver.recv() {
            // Resolve the live id on every event so re-registration takes
            // effect immediately — the registration mutex is uncontended
            // outside of the rare settings-driven swap.
            let live_id = current_for_thread.lock().ok().map(|h| h.id());
            if event.state == HotKeyState::Pressed && Some(event.id) == live_id {
                let weak = weak.clone();
                let settings = settings.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = weak.upgrade() {
                        window::show(&w, &settings);
                    }
                });
            }
        }
    }) {
        tracing::error!(%err, "failed to spawn hotkey listener thread; hotkey disabled");
        return None;
    }

    Some(HotkeyRegistration { manager, current })
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use super::parse_hotkey_or_default;
    use std::str::FromStr;

    use global_hotkey::hotkey::HotKey;

    use crate::settings::DEFAULT_HOTKEY;

    #[test]
    fn parses_valid_accelerator() {
        let hk = parse_hotkey_or_default("Cmd+K");
        let expected = HotKey::from_str("Cmd+K").expect("crate parses Cmd+K");
        assert_eq!(hk.id(), expected.id());
    }

    #[test]
    fn falls_back_on_garbage() {
        // Anything the global-hotkey parser refuses must degrade to the
        // schema default — the daemon must never refuse to start.
        let hk = parse_hotkey_or_default("Shftt+Space");
        let expected = HotKey::from_str(DEFAULT_HOTKEY).expect("default parses");
        assert_eq!(hk.id(), expected.id());
    }

    #[test]
    fn falls_back_on_empty_string() {
        let hk = parse_hotkey_or_default("");
        let expected = HotKey::from_str(DEFAULT_HOTKEY).expect("default parses");
        assert_eq!(hk.id(), expected.id());
    }

    #[test]
    fn default_hotkey_constant_is_parseable() {
        // Guard against someone editing DEFAULT_HOTKEY to something the
        // parser doesn't accept — the fallback path expect()s it.
        assert!(HotKey::from_str(DEFAULT_HOTKEY).is_ok());
    }
}
