//! Host built-in plugins — appear in the result list alongside JS plugins
//! but never go through rquickjs. Privileged actions (shut down, quit
//! daemon) must not be JS-pluggable.

pub mod core;
