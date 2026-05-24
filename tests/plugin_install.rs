//! End-to-end coverage for the install pipeline.
//!
//! Spins up a tiny localhost HTTP server that serves a manifest.json and a
//! tar.gz archive, then drives `plugins::install` against it. Verifies the
//! plugin lands on disk and the manifestUrl backfill works.

use std::io::Read;
use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use flate2::Compression;
use flate2::write::GzEncoder;
use high_beam::plugins::install::{self, ArchiveFormat, InstallError, cross_check_embedded, manifest_for_write};
use high_beam::plugins::manifest::Manifest;

mod common;
use common::fresh_tmp;

fn build_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let gz_buf = Vec::new();
    let encoder = GzEncoder::new(gz_buf, Compression::default());
    let mut tar_builder = tar::Builder::new(encoder);
    for (path, body) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar_builder.append_data(&mut header, path, *body).expect("tar append");
    }
    let encoder = tar_builder.into_inner().expect("finish tar");
    encoder.finish().expect("finish gz")
}

/// Tiny synchronous HTTP server. Each registered path maps to a (content-type,
/// body) pair. Returns the listening address. The server thread runs until
/// the listener is dropped (which happens when `Server` is dropped).
struct Server {
    addr: SocketAddr,
    _handle: thread::JoinHandle<()>,
}

impl Server {
    fn start(routes: Vec<(String, &'static str, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind localhost ephemeral port");
        let addr = listener.local_addr().expect("local_addr");
        let routes = Arc::new(routes);
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let routes = Arc::clone(&routes);
                thread::spawn(move || handle_conn(stream, &routes));
            }
        });
        Self { addr, _handle: handle }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

fn handle_conn(mut stream: TcpStream, routes: &[(String, &'static str, Vec<u8>)]) {
    let mut buf = [0u8; 4096];
    let _ = stream.read(&mut buf);
    let req = String::from_utf8_lossy(&buf);
    let path = req.split_whitespace().nth(1).unwrap_or("/").to_owned();
    if let Some((_, ct, body)) = routes.iter().find(|(p, _, _)| *p == path) {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(body);
    } else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt")
}

#[test]
fn install_pipeline_downloads_extracts_and_lands_in_plugins_dir() {
    let plugins_dir = fresh_tmp("install-happy");

    let archive = build_tar_gz(&[(
        "plugin.js",
        b"export async function* query() { yield { key: 'k', title: 'hi', action: { kind: 'noop' } }; }",
    )]);
    let manifest_json = br#"{
        "name": "demo",
        "version": "1.0.0",
        "entry": "plugin.js",
        "archiveUrl": "http://__server__/demo.tar.gz"
    }"#;

    let archive_for_server = archive.clone();
    let server = Server::start(vec![
        // Manifest body — note `__server__` placeholder is patched after
        // start so it resolves to the actual port.
        ("/demo/manifest.json".into(), "application/json", manifest_json.to_vec()),
        ("/demo.tar.gz".into(), "application/gzip", archive_for_server),
    ]);

    // Re-serve with the actual archive URL injected so the manifest matches
    // what the server can return.
    let real_archive_url = server.url("/demo.tar.gz");
    let patched_manifest = format!(
        r#"{{
            "name": "demo",
            "version": "1.0.0",
            "entry": "plugin.js",
            "archiveUrl": "{real_archive_url}"
        }}"#,
    );
    drop(server);
    let server = Server::start(vec![
        (
            "/demo/manifest.json".into(),
            "application/json",
            patched_manifest.into_bytes(),
        ),
        ("/demo.tar.gz".into(), "application/gzip", archive),
    ]);

    let manifest_url = server.url("/demo/manifest.json");
    let rt = rt();

    let manifest = rt
        .block_on(install::fetch_and_validate_manifest(&manifest_url))
        .expect("fetch manifest");
    assert_eq!(manifest.name, "demo");
    let archive_url = manifest.archive_url.as_deref().expect("archiveUrl");
    let (bytes, format) = rt.block_on(install::download_archive(archive_url)).expect("download");
    assert_eq!(format, ArchiveFormat::TarGz);

    let staging = fresh_tmp("install-staging");
    install::extract_archive(&bytes, format, &staging).expect("extract");
    let payload = install::find_payload_root(&staging);
    let _ = cross_check_embedded(&payload, &manifest, &manifest_url).expect("cross check");

    let writeable = manifest_for_write(&manifest, &manifest_url);
    install::write_manifest_json(&payload, &writeable).expect("write manifest");

    let dest = install::move_into_plugins_dir(&payload, &plugins_dir, &manifest.name).expect("move");
    assert!(dest.join("plugin.js").exists());
    assert!(dest.join("manifest.json").exists());

    // The manifestUrl was backfilled in the on-disk manifest.
    let final_manifest_bytes = std::fs::read(dest.join("manifest.json")).expect("read");
    let final_manifest = Manifest::parse(&final_manifest_bytes).expect("parse");
    assert_eq!(final_manifest.manifest_url.as_deref(), Some(manifest_url.as_str()));

    let _ = std::fs::remove_dir_all(&plugins_dir);
    let _ = std::fs::remove_dir_all(&staging);
}

