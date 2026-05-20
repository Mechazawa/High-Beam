//! Slint-generated UI types, re-exported.
//!
//! `slint::include_modules!` generates Rust types from `ui/query.slint` and
//! pollutes whatever module invokes it with `QueryWindow`, callback handles,
//! and helper structs. We isolate that mess here so the crate root stays
//! readable; callers reach for `crate::ui::QueryWindow` etc.

#![allow(clippy::pedantic)]
// reason: Slint's code generator emits identifiers and methods that don't pass
// our clippy::pedantic gate (single-character names, `must_use` candidates,
// etc.). Suppressing pedantic here is local to the generated-code module and
// keeps the rest of the crate strict.

slint::include_modules!();
