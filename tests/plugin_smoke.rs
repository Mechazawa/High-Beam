//! Catches QuickJS-specific regressions (Promise scheduling, regex engine
//! quirks) that wouldn't surface under Node-based vitest. Authoritative
//! correctness lives in each plugin's `*.test.js`; this just confirms each
//! example plugin loads in rquickjs and survives one round-trip without
//! panicking.
//!
//! The runner discovers every `plugins/*/manifest.json` on disk so adding a
//! new example automatically gets it smoke-tested — no list to drift.

use std::path::{Path, PathBuf};
use std::time::Duration;

use high_beam::plugins::manifest::Manifest;
use high_beam::plugins::runtime::LoadedPlugin;
use tokio_util::sync::CancellationToken;

const SMOKE_TIMEOUT: Duration = Duration::from_secs(5);

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt")
}

/// Run one plugin through load + a single `query("smoke")` cycle. Panics on
/// failure with the plugin's directory in the message so a looped runner
/// surfaces which plugin broke.
fn smoke_test(dir: &Path) {
    let label = dir.display().to_string();
    rt().block_on(async {
        let manifest_bytes = std::fs::read(dir.join("manifest.json"))
            .unwrap_or_else(|err| panic!("[{label}] read manifest.json: {err}"));
        let manifest = Manifest::parse(&manifest_bytes).unwrap_or_else(|err| panic!("[{label}] parse manifest: {err}"));

        // Smoke runner bypasses the loader, so re-apply the platforms gate
        // here to stay green on every OS.
        if !manifest.supports_current_platform() {
            return;
        }

        // Skip plugins whose entry file isn't on disk — TypeScript examples
        // (echo-ts) generate the .js via `tsc` as part of `just test-plugins`,
        // not `cargo test`. A missing entry there isn't a regression in the
        // host; it just means the JS build step hasn't run.
        let entry_path = manifest.entry_path(dir);
        if !entry_path.exists() {
            return;
        }

        let plugin = LoadedPlugin::load(dir, manifest)
            .await
            .unwrap_or_else(|err| panic!("[{label}] plugin loads in real rquickjs: {err}"));

        let mut rx = plugin.run_query_stream("smoke", CancellationToken::new());
        let outcome = tokio::time::timeout(SMOKE_TIMEOUT, async { while rx.recv().await.is_some() {} }).await;
        outcome.unwrap_or_else(|_| panic!("[{label}] query did not finish within {SMOKE_TIMEOUT:?}"));
    });
}

/// Every directory under `plugins/` that contains a `manifest.json`. Sorted
/// so the iteration order is deterministic (test output reads the same on
/// every run).
fn discover_plugin_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir("plugins")
        .expect("read plugins dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("manifest.json").exists())
        .collect();
    dirs.sort();
    dirs
}

#[test]
fn every_example_plugin_loads_in_rquickjs() {
    let dirs = discover_plugin_dirs();
    assert!(!dirs.is_empty(), "expected at least one example plugin under plugins/");
    for dir in dirs {
        smoke_test(&dir);
    }
}
