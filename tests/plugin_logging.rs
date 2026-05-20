//! End-to-end tests for per-plugin logging.
//!
//! Each scenario builds a throwaway plugin on disk, loads it through the real
//! runtime (so the JS `console` binding, the timeout interrupt hook, and the
//! capability gate all fire), and reads `plugin.log` back from disk to assert
//! what the user would see when diagnosing a misbehaving plugin.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use high_beam::plugins::manifest::Manifest;
use high_beam::plugins::runtime::LoadedPlugin;
use tokio_util::sync::CancellationToken;

fn unique_tmp_dir(label: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "high-beam-log-test-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    ));
    fs::create_dir_all(&p).expect("create tmp plugin dir");
    p
}

fn write_plugin(dir: &Path, manifest_json: &str, source: &str) {
    fs::write(dir.join("manifest.json"), manifest_json).expect("write manifest");
    fs::write(dir.join("plugin.js"), source).expect("write plugin.js");
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt")
}

async fn drain<T>(rx: &mut tokio::sync::mpsc::UnboundedReceiver<T>) {
    while rx.recv().await.is_some() {}
}

fn read_log(dir: &Path) -> String {
    fs::read_to_string(dir.join("plugin.log")).unwrap_or_default()
}

/// Confirm we're producing well-formed log lines, not just substring-matching
/// gibberish. Two shapes: leading `[timestamp] [LEVEL] body`, continuation
/// lines indented four spaces.
fn assert_well_formed_lines(body: &str) {
    for line in body.lines() {
        if line.starts_with("    ") || line.is_empty() {
            continue;
        }
        assert!(line.starts_with('['), "missing leading bracket: {line:?}");
        assert!(line.contains("Z]"), "missing UTC timestamp: {line:?}");
        let has_level = ["[DEBUG]", "[INFO ]", "[WARN ]", "[ERROR]"]
            .iter()
            .any(|lvl| line.contains(lvl));
        assert!(has_level, "missing level marker: {line:?}");
    }
}

