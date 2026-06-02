//! Integration coverage for the llrt-backed runtime surface: which globals
//! exist per capability set, and that the `node:*` modules actually work
//! end-to-end inside a loaded plugin.

use std::fs;

use high_beam::plugins::manifest::Manifest;
use high_beam::plugins::result::PluginResult;
use high_beam::plugins::runtime::LoadedPlugin;
use tokio_util::sync::CancellationToken;

mod common;
use common::fresh_tmp;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt")
}

async fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<PluginResult>) -> Vec<PluginResult> {
    let mut out = Vec::new();
    while let Some(r) = rx.recv().await {
        out.push(r);
    }
    out
}

/// Load a one-off plugin and return the subtitle of its single result —
/// the probe scripts below report through it.
fn probe(tag: &str, manifest_json: &str, source: &str) -> String {
    let dir = fresh_tmp(tag);
    fs::write(dir.join("manifest.json"), manifest_json).expect("write manifest");
    fs::write(dir.join("plugin.js"), source).expect("write plugin.js");
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();

    let runtime = rt();
    let out = runtime.block_on(async {
        let plugin = LoadedPlugin::load(&dir, manifest).await.expect("load plugin");
        let mut rx = plugin.run_query_stream("probe", CancellationToken::new());
        let results = drain(&mut rx).await;
        assert_eq!(results.len(), 1, "probe plugin must yield exactly one result");
        results[0].subtitle.clone().unwrap_or_default()
    });

    let _ = fs::remove_dir_all(&dir);
    out
}

#[test]
fn web_globals_present_without_any_capability() {
    let report = probe(
        "globals-ungated",
        r#"{"name":"globals-ungated","entry":"plugin.js","timeoutMs":2000}"#,
        r#"
export async function* query(_input, _signal) {
    const report = [
        typeof URL,
        typeof URLSearchParams,
        typeof Buffer,
        typeof Blob,
        typeof TextEncoder,
        typeof TextDecoder,
        typeof AbortController,
        typeof DOMException,
        typeof ReadableStream,
        typeof fetch,
    ].join("|");
    yield { key: "k", title: "t", subtitle: report, action: { kind: "noop" } };
}
"#,
    );
    // Everything pure-compute is present; fetch is absent without `http`.
    assert_eq!(
        report,
        "function|function|function|function|function|function|function|function|function|undefined"
    );
}

#[test]
fn fetch_present_with_http_capability() {
    let report = probe(
        "globals-http",
        r#"{"name":"globals-http","entry":"plugin.js","timeoutMs":2000,"capabilities":["http"]}"#,
        r#"
export async function* query(_input, _signal) {
    yield { key: "k", title: "t", subtitle: typeof fetch, action: { kind: "noop" } };
}
"#,
    );
    assert_eq!(report, "function");
}

#[test]
fn url_and_search_params_round_trip() {
    let report = probe(
        "globals-url",
        r#"{"name":"globals-url","entry":"plugin.js","timeoutMs":2000}"#,
        r#"
export async function* query(_input, _signal) {
    const u = new URL("https://example.com/a/b?x=1");
    u.searchParams.set("y", "two words");
    const report = `${u.hostname}|${u.pathname}|${u.searchParams.get("y")}|${u.search}`;
    yield { key: "k", title: "t", subtitle: report, action: { kind: "noop" } };
}
"#,
    );
    assert_eq!(report, "example.com|/a/b|two words|?x=1&y=two+words");
}

#[test]
fn node_path_imports_without_capability() {
    let report = probe(
        "node-path",
        r#"{"name":"node-path","entry":"plugin.js","timeoutMs":2000}"#,
        r#"
import path from "node:path";
export async function* query(_input, _signal) {
    const report = [
        path.basename("/foo/bar.txt"),
        path.dirname("/foo/bar.txt"),
        path.extname("/foo/bar.txt"),
        path.join("a", "b", "..", "c"),
    ].join("|");
    yield { key: "k", title: "t", subtitle: report, action: { kind: "noop" } };
}
"#,
    );
    assert_eq!(report, "bar.txt|/foo|.txt|a/c");
}

