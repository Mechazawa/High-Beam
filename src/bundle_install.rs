//! First-launch install of default plugins shipped inside the macOS .app bundle.
//!
//! When the daemon starts from a packaged `HighBeam.app`, the bundled
//! defaults live in `Contents/Resources/plugins/` — a read-only location the
//! user shouldn't edit. The user-editable copy lives in the platform plugin
//! directory ([`crate::plugins::loader`]'s default search target). The very
//! first time we boot we copy the bundled defaults into the user dir;
//! thereafter the user's copy wins and `cargo packager` updates never stomp
//! on it. Running unbundled (`cargo run`) hits the "no bundled dir" branch
//! and quietly does nothing.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Install bundled default plugins into the user's plugin directory if the
/// directory is empty or absent. Errors are logged and swallowed — a failed
/// install must not prevent the daemon from booting.
pub fn install_default_plugins_if_needed() {
    let Some(user_dir) = crate::paths::plugins_dir() else {
        tracing::debug!("bundle-install: no platform plugin dir; skipping");
        return;
    };

    let Some(bundled) = bundled_plugins_dir() else {
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

/// Resolve the bundled plugin dir relative to the running executable.
///
/// In a `.app` bundle the binary sits at `HighBeam.app/Contents/MacOS/high-beam`,
/// so the resources directory is two parents up + `Resources/plugins`. When
/// the binary lives anywhere else (`target/release/high-beam`, `cargo run`,
/// `cargo install`'d into `~/.cargo/bin`), the computed path won't exist and
/// we return `None`.
fn bundled_plugins_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // macOS `.app`: exe at Contents/MacOS/high-beam, plugins at
    // Contents/Resources/plugins.
    // Linux: exe at <prefix>/bin/highbeam, plugins at
    // <prefix>/share/highbeam/plugins (works for /usr, /usr/local, ~/.local,
    // and any tarball-relocated PREFIX).
    #[cfg(target_os = "macos")]
    let candidate = exe.parent()?.parent()?.join("Resources").join("plugins");
    #[cfg(not(target_os = "macos"))]
    let candidate = exe.parent()?.parent()?.join("share").join("highbeam").join("plugins");

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
}
