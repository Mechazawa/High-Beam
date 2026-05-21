//! High Beam — native Rust keyboard launcher.

pub mod app;
pub mod bundle_install;
pub mod cli;
pub mod confirm;
pub mod daemon;
pub mod frecency;
pub mod ipc;
pub mod logging;
pub mod paths;
pub mod plugins;
pub mod query_history;
pub mod sdk;
pub mod settings;
pub mod settings_ui;
pub mod theme;
pub mod ui;
pub mod window;

pub use ui::QueryWindow;
