//! `SQLite` storage for query history.
//!
//! Schema is intentionally minimal: `id` gives natural insertion order so the
//! loader can `ORDER BY id DESC` to return most-recent-first without a
//! secondary index. `created_at` is informational only.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};

use crate::frecency::now_seconds;
use crate::paths::ensure_parent_dir;

const SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS query_history (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    query      TEXT    NOT NULL,
    created_at INTEGER NOT NULL
);
";

/// Owned handle to the query-history database. `Arc<Mutex>` matches the
/// frecency-db pattern: single-threaded access, uncontended on the hot path.
#[derive(Clone)]
pub(crate) struct QueryHistoryDb {
    inner: Arc<Mutex<Connection>>,
}

impl QueryHistoryDb {
    /// Open (or create) the database at `path`. Creates the parent directory
    /// if missing. Initialises the schema on first open.
    ///
    /// # Errors
    ///
    /// Returns an error if the file or parent directory can't be created, or
    /// if `SQLite` can't open / initialise the schema.
    pub(crate) fn open(path: &Path) -> rusqlite::Result<Self> {
        ensure_parent_dir(path).map_err(|err| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                Some(format!("create parent dir for {}: {err}", path.display())),
            )
        })?;
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an in-memory database for tests.
    ///
    /// # Errors
    ///
    /// Returns the same SQL error set as [`Self::open`].
    #[cfg(test)]
    pub(crate) fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    /// Append `query` to history if it differs from the most recent entry.
    /// After inserting, trims the table to `max_entries` by deleting the
    /// oldest rows.
    ///
    /// # Errors
    ///
    /// Returns a SQL error on lock poisoning or query failure.
    pub(crate) fn push(&self, query: &str, max_entries: usize) -> rusqlite::Result<()> {
        let guard = self.lock()?;
        let last: Option<String> = guard
            .query_row("SELECT query FROM query_history ORDER BY id DESC LIMIT 1", [], |row| {
                row.get(0)
            })
            .ok();

        if last.as_deref() == Some(query) {
            return Ok(());
        }
        let now = now_seconds();
        guard.execute(
            "INSERT INTO query_history (query, created_at) VALUES (?1, ?2)",
            params![query, now],
        )?;
        // Trim oldest entries so the table stays at most `max_entries` rows.
        let count: i64 = guard.query_row("SELECT COUNT(*) FROM query_history", [], |row| row.get(0))?;
        let excess = count - i64::try_from(max_entries).unwrap_or(i64::MAX);

        if excess > 0 {
            guard.execute(
                "DELETE FROM query_history WHERE id IN \
                 (SELECT id FROM query_history ORDER BY id ASC LIMIT ?1)",
                params![excess],
            )?;
        }
        Ok(())
    }

    /// Load the most recent `limit` entries in chronological order (oldest
    /// first). Returns an empty `Vec` on any failure — the feature degrades
    /// gracefully if the DB is unavailable.
    #[must_use]
    pub(crate) fn load_recent(&self, limit: usize) -> Vec<String> {
        let Ok(guard) = self.lock() else {
            return Vec::new();
        };
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        // Select id + query for the most-recent `limit` rows, then re-sort
        // ascending so callers get oldest-first chronological order.
        let Ok(mut stmt) = guard.prepare_cached(
            "SELECT query FROM \
             (SELECT id, query FROM query_history ORDER BY id DESC LIMIT ?1) \
             ORDER BY id ASC",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map(params![limit_i64], |row| row.get::<_, String>(0));

        match rows {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, rusqlite::Error> {
        self.inner.lock().map_err(|err| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
                Some(format!("query_history db mutex poisoned: {err}")),
            )
        })
    }
}

/// Resolve the on-disk path for the query-history DB. Returns `None` if
/// the platform data directory can't be resolved.
#[must_use]
pub(crate) fn default_db_path() -> Option<PathBuf> {
    crate::paths::data_dir().map(|d| d.join("query_history.sqlite"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_creates_schema() {
        let db = QueryHistoryDb::open_in_memory().expect("open");
        let entries = db.load_recent(100);
        assert!(entries.is_empty());
    }

    #[test]
    fn push_single_entry_appears_in_load() {
        let db = QueryHistoryDb::open_in_memory().expect("open");
        db.push("hello", 100).unwrap();
        let entries = db.load_recent(100);
        assert_eq!(entries, vec!["hello"]);
    }

    #[test]
    fn push_deduplicates_consecutive_identical() {
        let db = QueryHistoryDb::open_in_memory().expect("open");
        db.push("same", 100).unwrap();
        db.push("same", 100).unwrap();
        db.push("same", 100).unwrap();
        let entries = db.load_recent(100);
        assert_eq!(entries, vec!["same"]);
    }

    #[test]
    fn push_allows_non_consecutive_duplicates() {
        let db = QueryHistoryDb::open_in_memory().expect("open");
        db.push("a", 100).unwrap();
        db.push("b", 100).unwrap();
        db.push("a", 100).unwrap();
        let entries = db.load_recent(100);
        assert_eq!(entries, vec!["a", "b", "a"]);
    }

    #[test]
    fn load_recent_returns_chronological_order() {
        let db = QueryHistoryDb::open_in_memory().expect("open");

        for q in &["first", "second", "third"] {
            db.push(q, 100).unwrap();
        }
        let entries = db.load_recent(100);
        assert_eq!(entries, vec!["first", "second", "third"]);
    }

    #[test]
    fn push_trims_to_max_entries() {
        let db = QueryHistoryDb::open_in_memory().expect("open");

        for i in 0..5u32 {
            db.push(&format!("query-{i}"), 3).unwrap();
        }
        let entries = db.load_recent(100);
        // The 3 most recent are kept; oldest two are pruned.
        assert_eq!(entries.len(), 3);
        assert_eq!(entries, vec!["query-2", "query-3", "query-4"]);
    }

    #[test]
    fn load_recent_respects_limit() {
        let db = QueryHistoryDb::open_in_memory().expect("open");

        for i in 0..5u32 {
            db.push(&format!("q{i}"), 100).unwrap();
        }
        let entries = db.load_recent(3);
        // Most recent 3 in chronological order.
        assert_eq!(entries, vec!["q2", "q3", "q4"]);
    }
}
