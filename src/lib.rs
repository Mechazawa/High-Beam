//! High Beam — native Rust keyboard launcher.
//!
//! See `docs/` for the design notes. The crate is split into a binary front
//! door (`main.rs`) and these modules so they can be unit-tested without
//! standing up the whole daemon.

pub mod app;
pub mod cli;
pub mod daemon;
pub mod frecency;
pub mod ipc;
pub mod paths;
pub mod plugins;
pub mod sdk;
pub mod ui;
pub mod window;

// Slint-generated UI types live in `ui::*` (`crate::ui::QueryWindow`, etc.).
// We re-export the window component at the crate root so existing call sites
// don't have to thread the `ui::` prefix everywhere.
pub use ui::QueryWindow;
