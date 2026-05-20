//! Slint-generated UI types, re-exported. `slint::include_modules!` would
//! pollute its host module with everything from `ui/query.slint`; isolating
//! it here keeps the crate root clean.

#![allow(clippy::pedantic)]
// reason: Slint codegen emits identifiers and methods that don't pass our
// pedantic gate (single-char names, must_use candidates, etc.). The allow is
// scoped to this module so the rest of the crate stays strict.

slint::include_modules!();
