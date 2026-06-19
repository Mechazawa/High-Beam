//! First-launch install + post-update reconcile of the default plugins and
//! themes shipped inside the packaged app.
//!
//! When the daemon starts from a packaged `HighBeam.app`, the bundled
//! defaults live in `Contents/Resources/<plugins|themes>/` — a read-only
//! location the user shouldn't edit. The user-editable copies live in the
//! platform plugin / themes directories. Plugins seed once (only into an
//! empty/absent dir, so the user's copy always wins thereafter); themes seed
//! per-file (any shipped theme not already present is copied, but an existing
//! file — possibly user-edited — is never overwritten). Running unbundled
//! (`cargo run`, `cargo install`) hits the "no bundled dir" branch and quietly
//! does nothing.
//!
//! [`reconcile_bundled_resources`] runs once per app-version change (the first
//! launch after a self-update or a manual `.dmg` install): it installs newly
//! bundled plugins/themes and updates existing ones the bundle ships a newer
//! copy of — but only when the user hasn't edited their copy since we last
//! shipped it. The "did we ship this exact bytes/version?" check is recorded
//! in `shipped-resources.json` (plugins by manifest `version`, themes by
//! content hash); a user-edited resource is never clobbered.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::logging::LogErr;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// State file (in the config dir) recording what we last copied out of the
/// bundle, so the reconcile can distinguish "user edited it" from "still
/// matches what we shipped".
const STATE_FILE: &str = "shipped-resources.json";

/// Install bundled default plugins into the user's plugin directory if the
/// directory is empty or absent. Errors are logged and swallowed — a failed
/// install must not prevent the daemon from booting.
pub fn install_default_plugins_if_needed() {
    let Some(user_dir) = crate::paths::plugins_dir() else {
        tracing::debug!("bundle-install: no platform plugin dir; skipping");
        return;
    };

    let Some(bundled) = bundled_resource_dir("plugins") else {
        // Running unbundled (cargo run) — not an error.
        tracing::debug!("bundle-install: no bundled resources; running unbundled");

        return;
    };

    match user_dir_needs_seeding(&user_dir) {
        Ok(false) => {
            tracing::debug!(
                plugins_dir = %user_dir.display(),
                "bundle-install: user plugin dir already populated"
            );

            return;
        }
        Ok(true) => {}
        Err(err) => {
            tracing::warn!(
                plugins_dir = %user_dir.display(),
                %err,
                "bundle-install: could not stat user plugin dir; skipping install",
            );

            return;
        }
    }

    if let Err(err) = fs::create_dir_all(&user_dir) {
        tracing::warn!(
            plugins_dir = %user_dir.display(),
            %err,
            "bundle-install: failed to create user plugin dir",
        );

        return;
    }

    match copy_dir_recursive(&bundled, &user_dir) {
        Ok(()) => tracing::info!(
            plugins_dir = %user_dir.display(),
            source = %bundled.display(),
            "bundle-install: copied default plugins into user dir",
        ),
        Err(err) => tracing::warn!(
            source = %bundled.display(),
            target = %user_dir.display(),
            %err,
            "bundle-install: copy failed; user must install plugins manually",
        ),
    }
}

/// Seed the shipped default themes into the user's themes dir, copying any
/// `.toml` not already present. Unlike plugins this runs per-file every
/// launch (add-missing-only): new builtin themes from an app update appear
/// without clearing the folder, but an existing file — which the user may
/// have edited — is never overwritten. Errors are logged and swallowed.
pub fn install_default_themes_if_needed() {
    let Some(user_dir) = crate::paths::themes_dir() else {
        tracing::debug!("bundle-install: no platform themes dir; skipping");

        return;
    };

    let Some(bundled) = bundled_resource_dir("themes") else {
        tracing::debug!("bundle-install: no bundled themes; running unbundled");

        return;
    };

    if let Err(err) = fs::create_dir_all(&user_dir) {
        tracing::warn!(themes_dir = %user_dir.display(), %err, "bundle-install: failed to create user themes dir");

        return;
    }

    match copy_missing_files(&bundled, &user_dir, "toml") {
        Ok(0) => tracing::debug!(themes_dir = %user_dir.display(), "bundle-install: no new themes to seed"),
        Ok(copied) => tracing::info!(
            themes_dir = %user_dir.display(),
            source = %bundled.display(),
            copied,
            "bundle-install: seeded default themes",
        ),
        Err(err) => tracing::warn!(
            source = %bundled.display(),
            target = %user_dir.display(),
            %err,
            "bundle-install: theme seed failed",
        ),
    }
}

