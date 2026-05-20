//! Host implementations of the `highbeam:*` SDK modules. The cap gate lives
//! in [`capability`]. `abort` and `timers` are polyfills installed on
//! `globalThis` rather than `highbeam:*` modules.

pub mod abort;
pub mod actions;
pub mod capability;
pub mod clipboard;
pub mod console;
pub(crate) mod errors;
pub mod fs;
pub mod http;
pub mod icons;
#[path = "match.rs"]
pub mod r#match;
pub mod platform;
pub mod system;
pub mod timers;
