//! Plugin-archive download + extraction.
//!
//! Called from the `install` and `update` host tasks. The flow is:
//!   1. Fetch the manifest.json the user pointed us at.
//!   2. Validate it has the fields the installer needs (`name`, `version`,
//!      `archiveUrl`).
//!   3. Download the archive.
//!   4. Detect the format from the URL extension first (then content-type as
//!      a fallback) and extract into a temp dir.
//!   5. If the archive bundles its own manifest.json, cross-check name +
//!      version against the URL-fetched manifest.
//!   6. Move the extracted directory into place under the user plugin
//!      directory, backing up any pre-existing entry.
//!
//! Everything here is engine-agnostic — the actual reload/registry swap
//! lives in `crate::app` because it owns the runtime thread and the
//! `PluginRegistry`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::Client;

use crate::plugins::manifest::Manifest;

/// Default per-network-op timeout. Distinct from the SDK's 30 s because the
/// install flow is interactive and the user is staring at a progress row —
/// give a slow CDN room without making the launcher feel hung.
const NETWORK_TIMEOUT_SECS: u64 = 60;
const NETWORK_TIMEOUT: Duration = Duration::from_secs(NETWORK_TIMEOUT_SECS);

/// Recognised file-suffix → format pairs. Compared with `eq_ignore_ascii_case`
/// against the lowercase tail of the URL path so casing in URLs (e.g. `.ZIP`)
/// doesn't hide a perfectly good archive.
const URL_FORMAT_SUFFIXES: &[(&str, ArchiveFormat)] = &[
    (".tar.gz", ArchiveFormat::TarGz),
    (".tgz", ArchiveFormat::TarGz),
    (".tar", ArchiveFormat::Tar),
    (".zip", ArchiveFormat::Zip),
];

/// Discriminator for the archive payloads the installer handles. URL-extension
/// detection happens first; content-type is the fallback for opaque URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    TarGz,
    Tar,
    Zip,
}

impl ArchiveFormat {
    /// Detect from a URL path or filename. `None` ⇒ caller should fall back
    /// to the response content-type.
    #[must_use]
    pub fn from_url(url: &str) -> Option<Self> {
        let path = url.split('?').next().unwrap_or(url);
        let lower = path.to_ascii_lowercase();
        URL_FORMAT_SUFFIXES
            .iter()
            .find(|(suffix, _)| lower.ends_with(suffix))
            .map(|(_, format)| *format)
    }

    /// Best-effort content-type detection. Accepts the common spellings
    /// servers send.
    #[must_use]
    pub fn from_content_type(content_type: &str) -> Option<Self> {
        let lower = content_type.to_ascii_lowercase();
        let bare = lower.split(';').next().unwrap_or(&lower).trim();

        match bare {
            "application/gzip" | "application/x-gzip" | "application/x-tar+gzip" => Some(Self::TarGz),
            "application/x-tar" => Some(Self::Tar),
            "application/zip" | "application/x-zip-compressed" => Some(Self::Zip),
            _ => None,
        }
    }
}

