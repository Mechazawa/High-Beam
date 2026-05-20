//! Host implementations of the `highbeam:*` SDK modules.
//!
//! The capability gate that decides which of these a plugin can import lives
//! in [`capability`]. `abort` and `timers` are cross-cutting polyfills
//! installed on `globalThis` rather than `highbeam:*` modules themselves.

pub mod abort;
pub mod actions;
pub mod capability;
pub mod clipboard;
pub(crate) mod errors;
pub mod fs;
pub mod http;
pub mod icons;
#[path = "match.rs"]
pub mod r#match;
pub mod platform;
pub mod system;
pub mod timers;
