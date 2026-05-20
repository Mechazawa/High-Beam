//! Frecency tracking: persist `(plugin_name, result_key)` pick history and
//! bias the dispatcher's sort so recently-picked results bubble up.
//!
//! Layout:
//!   * `db`    — `SQLite` open/schema/upsert + per-query [`db::Snapshot`]
//!   * `score` — pure modifier function used by `merge_into_live`
//!
//! See `docs/01-architecture.md` (frecency section) for the design and
//! `docs/06-stages.md` (Stage 5) for the spec.

pub mod db;
pub mod score;

pub use db::{FrecencyDb, PickRow, Snapshot, default_db_path, now_seconds};
pub use score::frecency_modifier;
