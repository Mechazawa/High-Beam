//! Plugin loading + dispatch.
//!
//! Rendering and action execution live in `crate::app` / `crate::window`.

pub mod actions;
pub mod builtin;
pub mod dispatch;
pub mod loader;
pub mod log;
pub mod manifest;
pub mod result;
pub mod runtime;
