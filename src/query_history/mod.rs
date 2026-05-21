//! Persistent query-history: storage and in-session cycling state machine.

mod db;
mod state;

pub(crate) use db::{QueryHistoryDb, default_db_path};
pub(crate) use state::{InputAction, QueryHistoryState};
