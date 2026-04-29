mod migrations;

use std::collections::HashMap;
use std::path::Path;
use rusqlite::Connection;
use serde::Serialize;
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

pub struct PluginDetail {
    pub id: i64,
    pub name: String,
    pub vendor: Option<String>,
    pub category: Option<String>,
    pub instances: Vec<PluginFormatInstance>,
    pub note: String,
}

#[derive(Serialize)]
pub struct DossierInstance {
    pub format: String,
    pub path: String,
    pub version: Option<String>,
}

#[derive(Serialize)]
pub struct DossierPlugin {
    pub name: String,
    pub vendor: Option<String>,
    pub category: Option<String>,
    pub formats: Vec<String>,
    pub instances: Vec<DossierInstance>,
    pub note: Option<String>,
    pub first_seen: String,
}

// ── Chain types ─────────────────────────────────────────────────────────────

pub struct ChainRecord {
    pub sync_id: String,
    pub name: String,
    pub daw: String,
    pub source_track: Option<String>,
    pub notes: Option<String>,
    pub tags: Option<String>,
    pub device_id: String,
}

pub struct ChainSlotRecord {
    pub plugin_id: Option<i64>,
    pub plugin_identity: String,
    pub position: i32,
    pub bypass: bool,
    pub wet: f64,
    pub preset_name: Option<String>,
    pub opaque_state: Option<String>,
}

#[derive(Serialize)]
pub struct ChainRow {
    pub id: i64,
    pub sync_id: String,
    pub name: String,
    pub daw: String,
    pub tags: Option<String>,
    pub source_track: Option<String>,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct ChainSlotRow {
    pub id: i64,
    pub plugin_id: Option<i64>,
    pub plugin_identity: String,
    pub position: i32,
    pub bypass: bool,
    pub wet: f64,
    pub preset_name: Option<String>,
    pub opaque_state: Option<String>,
}

#[derive(Serialize)]
pub struct ChainDetail {
    pub id: i64,
    pub sync_id: String,
    pub name: String,
    pub daw: String,
    pub source_track: Option<String>,
    pub notes: Option<String>,
    pub tags: Option<String>,
    pub created_at: String,
    pub slots: Vec<ChainSlotRow>,
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

    /// Return all format instances for a plugin name, plus user note.
    /// Groups every row with matching name (VST3 + AU + VST2 may all exist).
    /// The primary row (lowest id) is used as the anchor for the note.
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

