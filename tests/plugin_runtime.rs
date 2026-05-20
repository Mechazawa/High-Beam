//! End-to-end check that the plugin runtime can load + run a tiny JS
//! plugin and surface its results in the shape the dispatcher consumes.
//!
//! Integration test (not in `src/`) because rquickjs evaluation needs a
//! real tokio runtime, and the inline `cfg(test)` modules in `src/` would
//! drag the `QuickJS` engine into unit-test builds we don't want it in.

use std::fs;
use std::path::PathBuf;

use high_beam::plugins::manifest::Manifest;
use high_beam::plugins::result::Action;
use high_beam::plugins::runtime::LoadedPlugin;

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

fn unique_tmp_dir(label: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "high-beam-test-{label}-{}-{}",
        std::process::id(),
        // monotonic-ish: nanos since UNIX epoch
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
        let results = plugin.run_query("hello").await.expect("query ok");
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.key, "echo");
        assert_eq!(r.title, "echo: hello");
        assert_eq!(r.subtitle.as_deref(), Some("press Enter to copy"));
        match &r.action {
            Action::Copy { text } => assert_eq!(text, "hello"),
            other @ Action::OpenUrl { .. } => panic!("expected Copy action, got {other:?}"),
        }

        // Empty input yields nothing.
        let empty = plugin.run_query("").await.expect("query ok");
        assert!(empty.is_empty());
    });

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn missing_actions_capability_rejects_import() {
    let dir = unique_tmp_dir("nocap");
    // No "actions" capability — the loader must refuse to bind highbeam:actions.
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
