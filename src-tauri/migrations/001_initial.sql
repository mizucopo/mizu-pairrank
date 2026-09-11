CREATE TABLE lists (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
    model_version INTEGER NOT NULL,
    model_parameters TEXT NOT NULL
);

CREATE TABLE items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    list_id INTEGER NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    image_path TEXT,
    image_source_url TEXT,
    mu REAL NOT NULL,
    sigma REAL NOT NULL CHECK (sigma > 0),
    comparison_count INTEGER NOT NULL DEFAULT 0 CHECK (comparison_count >= 0),
    deleted INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1))
);
CREATE INDEX items_by_list ON items(list_id, deleted);

CREATE TABLE comparisons (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    list_id INTEGER NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
    a_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    b_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    preference TEXT NOT NULL CHECK (preference IN ('a_strong', 'a_weak', 'equal', 'b_weak', 'b_strong')),
    CHECK (a_id <> b_id)
);
CREATE INDEX comparisons_by_list ON comparisons(list_id);

CREATE TABLE rank_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    list_id INTEGER NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
    ranking TEXT NOT NULL
);
CREATE INDEX snapshots_by_list ON rank_snapshots(list_id, id);