#[test]
fn console_log_writes_to_plugin_log() {
    let dir = unique_tmp_dir("console");
    write_plugin(
        &dir,
        r#"{"name":"console-test","entry":"plugin.js","timeoutMs":2000,"capabilities":["actions"]}"#,
        r#"
import { copy } from "highbeam:actions";

export async function* query(input, _signal) {
    console.log("hello", input);
    console.warn("warned about", { count: 2 });
    console.error("boom");
    console.debug("noisy detail");
    if (!input) return;
    yield { key: "k", title: input, action: copy(input) };
}
"#,
    );
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let runtime = rt();
    runtime.block_on(async {
        let plugin = LoadedPlugin::load(&dir, manifest).await.expect("load");
        let mut rx = plugin.run_query_stream("world", CancellationToken::new());
        drain(&mut rx).await;
    });

    let body = read_log(&dir);
    assert_well_formed_lines(&body);
    assert!(body.contains("[INFO ] hello world"), "got:\n{body}");
    assert!(
        body.contains(r#"[WARN ] warned about {"count":2}"#),
        "got:\n{body}",
    );
    assert!(body.contains("[ERROR] boom"), "got:\n{body}");
    assert!(body.contains("[DEBUG] noisy detail"), "got:\n{body}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn query_exception_is_logged() {
    let dir = unique_tmp_dir("throws");
    write_plugin(
        &dir,
        r#"{"name":"throws","entry":"plugin.js","timeoutMs":2000,"capabilities":[]}"#,
        r#"
export async function* query(input, _signal) {
    throw new Error("kaboom: " + input);
    yield 0;
}
"#,
    );
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let runtime = rt();
    runtime.block_on(async {
        let plugin = LoadedPlugin::load(&dir, manifest).await.expect("load");
        let mut rx = plugin.run_query_stream("oops", CancellationToken::new());
        drain(&mut rx).await;
        // Outcome logging runs on a spawn task; give it a tick to flush
        // before we read the file or we race the final `log.write`.
        tokio::time::sleep(Duration::from_millis(50)).await;
    });

    let body = read_log(&dir);
    assert_well_formed_lines(&body);
    assert!(body.contains("[ERROR] query threw"), "got:\n{body}");
    assert!(body.contains("kaboom: oops"), "got:\n{body}");
    assert!(body.contains("\"oops\""), "input not echoed: {body}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn timeout_is_logged_at_warn() {
    let dir = unique_tmp_dir("timeout");
    write_plugin(
        &dir,
        // 50ms budget vs setTimeout(1000) — the host timer task cancels and
        // the outcome logger classifies as Timeout via `timed_out`.
        r#"{"name":"slowpoke","entry":"plugin.js","timeoutMs":50,"capabilities":[]}"#,
        r#"
export async function* query(_input, _signal) {
    await new Promise(r => setTimeout(r, 1000));
    yield { key: "late", title: "should never appear", action: { kind: "copy", text: "" } };
}
"#,
    );
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let runtime = rt();
    runtime.block_on(async {
        let plugin = LoadedPlugin::load(&dir, manifest).await.expect("load");
        let mut rx = plugin.run_query_stream("spin", CancellationToken::new());
        drain(&mut rx).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    });

    let body = read_log(&dir);
    assert_well_formed_lines(&body);
    assert!(
        body.contains("[WARN ] query timed out"),
        "expected timeout warning, got:\n{body}",
    );
    assert!(body.contains("50ms"), "expected budget echo, got:\n{body}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn tight_sync_loop_still_hits_timeout() {
    // Regression: a `while(true){}` body never yields, so a tokio-scheduled
    // watchdog on the same executor never wakes. The watchdog now runs on
    // the blocking pool; this proves the deadline still fires.
    let dir = unique_tmp_dir("tight-loop");
    write_plugin(
        &dir,
        r#"{"name":"tightloop","entry":"plugin.js","timeoutMs":50,"capabilities":[]}"#,
        r#"
export async function* query(_input, _signal) {
    while (true) {}
    yield { key: "never", title: "unreachable", action: { kind: "copy", text: "" } };
}
"#,
    );
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let runtime = rt();
    let start = std::time::Instant::now();
    runtime.block_on(async {
        let plugin = LoadedPlugin::load(&dir, manifest).await.expect("load");
        let mut rx = plugin.run_query_stream("spin", CancellationToken::new());
        // 500ms cap is generous; budget is 50ms. If the watchdog can't
        // interrupt the tight loop this hangs forever.
        tokio::time::timeout(Duration::from_millis(500), drain(&mut rx))
            .await
            .expect("plugin should be killed within the test budget");
        tokio::time::sleep(Duration::from_millis(100)).await;
    });
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "plugin took {elapsed:?} to die — watchdog did not interrupt the tight loop",
    );

    let body = read_log(&dir);
    assert_well_formed_lines(&body);
    assert!(
        body.contains("[WARN ] query timed out"),
        "expected timeout warning, got:\n{body}",
    );
    assert!(body.contains("50ms"), "expected budget echo, got:\n{body}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn capability_violation_at_load_writes_to_plugin_log() {
    // Plugin imports `highbeam:http` without declaring `http`. The loader
    // must reject AND write the failure into plugin.log so users can debug.
    let dir = unique_tmp_dir("capviolation");
    write_plugin(
        &dir,
        r#"{"name":"capviolation","entry":"plugin.js","timeoutMs":2000,"capabilities":[]}"#,
        r#"
import { get } from "highbeam:http";
export async function* query(input, _signal) {
    const _ = get;
    yield { key: "k", title: input, action: { kind: "copy", text: input } };
}
"#,
    );

    // The loader scans a parent and treats each child as a plugin dir;
    // re-home the bad plugin under a scratch parent so siblings can't bleed in.
    let scratch_parent = unique_tmp_dir("cap-parent");
    let plugin_dir = scratch_parent.join("capviolation");
    fs::create_dir_all(&plugin_dir).expect("mk plugin dir");
    fs::rename(dir.join("manifest.json"), plugin_dir.join("manifest.json")).unwrap();
    fs::rename(dir.join("plugin.js"), plugin_dir.join("plugin.js")).unwrap();
    let _ = fs::remove_dir_all(&dir);

    let opts = high_beam::plugins::loader::LoaderOptions {
        plugins_dir: scratch_parent.clone(),
    };
    let runtime = rt();
    let settings = high_beam::settings::Settings::default();
    let plugins = runtime.block_on(high_beam::plugins::loader::load_all(&opts, &settings));
    assert!(
        plugins.is_empty(),
        "plugin missing the http capability must not load",
    );

    let body = read_log(&plugin_dir);
    assert_well_formed_lines(&body);
    assert!(body.contains("[ERROR] load failed"), "got:\n{body}");
    assert!(
        body.contains("highbeam:http") || body.contains("capability"),
        "log should mention the rejected module / capability: {body}",
    );

    let _ = fs::remove_dir_all(&scratch_parent);
}
