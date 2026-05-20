//! Daemon entry point — owns the Slint event loop, the global hotkey, and the
//! IPC socket. Cross-thread work routes through `slint::invoke_from_event_loop`
//! so UI work stays on the main thread.

use std::io;
use std::path::{Path, PathBuf};
use std::thread;

use slint::ComponentHandle;

use crate::QueryWindow;
use crate::app;
use crate::ipc::{Command, Server};
use crate::logging;
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

    // Pin the winit backend explicitly — we rely on it for monitor enumeration
    // and focus events; a default-backend swap would otherwise fail opaquely.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .select()?;

    let window = QueryWindow::new()?;
    window::configure(&window);
    window::apply_theme(&window, &Theme::load_or_default());

    // Keep the host alive for the daemon's lifetime; `Drop` sends Shutdown.
    let _plugin_host = app::start(&window, options.plugins_dir.clone())?;

    spawn_ipc_listener(&options.socket_path, window.as_weak())?;

    #[cfg(target_os = "macos")]
    let _hotkey_guard = spawn_hotkey_listener(window.as_weak());

    if options.open_on_start {
        window::show(&window);
    }

    // `run_event_loop_until_quit` (not `window.run()`) — the daemon must
    // survive window hide/show; `ComponentHandle::run()` ends the loop when
    // the last window closes, which would kill the daemon on the first Esc.
    slint::run_event_loop_until_quit()?;
    Ok(())
}

fn spawn_ipc_listener(socket_path: &Path, weak: slint::Weak<QueryWindow>) -> io::Result<()> {
    let server = Server::bind(socket_path)?;
    thread::Builder::new()
        .name("highbeam-ipc".into())
        .spawn(move || {
            let result = server.run(move |cmd| match cmd {
                Command::Open => {
                    let weak = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = weak.upgrade() {
                            window::show(&w);
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

#[cfg(target_os = "macos")]
fn spawn_hotkey_listener(
    weak: slint::Weak<QueryWindow>,
) -> Option<global_hotkey::GlobalHotKeyManager> {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
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

    let hotkey = HotKey::new(Some(Modifiers::SHIFT), Code::Space);
    if let Err(err) = manager.register(hotkey) {
        tracing::error!(%err, "failed to register Shift+Space hotkey");
        return None;
    }
    let hotkey_id = hotkey.id();

    thread::Builder::new()
        .name("highbeam-hotkey".into())
        .spawn(move || {
            let receiver = GlobalHotKeyEvent::receiver();
            while let Ok(event) = receiver.recv() {
                if event.id == hotkey_id && event.state == HotKeyState::Pressed {
                    let weak = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = weak.upgrade() {
                            window::show(&w);
                        }
                    });
                }
            }
        })
        .expect("spawn hotkey thread");

    Some(manager)
}