/// Errors surfaced by the installer. Each variant becomes the subtitle of a
/// "Failed to install …" row, so the wording is the user-facing diagnostic.
#[derive(Debug)]
pub enum InstallError {
    /// Manifest fetch over HTTP failed.
    Fetch(String),
    /// Manifest JSON was structurally invalid.
    BadManifest(String),
    /// Manifest parsed but is missing one of the installer-required fields.
    MissingField(&'static str),
    /// Manifest declared two mutually-exclusive fields (e.g. both
    /// `archiveUrl` and `entryUrl`).
    ConflictingFields { a: &'static str, b: &'static str },
    /// Archive download failed.
    Download(String),
    /// Couldn't identify the archive format from URL or content-type.
    UnknownFormat { url: String, content_type: String },
    /// Archive byte-stream rejected by the extractor.
    Extract(String),
    /// The archive bundled its own manifest.json and it disagrees with the
    /// URL-fetched manifest. `field` names the disagreement
    /// (`"version"` / `"manifestUrl"`).
    EmbeddedMismatch { field: &'static str, detail: String },
    /// Filesystem step (rename, mkdir, etc.) failed.
    Io(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fetch(msg) => write!(f, "fetch manifest: {msg}"),
            Self::BadManifest(msg) => write!(f, "bad manifest: {msg}"),
            Self::MissingField(name) => {
                write!(f, "manifest missing required field `{name}`")
            }
            Self::ConflictingFields { a, b } => {
                write!(f, "manifest declares both `{a}` and `{b}` — pick one")
            }
            Self::Download(msg) => write!(f, "download archive: {msg}"),
            Self::UnknownFormat { url, content_type } => {
                write!(f, "unknown archive format (url={url}, content-type={content_type})")
            }
            Self::Extract(msg) => write!(f, "extract archive: {msg}"),
            Self::EmbeddedMismatch { field, detail } => {
                write!(f, "embedded manifest {field} mismatch: {detail}")
            }
            Self::Io(msg) => write!(f, "io: {msg}"),
        }
    }
}

impl std::error::Error for InstallError {}

/// HTTP client tuned for the installer's longer timeout. Lazily initialised
/// so the binary only pays for the connection pool when a plugin install is
/// requested.
fn client() -> &'static Client {
    static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent(concat!("high-beam/", env!("CARGO_PKG_VERSION")))
            .timeout(NETWORK_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

/// Pre-installer validation: turn the user-supplied manifest URL into a
/// parsed `Manifest` and confirm the installer-required fields are present.
///
/// # Errors
///
/// Returns [`InstallError::Fetch`] for transport problems, `BadManifest` for
/// parse failures, and `MissingField` when one of `name` / `version` /
/// `archiveUrl` is absent.
pub async fn fetch_and_validate_manifest(url: &str) -> Result<Manifest, InstallError> {
    let response = client()
        .get(url)
        .send()
        .await
        .map_err(|e| InstallError::Fetch(e.to_string()))?;

    if !response.status().is_success() {
        return Err(InstallError::Fetch(format!(
            "{} {}",
            response.status().as_u16(),
            response.status().canonical_reason().unwrap_or(""),
        )));
    }

    let bytes = response.bytes().await.map_err(|e| InstallError::Fetch(e.to_string()))?;
    let manifest = Manifest::parse(&bytes).map_err(|e| InstallError::BadManifest(e.to_string()))?;
    require_installer_fields(&manifest)?;
    Ok(manifest)
}

/// Confirm the manifest has the bare-minimum fields the installer needs.
/// Used both for the URL-fetched manifest and for the optional embedded one.
fn require_installer_fields(manifest: &Manifest) -> Result<(), InstallError> {
    if manifest.name.is_empty() {
        return Err(InstallError::MissingField("name"));
    }

    if manifest.version.is_none() {
        return Err(InstallError::MissingField("version"));
    }

    // archiveUrl XOR entryUrl: a plugin chooses one distribution shape;
    // setting both is ambiguous, setting neither leaves nothing to install.
    match (manifest.archive_url.as_deref(), manifest.entry_url.as_deref()) {
        (Some(_), Some(_)) => Err(InstallError::ConflictingFields {
            a: "archiveUrl",
            b: "entryUrl",
        }),
        (None, None) => Err(InstallError::MissingField("archiveUrl|entryUrl")),
        (Some(_), None) | (None, Some(_)) => Ok(()),
    }
}

/// Download a single JS entry file (for the `entryUrl` install path). Caller
/// is responsible for writing the bytes to the right path under the staging
/// dir.
///
/// # Errors
///
/// Returns [`InstallError::Download`] on HTTP failure.
pub async fn download_entry(url: &str) -> Result<Vec<u8>, InstallError> {
    let response = client()
        .get(url)
        .send()
        .await
        .map_err(|e| InstallError::Download(e.to_string()))?;

    if !response.status().is_success() {
        return Err(InstallError::Download(format!(
            "{} {}",
            response.status().as_u16(),
            response.status().canonical_reason().unwrap_or(""),
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| InstallError::Download(e.to_string()))?
        .to_vec();
    Ok(bytes)
}

/// Download the archive bytes and pick the format from URL extension or
/// content-type.
///
/// # Errors
///
/// Returns [`InstallError::Download`] on HTTP failure and
/// [`InstallError::UnknownFormat`] when neither URL nor content-type
/// resolves to a supported format.
pub async fn download_archive(url: &str) -> Result<(Vec<u8>, ArchiveFormat), InstallError> {
    let response = client()
        .get(url)
        .send()
        .await
        .map_err(|e| InstallError::Download(e.to_string()))?;

    if !response.status().is_success() {
        return Err(InstallError::Download(format!(
            "{} {}",
            response.status().as_u16(),
            response.status().canonical_reason().unwrap_or(""),
        )));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| InstallError::Download(e.to_string()))?
        .to_vec();
    let format = ArchiveFormat::from_url(url)
        .or_else(|| ArchiveFormat::from_content_type(&content_type))
        .ok_or_else(|| InstallError::UnknownFormat {
            url: url.to_owned(),
            content_type,
        })?;
    Ok((bytes, format))
}

/// Extract `bytes` into `target_dir`. Caller owns the directory and is
/// responsible for cleanup if extraction fails partway through.
///
/// # Errors
///
/// Returns [`InstallError::Extract`] on any byte-stream / I/O problem.
pub fn extract_archive(bytes: &[u8], format: ArchiveFormat, target_dir: &Path) -> Result<(), InstallError> {
    std::fs::create_dir_all(target_dir).map_err(|e| InstallError::Io(e.to_string()))?;

    match format {
        ArchiveFormat::TarGz => extract_tar(flate2::read::GzDecoder::new(bytes), target_dir),
        ArchiveFormat::Tar => extract_tar(bytes, target_dir),
        ArchiveFormat::Zip => extract_zip(bytes, target_dir),
    }
}

fn extract_tar<R: std::io::Read>(reader: R, target_dir: &Path) -> Result<(), InstallError> {
    let mut archive = tar::Archive::new(reader);
    // Refuse to follow `..` segments out of the target dir — a tarball
    // crafted with `../../etc/passwd` entries shouldn't be able to land
    // outside the plugin sandbox.
    archive
        .unpack(target_dir)
        .map_err(|e| InstallError::Extract(e.to_string()))
}

fn extract_zip(bytes: &[u8], target_dir: &Path) -> Result<(), InstallError> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| InstallError::Extract(e.to_string()))?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| InstallError::Extract(e.to_string()))?;

        // `enclosed_name` rejects `..` traversal and absolute paths.
        let Some(rel) = entry.enclosed_name() else {
            return Err(InstallError::Extract(format!(
                "rejected unsafe zip entry name {:?}",
                entry.name()
            )));
        };

        let outpath = target_dir.join(rel);

        if entry.is_dir() {
            std::fs::create_dir_all(&outpath).map_err(|e| InstallError::Io(e.to_string()))?;
            continue;
        }

        if let Some(parent) = outpath.parent() {
            std::fs::create_dir_all(parent).map_err(|e| InstallError::Io(e.to_string()))?;
        }

        let mut out = std::fs::File::create(&outpath).map_err(|e| InstallError::Io(e.to_string()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| InstallError::Extract(e.to_string()))?;
    }
    Ok(())
}

/// If the extracted dir bundles its own `manifest.json`, parse it and
/// cross-check `version` (must match the URL-fetched manifest) and
/// `manifestUrl` (if present in the embedded one, must match the install
/// URL).
///
/// Returns `Ok(true)` when an embedded manifest was found and validated,
/// `Ok(false)` when none was present (which the spec explicitly allows).
///
/// # Errors
///
/// Returns [`InstallError::EmbeddedMismatch`] on either disagreement and
/// [`InstallError::BadManifest`] when the embedded manifest exists but is
/// unparseable.
pub fn cross_check_embedded(
    extracted_dir: &Path,
    expected: &Manifest,
    install_url: &str,
) -> Result<bool, InstallError> {
    let embedded_path = find_embedded_manifest(extracted_dir);
    let Some(embedded_path) = embedded_path else {
        return Ok(false);
    };
    let bytes = std::fs::read(&embedded_path).map_err(|e| InstallError::Io(e.to_string()))?;
    let embedded = Manifest::parse(&bytes).map_err(|e| InstallError::BadManifest(e.to_string()))?;
    let expected_version = expected
        .version
        .as_deref()
        .ok_or(InstallError::MissingField("version"))?;
    let embedded_version = embedded.version.as_deref().unwrap_or("");

    if embedded_version != expected_version {
        return Err(InstallError::EmbeddedMismatch {
            field: "version",
            detail: format!("embedded={embedded_version}, expected={expected_version}"),
        });
    }

    if let Some(embedded_url) = embedded.manifest_url.as_deref()
        && embedded_url != install_url
    {
        return Err(InstallError::EmbeddedMismatch {
            field: "manifestUrl",
            detail: format!("embedded={embedded_url}, install_url={install_url}"),
        });
    }
    Ok(true)
}

/// Look one level deep for `manifest.json` — `tar` and `zip` archives
/// sometimes wrap their contents in a single top-level directory, sometimes
/// don't. Either layout is accepted.
fn find_embedded_manifest(extracted_dir: &Path) -> Option<PathBuf> {
    let direct = extracted_dir.join("manifest.json");

    if direct.exists() {
        return Some(direct);
    }
    let entries = std::fs::read_dir(extracted_dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            let candidate = path.join("manifest.json");

            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Where the extracted plugin tree actually lives inside `extracted_dir`.
/// Some archives wrap everything in a top-level directory; some unpack
/// directly. Tries the direct path first, then a single-child-directory
/// fallback.
#[must_use]
pub fn find_payload_root(extracted_dir: &Path) -> PathBuf {
    if extracted_dir.join("manifest.json").exists() {
        return extracted_dir.to_path_buf();
    }

    if let Ok(entries) = std::fs::read_dir(extracted_dir) {
        let mut dirs = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir());

        if let Some(single) = dirs.next()
            && dirs.next().is_none()
        {
            return single;
        }
    }
    extracted_dir.to_path_buf()
}

/// Move `payload_root` into `<plugins_dir>/<name>/`, backing up any
/// pre-existing directory at the destination to
/// `<name>.backup.<unix-millis>` so the user can manually roll back a bad
/// install.
///
/// # Errors
///
/// Returns [`InstallError::Io`] on any filesystem step.
pub fn move_into_plugins_dir(payload_root: &Path, plugins_dir: &Path, name: &str) -> Result<PathBuf, InstallError> {
    std::fs::create_dir_all(plugins_dir).map_err(|e| InstallError::Io(e.to_string()))?;

    let destination = plugins_dir.join(name);

    if destination.exists() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| InstallError::Io(e.to_string()))?
            .as_millis();
        let backup = plugins_dir.join(format!("{name}.backup.{now}"));
        std::fs::rename(&destination, &backup).map_err(|e| InstallError::Io(e.to_string()))?;
    }

    // `rename` works for cross-directory moves within one filesystem; for
    // cross-fs we fall back to copy + remove.
    if let Err(err) = std::fs::rename(payload_root, &destination) {
        if err.kind() == std::io::ErrorKind::CrossesDevices {
            copy_dir_recursive(payload_root, &destination).map_err(|e| InstallError::Io(e.to_string()))?;
            std::fs::remove_dir_all(payload_root).map_err(|e| InstallError::Io(e.to_string()))?;
        } else {
            return Err(InstallError::Io(err.to_string()));
        }
    }

    Ok(destination)
}

/// Recursive copy used as the cross-filesystem fallback for the installer's
/// rename-into-place step. `entry.file_type()` is `lstat`-equivalent — it
/// does NOT follow symlinks. This is deliberately defensive: the source is a
/// freshly-extracted plugin archive (potentially attacker-controlled), so a
/// symlink inside the staging dir could point at `/etc/passwd` or similar.
/// Skipping symlinks matches the `..`-traversal rejection the extractors do.
///
/// `bundle_install` has the sister implementation that DOES follow symlinks
/// — its source is the trusted `.app` bundle, where following symlinks is
/// what keeps the user-dir copy self-contained.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = entry.file_type()?;

