CREATE TABLE settings (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    primary_enabled INTEGER NOT NULL CHECK (primary_enabled IN (0, 1)),
    max_history_entries INTEGER NOT NULL CHECK (max_history_entries >= 1),
    max_menu_entries INTEGER NOT NULL CHECK (max_menu_entries >= 1),
    CHECK (max_menu_entries <= max_history_entries)
);

INSERT INTO settings (
    singleton_id,
    primary_enabled,
    max_history_entries,
    max_menu_entries
) VALUES (1, 1, 20, 10);

CREATE TABLE history_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fingerprint BLOB NOT NULL UNIQUE CHECK (length(fingerprint) = 32),
    plain_text TEXT NOT NULL,
    captured_at_ms INTEGER NOT NULL,
    last_used_at_ms INTEGER NOT NULL,
    last_used_seq INTEGER NOT NULL
);

CREATE INDEX history_entries_last_used
    ON history_entries (last_used_seq DESC);

CREATE TABLE history_representations (
    entry_id INTEGER NOT NULL
        REFERENCES history_entries (id) ON DELETE CASCADE,
    mime_type TEXT NOT NULL,
    data BLOB NOT NULL,
    PRIMARY KEY (entry_id, mime_type)
);

CREATE TABLE primary_entry (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    fingerprint BLOB NOT NULL CHECK (length(fingerprint) = 32),
    plain_text TEXT NOT NULL,
    captured_at_ms INTEGER NOT NULL
);

CREATE TABLE primary_representations (
    mime_type TEXT PRIMARY KEY,
    data BLOB NOT NULL
);
