//! Catches QuickJS-specific regressions (Promise scheduling, regex
//! engine quirks) that wouldn't surface under Node-based vitest tests.
//!
//! Authoritative correctness lives in each plugin's `*.test.js`; this only
//! confirms the plugin loads in real rquickjs and survives one round-trip
//! through `query()` without panicking.

use std::path::Path;
use std::time::Duration;

use high_beam::plugins::manifest::Manifest;
use high_beam::plugins::runtime::LoadedPlugin;
use tokio_util::sync::CancellationToken;

const SMOKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Example plugins under `examples/plugins/` whose code is loadable today.
/// `http-codes` is intentionally skipped — its directory is a placeholder
/// for Stage 8 and contains no manifest yet.
const EXAMPLES: &[&str] = &[
    "examples/plugins/echo",
    "examples/plugins/echo-ts",
    "examples/plugins/frecency-demo",
    "examples/plugins/slow-echo",
];

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt")
}

fn smoke_test(dir: &str) {
    rt().block_on(async {
        let path = Path::new(dir);
        let manifest_bytes = std::fs::read(path.join("manifest.json")).expect("read manifest.json");
        let manifest = Manifest::parse(&manifest_bytes).expect("parse manifest");
        let plugin = LoadedPlugin::load(path, manifest)
            .await
            .expect("plugin loads in real rquickjs");

        let mut rx = plugin.run_query_stream("smoke", CancellationToken::new());
        let outcome =
            tokio::time::timeout(SMOKE_TIMEOUT, async { while rx.recv().await.is_some() {} }).await;
        outcome.expect("plugin query finished within smoke budget");
    });
}

#[test]
fn echo_loads_in_rquickjs() {
    smoke_test("examples/plugins/echo");
}

#[test]
fn echo_ts_loads_in_rquickjs() {
    smoke_test("examples/plugins/echo-ts");
}

#[test]
fn frecency_demo_loads_in_rquickjs() {
    smoke_test("examples/plugins/frecency-demo");
}

#[test]
fn slow_echo_loads_in_rquickjs() {
    smoke_test("examples/plugins/slow-echo");
}

#[test]
fn examples_list_matches_disk() {
    // Hardcoded list above; this test fails loudly if a new example plugin
    // shows up on disk so the smoke runner doesn't silently skip it.
    let mut found: Vec<String> = std::fs::read_dir("examples/plugins")
        .expect("read examples/plugins")
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter(|e| e.path().join("manifest.json").exists())
        .map(|e| {
            format!(
                "examples/plugins/{}",
                e.file_name().to_string_lossy().into_owned()
            )
        })
        .collect();
    found.sort();
    let mut expected: Vec<String> = EXAMPLES.iter().map(|s| (*s).to_string()).collect();
    expected.sort();
    assert_eq!(
        found, expected,
        "examples/plugins/ membership changed — update EXAMPLES (and add a smoke test)"
    );
}
