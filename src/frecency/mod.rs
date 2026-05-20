//! Frecency tracking: persist `(plugin_name, result_key)` pick history and
//! bias the dispatcher's sort so recently-picked results bubble up.
//!
//! `db` handles `SQLite` open/schema/upsert + per-query [`db::Snapshot`];
//! `score` is the pure modifier function `merge_into_live` consumes.

mod db;
mod score;

pub(crate) use db::{FrecencyDb, Snapshot, default_db_path, now_seconds};
pub(crate) use score::frecency_modifier;

#[cfg(test)]
pub(crate) use db::PickRow;
