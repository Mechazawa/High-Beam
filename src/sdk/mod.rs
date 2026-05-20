//! Host implementations of the `highbeam:*` SDK modules.
//!
//! Modules:
//!   * `highbeam:actions`   — `openUrl`, `copy`, `exec`, `reveal` action builders
//!   * `highbeam:http`      — `get`, `post` (cap-gated by `http`)
//!   * `highbeam:clipboard` — `read`, `write` (cap-gated by `clipboard.read`
//!     and `clipboard.write` respectively)
//!   * `highbeam:fs`        — `readDir`, `readFile`, `readText`, `readCache`,
//!     `writeCache` (cap-gated by `fs.read` / `fs.cache`)
//!   * `highbeam:icons`     — `forPath` (cap-gated by `icons`)
//!   * `highbeam:system`    — `exec`, `applescript` (cap-gated by `system.exec`
//!     / `system.applescript`; `applescript` is a no-op on non-macOS)
//!   * `highbeam:match`     — `fuzzy` (no capability — pure compute)
//!   * `highbeam:platform`  — `os`, `arch`, `version`, `isMacOS`, `isLinux`
//!     (no capability — metadata)
//!
//! `abort` is the cross-cutting `AbortController` / `AbortSignal` polyfill —
//! not a `highbeam:` module itself, just a host-side helper that exposes
//! `AbortController` on the global object and provides the abort-token plumbing
//! the other modules consume.

pub mod abort;
pub mod actions;
pub mod capability;
pub mod clipboard;
pub mod fs;
pub mod http;
pub mod icons;
#[path = "match.rs"]
pub mod r#match;
pub mod platform;
pub mod system;
pub mod timers;
