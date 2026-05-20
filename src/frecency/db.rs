//! `SQLite` storage for the `picks` frecency table.
//!
//! `CREATE TABLE IF NOT EXISTS` is the whole migration system — no history
//! to evolve yet. TODO: switch to `rusqlite_migration` the first time the
//! schema actually changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{Connection, params};

use crate::paths::ensure_parent_dir;

// Composite PRIMARY KEY generates an implicit unique index used for both
// lookups and upserts — no additional index needed.
const SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS picks (
    plugin_name      TEXT    NOT NULL,
    result_key       TEXT    NOT NULL,
    picks            INTEGER NOT NULL DEFAULT 1,
    last_picked_at   INTEGER NOT NULL,
    PRIMARY KEY (plugin_name, result_key)
);
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PickRow {
    pub(crate) picks: u32,
    pub(crate) last_picked_at: i64,
}

type ResultId = (String, String);

/// Owned handle to the frecency database. `Arc<Mutex>` is enough — picks
/// happen at most a few times per second from one thread.
#[derive(Clone)]
pub(crate) struct FrecencyDb {
    inner: Arc<Mutex<Connection>>,
}

impl FrecencyDb {
    /// Open (or create) the database at `path`. Creates the parent directory
    /// if missing. Initializes the schema if it isn't there yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the file/parent can't be created or `SQLite` can't
    /// open or initialize the schema. Callers are expected to treat a failure
    /// as "continue without frecency" rather than aborting the daemon.
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

    /// Read every `(plugin_name, result_key) -> (picks, last_picked_at)` row
    /// into a map. SQL/lock failure returns an empty map (daemon stays usable
    /// with default ranking).
    #[must_use]
    pub(crate) fn snapshot(&self) -> Snapshot {
        let guard = match self.inner.lock() {
            Ok(g) => g,
            Err(err) => {
                tracing::error!(%err, "frecency: snapshot lock poisoned");
                return Snapshot::default();
            }
        };
        let mut stmt = match guard
            .prepare_cached("SELECT plugin_name, result_key, picks, last_picked_at FROM picks")
        {
            Ok(s) => s,
            Err(err) => {
                tracing::error!(%err, "frecency: snapshot prepare failed");
                return Snapshot::default();
            }
        };
        let rows = stmt.query_map([], |row| {
            let plugin_name: String = row.get(0)?;
            let result_key: String = row.get(1)?;
            let picks: u32 = row.get(2)?;
            let last_picked_at: i64 = row.get(3)?;
            Ok((
                (plugin_name, result_key),
                PickRow {
                    picks,
                    last_picked_at,
                },
            ))
        });
        let mut map = HashMap::new();
        match rows {
            Ok(iter) => {
                for row in iter {
                    match row {
                        Ok((id, pick)) => {
                            map.insert(id, pick);
                        }
                        Err(err) => tracing::warn!(%err, "frecency: row decode failed"),
                    }
                }
            }
            Err(err) => tracing::error!(%err, "frecency: snapshot query failed"),
        }
        Snapshot { picks: map }
    }

    /// Insert or bump `(plugin_name, result_key)`. Stamp uses the supplied
    /// `now` (seconds since epoch) so tests can be deterministic; production
    /// callers pass [`now_seconds`].
    ///
    /// # Errors
    ///
    /// Returns a SQL error if the upsert can't run (locked db, etc.).
    pub(crate) fn bump_with_now(
        &self,
        plugin_name: &str,
        result_key: &str,
        now: i64,
    ) -> rusqlite::Result<()> {
        let guard = self
            .inner
            .lock()
            .map_err(|err| rusqlite::Error::ToSqlConversionFailure(format!("{err}").into()))?;
        guard.execute(
            "INSERT INTO picks (plugin_name, result_key, picks, last_picked_at)
             VALUES (?1, ?2, 1, ?3)
             ON CONFLICT (plugin_name, result_key)
             DO UPDATE SET picks = picks + 1, last_picked_at = excluded.last_picked_at",
            params![plugin_name, result_key, now],
        )?;
        Ok(())
    }

    /// Convenience wrapper using the system clock.
    ///
    /// # Errors
    ///
    /// Returns the same SQL error set as [`Self::bump_with_now`].
    pub(crate) fn bump(&self, plugin_name: &str, result_key: &str) -> rusqlite::Result<()> {
        self.bump_with_now(plugin_name, result_key, now_seconds())
    }

