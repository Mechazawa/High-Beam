//! Host implementations of the `highbeam:*` SDK modules.
//!
//! Stage 4 surface:
//!   * `highbeam:actions`   — `openUrl`, `copy`, `exec`, `reveal` action builders
//!   * `highbeam:http`      — `get`, `post` (cap-gated by `http`)
//!   * `highbeam:clipboard` — `read`, `write` (cap-gated by `clipboard.read`
//!     and `clipboard.write` respectively)
//!
//! `abort` is the cross-cutting `AbortController` / `AbortSignal` polyfill —
//! not a `highbeam:` module itself, just a host-side helper that exposes
//! `AbortController` on the global object and provides the abort-token plumbing
//! the other modules consume.

pub mod abort;
pub mod actions;
pub mod capability;
pub mod clipboard;
pub mod http;
pub mod timers;
