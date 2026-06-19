//! macOS in-place self-update.
//!
//! Wraps [`cargo-packager-updater`] (the sibling crate to the `cargo-packager`
//! bundler we already ship with): it fetches a minisign-signed `latest.json`
//! off GitHub Releases, verifies + swaps the `.app` bundle, then we relaunch.
//!
//! The whole feature is gated on running from inside a `.app`, so Homebrew /
//! `cargo install` / dev runs never self-update. Non-macOS targets compile the
//! public surface to inert no-ops.
//!
//! The pending-update flag lives here (cross-platform) so the Core built-in
//! can surface an "Update available" row without `cfg` noise; the actual
//! check / download work is macOS-only.

use std::sync::{OnceLock, RwLock};
use std::time::Duration;

/// Delay before the first background check so we don't compete with the
/// cold-start work (plugin load, window construction).
const INITIAL_CHECK_DELAY: Duration = Duration::from_secs(15);

/// Interval between background checks. The macOS daemon is long-lived, so a
/// few times a day is plenty.
const CHECK_INTERVAL: Duration = Duration::from_hours(6);

fn pending_slot() -> &'static RwLock<Option<String>> {
    static PENDING: OnceLock<RwLock<Option<String>>> = OnceLock::new();

    PENDING.get_or_init(|| RwLock::new(None))
}

/// Version string of a known-available update, once a check has found one.
/// Read by the Core built-in to surface an "Update available → vX" row.
#[must_use]
pub fn pending_version() -> Option<String> {
    pending_slot().read().ok().and_then(|g| g.clone())
}

fn set_pending(version: Option<String>) {
    if let Ok(mut g) = pending_slot().write() {
        *g = version;
    }
}