#[test]
fn install_rejects_manifest_missing_required_fields() {
    let server = Server::start(vec![(
        "/bad/manifest.json".into(),
        "application/json",
        br#"{ "name": "bad" }"#.to_vec(),
    )]);
    let rt = rt();
    let err = rt
        .block_on(install::fetch_and_validate_manifest(&server.url("/bad/manifest.json")))
        .expect_err("should be MissingField");
    match err {
        InstallError::MissingField(name) => assert!(matches!(name, "version" | "archiveUrl")),
        other => panic!("expected MissingField, got {other:?}"),
    }
}

#[test]
fn install_rejects_embedded_version_mismatch() {
    // The archive's manifest disagrees with the URL-fetched one — the
    // installer must reject before any rename happens.
    let archive = build_tar_gz(&[
        (
            "plugin/manifest.json",
            br#"{
                "name": "demo",
                "version": "9.9.9",
                "archiveUrl": "http://x/a.tar.gz"
            }"#,
        ),
        ("plugin/plugin.js", b"// nop"),
    ]);
    let staging = fresh_tmp("install-version-mismatch");
    install::extract_archive(&archive, ArchiveFormat::TarGz, &staging).expect("extract");
    let payload = install::find_payload_root(&staging);

    let url_fetched = Manifest::parse(
        br#"{
            "name": "demo",
            "version": "1.0.0",
            "archiveUrl": "http://x/a.tar.gz"
        }"#,
    )
    .expect("parse");
    let err = install::cross_check_embedded(&payload, &url_fetched, "http://x/m.json").expect_err("mismatch");
    match err {
        InstallError::EmbeddedMismatch { field, .. } => assert_eq!(field, "version"),
        other => panic!("expected EmbeddedMismatch, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&staging);
}

#[test]
fn install_accepts_archive_without_embedded_manifest_and_backfills_manifest_url() {
    let archive = build_tar_gz(&[("plugin.js", b"export async function* query() {}")]);
    let staging = fresh_tmp("install-no-embedded");
    install::extract_archive(&archive, ArchiveFormat::TarGz, &staging).expect("extract");
    let payload = install::find_payload_root(&staging);

    let url_fetched = Manifest::parse(
        br#"{
            "name": "loose",
            "version": "1.0.0",
            "archiveUrl": "http://x/a.tar.gz"
        }"#,
    )
    .expect("parse");
    let install_url = "http://example.com/loose/manifest.json";
    let found = install::cross_check_embedded(&payload, &url_fetched, install_url).expect("no embedded is fine");
    assert!(!found, "no embedded manifest expected");

    let writeable = install::manifest_for_write(&url_fetched, install_url);
    install::write_manifest_json(&payload, &writeable).expect("write manifest");
    assert!(payload.join("manifest.json").exists());

    let written = Manifest::parse(&std::fs::read(payload.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(written.manifest_url.as_deref(), Some(install_url));

    let _ = std::fs::remove_dir_all(&staging);
}
