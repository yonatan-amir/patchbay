mod migrations;

use std::path::Path;
use rusqlite::Connection;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub struct Database {
    conn: Connection,
}

pub struct PluginRecord {
    pub sync_id: String,
    pub name: String,
    pub vendor: Option<String>,
    pub format: String,
    pub path: String,
    pub version: Option<String>,
    pub class_id: Option<String>,
    pub category: Option<String>,
    pub device_id: String,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )?;
        let db = Self { conn };
        db.run_migrations()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let db = Self { conn };
        db.run_migrations()?;
        Ok(db)
    }

    pub fn upsert_plugin(&self, p: &PluginRecord) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO plugins
                 (sync_id, name, vendor, format, path, version, class_id, category, device_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(path) DO UPDATE SET
                 name       = excluded.name,
                 vendor     = excluded.vendor,
                 version    = excluded.version,
                 class_id   = excluded.class_id,
                 category   = excluded.category,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
            rusqlite::params![
                p.sync_id, p.name, p.vendor, p.format, p.path,
                p.version, p.class_id, p.category, p.device_id
            ],
        )?;
        Ok(())
    }

    fn run_migrations(&self) -> Result<(), DbError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version    INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            );",
        )?;

        for m in migrations::ALL {
            let already_applied: bool = self.conn.query_row(
                "SELECT COUNT(*) > 0 FROM schema_migrations WHERE version = ?1",
                [m.version],
                |row| row.get(0),
            )?;

            if !already_applied {
                self.conn.execute_batch(m.sql)?;
                self.conn.execute(
                    "INSERT INTO schema_migrations (version) VALUES (?1)",
                    [m.version],
                )?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_run_cleanly() {
        Database::open_in_memory().expect("migrations failed");
    }

    #[test]
    fn migrations_are_idempotent() {
        let db = Database::open_in_memory().expect("first open failed");
        db.run_migrations().expect("second migration pass failed");
    }
}