        if meta.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if meta.is_file() {
            std::fs::copy(&from, &to)?;
        }
        // Symlinks / sockets / devices fall through — see fn docs.
    }
    Ok(())
}

/// Write `manifest` into `<dest>/manifest.json`, pretty-printed. The
/// installer calls this after extraction to backfill `manifestUrl` when the
/// archived manifest didn't carry one — future `update` runs need it to
/// re-fetch.
///
/// # Errors
///
/// Returns [`InstallError::Io`] on serialisation or write failure.
pub fn write_manifest_json(dest_dir: &Path, manifest: &ManifestForWrite) -> Result<(), InstallError> {
    let path = dest_dir.join("manifest.json");
    let body = serde_json::to_string_pretty(manifest).map_err(|e| InstallError::Io(e.to_string()))?;
    std::fs::write(&path, body).map_err(|e| InstallError::Io(e.to_string()))
}

/// Minimal mirror of [`Manifest`] used purely as the JSON write shape — we
/// only persist the fields the installer needs to set / preserve, so we
/// can't accidentally drop unknown fields the loader tolerated by mistake.
/// Use [`manifest_for_write`] to construct.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestForWrite {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub entry: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platforms: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_url: Option<String>,
}

/// Distill a parsed `Manifest` into the writable shape with `manifest_url`
/// backfilled — the canonical reason this helper exists.
#[must_use]
pub fn manifest_for_write(source: &Manifest, install_url: &str) -> ManifestForWrite {
    ManifestForWrite {
        name: source.name.clone(),
        display_name: source.display_name.clone(),
        version: source.version.clone(),
        description: source.description.clone(),
        entry: source.entry.clone(),
        capabilities: source.capabilities.clone(),
        platforms: source.platforms.clone(),
        archive_url: source.archive_url.clone(),
        entry_url: source.entry_url.clone(),
        manifest_url: source.manifest_url.clone().or_else(|| Some(install_url.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn fresh_tmp(tag: &str) -> PathBuf {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("high-beam-install-test-{tag}-{now}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

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

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use zip::write::SimpleFileOptions;
        let buf: Vec<u8> = Vec::new();
        let cursor = std::io::Cursor::new(buf);
        let mut writer = zip::ZipWriter::new(cursor);
        let opts: SimpleFileOptions = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for (path, body) in entries {
            writer.start_file(*path, opts).expect("start_file");
            writer.write_all(body).expect("write body");
        }
        let cursor = writer.finish().expect("finish zip");
        cursor.into_inner()
    }

    #[test]
    fn format_detection_from_url_handles_common_extensions() {
        assert_eq!(
            ArchiveFormat::from_url("https://x/y.tar.gz"),
            Some(ArchiveFormat::TarGz)
        );
        assert_eq!(
            ArchiveFormat::from_url("https://x/y.tgz?token=abc"),
            Some(ArchiveFormat::TarGz)
        );
        assert_eq!(ArchiveFormat::from_url("https://x/y.tar"), Some(ArchiveFormat::Tar));
        assert_eq!(ArchiveFormat::from_url("https://x/y.ZIP"), Some(ArchiveFormat::Zip));
        assert_eq!(ArchiveFormat::from_url("https://x/opaque"), None);
    }

    #[test]
    fn format_detection_from_content_type_falls_back() {
        assert_eq!(
            ArchiveFormat::from_content_type("application/gzip"),
            Some(ArchiveFormat::TarGz)
        );
        assert_eq!(
            ArchiveFormat::from_content_type("application/zip; charset=binary"),
            Some(ArchiveFormat::Zip)
        );
        assert_eq!(ArchiveFormat::from_content_type("text/html"), None);
    }

    #[test]
    fn require_installer_fields_rejects_missing_archive_url() {
        let m = Manifest::parse(br#"{ "name": "x", "version": "1.0.0" }"#).unwrap();

        match require_installer_fields(&m) {
            Err(InstallError::MissingField(name)) => {
                assert_eq!(name, "archiveUrl|entryUrl");
            }
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    #[test]
    fn require_installer_fields_accepts_entry_url() {
        let m =
            Manifest::parse(br#"{ "name": "x", "version": "1.0.0", "entryUrl": "https://example.com/p.js" }"#).unwrap();
        require_installer_fields(&m).expect("entryUrl-only manifests are valid");
    }

    #[test]
    fn require_installer_fields_rejects_both_archive_and_entry_url() {
        let m = Manifest::parse(
            br#"{
                "name": "x",
                "version": "1.0.0",
                "archiveUrl": "https://example.com/p.tar.gz",
                "entryUrl": "https://example.com/p.js"
            }"#,
        )
        .unwrap();
        match require_installer_fields(&m) {
            Err(InstallError::ConflictingFields { a, b }) => {
                assert_eq!(a, "archiveUrl");
                assert_eq!(b, "entryUrl");
            }
            other => panic!("expected ConflictingFields, got {other:?}"),
        }
    }

    #[test]
    fn require_installer_fields_rejects_missing_version() {
        let m = Manifest::parse(br#"{ "name": "x", "archiveUrl": "https://example.com/a.tar.gz" }"#).unwrap();
        match require_installer_fields(&m) {
            Err(InstallError::MissingField(name)) => assert_eq!(name, "version"),
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    #[test]
    fn extract_tar_gz_unpacks_files() {
        let root = fresh_tmp("extract-tgz");
        let bytes = build_tar_gz(&[
            ("hello/manifest.json", br#"{"name":"hello","version":"1.0.0"}"#),
            ("hello/plugin.js", b"// js body"),
        ]);
        extract_archive(&bytes, ArchiveFormat::TarGz, &root).expect("extract");
        assert!(root.join("hello/manifest.json").exists());
        assert!(root.join("hello/plugin.js").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_zip_unpacks_files() {
        let root = fresh_tmp("extract-zip");
        let bytes = build_zip(&[
            ("hello/manifest.json", br#"{"name":"hello","version":"1.0.0"}"#),
            ("hello/plugin.js", b"// js body"),
        ]);
        extract_archive(&bytes, ArchiveFormat::Zip, &root).expect("extract");
        assert!(root.join("hello/manifest.json").exists());
        assert!(root.join("hello/plugin.js").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cross_check_accepts_matching_embedded_manifest() {
        let root = fresh_tmp("cross-check-ok");
        std::fs::create_dir_all(root.join("hello")).unwrap();
        std::fs::write(
            root.join("hello/manifest.json"),
            br#"{
                "name": "hello",
                "version": "1.0.0",
                "archiveUrl": "https://example.com/h.tar.gz"
            }"#,
        )
        .unwrap();
        let expected = Manifest::parse(
            br#"{
                "name": "hello",
                "version": "1.0.0",
                "archiveUrl": "https://example.com/h.tar.gz"
            }"#,
        )
        .unwrap();
        let found = cross_check_embedded(&root, &expected, "https://example.com/h/manifest.json").expect("cross check");
        assert!(found, "embedded manifest should have been found");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cross_check_rejects_version_mismatch() {
        let root = fresh_tmp("cross-check-version");
        std::fs::create_dir_all(root.join("hello")).unwrap();
        std::fs::write(
            root.join("hello/manifest.json"),
            br#"{
                "name": "hello",
                "version": "0.9.0",
                "archiveUrl": "https://example.com/h.tar.gz"
            }"#,
        )
        .unwrap();
        let expected = Manifest::parse(
            br#"{
                "name": "hello",
                "version": "1.0.0",
                "archiveUrl": "https://example.com/h.tar.gz"
            }"#,
        )
        .unwrap();
        let err = cross_check_embedded(&root, &expected, "https://example.com/h/manifest.json").expect_err("mismatch");

        match err {
            InstallError::EmbeddedMismatch { field, .. } => assert_eq!(field, "version"),
            other => panic!("expected EmbeddedMismatch(version), got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cross_check_rejects_manifest_url_mismatch() {
        let root = fresh_tmp("cross-check-url");
        std::fs::create_dir_all(root.join("hello")).unwrap();
        std::fs::write(
            root.join("hello/manifest.json"),
            br#"{
                "name": "hello",
                "version": "1.0.0",
                "archiveUrl": "https://example.com/h.tar.gz",
                "manifestUrl": "https://example.com/other/manifest.json"
            }"#,
        )
        .unwrap();
        let expected = Manifest::parse(
            br#"{
                "name": "hello",
                "version": "1.0.0",
                "archiveUrl": "https://example.com/h.tar.gz"
            }"#,
        )
        .unwrap();
        let err = cross_check_embedded(&root, &expected, "https://example.com/h/manifest.json").expect_err("mismatch");

        match err {
            InstallError::EmbeddedMismatch { field, .. } => assert_eq!(field, "manifestUrl"),
            other => panic!("expected EmbeddedMismatch(manifestUrl), got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cross_check_accepts_archive_without_embedded_manifest() {
        let root = fresh_tmp("cross-check-absent");
        let expected = Manifest::parse(
            br#"{
                "name": "hello",
                "version": "1.0.0",
                "archiveUrl": "https://example.com/h.tar.gz"
            }"#,
        )
        .unwrap();
        let found =
            cross_check_embedded(&root, &expected, "https://example.com/h/manifest.json").expect("absent is fine");
        assert!(!found, "no embedded manifest means cross-check returns false");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_payload_root_picks_single_top_level_dir() {
        let root = fresh_tmp("payload-root");
        std::fs::create_dir_all(root.join("inner")).unwrap();
        std::fs::write(root.join("inner/manifest.json"), b"{}").unwrap();
        let payload = find_payload_root(&root);
        assert_eq!(payload, root.join("inner"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_payload_root_handles_direct_layout() {
        let root = fresh_tmp("payload-direct");
        std::fs::write(root.join("manifest.json"), b"{}").unwrap();
        let payload = find_payload_root(&root);
        assert_eq!(payload, root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn move_into_plugins_dir_backs_up_existing() {
        let root = fresh_tmp("move-backup");
        let plugins_dir = root.join("plugins");
        let payload = root.join("payload");
        std::fs::create_dir_all(&plugins_dir).unwrap();
        std::fs::create_dir_all(&payload).unwrap();
        std::fs::write(payload.join("manifest.json"), b"{}").unwrap();
        // Pre-existing destination.
        std::fs::create_dir_all(plugins_dir.join("plug")).unwrap();
        std::fs::write(plugins_dir.join("plug/old.txt"), b"old").unwrap();

        let dst = move_into_plugins_dir(&payload, &plugins_dir, "plug").expect("move");
        assert_eq!(dst, plugins_dir.join("plug"));
        assert!(dst.join("manifest.json").exists(), "new payload landed");

        // A backup directory exists alongside the new install.
        let backups: Vec<_> = std::fs::read_dir(&plugins_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("plug.backup."))
            .collect();
        assert_eq!(backups.len(), 1, "exactly one backup expected");
        assert!(backups[0].path().join("old.txt").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn manifest_for_write_backfills_manifest_url() {
        let m = Manifest::parse(
            br#"{
                "name": "hello",
                "version": "1.0.0",
                "archiveUrl": "https://example.com/h.tar.gz"
            }"#,
        )
        .unwrap();
        let writeable = manifest_for_write(&m, "https://example.com/h/manifest.json");
        assert_eq!(
            writeable.manifest_url.as_deref(),
            Some("https://example.com/h/manifest.json")
        );
    }

    #[test]
    fn manifest_for_write_keeps_existing_manifest_url() {
        let m = Manifest::parse(
            br#"{
                "name": "hello",
                "version": "1.0.0",
                "archiveUrl": "https://example.com/h.tar.gz",
                "manifestUrl": "https://example.com/old/manifest.json"
            }"#,
        )
        .unwrap();
        let writeable = manifest_for_write(&m, "https://example.com/h/manifest.json");
        // Existing value wins — installer never overwrites an author-declared URL.
        assert_eq!(
            writeable.manifest_url.as_deref(),
            Some("https://example.com/old/manifest.json")
        );
    }
}
