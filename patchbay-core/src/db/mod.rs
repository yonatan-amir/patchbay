mod migrations;

use std::collections::HashMap;
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
    pub file_mtime: Option<i64>,
}

pub struct PluginRow {
    pub name: String,
    pub vendor: Option<String>,
    pub format: String,
    pub category: Option<String>,
}

pub struct PluginFormatInstance {
    pub format: String,
    pub path: String,
    pub version: Option<String>,
}

pub struct PluginManual {
    pub id: i64,
    pub source: String,
    pub path_or_url: String,
}

pub struct PluginDetail {
    pub id: i64,
    pub name: String,
    pub vendor: Option<String>,
    pub category: Option<String>,
    pub instances: Vec<PluginFormatInstance>,
    pub note: String,
    pub manuals: Vec<PluginManual>,
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
                 (sync_id, name, vendor, format, path, version, class_id, category, device_id, file_mtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(path, class_id) DO UPDATE SET
                 name       = excluded.name,
                 vendor     = excluded.vendor,
                 version    = excluded.version,
                 category   = excluded.category,
                 file_mtime = excluded.file_mtime,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
            rusqlite::params![
                p.sync_id, p.name, p.vendor, p.format, p.path,
                p.version, p.class_id, p.category, p.device_id, p.file_mtime
            ],
        )?;
        Ok(())
    }

    /// Return all plugins indexed for this device, sorted by format then name.
    pub fn list_plugins(&self, device_id: &str) -> Result<Vec<PluginRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT name, vendor, format, category FROM plugins
             WHERE device_id = ?1
             ORDER BY format, name COLLATE NOCASE",
        )?;
        let rows = stmt
            .query_map([device_id], |row| {
                Ok(PluginRow {
                    name: row.get(0)?,
                    vendor: row.get(1)?,
                    format: row.get(2)?,
                    category: row.get(3)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Return all format instances for a plugin name, plus user note and manuals.
    /// Groups every row with matching name (VST3 + AU + VST2 may all exist).
    /// The primary row (lowest id) is used as the anchor for notes and manuals.
    pub fn get_plugin_detail(&self, name: &str, device_id: &str) -> Result<Option<PluginDetail>, DbError> {
        struct Row {
            id: i64,
            vendor: Option<String>,
            format: String,
            path: String,
            version: Option<String>,
            category: Option<String>,
        }

        let mut stmt = self.conn.prepare(
            "SELECT id, vendor, format, path, version, category
             FROM plugins
             WHERE name = ?1 AND device_id = ?2
             ORDER BY id ASC",
        )?;

        let rows: Vec<Row> = stmt
            .query_map(rusqlite::params![name, device_id], |row| {
                Ok(Row {
                    id: row.get(0)?,
                    vendor: row.get(1)?,
                    format: row.get(2)?,
                    path: row.get(3)?,
                    version: row.get(4)?,
                    category: row.get(5)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        if rows.is_empty() {
            return Ok(None);
        }

        let primary_id = rows[0].id;
        let vendor = rows[0].vendor.clone();
        let category = rows[0].category.clone();

        let instances = rows
            .into_iter()
            .map(|r| PluginFormatInstance {
                format: r.format,
                path: r.path,
                version: r.version,
            })
            .collect();

        // No row → empty note (QueryReturnedNoRows is expected when no note exists).
        let note: String = self
            .conn
            .query_row(
                "SELECT body FROM plugin_notes WHERE plugin_id = ?1",
                [primary_id],
                |row| row.get(0),
            )
            .unwrap_or_default();

        let mut manual_stmt = self.conn.prepare(
            "SELECT id, source, path_or_url FROM plugin_manuals
             WHERE plugin_id = ?1
             ORDER BY id",
        )?;
        let manuals: Vec<PluginManual> = manual_stmt
            .query_map([primary_id], |row| {
                Ok(PluginManual {
                    id: row.get(0)?,
                    source: row.get(1)?,
                    path_or_url: row.get(2)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(Some(PluginDetail {
            id: primary_id,
            name: name.to_string(),
            vendor,
            category,
            instances,
            note,
            manuals,
        }))
    }

    /// Insert or replace the user note for a plugin row.
    pub fn upsert_plugin_note(&self, plugin_id: i64, body: &str) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO plugin_notes (plugin_id, body, updated_at)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(plugin_id) DO UPDATE SET
                 body       = excluded.body,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
            rusqlite::params![plugin_id, body],
        )?;
        Ok(())
    }

    /// Attach a manual reference (URL or local path) to a plugin row.
    /// Returns the new manual's row id.
    pub fn save_plugin_manual(&self, plugin_id: i64, source: &str, path_or_url: &str) -> Result<i64, DbError> {
        self.conn.execute(
            "INSERT INTO plugin_manuals (plugin_id, source, path_or_url) VALUES (?1, ?2, ?3)",
            rusqlite::params![plugin_id, source, path_or_url],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Remove a manual entry by its id.
    pub fn delete_plugin_manual(&self, manual_id: i64) -> Result<(), DbError> {
        self.conn.execute(
            "DELETE FROM plugin_manuals WHERE id = ?1",
            [manual_id],
        )?;
        Ok(())
    }

    /// Return a map of `path → file_mtime` for all indexed plugins on this device
    /// that have a recorded mtime. Used to skip unchanged bundles on re-scan.
    pub fn get_known_mtimes(&self, device_id: &str) -> Result<HashMap<String, i64>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT path, file_mtime FROM plugins WHERE device_id = ?1 AND file_mtime IS NOT NULL",
        )?;
        let map = stmt
            .query_map([device_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(map)
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

    #[test]
    fn get_known_mtimes_returns_only_nonnull() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_plugin(&PluginRecord {
            sync_id: "a".to_string(),
            name: "Plugin A".to_string(),
            vendor: None,
            format: "VST3".to_string(),
            path: "/path/a.vst3".to_string(),
            version: None,
            class_id: None,
            category: None,
            device_id: "dev1".to_string(),
            file_mtime: Some(1_000_000),
        }).unwrap();
        db.upsert_plugin(&PluginRecord {
            sync_id: "b".to_string(),
            name: "Plugin B".to_string(),
            vendor: None,
            format: "VST3".to_string(),
            path: "/path/b.vst3".to_string(),
            version: None,
            class_id: None,
            category: None,
            device_id: "dev1".to_string(),
            file_mtime: None,
        }).unwrap();

        let mtimes = db.get_known_mtimes("dev1").unwrap();
        assert_eq!(mtimes.len(), 1);
        assert_eq!(mtimes["/path/a.vst3"], 1_000_000);
    }

    #[test]
    fn get_known_mtimes_filters_by_device() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_plugin(&PluginRecord {
            sync_id: "a".to_string(),
            name: "Plugin A".to_string(),
            vendor: None,
            format: "VST3".to_string(),
            path: "/path/a.vst3".to_string(),
            version: None,
            class_id: None,
            category: None,
            device_id: "dev1".to_string(),
            file_mtime: Some(1_000_000),
        }).unwrap();

        let mtimes_dev1 = db.get_known_mtimes("dev1").unwrap();
        let mtimes_dev2 = db.get_known_mtimes("dev2").unwrap();
        assert_eq!(mtimes_dev1.len(), 1);
        assert_eq!(mtimes_dev2.len(), 0);
    }

    #[test]
    fn get_plugin_detail_groups_formats() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_plugin(&PluginRecord {
            sync_id: "vst3-1".to_string(),
            name: "Pro-Q 3".to_string(),
            vendor: Some("FabFilter".to_string()),
            format: "VST3".to_string(),
            path: "/plugins/Pro-Q 3.vst3".to_string(),
            version: Some("3.56".to_string()),
            class_id: None,
            category: Some("EQ".to_string()),
            device_id: "dev1".to_string(),
            file_mtime: None,
        }).unwrap();
        db.upsert_plugin(&PluginRecord {
            sync_id: "au-1".to_string(),
            name: "Pro-Q 3".to_string(),
            vendor: Some("FabFilter".to_string()),
            format: "AU".to_string(),
            path: "/plugins/Pro-Q 3.component".to_string(),
            version: Some("3.56".to_string()),
            class_id: Some("FabF:PrQ3:au  :".to_string()),
            category: Some("EQ".to_string()),
            device_id: "dev1".to_string(),
            file_mtime: None,
        }).unwrap();

        let detail = db.get_plugin_detail("Pro-Q 3", "dev1").unwrap().unwrap();
        assert_eq!(detail.name, "Pro-Q 3");
        assert_eq!(detail.vendor.as_deref(), Some("FabFilter"));
        assert_eq!(detail.instances.len(), 2);
        assert!(detail.instances.iter().any(|i| i.format == "VST3"));
        assert!(detail.instances.iter().any(|i| i.format == "AU"));
        assert_eq!(detail.note, "");
    }

    #[test]
    fn upsert_plugin_note_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_plugin(&PluginRecord {
            sync_id: "x".to_string(),
            name: "TestPlugin".to_string(),
            vendor: None,
            format: "VST3".to_string(),
            path: "/p/x.vst3".to_string(),
            version: None,
            class_id: None,
            category: None,
            device_id: "dev1".to_string(),
            file_mtime: None,
        }).unwrap();

        let detail = db.get_plugin_detail("TestPlugin", "dev1").unwrap().unwrap();
        db.upsert_plugin_note(detail.id, "Great reverb tail").unwrap();

        let detail2 = db.get_plugin_detail("TestPlugin", "dev1").unwrap().unwrap();
        assert_eq!(detail2.note, "Great reverb tail");

        // overwrite
        db.upsert_plugin_note(detail.id, "Updated note").unwrap();
        let detail3 = db.get_plugin_detail("TestPlugin", "dev1").unwrap().unwrap();
        assert_eq!(detail3.note, "Updated note");
    }

    #[test]
    fn save_and_delete_plugin_manual() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_plugin(&PluginRecord {
            sync_id: "y".to_string(),
            name: "TestPlugin".to_string(),
            vendor: None,
            format: "VST3".to_string(),
            path: "/p/y.vst3".to_string(),
            version: None,
            class_id: None,
            category: None,
            device_id: "dev1".to_string(),
            file_mtime: None,
        }).unwrap();

        let detail = db.get_plugin_detail("TestPlugin", "dev1").unwrap().unwrap();
        let mid = db.save_plugin_manual(detail.id, "url", "https://example.com/manual.pdf").unwrap();

        let detail2 = db.get_plugin_detail("TestPlugin", "dev1").unwrap().unwrap();
        assert_eq!(detail2.manuals.len(), 1);
        assert_eq!(detail2.manuals[0].source, "url");

        db.delete_plugin_manual(mid).unwrap();
        let detail3 = db.get_plugin_detail("TestPlugin", "dev1").unwrap().unwrap();
        assert!(detail3.manuals.is_empty());
    }
}
