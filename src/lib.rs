//! High Beam — native Rust keyboard launcher.
//!
//! See `docs/` for the design notes. The crate is split into a binary front
//! door (`main.rs`) and these modules so they can be unit-tested without
//! standing up the whole daemon.

pub mod cli;
pub mod daemon;
pub mod ipc;
pub mod paths;
pub mod window;

// Pull the Slint-generated component types into scope so other modules can
// refer to them as `crate::QueryWindow` without re-running the macro.
slint::include_modules!();
