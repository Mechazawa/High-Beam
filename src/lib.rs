//! High Beam — native Rust keyboard launcher.

pub mod app;
pub mod cli;
pub mod daemon;
pub mod frecency;
pub mod ipc;
pub mod paths;
pub mod plugins;
pub mod sdk;
pub mod theme;
pub mod ui;
pub mod window;

pub use ui::QueryWindow;
