CREATE TABLE state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    word TEXT NOT NULL,
    blob BLOB NOT NULL
);
