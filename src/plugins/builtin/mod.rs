//! Host built-in plugins.
//!
//! These appear alongside JS plugins in the result list but never go through
//! rquickjs — privileged actions like shutting the machine down or quitting
//! the daemon must not be JS-pluggable.

pub mod core;
