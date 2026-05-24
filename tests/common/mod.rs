//! Shared helpers for integration tests. Each test binary that needs these
//! declares `mod common;` at the top of its file; Rust's
//! `tests/<file>.rs` + `tests/common/mod.rs` convention means this
//! module is NOT compiled as its own integration test.

use std::path::PathBuf;

/// Return a fresh unique temp directory under `std::env::temp_dir()`. `tag`
/// distinguishes parallel test runs in the same suite. The directory is
/// created on the filesystem; caller is responsible for cleanup.
#[allow(dead_code)] // some integration test files don't use this helper
pub fn fresh_tmp(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("high-beam-test-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    dir
}
