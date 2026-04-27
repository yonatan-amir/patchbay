pub struct Migration {
    pub version: u32,
    pub sql: &'static str,
}

pub const ALL: &[Migration] = &[
    Migration {
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
},
Migration {
    version: 2,
    sql: "
        ALTER TABLE plugins ADD COLUMN class_id TEXT;
        ALTER TABLE plugins ADD COLUMN category TEXT;
        CREATE UNIQUE INDEX idx_plugins_path_unique ON plugins(path);
    ",
},
Migration {
    version: 3,
    sql: "
        -- Plugin documentation attachments (local file or remote URL)
        CREATE TABLE plugin_manuals (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            plugin_id   INTEGER NOT NULL REFERENCES plugins(id) ON DELETE CASCADE,
            source      TEXT    NOT NULL CHECK (source IN ('local', 'url')),
            path_or_url TEXT    NOT NULL,
            uploaded_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );

        -- Freeform user notes — one body per plugin, upserted in place
        CREATE TABLE plugin_notes (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            plugin_id  INTEGER NOT NULL UNIQUE REFERENCES plugins(id) ON DELETE CASCADE,
            body       TEXT    NOT NULL DEFAULT '',
            updated_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );

        -- Stem type catalog used when rendering chain audio previews
        CREATE TABLE chain_stems (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT    NOT NULL UNIQUE,
            description TEXT,
            created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );

        -- One rendered audio file per chain + stem combination
        CREATE TABLE chain_previews (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            chain_id    INTEGER NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
            stem_id     INTEGER NOT NULL REFERENCES chain_stems(id),
            audio_path  TEXT    NOT NULL,
            rendered_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            UNIQUE(chain_id, stem_id)
        );

        -- Local mirror of Phase 3 marketplace comments
        CREATE TABLE chain_comments (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            sync_id    TEXT    NOT NULL UNIQUE,
            chain_id   INTEGER NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
            author_id  TEXT    NOT NULL,
            body       TEXT    NOT NULL,
            created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            updated_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );

        -- Local mirror of Phase 3 marketplace likes (one per user per chain)
        CREATE TABLE chain_likes (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            sync_id    TEXT    NOT NULL UNIQUE,
            chain_id   INTEGER NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
            user_id    TEXT    NOT NULL,
            created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            UNIQUE(chain_id, user_id)
        );

        -- Local mirror of Phase 3 marketplace fork relationships
        CREATE TABLE chain_forks (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            sync_id         TEXT    NOT NULL UNIQUE,
            source_chain_id INTEGER NOT NULL REFERENCES chains(id),
            forked_chain_id INTEGER NOT NULL REFERENCES chains(id),
            created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            UNIQUE(source_chain_id, forked_chain_id)
        );

        CREATE INDEX idx_plugin_manuals_plugin_id ON plugin_manuals(plugin_id);
        CREATE INDEX idx_chain_previews_chain_id  ON chain_previews(chain_id);
        CREATE INDEX idx_chain_comments_chain_id  ON chain_comments(chain_id);
        CREATE INDEX idx_chain_likes_chain_id     ON chain_likes(chain_id);
        CREATE INDEX idx_chain_forks_source       ON chain_forks(source_chain_id);
        CREATE INDEX idx_chain_forks_forked       ON chain_forks(forked_chain_id);
    ",
}];