/// Resolve a bundled resource subdirectory (`plugins`, `themes`, …) relative
/// to the running executable.
///
/// In a `.app` bundle the binary sits at `HighBeam.app/Contents/MacOS/high-beam`,
/// so the resources directory is two parents up + `Resources/<subdir>`. On
/// Linux the binary lives at `<prefix>/bin/highbeam` and resources at
/// `<prefix>/share/highbeam/<subdir>` (works for /usr, /usr/local, ~/.local,
/// and any tarball-relocated PREFIX). When the binary lives anywhere else
/// (`target/release/high-beam`, `cargo run`, `cargo install`'d into
/// `~/.cargo/bin`), the computed path won't exist and we return `None`.
fn bundled_resource_dir(subdir: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    #[cfg(target_os = "macos")]
    let candidate = exe.parent()?.parent()?.join("Resources").join(subdir);
    #[cfg(not(target_os = "macos"))]
    let candidate = exe.parent()?.parent()?.join("share").join("highbeam").join(subdir);

    candidate.is_dir().then_some(candidate)
}

/// Treat the user plugin dir as needing the install only when it doesn't
/// exist OR exists-but-empty. Any existing plugin directory means the user
/// has either already been seeded once or installed plugins manually; either
/// way we mustn't clobber it.
fn user_dir_needs_seeding(dir: &Path) -> io::Result<bool> {
    match fs::read_dir(dir) {
        Ok(mut entries) => Ok(entries.next().is_none()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(err) => Err(err),
    }
}

/// Recursive copy from `src` → `dst`. Creates `dst` and intermediate
/// directories. Symlinks are followed and the target's contents are copied
/// as plain files; this avoids leaving dangling symlinks into the read-only
/// `.app` after the user updates High Beam.
fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        // `fs::metadata` follows symlinks; that's deliberate (see fn docs).
        let meta = fs::metadata(&from)?;

        if meta.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if meta.is_file() {
            fs::copy(&from, &to)?;
        }
        // Other file types (devices, sockets) are ignored — the bundle
        // resources directory only ever contains regular files/dirs.
    }

    Ok(())
}

/// Copy every file directly under `src` with the given extension into `dst`,
/// skipping any whose name already exists there. Returns how many files were
/// copied. Non-recursive — the themes dir is flat. The skip-existing rule is
/// what makes theme seeding safe to run on every launch.
///
/// A per-file copy failure is logged and skipped so one unwritable file can't
/// abort the rest of the seed; only failing to read `src` returns `Err`.
fn copy_missing_files(src: &Path, dst: &Path, extension: &str) -> io::Result<usize> {
    let copied = fs::read_dir(src)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().and_then(|e| e.to_str()) == Some(extension))
        .fold(0_usize, |copied, from| {
            let Some(name) = from.file_name() else {
                return copied;
            };
            let to = dst.join(name);

            if to.exists() {
                return copied;
            }

            // Log-and-skip so one unwritable file can't abort the rest. The
            // paths go in the context since `io::Error` alone won't name them.
            let context = format!(
                "bundle-install: theme copy {} -> {} failed; skipping",
                from.display(),
                to.display()
            );

            fs::copy(&from, &to).log_warn(&context).map_or(copied, |_| copied + 1)
        });

    Ok(copied)
}

/// Record of what we last copied out of the app bundle. Plugins are
/// fingerprinted by manifest `version` (already authoritative); themes by
/// content hash (no version field). `BTreeMap` keeps the on-disk JSON stable.
#[derive(Default, Serialize, Deserialize)]
struct ShippedState {
    #[serde(default)]
    app_version: Option<String>,
    #[serde(default)]
    plugins: BTreeMap<String, String>,
    #[serde(default)]
    themes: BTreeMap<String, String>,
}

