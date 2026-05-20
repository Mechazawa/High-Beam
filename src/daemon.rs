//! Daemon entry point — owns the Slint event loop, the global hotkey, and the
//! IPC socket. All cross-thread work routes through `slint::invoke_from_event_loop`
//! so UI work stays on the main thread.

use std::io;
use std::path::{Path, PathBuf};
use std::thread;

use slint::ComponentHandle;

use crate::QueryWindow;
use crate::ipc::{Command, Server};
use crate::window;

pub struct Options {
    /// Open the window immediately after the daemon starts.
    pub open_on_start: bool,
    /// Path to the unix socket we'll bind for single-instance.
    pub socket_path: PathBuf,
}

/// Run the daemon. Blocks until the Slint event loop exits.
///
/// # Errors
///
/// Returns an error if the Slint backend fails to initialize, the window
/// fails to construct, the unix socket can't be bound (e.g. another daemon
/// is already running), or the event loop reports a runtime error.
// reason: `Options` is a config struct created by the caller and consumed
// here; taking it by value is more ergonomic than forcing the caller to keep
// it alive across `run`.
#[allow(clippy::needless_pass_by_value)]
pub fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    // Pin the winit backend explicitly. We rely on it for monitor enumeration
    // and focus events; if a future Slint default-backend swap changed that,
    // the failure would be opaque.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .select()?;

    let window = QueryWindow::new()?;
    window::configure(&window);

    spawn_ipc_listener(&options.socket_path, window.as_weak())?;

    #[cfg(target_os = "macos")]
    let _hotkey_guard = spawn_hotkey_listener(window.as_weak());

    if options.open_on_start {
        window::show(&window);
    }

    window.run()?;
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
                eprintln!("ipc server exited: {err}");
            }
        })?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn spawn_hotkey_listener(weak: slint::Weak<QueryWindow>) -> HotkeyGuard {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    // GlobalHotKeyManager has to be created and kept alive on the same thread
    // for the duration we want hotkey events. We construct it on the main
    // thread (current thread) and stash it in the returned guard so it stays
    // alive for the life of the daemon.
    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => m,
        Err(err) => {
            eprintln!("failed to create global hotkey manager: {err}");
            return HotkeyGuard { _manager: None };
        }
    };

    let hotkey = HotKey::new(Some(Modifiers::SHIFT), Code::Space);
    if let Err(err) = manager.register(hotkey) {
        eprintln!("failed to register Shift+Space hotkey: {err}");
        return HotkeyGuard { _manager: None };
    }
    let hotkey_id = hotkey.id();

    // Drain events on a dedicated thread; for each Pressed event matching our
    // hotkey, hop back to the Slint event loop and toggle the window.
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

    HotkeyGuard {
        _manager: Some(manager),
    }
}

#[cfg(target_os = "macos")]
struct HotkeyGuard {
    _manager: Option<global_hotkey::GlobalHotKeyManager>,
}
