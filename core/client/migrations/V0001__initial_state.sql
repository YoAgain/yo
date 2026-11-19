CREATE TABLE state_meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    word TEXT,
    key_check BLOB
);
INSERT INTO state_meta (id) VALUES (1);

-- The state's parts (statedb/tables.rs), each value sealed under the state key.
CREATE TABLE rows (
    kind TEXT NOT NULL,
    owner TEXT NOT NULL,
    entry TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (kind, owner, entry)
) WITHOUT ROWID;