/// Reconcile bundled plugins + themes into the user dirs after an app update.
///
/// Runs the work only once per app-version change (early-returns when the
/// recorded version already matches the running binary). For each bundled
/// resource: install it if missing, or update it when the bundle ships a newer
/// copy AND the user hasn't edited theirs since we last shipped it. A
/// user-edited resource is left untouched. On the very first run for a given
/// resource we only record its current state as the baseline — never clobber
/// on first sight, since we can't yet know whether it was user-edited.
///
/// `reconcile_plugins` is `false` under a `--plugins-dir` override (the dev
/// path that bypasses seeding entirely). Errors are logged and swallowed.
pub fn reconcile_bundled_resources(reconcile_plugins: bool) {
    let mut state = load_state();

    if state.app_version.as_deref() == Some(CURRENT_VERSION) {
        return;
    }

    if reconcile_plugins {
        reconcile_plugins_into(&mut state);
    }

    reconcile_themes_into(&mut state);
    state.app_version = Some(CURRENT_VERSION.to_owned());
    save_state(&state);
}

fn state_path() -> Option<PathBuf> {
    crate::paths::config_dir().ok().map(|dir| dir.join(STATE_FILE))
}

fn load_state() -> ShippedState {
    let Some(path) = state_path() else {
        return ShippedState::default();
    };
    let Ok(bytes) = fs::read(&path) else {
        return ShippedState::default();
    };

    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_state(state: &ShippedState) {
    let Some(path) = state_path() else {
        return;
    };
    let Ok(json) = serde_json::to_vec_pretty(state) else {
        tracing::warn!("bundle-install: failed to serialise shipped-resources state");
        return;
    };

    if let Err(err) = fs::write(&path, json) {
        tracing::warn!(%err, path = %path.display(), "bundle-install: failed to write shipped-resources state");
    }
}

/// Resolve the bundled-plugins + user-plugins dirs and reconcile between them.
fn reconcile_plugins_into(state: &mut ShippedState) {
    let (Some(bundled), Some(user_dir)) = (bundled_resource_dir("plugins"), crate::paths::plugins_dir()) else {
        return;
    };

    reconcile_plugins_dir(&bundled, &user_dir, &mut state.plugins);
}

/// Per-plugin reconcile: install missing, version-gated update of unmodified.
/// Pure over `(bundled, user_dir)` + the recorded `shipped` versions so it can
/// be unit-tested without resolving `current_exe` / platform dirs.
fn reconcile_plugins_dir(bundled: &Path, user_dir: &Path, shipped: &mut BTreeMap<String, String>) {
    let Ok(entries) = fs::read_dir(bundled) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        let src = entry.path();

        if !src.join("manifest.json").is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let bundle_version = manifest_version(&src.join("manifest.json"));
        let dest = user_dir.join(&name);

        if !dest.exists() {
            if copy_dir_recursive(&src, &dest)
                .log_warn("bundle-install: install bundled plugin")
                .is_some()
            {
                tracing::info!(%name, "bundle-install: installed newly bundled plugin");
                shipped.insert(name, bundle_version);
            }

            continue;
        }
        let installed = manifest_version(&dest.join("manifest.json"));

        match shipped.get(&name) {
            // First sight of an already-installed plugin — record the baseline,
            // don't touch it.
            None => {
                shipped.insert(name, installed);
            }
            Some(recorded) => {
                let unmodified = &installed == recorded;
                let newer = crate::plugins::manifest::is_newer_version(&bundle_version, &installed);

                if unmodified
                    && newer
                    && copy_dir_recursive(&src, &dest)
                        .log_warn("bundle-install: update bundled plugin")
                        .is_some()
                {
                    tracing::info!(%name, from = %installed, to = %bundle_version, "bundle-install: updated bundled plugin");
                    shipped.insert(name, bundle_version);
                }
            }
        }
    }
}

/// Resolve the bundled-themes + user-themes dirs and reconcile between them.
fn reconcile_themes_into(state: &mut ShippedState) {
    let (Some(bundled), Some(user_dir)) = (bundled_resource_dir("themes"), crate::paths::themes_dir()) else {
        return;
    };

    reconcile_themes_dir(&bundled, &user_dir, &mut state.themes);
}

