CREATE TABLE tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    list_id INTEGER NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    UNIQUE (list_id, name),
    UNIQUE (id, list_id)
);

CREATE UNIQUE INDEX items_id_list_unique ON items(id, list_id);

CREATE TABLE item_tags (
    list_id INTEGER NOT NULL,
    item_id INTEGER NOT NULL,
    tag_id INTEGER NOT NULL,
    PRIMARY KEY (item_id, tag_id),
    FOREIGN KEY (item_id, list_id) REFERENCES items(id, list_id) ON DELETE CASCADE,
    FOREIGN KEY (tag_id, list_id) REFERENCES tags(id, list_id) ON DELETE CASCADE
);
CREATE INDEX item_tags_by_list_tag ON item_tags(list_id, tag_id);
