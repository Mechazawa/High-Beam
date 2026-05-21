//! Catches QuickJS-specific regressions (Promise scheduling, regex engine
//! quirks) that wouldn't surface under Node-based vitest. Authoritative
//! correctness lives in each plugin's `*.test.js`; this just confirms the
//! plugin loads and survives one round-trip without panicking.

use std::path::Path;
use std::time::Duration;

use high_beam::plugins::manifest::Manifest;
use high_beam::plugins::runtime::LoadedPlugin;
use tokio_util::sync::CancellationToken;

const SMOKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Plugins under `plugins/` whose code is loadable today.
const EXAMPLES: &[&str] = &[
    "plugins/1password",
    "plugins/app-launcher",
    "plugins/bitwarden",
    "plugins/calculator",
    "plugins/clipboard-history",
    "plugins/color-converter",
    "plugins/currency-converter",
    "plugins/dictionary-linux",
    "plugins/dictionary-macos",
    "plugins/dnd",
    "plugins/echo",
    "plugins/echo-ts",
    "plugins/emoji-picker",
    "plugins/file-search",
    "plugins/frecency-demo",
    "plugins/http-codes",
    "plugins/kill-process",
    "plugins/obsidian",
    "plugins/paper-size",
    "plugins/prefpanes",
    "plugins/quick-links",
    "plugins/slow-echo",
    "plugins/unit-conversions",
    "plugins/web-search",
    "plugins/window-mgmt",
    "plugins/xkcd",
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

        // Smoke runner bypasses the loader, so re-apply the platforms gate
        // here to stay green on every OS.
        if !manifest.supports_current_platform() {
            return;
        }

        let plugin = LoadedPlugin::load(path, manifest)
            .await
            .expect("plugin loads in real rquickjs");

        let mut rx = plugin.run_query_stream("smoke", CancellationToken::new());
        let outcome = tokio::time::timeout(SMOKE_TIMEOUT, async { while rx.recv().await.is_some() {} }).await;
        outcome.expect("plugin query finished within smoke budget");
    });
}

#[test]
fn calculator_loads_in_rquickjs() {
    smoke_test("plugins/calculator");
}

#[test]
fn dnd_loads_in_rquickjs() {
    smoke_test("plugins/dnd");
}

#[test]
fn echo_loads_in_rquickjs() {
    smoke_test("plugins/echo");
}

#[test]
fn frecency_demo_loads_in_rquickjs() {
    smoke_test("plugins/frecency-demo");
}

#[test]
fn http_codes_loads_in_rquickjs() {
    smoke_test("plugins/http-codes");
}

#[test]
fn paper_size_loads_in_rquickjs() {
    smoke_test("plugins/paper-size");
}

#[test]
fn slow_echo_loads_in_rquickjs() {
    smoke_test("plugins/slow-echo");
}

#[test]
fn app_launcher_loads_in_rquickjs() {
    smoke_test("plugins/app-launcher");
}

#[test]
fn color_converter_loads_in_rquickjs() {
    smoke_test("plugins/color-converter");
}

#[test]
fn dictionary_linux_loads_in_rquickjs() {
    smoke_test("plugins/dictionary-linux");
}

#[test]
fn dictionary_macos_loads_in_rquickjs() {
    smoke_test("plugins/dictionary-macos");
}

#[test]
fn file_search_loads_in_rquickjs() {
    smoke_test("plugins/file-search");
}

#[test]
fn kill_process_loads_in_rquickjs() {
    smoke_test("plugins/kill-process");
}

#[test]
fn prefpanes_loads_in_rquickjs() {
    smoke_test("plugins/prefpanes");
}

#[test]
fn quick_links_loads_in_rquickjs() {
    smoke_test("plugins/quick-links");
}

#[test]
fn unit_conversions_loads_in_rquickjs() {
    smoke_test("plugins/unit-conversions");
}

#[test]
fn web_search_loads_in_rquickjs() {
    smoke_test("plugins/web-search");
}

#[test]
fn window_mgmt_loads_in_rquickjs() {
    smoke_test("plugins/window-mgmt");
}

#[test]
fn xkcd_loads_in_rquickjs() {
    smoke_test("plugins/xkcd");
}

#[test]
fn onepassword_loads_in_rquickjs() {
    smoke_test("plugins/1password");
}

#[test]
fn bitwarden_loads_in_rquickjs() {
    smoke_test("plugins/bitwarden");
}

#[test]
fn clipboard_history_loads_in_rquickjs() {
    smoke_test("plugins/clipboard-history");
}

#[test]
fn currency_converter_loads_in_rquickjs() {
    smoke_test("plugins/currency-converter");
}

#[test]
fn emoji_picker_loads_in_rquickjs() {
    smoke_test("plugins/emoji-picker");
}

#[test]
fn obsidian_loads_in_rquickjs() {
    smoke_test("plugins/obsidian");
}

#[test]
fn examples_list_matches_disk() {
    // Fail loudly when a new example shows up so the smoke runner doesn't
    // silently skip it.
    let mut found: Vec<String> = std::fs::read_dir("plugins")
        .expect("read plugins")
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter(|e| e.path().join("manifest.json").exists())
        .map(|e| format!("plugins/{}", e.file_name().to_string_lossy().into_owned()))
        .collect();
    found.sort();
    let mut expected: Vec<String> = EXAMPLES.iter().map(|s| (*s).to_string()).collect();
    expected.sort();
    assert_eq!(
        found, expected,
        "plugins/ membership changed — update EXAMPLES (and add a smoke test)"
    );
}
