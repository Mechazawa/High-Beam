//! Host implementations of the `highbeam:*` SDK modules. The cap gate lives
//! in [`capability`]. Web/Node globals (`AbortController`, `Buffer`, `URL`,
//! `TextEncoder`/`TextDecoder`, streams, `fetch`) come from the llrt crates and
//! are installed per-context in `plugins::runtime::install_host_globals`;
//! `timers` stays a local polyfill — `llrt_timers` keys its state in a
//! process-global table by raw runtime pointer, which leaks and aliases
//! under the one-runtime-per-plugin + reload model.

pub mod abort;
pub mod actions;
pub mod capability;
pub mod clipboard;
pub mod console;
pub(crate) mod errors;
pub mod fs;
pub mod icons;
#[path = "match.rs"]
pub mod r#match;
pub mod settings;
pub mod system;
pub mod timers;
pub mod view;