/// Per-theme reconcile: install missing, hash-gated update of unmodified.
/// Pure over `(bundled, user_dir)` + the recorded `shipped` hashes, so it can
/// be unit-tested without resolving `current_exe` / platform dirs.
fn reconcile_themes_dir(bundled: &Path, user_dir: &Path, shipped: &mut BTreeMap<String, String>) {
    let Ok(entries) = fs::read_dir(bundled) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        let src = entry.path();

        if !(src.is_file() && src.extension().and_then(|e| e.to_str()) == Some("toml")) {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(bundle_hash) = file_hash(&src) else {
            continue;
        };
        let dest = user_dir.join(&name);

        if !dest.exists() {
            if fs::copy(&src, &dest)
                .log_warn("bundle-install: install bundled theme")
                .is_some()
            {
                tracing::info!(%name, "bundle-install: installed newly bundled theme");
                shipped.insert(name, bundle_hash);
            }

            continue;
        }
        let Some(installed_hash) = file_hash(&dest) else {
            continue;
        };

        match shipped.get(&name) {
            None => {
                shipped.insert(name, installed_hash);
            }
            Some(recorded) => {
                let unmodified = &installed_hash == recorded;
                let changed = bundle_hash != installed_hash;

                if unmodified
                    && changed
                    && fs::copy(&src, &dest)
                        .log_warn("bundle-install: update bundled theme")
                        .is_some()
                {
                    tracing::info!(%name, "bundle-install: updated bundled theme");
                    shipped.insert(name, bundle_hash);
                }
            }
        }
    }
}

/// Read a manifest's `version` string, or `""` when absent/unparseable.
fn manifest_version(manifest_path: &Path) -> String {
    fs::read(manifest_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value.get("version").and_then(|v| v.as_str()).map(str::to_owned))
        .unwrap_or_default()
}