        Ok(Some(PluginDetail {
            id: primary_id,
            name: name.to_string(),
            vendor,
            category,
            instances,
            note,
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

    /// Return all plugins for this device grouped by name, with all format instances
    /// and user note. Used to generate the library dossier export.
    pub fn export_dossier(&self, device_id: &str) -> Result<Vec<DossierPlugin>, DbError> {
        struct Row {
            name: String,
            vendor: Option<String>,
            category: Option<String>,
            format: String,
            path: String,
            version: Option<String>,
            created_at: String,
            note: Option<String>,
        }

        let mut stmt = self.conn.prepare(
            "SELECT p.name, p.vendor, p.category, p.format, p.path, p.version, p.created_at, pn.body
             FROM plugins p
             LEFT JOIN plugin_notes pn ON pn.plugin_id = p.id
             WHERE p.device_id = ?1
             ORDER BY p.name COLLATE NOCASE, p.id ASC",
        )?;

        let rows: Vec<Row> = stmt
            .query_map([device_id], |row| {
                Ok(Row {
                    name: row.get(0)?,
                    vendor: row.get(1)?,
                    category: row.get(2)?,
                    format: row.get(3)?,
                    path: row.get(4)?,
                    version: row.get(5)?,
                    created_at: row.get(6)?,
                    note: row.get(7)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut plugins: Vec<DossierPlugin> = Vec::new();
        for row in rows {
            if let Some(last) = plugins.last_mut() {
                if last.name == row.name {
                    if !last.formats.contains(&row.format) {
                        last.formats.push(row.format.clone());
                    }
                    last.instances.push(DossierInstance {
                        format: row.format,
                        path: row.path,
                        version: row.version,
                    });
                    if last.note.is_none() {
                        last.note = row.note;
                    }
                    if row.created_at < last.first_seen {
                        last.first_seen = row.created_at;
                    }
                    continue;
                }
            }
            plugins.push(DossierPlugin {
                formats: vec![row.format.clone()],
                instances: vec![DossierInstance {
                    format: row.format,
                    path: row.path,
                    version: row.version,
                }],
                note: row.note,
                first_seen: row.created_at,
                name: row.name,
                vendor: row.vendor,
                category: row.category,
            });
        }

        Ok(plugins)
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

    // ── Chain CRUD ───────────────────────────────────────────────────────────

    /// Upsert a chain (keyed on sync_id) and replace all its slots atomically.
    /// Returns the chain's row id.
    pub fn save_chain(&self, chain: &ChainRecord, slots: &[ChainSlotRecord]) -> Result<i64, DbError> {
        self.conn.execute_batch("BEGIN")?;

        let result = (|| -> Result<i64, DbError> {
            self.conn.execute(
                "INSERT INTO chains (sync_id, name, daw, source_track, notes, tags, device_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(sync_id) DO UPDATE SET
                     name         = excluded.name,
                     daw          = excluded.daw,
                     source_track = excluded.source_track,
                     notes        = excluded.notes,
                     tags         = excluded.tags,
                     updated_at   = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
                rusqlite::params![
                    chain.sync_id, chain.name, chain.daw,
                    chain.source_track, chain.notes, chain.tags, chain.device_id
                ],
            )?;
            // last_insert_rowid() is unreliable for ON CONFLICT DO UPDATE — query explicitly.
            let chain_id: i64 = self.conn.query_row(
                "SELECT id FROM chains WHERE sync_id = ?1",
                [&chain.sync_id],
                |row| row.get(0),
            )?;

            self.conn.execute(
                "DELETE FROM chain_slots WHERE chain_id = ?1",
                [chain_id],
            )?;

            for slot in slots {
                self.conn.execute(
                    "INSERT INTO chain_slots
                         (chain_id, plugin_id, plugin_identity, position,
                          bypass, wet, preset_name, opaque_state)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        chain_id,
                        slot.plugin_id,
                        slot.plugin_identity,
                        slot.position,
                        slot.bypass as i32,
                        slot.wet,
                        slot.preset_name,
                        slot.opaque_state,
                    ],
                )?;
            }

            Ok(chain_id)
        })();

        match result {
            Ok(id) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(id)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    pub fn list_chains(&self, device_id: &str) -> Result<Vec<ChainRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, sync_id, name, daw, tags, source_track, created_at
             FROM chains
             WHERE device_id = ?1
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map([device_id], |row| {
                Ok(ChainRow {
                    id: row.get(0)?,
                    sync_id: row.get(1)?,
                    name: row.get(2)?,
                    daw: row.get(3)?,
                    tags: row.get(4)?,
                    source_track: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn get_chain(&self, chain_id: i64) -> Result<Option<ChainDetail>, DbError> {
        let chain = self.conn.query_row(
            "SELECT id, sync_id, name, daw, source_track, notes, tags, created_at
             FROM chains WHERE id = ?1",
            [chain_id],
            |row| {
                Ok(ChainDetail {
                    id: row.get(0)?,
                    sync_id: row.get(1)?,
                    name: row.get(2)?,
                    daw: row.get(3)?,
                    source_track: row.get(4)?,
                    notes: row.get(5)?,
                    tags: row.get(6)?,
                    created_at: row.get(7)?,
                    slots: Vec::new(),
                })
            },
        );

        let mut chain = match chain {
            Ok(c) => c,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let mut stmt = self.conn.prepare(
            "SELECT id, plugin_id, plugin_identity, position, bypass, wet, preset_name, opaque_state
             FROM chain_slots WHERE chain_id = ?1 ORDER BY position ASC",
        )?;

        chain.slots = stmt
            .query_map([chain_id], |row| {
                Ok(ChainSlotRow {
                    id: row.get(0)?,
                    plugin_id: row.get(1)?,
                    plugin_identity: row.get(2)?,
                    position: row.get(3)?,
                    bypass: row.get::<_, i32>(4)? != 0,
                    wet: row.get(5)?,
                    preset_name: row.get(6)?,
                    opaque_state: row.get(7)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(Some(chain))
    }

    pub fn delete_chain(&self, chain_id: i64) -> Result<(), DbError> {
        self.conn.execute("DELETE FROM chains WHERE id = ?1", [chain_id])?;
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

    fn make_chain(sync_id: &str, device_id: &str) -> ChainRecord {
        ChainRecord {
            sync_id: sync_id.to_string(),
            name: "Test Chain".to_string(),
            daw: "Ableton".to_string(),
            source_track: Some("Kick".to_string()),
            notes: Some("Punchy".to_string()),
            tags: Some("drums,kick".to_string()),
            device_id: device_id.to_string(),
        }
    }

    fn make_slots() -> Vec<ChainSlotRecord> {
        vec![
            ChainSlotRecord {
                plugin_id: None,
                plugin_identity: r#"{"name":"Pro-Q 3","vendor":"FabFilter","format":"VST3"}"#.to_string(),
                position: 0,
                bypass: false,
                wet: 1.0,
                preset_name: Some("Tight Low Cut".to_string()),
                opaque_state: Some("abc123".to_string()),
            },
            ChainSlotRecord {
                plugin_id: None,
                plugin_identity: r#"{"name":"Pro-C 2","vendor":"FabFilter","format":"VST3"}"#.to_string(),
                position: 1,
                bypass: true,
                wet: 0.5,
                preset_name: None,
                opaque_state: None,
            },
        ]
    }

    #[test]
    fn save_and_load_chain_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        let chain_id = db.save_chain(&make_chain("c1", "dev1"), &make_slots()).unwrap();

        let detail = db.get_chain(chain_id).unwrap().unwrap();
        assert_eq!(detail.name, "Test Chain");
        assert_eq!(detail.daw, "Ableton");
        assert_eq!(detail.source_track.as_deref(), Some("Kick"));
        assert_eq!(detail.notes.as_deref(), Some("Punchy"));
        assert_eq!(detail.tags.as_deref(), Some("drums,kick"));
        assert_eq!(detail.slots.len(), 2);

        let s0 = &detail.slots[0];
        assert_eq!(s0.position, 0);
        assert!(!s0.bypass);
        assert_eq!(s0.wet, 1.0);
        assert_eq!(s0.preset_name.as_deref(), Some("Tight Low Cut"));
        assert_eq!(s0.opaque_state.as_deref(), Some("abc123"));

        let s1 = &detail.slots[1];
        assert_eq!(s1.position, 1);
        assert!(s1.bypass);
        assert_eq!(s1.wet, 0.5);
        assert!(s1.preset_name.is_none());
        assert!(s1.opaque_state.is_none());
    }

    #[test]
    fn list_chains_device_isolation() {
        let db = Database::open_in_memory().unwrap();
        db.save_chain(&make_chain("c1", "dev1"), &[]).unwrap();
        db.save_chain(&make_chain("c2", "dev2"), &[]).unwrap();

        let dev1 = db.list_chains("dev1").unwrap();
        let dev2 = db.list_chains("dev2").unwrap();
        assert_eq!(dev1.len(), 1);
        assert_eq!(dev2.len(), 1);
        assert_eq!(dev1[0].sync_id, "c1");
        assert_eq!(dev2[0].sync_id, "c2");
    }

    #[test]
    fn delete_chain_cascades_slots() {
        let db = Database::open_in_memory().unwrap();
        let chain_id = db.save_chain(&make_chain("c1", "dev1"), &make_slots()).unwrap();

        db.delete_chain(chain_id).unwrap();
        assert!(db.get_chain(chain_id).unwrap().is_none());

        let slot_count: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM chain_slots WHERE chain_id = ?1",
            [chain_id],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(slot_count, 0);
    }

    #[test]
    fn save_chain_upsert_replaces_slots() {
        let db = Database::open_in_memory().unwrap();
        let chain_id = db.save_chain(&make_chain("c1", "dev1"), &make_slots()).unwrap();

        let new_slots = vec![ChainSlotRecord {
            plugin_id: None,
            plugin_identity: r#"{"name":"Limiter","vendor":"FabFilter","format":"VST3"}"#.to_string(),
            position: 0,
            bypass: false,
            wet: 1.0,
            preset_name: None,
            opaque_state: None,
        }];
        let same_id = db.save_chain(&make_chain("c1", "dev1"), &new_slots).unwrap();

        assert_eq!(chain_id, same_id);
        let detail = db.get_chain(chain_id).unwrap().unwrap();
        assert_eq!(detail.slots.len(), 1);
        assert!(detail.slots[0].plugin_identity.contains("Limiter"));
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

}