#[test]
fn node_fs_reads_and_writes_with_fs_capability() {
    let dir = fresh_tmp("node-fs");
    let target = dir.join("probe-data.txt");
    fs::write(&target, "from-disk").expect("seed file");
    let target_str = target.to_string_lossy().into_owned();
    let out_str = dir.join("written-by-plugin.txt").to_string_lossy().into_owned();

    fs::write(
        dir.join("manifest.json"),
        r#"{"name":"node-fs","entry":"plugin.js","timeoutMs":2000,"capabilities":["fs"]}"#,
    )
    .unwrap();
    fs::write(
        dir.join("plugin.js"),
        format!(
            r#"
import {{ readFile, writeFile }} from "node:fs/promises";
import {{ readFileSync }} from "node:fs";
export async function* query(_input, _signal) {{
    const viaPromises = await readFile({target_str:?}, "utf-8");
    const viaSync = readFileSync({target_str:?}, "utf-8");
    await writeFile({out_str:?}, "plugin-wrote-this");
    yield {{ key: "k", title: "t", subtitle: `${{viaPromises}}|${{viaSync}}`, action: {{ kind: "noop" }} }};
}}
"#
        ),
    )
    .unwrap();
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();

    let runtime = rt();
    runtime.block_on(async {
        let plugin = LoadedPlugin::load(&dir, manifest).await.expect("load plugin");
        let mut rx = plugin.run_query_stream("probe", CancellationToken::new());
        let results = drain(&mut rx).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].subtitle.as_deref(), Some("from-disk|from-disk"));
    });
    let written = fs::read_to_string(dir.join("written-by-plugin.txt")).expect("plugin wrote file");
    assert_eq!(written, "plugin-wrote-this");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn node_fs_rejected_without_fs_capability() {
    let dir = fresh_tmp("node-fs-nocap");
    fs::write(
        dir.join("manifest.json"),
        r#"{"name":"node-fs-nocap","entry":"plugin.js","timeoutMs":2000,"capabilities":["fs.read"]}"#,
    )
    .unwrap();
    fs::write(
        dir.join("plugin.js"),
        r#"
import { readFileSync } from "node:fs";
export async function* query(input, _signal) {
    const _ = readFileSync;
    yield { key: "k", title: input, action: { kind: "noop" } };
}
"#,
    )
    .unwrap();
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();

    let runtime = rt();
    runtime.block_on(async {
        // The scoped fs.read cap must NOT unlock full node:fs.
        let err = LoadedPlugin::load(&dir, manifest).await.err();
        let msg = err.map(|e| e.to_string()).unwrap_or_default();
        assert!(
            msg.contains("missing capability") && msg.contains("node:fs"),
            "expected capability rejection for node:fs, got: {msg}"
        );
    });

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn full_fs_capability_implies_scoped_helpers() {
    let report = probe(
        "fs-implies-scoped",
        r#"{"name":"fs-implies-scoped","entry":"plugin.js","timeoutMs":2000,"capabilities":["fs"]}"#,
        r#"
import { readCache, writeCache } from "highbeam:fs";
export async function* query(_input, _signal) {
    await writeCache("probe.txt", "cached");
    const raw = await readCache("probe.txt");
    const text = new TextDecoder().decode(raw);
    yield { key: "k", title: "t", subtitle: text, action: { kind: "noop" } };
}
"#,
    );
    assert_eq!(report, "cached");
}

#[test]
fn abort_signal_statics_reach_plugins() {
    let report = probe(
        "abort-statics",
        r#"{"name":"abort-statics","entry":"plugin.js","timeoutMs":2000}"#,
        r#"
export async function* query(_input, signal) {
    const merged = AbortSignal.any([signal, AbortSignal.timeout(60_000)]);
    yield { key: "k", title: "t", subtitle: String(merged.aborted), action: { kind: "noop" } };
}
"#,
    );
    assert_eq!(report, "false");
}

#[test]
fn abort_signal_timeout_actually_fires() {
    // The native AbortSignal.timeout is replaced at install with a
    // setTimeout-backed impl (the native one routes into llrt_timers'
    // uninitialised global table and aborts the process). Pin that the
    // replacement fires, not merely exists.
    let report = probe(
        "abort-timeout-fires",
        r#"{"name":"abort-timeout-fires","entry":"plugin.js","timeoutMs":2000}"#,
        r#"
export async function* query(_input, _signal) {
    const s = AbortSignal.timeout(10);
    await new Promise((resolve) => setTimeout(resolve, 100));
    yield { key: "k", title: "t", subtitle: `${s.aborted}|${s.reason?.name}`, action: { kind: "noop" } };
}
"#,
    );
    assert_eq!(report, "true|TimeoutError");
}

#[test]
fn host_only_action_still_rejected() {
    // Re-pin the host-only guard against the new runtime wiring.
    let dir = fresh_tmp("host-only-recheck");
    fs::write(
        dir.join("manifest.json"),
        r#"{"name":"host-only-recheck","entry":"plugin.js","timeoutMs":2000}"#,
    )
    .unwrap();
    fs::write(
        dir.join("plugin.js"),
        r#"
export async function* query(_input, _signal) {
    yield { key: "k", title: "t", action: { kind: "quit" } };
}
"#,
    )
    .unwrap();
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();

    let runtime = rt();
    runtime.block_on(async {
        let plugin = LoadedPlugin::load(&dir, manifest).await.expect("load plugin");
        let mut rx = plugin.run_query_stream("probe", CancellationToken::new());
        let results = drain(&mut rx).await;
        assert!(results.is_empty(), "quit row must be rejected");
    });

    let _ = fs::remove_dir_all(&dir);
}