    /// Read one row for assertion in tests.
    #[cfg(test)]
    fn get(&self, plugin_name: &str, result_key: &str) -> Option<PickRow> {
        let guard = self.inner.lock().ok()?;
        guard
            .query_row(
                "SELECT picks, last_picked_at FROM picks WHERE plugin_name = ?1 AND result_key = ?2",
                params![plugin_name, result_key],
                |row| {
                    Ok(PickRow {
                        picks: row.get(0)?,
                        last_picked_at: row.get(1)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }
}

/// Wall-clock seconds since Unix epoch. Saturates to 0 on pre-epoch clocks
/// rather than panicking.
#[must_use]
pub(crate) fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Per-query snapshot of the picks table — owned `HashMap` so the dispatcher
/// can pass it around without juggling lifetimes against the DB connection.
#[derive(Debug, Default, Clone)]
pub(crate) struct Snapshot {
    picks: HashMap<ResultId, PickRow>,
}

impl Snapshot {
    #[must_use]
    pub(crate) fn get(&self, plugin_name: &str, result_key: &str) -> Option<PickRow> {
        self.picks
            .get(&(plugin_name.to_owned(), result_key.to_owned()))
            .copied()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn from_rows(rows: Vec<(String, String, PickRow)>) -> Self {
        let picks = rows.into_iter().map(|(p, k, r)| ((p, k), r)).collect();
        Self { picks }
    }
}

/// Resolve the on-disk path for the frecency DB. Returns `None` if
/// `ProjectDirs` won't resolve.
#[must_use]
pub(crate) fn default_db_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "high-beam")?;
    Some(dirs.data_dir().join("frecency.sqlite"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_creates_schema() {
        let db = FrecencyDb::open_in_memory().expect("open");
        let snap = db.snapshot();
        assert!(snap.get("anything", "at-all").is_none());
    }

    #[test]
    fn first_bump_inserts_row_with_picks_one() {
        let db = FrecencyDb::open_in_memory().expect("open");
        db.bump_with_now("demo", "alpha", 1_700_000_000).unwrap();
        let row = db.get("demo", "alpha").expect("row exists");
        assert_eq!(row.picks, 1);
        assert_eq!(row.last_picked_at, 1_700_000_000);
    }

    #[test]
    fn second_bump_increments_picks_and_updates_timestamp() {
        let db = FrecencyDb::open_in_memory().expect("open");
        db.bump_with_now("demo", "alpha", 1_700_000_000).unwrap();
        db.bump_with_now("demo", "alpha", 1_700_000_100).unwrap();
        let row = db.get("demo", "alpha").expect("row exists");
        assert_eq!(row.picks, 2);
        assert_eq!(row.last_picked_at, 1_700_000_100);
    }

    #[test]
    fn bumps_for_different_keys_dont_collide() {
        let db = FrecencyDb::open_in_memory().expect("open");
        db.bump_with_now("demo", "alpha", 1_700_000_000).unwrap();
        db.bump_with_now("demo", "beta", 1_700_000_100).unwrap();
        db.bump_with_now("other-plugin", "alpha", 1_700_000_200)
            .unwrap();
        let snap = db.snapshot();
        assert_eq!(snap.get("demo", "alpha").unwrap().picks, 1);
        assert_eq!(snap.get("demo", "beta").unwrap().picks, 1);
        assert_eq!(snap.get("other-plugin", "alpha").unwrap().picks, 1);
    }

    #[test]
    fn snapshot_reflects_inserts() {
        let db = FrecencyDb::open_in_memory().expect("open");
        db.bump_with_now("p", "k", 42).unwrap();
        db.bump_with_now("p", "k", 43).unwrap();
        let snap = db.snapshot();
        let row = snap.get("p", "k").expect("present");
        assert_eq!(row.picks, 2);
        assert_eq!(row.last_picked_at, 43);
    }

    #[test]
    fn snapshot_from_rows_round_trip() {
        let snap = Snapshot::from_rows(vec![(
            "x".into(),
            "y".into(),
            PickRow {
                picks: 5,
                last_picked_at: 99,
            },
        )]);
        let row = snap.get("x", "y").expect("present");
        assert_eq!(row.picks, 5);
        assert_eq!(row.last_picked_at, 99);
    }
}