/// Spawn the background update poller: one check shortly after launch, then
/// every [`CHECK_INTERVAL`]. No-op when self-update is unsupported. A plain OS
/// thread — the check is blocking and needs no async runtime.
pub fn spawn_background_check() {
    if !is_supported() {
        return;
    }

    let spawned = std::thread::Builder::new()
        .name("highbeam-update-check".into())
        .spawn(|| {
            std::thread::sleep(INITIAL_CHECK_DELAY);

            loop {
                match check() {
                    Ok(Some(version)) => tracing::info!(%version, "update: newer version available"),
                    Ok(None) => tracing::debug!("update: up to date"),
                    Err(err) => tracing::warn!(%err, "update: background check failed"),
                }

                std::thread::sleep(CHECK_INTERVAL);
            }
        });

    if let Err(err) = spawned {
        tracing::warn!(%err, "update: failed to spawn background check thread");
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use cargo_packager_updater::{Config, check_update};

    use super::set_pending;

    /// Minisign public key verifying release artifacts. Paste the key printed
    /// by `cargo packager signer generate` here — it is NOT secret. Empty ⇒
    /// the self-updater stays disabled, so the feature is inert until a key +
    /// a signed `latest.json` are published.
    const UPDATER_PUBKEY: &str = "";

    /// Static manifest published by the release workflow. `latest/download`
    /// always resolves to the newest non-prerelease asset, so there are no
    /// GitHub API calls and no rate limits.
    const UPDATE_ENDPOINT: &str = "https://github.com/Mechazawa/high-beam/releases/latest/download/latest.json";

    const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

    /// `true` when self-update is possible: a pubkey is configured and we're
    /// running from inside a `.app`. The bundle check is the Homebrew / cargo
    /// / dev exclusion — none of those live inside an `.app`.
    #[must_use]
    pub fn is_supported() -> bool {
        !UPDATER_PUBKEY.trim().is_empty() && current_app_bundle().is_some()
    }

    /// The `.app` bundle the running binary lives in, if any.
    /// `…/HighBeam.app/Contents/MacOS/high-beam` → `…/HighBeam.app`.
    fn current_app_bundle() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;

        exe.ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .map(Path::to_path_buf)
    }

    fn config() -> Result<Config, String> {
        let endpoint = UPDATE_ENDPOINT
            .parse()
            .map_err(|err| format!("bad update endpoint: {err}"))?;

        Ok(Config {
            endpoints: vec![endpoint],
            pubkey: UPDATER_PUBKEY.trim().into(),
            ..Default::default()
        })
    }

    /// Blocking. Ask the endpoint whether a newer version exists; record it as
    /// pending (for the Core row) and return it. `Ok(None)` ⇒ up to date.
    ///
    /// # Errors
    ///
    /// Returns the failure as a string when the endpoint is unreachable, the
    /// manifest is malformed, or the current version doesn't parse.
    pub fn check() -> Result<Option<String>, String> {
        if !is_supported() {
            return Ok(None);
        }
        let current = CURRENT_VERSION
            .parse()
            .map_err(|err| format!("bad current version: {err}"))?;
        let version = check_update(current, config()?)
            .map_err(|err| format!("update check failed: {err}"))?
            .map(|update| update.version);

        set_pending(version.clone());

        Ok(version)
    }

    /// Blocking. Re-check, then download + verify + swap the bundle in place.
    /// `Ok(Some(version))` ⇒ installed; `Ok(None)` ⇒ already up to date. Does
    /// NOT relaunch — the caller decides that (so a cancelled update can be
    /// left to apply on next launch). Pairs with [`relaunch`].
    ///
    /// # Errors
    ///
    /// Returns the failure as a string when self-update is unsupported, the
    /// check fails, or the download / signature-verify / swap fails.
    pub fn install() -> Result<Option<String>, String> {
        if !is_supported() {
            return Err("self-update is unsupported in this install".into());
        }
        let current = CURRENT_VERSION
            .parse()
            .map_err(|err| format!("bad current version: {err}"))?;

        let Some(update) = check_update(current, config()?).map_err(|err| format!("update check failed: {err}"))?
        else {
            set_pending(None);

            return Ok(None);
        };
        let version = update.version.clone();

        update
            .download_and_install()
            .map_err(|err| format!("download/install failed: {err}"))?;
        set_pending(None);
        tracing::info!(%version, "update: installed");

        Ok(Some(version))
    }

    /// Defer a relaunch of the freshly-swapped bundle, then exit so the
    /// single-instance IPC socket frees up before the new process binds it.
    pub fn relaunch() -> ! {
        if let Some(app) = current_app_bundle() {
            // Sleep before `open` so the new daemon starts only after we've
            // exited — otherwise it connects to our still-live IPC socket,
            // treats us as the running instance, forwards + quits, and leaves
            // no daemon behind. The new instance recovers the now-stale
            // socket (see `ipc::Server::bind`).
            let script = format!("sleep 1; open -n {}", shell_quote(&app));
            let spawned = Command::new("/bin/sh").arg("-c").arg(script).spawn();

            if let Err(err) = spawned {
                tracing::error!(%err, "update: failed to schedule relaunch; restart manually");
            }
        } else {
            tracing::warn!("update: could not resolve .app to relaunch");
        }

        std::process::exit(0);
    }

    /// Single-quote a path for `/bin/sh -c`, escaping embedded single quotes.
    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    #[must_use]
    pub fn is_supported() -> bool {
        false
    }

    /// No-op on non-macOS — always reports "up to date".
    ///
    /// # Errors
    ///
    /// Never errors; the signature mirrors the macOS implementation.
    pub fn check() -> Result<Option<String>, String> {
        // There is never a pending update off macOS; keep the shared flag
        // consistent (and the writer referenced in non-macOS builds).
        super::set_pending(None);

        Ok(None)
    }

    /// Unsupported off macOS.
    ///
    /// # Errors
    ///
    /// Always returns an error: self-update is macOS-only.
    pub fn install() -> Result<Option<String>, String> {
        Err("self-update is macOS-only".into())
    }

    pub fn relaunch() -> ! {
        std::process::exit(0)
    }
}

pub use platform::{check, install, is_supported, relaunch};

#[cfg(test)]
mod tests {
    use super::{is_supported, pending_version, set_pending};

    #[test]
    fn is_supported_false_in_test_env() {
        // Tests run from `target/.../deps`, never inside a `.app`, and no
        // pubkey is configured — so self-update must report unsupported.
        assert!(!is_supported());
    }

    #[test]
    fn pending_version_round_trips() {
        set_pending(Some("9.9.9".to_owned()));
        assert_eq!(pending_version().as_deref(), Some("9.9.9"));

        set_pending(None);
        assert!(pending_version().is_none());
    }
}
