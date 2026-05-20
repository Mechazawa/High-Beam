//! Plugin loading + dispatch.
//!
//! Layout:
//!   * `manifest` — `manifest.json` schema + parsing
//!   * `result`   — `Result` / `Action` types + sort/merge logic
//!   * `runtime`  — rquickjs Context per plugin; runs `query()`
//!   * `loader`   — scans `plugins/` for subdirectories and loads them
//!   * `dispatch` — fan a keystroke out to every loaded plugin and merge
//!
//! Anything beyond the dispatcher (rendering, action execution) lives in
//! `crate::daemon` / `crate::window`.

pub mod actions;
pub mod builtin;
pub mod dispatch;
pub mod loader;
pub mod manifest;
pub mod result;
pub mod runtime;
