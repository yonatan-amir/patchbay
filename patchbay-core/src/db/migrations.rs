pub struct Migration {
    pub version: u32,
    pub sql: &'static str,
}

pub const ALL: &[Migration] = &[Migration {
    version: 1,
    sql: "
        CREATE TABLE plugins (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            sync_id      TEXT    NOT NULL UNIQUE,
            name         TEXT    NOT NULL,
            vendor       TEXT,
            format       TEXT    NOT NULL,
            path         TEXT    NOT NULL,
            version      TEXT,
            installed_at TEXT,
            device_id    TEXT    NOT NULL,
            created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            updated_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );

        CREATE TABLE presets (
            id                 INTEGER PRIMARY KEY AUTOINCREMENT,
            sync_id            TEXT    NOT NULL UNIQUE,
            plugin_id          INTEGER NOT NULL REFERENCES plugins(id) ON DELETE CASCADE,
            name               TEXT    NOT NULL,
            path               TEXT,
            format             TEXT,
            tags               TEXT,
            timbral_brightness REAL,
            timbral_warmth     REAL,
            timbral_attack     REAL,
            key                TEXT,
            bpm                REAL,
            device_id          TEXT    NOT NULL,
            created_at         TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            updated_at         TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );

        CREATE TABLE chains (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            sync_id     TEXT    NOT NULL UNIQUE,
            name        TEXT    NOT NULL,
            daw         TEXT    NOT NULL,
            description TEXT,
            device_id   TEXT    NOT NULL,
            created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            updated_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );

        CREATE TABLE chain_slots (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            chain_id  INTEGER NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
            plugin_id INTEGER NOT NULL REFERENCES plugins(id),
            position  INTEGER NOT NULL,
            params    TEXT,
            UNIQUE(chain_id, position)
        );

        CREATE TABLE scan_sessions (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            sync_id       TEXT    NOT NULL UNIQUE,
            started_at    TEXT    NOT NULL,
            completed_at  TEXT,
            status        TEXT    NOT NULL DEFAULT 'running',
            plugins_found INTEGER NOT NULL DEFAULT 0,
            presets_found INTEGER NOT NULL DEFAULT 0,
            device_id     TEXT    NOT NULL,
            created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            updated_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );

        -- FTS5 search index (denormalized: includes plugin name + vendor for one-box search)
        CREATE VIRTUAL TABLE presets_fts USING fts5(
            name,
            tags,
            plugin_name,
            vendor,
            tokenize = 'unicode61 remove_diacritics 1'
        );

        CREATE TRIGGER presets_ai AFTER INSERT ON presets BEGIN
            INSERT INTO presets_fts (rowid, name, tags, plugin_name, vendor)
            SELECT NEW.id, NEW.name, COALESCE(NEW.tags, ''),
                   p.name, COALESCE(p.vendor, '')
            FROM plugins p WHERE p.id = NEW.plugin_id;
        END;

        CREATE TRIGGER presets_ad AFTER DELETE ON presets BEGIN
            INSERT INTO presets_fts (presets_fts, rowid, name, tags, plugin_name, vendor)
            VALUES (
                'delete', OLD.id, OLD.name, COALESCE(OLD.tags, ''),
                (SELECT name FROM plugins WHERE id = OLD.plugin_id),
                COALESCE((SELECT vendor FROM plugins WHERE id = OLD.plugin_id), '')
            );
        END;

        CREATE TRIGGER presets_au AFTER UPDATE ON presets BEGIN
            INSERT INTO presets_fts (presets_fts, rowid, name, tags, plugin_name, vendor)
            VALUES (
                'delete', OLD.id, OLD.name, COALESCE(OLD.tags, ''),
                (SELECT name FROM plugins WHERE id = OLD.plugin_id),
                COALESCE((SELECT vendor FROM plugins WHERE id = OLD.plugin_id), '')
            );
            INSERT INTO presets_fts (rowid, name, tags, plugin_name, vendor)
            SELECT NEW.id, NEW.name, COALESCE(NEW.tags, ''),
                   p.name, COALESCE(p.vendor, '')
            FROM plugins p WHERE p.id = NEW.plugin_id;
        END;

        CREATE INDEX idx_presets_plugin_id    ON presets(plugin_id);
        CREATE INDEX idx_plugins_format       ON plugins(format);
        CREATE INDEX idx_plugins_path         ON plugins(path);
        CREATE INDEX idx_chain_slots_chain_id ON chain_slots(chain_id);
        CREATE INDEX idx_scan_sessions_status ON scan_sessions(status);
    ",
}];