/// Lowercase hex SHA-256 of a file's bytes, `None` if it can't be read.
fn file_hash(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;

    Some(format!("{:x}", Sha256::digest(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_tmp(tag: &str) -> PathBuf {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("high-beam-bundle-install-{tag}-{now}"));
        fs::create_dir_all(&dir).expect("mkdir tmp");
        dir
    }

    #[test]
    fn copy_dir_recursive_copies_files_and_nested_dirs() {
        let root = fresh_tmp("recursive");
        let src = root.join("src");
        let dst = root.join("dst");
        fs::create_dir_all(src.join("a/b")).unwrap();
        fs::write(src.join("top.txt"), b"top").unwrap();
        fs::write(src.join("a/inner.txt"), b"inner").unwrap();
        fs::write(src.join("a/b/leaf.json"), b"{\"k\":1}").unwrap();

        copy_dir_recursive(&src, &dst).expect("copy");

        assert_eq!(fs::read(dst.join("top.txt")).unwrap(), b"top");
        assert_eq!(fs::read(dst.join("a/inner.txt")).unwrap(), b"inner");
        assert_eq!(fs::read(dst.join("a/b/leaf.json")).unwrap(), b"{\"k\":1}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn copy_dir_recursive_overlays_into_existing_dir() {
        // The first-launch install only triggers on an empty dir, but the
        // copy helper itself must be safe to call against a pre-existing
        // target — e.g. for future re-seed scenarios.
        let root = fresh_tmp("overlay");
        let src = root.join("src");
        let dst = root.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("new.txt"), b"new").unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("preexisting.txt"), b"keep").unwrap();

        copy_dir_recursive(&src, &dst).expect("copy");

        assert_eq!(fs::read(dst.join("new.txt")).unwrap(), b"new");
        assert_eq!(fs::read(dst.join("preexisting.txt")).unwrap(), b"keep");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn copy_missing_files_skips_existing_and_filters_extension() {
        // Theme seeding must add new shipped themes without clobbering a
        // file the user already edited, and ignore non-theme files.
        let root = fresh_tmp("copy-missing");
        let src = root.join("src");
        let dst = root.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("new.toml"), b"new").unwrap();
        fs::write(src.join("edited.toml"), b"shipped").unwrap();
        fs::write(src.join("README.md"), b"ignore me").unwrap();
        // Pre-existing, user-edited copy that must survive untouched.
        fs::write(dst.join("edited.toml"), b"user edit").unwrap();

        let copied = copy_missing_files(&src, &dst, "toml").expect("copy");

        assert_eq!(copied, 1, "only the missing .toml is copied");
        assert_eq!(fs::read(dst.join("new.toml")).unwrap(), b"new");
        assert_eq!(
            fs::read(dst.join("edited.toml")).unwrap(),
            b"user edit",
            "existing file untouched"
        );
        assert!(!dst.join("README.md").exists(), "non-toml ignored");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn user_dir_needs_seeding_when_missing() {
        let root = fresh_tmp("needs-missing");
        let missing = root.join("absent");
        assert!(user_dir_needs_seeding(&missing).expect("stat"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn user_dir_needs_seeding_when_empty() {
        let root = fresh_tmp("needs-empty");
        let empty = root.join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(user_dir_needs_seeding(&empty).expect("stat"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn user_dir_does_not_need_seeding_when_populated() {
        let root = fresh_tmp("needs-populated");
        let populated = root.join("populated");
        fs::create_dir_all(populated.join("calculator")).unwrap();
        fs::write(populated.join("calculator/manifest.json"), b"{}").unwrap();
        assert!(!user_dir_needs_seeding(&populated).expect("stat"));
        let _ = fs::remove_dir_all(&root);
    }

    /// Lay down a one-plugin dir (`manifest.json` + `plugin.js`) at `dir`.
    fn write_plugin(dir: &Path, version: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("manifest.json"),
            format!(r#"{{"name":"x","version":"{version}"}}"#),
        )
        .unwrap();
        fs::write(dir.join("plugin.js"), b"export function* query() {}").unwrap();
    }

    #[test]
    fn reconcile_plugins_installs_missing_and_records_version() {
        let root = fresh_tmp("recon-plug-missing");
        let bundled = root.join("bundled");
        let user = root.join("user");
        write_plugin(&bundled.join("calc"), "1.2.0");
        fs::create_dir_all(&user).unwrap();
        let mut shipped = BTreeMap::new();

        reconcile_plugins_dir(&bundled, &user, &mut shipped);

        assert!(user.join("calc/plugin.js").is_file(), "missing plugin copied in");
        assert_eq!(shipped.get("calc").map(String::as_str), Some("1.2.0"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reconcile_plugins_first_sight_records_baseline_without_overwriting() {
        // An already-installed plugin we've never recorded: capture its current
        // version as the baseline, leave the user's copy alone — we can't yet
        // know if they edited it.
        let root = fresh_tmp("recon-plug-baseline");
        let bundled = root.join("bundled");
        let user = root.join("user");
        write_plugin(&bundled.join("calc"), "2.0.0");
        write_plugin(&user.join("calc"), "1.0.0");
        let mut shipped = BTreeMap::new();

        reconcile_plugins_dir(&bundled, &user, &mut shipped);

        assert_eq!(
            shipped.get("calc").map(String::as_str),
            Some("1.0.0"),
            "baseline recorded"
        );
        assert_eq!(
            manifest_version(&user.join("calc/manifest.json")),
            "1.0.0",
            "user copy untouched"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reconcile_plugins_updates_unmodified_when_bundle_newer() {
        let root = fresh_tmp("recon-plug-update");
        let bundled = root.join("bundled");
        let user = root.join("user");
        write_plugin(&bundled.join("calc"), "2.0.0");
        write_plugin(&user.join("calc"), "1.0.0");
        let mut shipped = BTreeMap::from([("calc".to_owned(), "1.0.0".to_owned())]);

        reconcile_plugins_dir(&bundled, &user, &mut shipped);

        assert_eq!(
            manifest_version(&user.join("calc/manifest.json")),
            "2.0.0",
            "updated to bundle version"
        );
        assert_eq!(shipped.get("calc").map(String::as_str), Some("2.0.0"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reconcile_plugins_preserves_user_edited() {
        // We shipped 1.0.0; the user's installed manifest now reads 1.5.0, so
        // they edited it. Even though the bundle has 2.0.0, leave it alone.
        let root = fresh_tmp("recon-plug-preserve");
        let bundled = root.join("bundled");
        let user = root.join("user");
        write_plugin(&bundled.join("calc"), "2.0.0");
        write_plugin(&user.join("calc"), "1.5.0");
        let mut shipped = BTreeMap::from([("calc".to_owned(), "1.0.0".to_owned())]);

        reconcile_plugins_dir(&bundled, &user, &mut shipped);

        assert_eq!(
            manifest_version(&user.join("calc/manifest.json")),
            "1.5.0",
            "user edit preserved"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reconcile_plugins_skips_when_bundle_not_newer() {
        let root = fresh_tmp("recon-plug-older");
        let bundled = root.join("bundled");
        let user = root.join("user");
        write_plugin(&bundled.join("calc"), "1.0.0");
        write_plugin(&user.join("calc"), "2.0.0");
        let mut shipped = BTreeMap::from([("calc".to_owned(), "2.0.0".to_owned())]);

        reconcile_plugins_dir(&bundled, &user, &mut shipped);

        assert_eq!(
            manifest_version(&user.join("calc/manifest.json")),
            "2.0.0",
            "older bundle does not downgrade"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reconcile_themes_installs_missing_and_records_hash() {
        let root = fresh_tmp("recon-theme-missing");
        let bundled = root.join("bundled");
        let user = root.join("user");
        fs::create_dir_all(&bundled).unwrap();
        fs::create_dir_all(&user).unwrap();
        fs::write(bundled.join("dracula.toml"), b"v1").unwrap();
        let mut shipped = BTreeMap::new();

        reconcile_themes_dir(&bundled, &user, &mut shipped);

        assert_eq!(fs::read(user.join("dracula.toml")).unwrap(), b"v1");
        assert_eq!(
            shipped.get("dracula.toml"),
            file_hash(&bundled.join("dracula.toml")).as_ref()
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reconcile_themes_updates_unmodified_when_bundle_changed() {
        let root = fresh_tmp("recon-theme-update");
        let bundled = root.join("bundled");
        let user = root.join("user");
        fs::create_dir_all(&bundled).unwrap();
        fs::create_dir_all(&user).unwrap();
        fs::write(bundled.join("dracula.toml"), b"new").unwrap();
        fs::write(user.join("dracula.toml"), b"old").unwrap();
        let old_hash = file_hash(&user.join("dracula.toml")).unwrap();
        let mut shipped = BTreeMap::from([("dracula.toml".to_owned(), old_hash)]);

        reconcile_themes_dir(&bundled, &user, &mut shipped);

        assert_eq!(
            fs::read(user.join("dracula.toml")).unwrap(),
            b"new",
            "unmodified theme updated"
        );
        assert_eq!(
            shipped.get("dracula.toml"),
            file_hash(&bundled.join("dracula.toml")).as_ref()
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reconcile_themes_preserves_user_edited() {
        // We shipped "old"; the user's file is now "useredit". The bundle has
        // "new" but the user edited theirs, so leave it untouched.
        let root = fresh_tmp("recon-theme-preserve");
        let bundled = root.join("bundled");
        let user = root.join("user");
        fs::create_dir_all(&bundled).unwrap();
        fs::create_dir_all(&user).unwrap();
        fs::write(bundled.join("dracula.toml"), b"new").unwrap();
        fs::write(user.join("dracula.toml"), b"useredit").unwrap();
        let shipped_old = format!("{:x}", Sha256::digest(b"old"));
        let mut shipped = BTreeMap::from([("dracula.toml".to_owned(), shipped_old.clone())]);

        reconcile_themes_dir(&bundled, &user, &mut shipped);

        assert_eq!(
            fs::read(user.join("dracula.toml")).unwrap(),
            b"useredit",
            "user edit preserved"
        );
        assert_eq!(shipped.get("dracula.toml"), Some(&shipped_old), "record unchanged");

        let _ = fs::remove_dir_all(&root);
    }
}
