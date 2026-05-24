//! End-to-end coverage for the registry's hot-reload primitive.
//!
//! Loads a plugin via the real loader pipeline, mutates the on-disk source,
//! and verifies the reloaded `LoadedPlugin` reflects the new code without
//! restarting the runtime thread.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use high_beam::plugins::loader::LoaderOptions;
use high_beam::plugins::registry::{PluginRegistry, ReloadError};
use high_beam::plugins::runtime::LoadedPlugin;
use high_beam::settings::Settings;
use tokio_util::sync::CancellationToken;

mod common;
use common::fresh_tmp;

fn write_plugin(root: &Path, name: &str, manifest: &str, entry: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    std::fs::write(dir.join("plugin.js"), entry).unwrap();
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt")
}

async fn first_title(plugin: &Arc<LoadedPlugin>) -> Option<String> {
    let mut rx = plugin.run_query_stream("anything", CancellationToken::new());
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("plugin must yield within 2s")
        .map(|r| r.title)
}

#[test]
fn reload_one_swaps_the_running_js_context() {
    let root = fresh_tmp("swap-js-context");
    write_plugin(
        &root,
        "swappy",
        r#"{ "name": "swappy", "entry": "plugin.js", "version": "0.1.0" }"#,
        r"
        export async function* query(input) {
            yield { key: 'k', title: 'BEFORE', action: { kind: 'noop' } };
        }
        ",
    );

    let opts = LoaderOptions {
        plugins_dir: root.clone(),
    };
    let reg = PluginRegistry::new(opts, Vec::new());
    let rt = rt();

    rt.block_on(reg.reload_all(&Settings::default()));
    let before = rt.block_on(reg.snapshot()).into_iter().next().unwrap();
    assert_eq!(rt.block_on(first_title(&before)), Some("BEFORE".into()));

    // Mutate the JS on disk + reload by name.
    std::fs::write(
        root.join("swappy/plugin.js"),
        r"
        export async function* query(input) {
            yield { key: 'k', title: 'AFTER', action: { kind: 'noop' } };
        }
        ",
    )
    .unwrap();
    rt.block_on(reg.reload_one("swappy", &Settings::default()))
        .expect("reload swappy");

    let after = rt.block_on(reg.snapshot()).into_iter().next().unwrap();
    // The Arc changed — reload swapped the underlying plugin.
    assert!(!Arc::ptr_eq(&before, &after), "reload_one must replace the Arc");
    assert_eq!(rt.block_on(first_title(&after)), Some("AFTER".into()));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn reload_all_swaps_every_plugin_arc() {
    let root = fresh_tmp("swap-all");
    write_plugin(
        &root,
        "alpha",
        r#"{ "name": "alpha", "entry": "plugin.js", "version": "0.1.0" }"#,
        "export async function* query() {}",
    );
    write_plugin(
        &root,
        "beta",
        r#"{ "name": "beta", "entry": "plugin.js", "version": "0.1.0" }"#,
        "export async function* query() {}",
    );

    let opts = LoaderOptions {
        plugins_dir: root.clone(),
    };
    let reg = PluginRegistry::new(opts, Vec::new());
    let rt = rt();
    rt.block_on(reg.reload_all(&Settings::default()));
    let before: Vec<Arc<LoadedPlugin>> = rt.block_on(reg.snapshot());
    assert_eq!(before.len(), 2);

    rt.block_on(reg.reload_all(&Settings::default()));
    let after: Vec<Arc<LoadedPlugin>> = rt.block_on(reg.snapshot());
    assert_eq!(after.len(), 2);

    // Every Arc must have been replaced — reload_all is a wholesale swap.
    for (b, a) in before.iter().zip(after.iter()) {
        assert!(
            !Arc::ptr_eq(b, a),
            "reload_all must replace every Arc (matching names: {} / {})",
            b.manifest.name,
            a.manifest.name,
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn reload_unknown_plugin_returns_not_found() {
    let root = fresh_tmp("unknown-plugin");
    let opts = LoaderOptions {
        plugins_dir: root.clone(),
    };
    let reg = PluginRegistry::new(opts, Vec::new());
    let rt = rt();
    match rt.block_on(reg.reload_one("ghost", &Settings::default())) {
        Err(ReloadError::NotFound(name)) => assert_eq!(name, "ghost"),
        Err(other) => panic!("expected NotFound, got {other:?}"),
        Ok(_) => panic!("expected NotFound, got Ok"),
    }
    let _ = std::fs::remove_dir_all(&root);
}
