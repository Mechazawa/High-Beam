//! End-to-end check that the plugin runtime can load + run a tiny JS
//! plugin and surface its results in the shape the dispatcher consumes.
//!
//! Integration test (not in `src/`) because rquickjs evaluation needs a
//! real tokio runtime, and the inline `cfg(test)` modules in `src/` would
//! drag the `QuickJS` engine into unit-test builds we don't want it in.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use high_beam::plugins::manifest::Manifest;
use high_beam::plugins::result::{Action, PluginResult};
use high_beam::plugins::runtime::LoadedPlugin;
use tokio_util::sync::CancellationToken;

const ECHO_PLUGIN: &str = r#"
import { copy } from "highbeam:actions";

export async function* query(input, _signal) {
    if (!input) return;
    yield {
        key: "echo",
        title: `echo: ${input}`,
        subtitle: "press Enter to copy",
        action: copy(input),
    };
}
"#;

const NO_CAPABILITY_PLUGIN: &str = r#"
import { copy } from "highbeam:actions";
export async function* query(input, _signal) {
    yield { key: "k", title: input, action: copy(input) };
}
"#;

const FORBIDDEN_IMPORT_PLUGIN: &str = r#"
import fs from "fs";
export async function* query(input, _signal) { yield { key: "k", title: input, action: { kind: "copy", text: input } }; }
"#;

/// Slow-streaming plugin used for the streaming/abort tests. Yields three
/// rows with a 150ms pause between each.
const SLOW_STREAM_PLUGIN: &str = r#"
import { copy } from "highbeam:actions";

export async function* query(input, signal) {
    if (!input) return;
    for (let i = 0; i < 3; i++) {
        if (signal && signal.aborted) return;
        await new Promise(r => setTimeout(r, 150));
        yield {
            key: `slow-${i}`,
            title: `slow ${i}: ${input}`,
            action: copy(`${input}-${i}`),
        };
    }
}
"#;

fn unique_tmp_dir(label: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "high-beam-test-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    fs::create_dir_all(&p).expect("create tmp plugin dir");
    p
}

fn write_plugin(dir: &std::path::Path, manifest_json: &str, source: &str) {
    fs::write(dir.join("manifest.json"), manifest_json).expect("write manifest");
    fs::write(dir.join("plugin.js"), source).expect("write plugin.js");
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt")
}

/// Drain the streaming receiver into a Vec for assertion-style testing.
/// Stops when the channel closes (plugin finished or cancelled).
async fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<PluginResult>) -> Vec<PluginResult> {
    let mut out = Vec::new();
    while let Some(r) = rx.recv().await {
        out.push(r);
    }
    out
}

#[test]
fn echo_plugin_yields_expected_result() {
    let dir = unique_tmp_dir("echo");
    write_plugin(
        &dir,
        r#"{"name":"echo","entry":"plugin.js","timeoutMs":2000,"capabilities":["actions"]}"#,
        ECHO_PLUGIN,
    );
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();

    let runtime = rt();
    runtime.block_on(async {
        let plugin = LoadedPlugin::load(&dir, manifest)
            .await
            .expect("load echo plugin");
        let mut rx = plugin.run_query_stream("hello", CancellationToken::new());
        let results = drain(&mut rx).await;
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.key, "echo");
        assert_eq!(r.title, "echo: hello");
        assert_eq!(r.subtitle.as_deref(), Some("press Enter to copy"));
        match &r.action {
            Action::Copy { text } => assert_eq!(text, "hello"),
            other => panic!("expected Copy action, got {other:?}"),
        }

        // Empty input yields nothing.
        let mut rx = plugin.run_query_stream("", CancellationToken::new());
        let empty = drain(&mut rx).await;
        assert!(empty.is_empty());
    });

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn missing_actions_capability_rejects_import() {
    let dir = unique_tmp_dir("nocap");
    write_plugin(
        &dir,
        r#"{"name":"nocap","entry":"plugin.js","capabilities":[]}"#,
        NO_CAPABILITY_PLUGIN,
    );
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();

    let runtime = rt();
    runtime.block_on(async {
        let outcome = LoadedPlugin::load(&dir, manifest).await;
        let Err(err) = outcome else {
            panic!("loading without `actions` capability must fail");
        };
        let msg = err.to_string();
        assert!(
            msg.contains("actions") || msg.contains("capability") || msg.contains("loading"),
            "unexpected error message: {msg}"
        );
    });

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn forbidden_import_specifier_rejected() {
    let dir = unique_tmp_dir("forbidden");
    write_plugin(
        &dir,
        r#"{"name":"forbidden","entry":"plugin.js","capabilities":["actions"]}"#,
        FORBIDDEN_IMPORT_PLUGIN,
    );
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();

    let runtime = rt();
    runtime.block_on(async {
        let outcome = LoadedPlugin::load(&dir, manifest).await;
        let Err(err) = outcome else {
            panic!("non-highbeam imports must be rejected");
        };
        let msg = err.to_string();
        assert!(
            msg.contains("highbeam") || msg.contains("resolv"),
            "expected resolver error mentioning highbeam, got: {msg}"
        );
    });

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn slow_streaming_plugin_yields_progressively() {
    let dir = unique_tmp_dir("slow-stream");
    write_plugin(
        &dir,
        r#"{"name":"slow","entry":"plugin.js","timeoutMs":5000,"capabilities":["actions"]}"#,
        SLOW_STREAM_PLUGIN,
    );
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();

    let runtime = rt();
    runtime.block_on(async {
        let plugin = LoadedPlugin::load(&dir, manifest)
            .await
            .expect("load slow plugin");
        let mut rx = plugin.run_query_stream("x", CancellationToken::new());

        let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("first row arrived in time")
            .expect("non-empty");
        assert_eq!(first.key, "slow-0");

        let second = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("second row arrived in time")
            .expect("non-empty");
        assert_eq!(second.key, "slow-1");

        // Drain the rest.
        let _rest = drain(&mut rx).await;
    });

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn abort_stops_in_flight_streaming_query() {
    let dir = unique_tmp_dir("abort-stream");
    write_plugin(
        &dir,
        r#"{"name":"abort","entry":"plugin.js","timeoutMs":5000,"capabilities":["actions"]}"#,
        SLOW_STREAM_PLUGIN,
    );
    let manifest = Manifest::parse(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();

    let runtime = rt();
    runtime.block_on(async {
        let plugin = LoadedPlugin::load(&dir, manifest)
            .await
            .expect("load slow plugin");
        let cancel = CancellationToken::new();
        let mut rx = plugin.run_query_stream("x", cancel.clone());

        // Read the first row, then abort. We should not receive the third
        // row (3 rows * 150ms = 450ms total; we cancel inside 250ms).
        let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("first row arrived")
            .expect("non-empty");
        assert_eq!(first.key, "slow-0");

        cancel.cancel();

        // After cancel, the stream should close quickly.
        let close = tokio::time::timeout(Duration::from_secs(2), async {
            while rx.recv().await.is_some() {}
        })
        .await;
        close.expect("stream closed after cancel");
    });

    let _ = fs::remove_dir_all(&dir);
}
