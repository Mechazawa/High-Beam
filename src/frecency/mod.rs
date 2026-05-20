//! Frecency tracking: persist `(plugin_name, result_key)` pick history and
//! bias the dispatcher's sort so recently-picked results bubble up.
//!
//! Layout:
//!   * `db`    — `SQLite` open/schema/upsert + per-query [`db::Snapshot`]
//!   * `score` — pure modifier function used by `merge_into_live`
//!
//! See `docs/01-architecture.md` (frecency section) for the design and
//! `docs/06-stages.md` (Stage 5) for the spec.

mod db;
mod score;

pub(crate) use db::{FrecencyDb, Snapshot, default_db_path, now_seconds};
pub(crate) use score::frecency_modifier;

#[cfg(test)]
pub(crate) use db::PickRow;
