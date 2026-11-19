//! The state as rows of one table: its JSON split so that a write touches only what changed (a contact, an outbox
//! item, a session), every value sealed under the state key.
use crate::{error::Error, signal::Entries, state::State};
use rusqlite::{params, Connection};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

type Object = Map<String, Value>;
/// (kind, owner, entry) → the row's plain value.
pub(super) type Rows = BTreeMap<(String, String, String), Vec<u8>>;

#[derive(Clone, Copy)]
enum Shape {
    /// One row per item, keyed by position.
    List,
    /// One row per entry, keyed by its key; its own parts split under it (owner: the key).
    Map(&'static [Part]),
    /// One row; its own parts split beside it.
    One(&'static [Part]),
}
use Shape::*;

type Part = (&'static str, Shape);

/// How the state splits. What no part names stays in the root row.
const STATE: &[Part] = &[
    ("outbox", List),
    ("events", List),
    ("handed", List),
    ("group_held", List),
    ("seen", List),
    ("received", List),
    ("deferred", List),
    ("roster_asked", Map(&[])),
    ("contacts", Map(&[("handles", List), ("sent", List), ("decryption_errors", List)])),
    ("hub_groups", Map(&[("members", List), ("endorsements", Map(&[]))])),
    ("storage", One(&[("friends", Map(&[])), ("g", Map(&[])), ("k", Map(&[]))])),
    ("prekeys", One(&[("retired", List)])),
];

const ROOT: &str = "state";
/// libsignal's records, one row each; the Kyber replay list in 37-byte entries, as it only grows.
const SIGNAL: &str = "signal";
const REPLAYS: &str = "signal.seen";
const REPLAY_LEN: usize = 37;

fn ordinal(index: usize) -> String {
    format!("{index:020}")
}

fn json(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON value encodes")
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> crate::Result<T> {
    serde_json::from_slice(bytes).map_err(|_| Error::State)
}

fn path(kind: &str, field: &str) -> String {
    if kind.is_empty() {
        field.to_owned()
    } else {
        format!("{kind}.{field}")
    }
}

fn put(rows: &mut Rows, kind: &str, owner: &str, entry: &str, value: Vec<u8>) {
    rows.insert((kind.into(), owner.into(), entry.into()), value);
}

fn split(rows: &mut Rows, kind: &str, owner: &str, fields: &mut Object, parts: &[Part]) {
    for &(field, shape) in parts {
        let kind = path(kind, field);
        match (fields.remove(field), shape) {
            (None, _) => {}
            (Some(Value::Array(items)), List) => {
                for (index, item) in items.iter().enumerate() {
                    put(rows, &kind, owner, &ordinal(index), json(item));
                }
            }
            (Some(Value::Object(entries)), Map(children)) => {
                for (key, mut value) in entries {
                    if let (Value::Object(fields), false) = (&mut value, children.is_empty()) {
                        split(rows, &kind, &key, fields, children);
                    }
                    put(rows, &kind, owner, &key, json(&value));
                }
            }
            (Some(Value::Object(mut inner)), One(children)) => {
                split(rows, &kind, owner, &mut inner, children);
                put(rows, &kind, owner, "", json(&Value::Object(inner)));
            }
            _ => unreachable!("state field matches its row layout"),
        }
    }
}

fn signal_rows(rows: &mut Rows, records: &Entries) {
    for (key, value) in records {
        if key == "seen" {
            for (index, replay) in value.chunks(REPLAY_LEN).enumerate() {
                put(rows, REPLAYS, "", &ordinal(index), replay.to_vec());
            }
        } else {
            put(rows, SIGNAL, "", key, value.clone());
        }
    }
}

fn encode(state: &State, records: &Entries) -> Rows {
    let mut rows = Rows::new();
    let Value::Object(mut root) = serde_json::to_value(state).expect("state encodes") else {
        unreachable!("the state serializes as an object")
    };
    split(&mut rows, "", "", &mut root, STATE);
    put(&mut rows, ROOT, "", "", json(&Value::Object(root)));
    signal_rows(&mut rows, records);
    rows
}

/// The rows of `kind` under `owner`, in key order.
fn under<'a>(rows: &'a Rows, kind: &str, owner: &str) -> impl Iterator<Item = (&'a str, &'a [u8])> {
    let (kind, owner) = (kind.to_owned(), owner.to_owned());
    rows.range((kind.clone(), owner.clone(), String::new())..)
        .take_while(move |((k, o, _), _)| *k == kind && *o == owner)
        .map(|((_, _, entry), value)| (entry.as_str(), value.as_slice()))
}

fn join(rows: &Rows, kind: &str, owner: &str, fields: &mut Object, parts: &[Part]) -> crate::Result<()> {
    for &(field, shape) in parts {
        let kind = path(kind, field);
        let value = match shape {
            List => Value::Array(under(rows, &kind, owner).map(|(_, v)| parse(v)).collect::<crate::Result<_>>()?),
            Map(children) => {
                let mut entries = Object::new();
                for (key, value) in under(rows, &kind, owner) {
                    let mut value: Value = parse(value)?;
                    if let (Value::Object(fields), false) = (&mut value, children.is_empty()) {
                        join(rows, &kind, key, fields, children)?;
                    }
                    entries.insert(key.to_owned(), value);
                }
                Value::Object(entries)
            }
            One(children) => {
                let (_, value) = under(rows, &kind, owner).next().ok_or(Error::State)?;
                let mut inner: Object = parse(value)?;
                join(rows, &kind, owner, &mut inner, children)?;
                Value::Object(inner)
            }
        };
        fields.insert(field.into(), value);
    }
    Ok(())
}

fn aad<'a>(kind: &'a str, owner: &'a str, entry: &'a str) -> [&'a str; 4] {
    [kind, owner, entry, "row"]
}

fn sql(error: rusqlite::Error) -> Error {
    super::cipher::sqlite_error(error)
}

/// Every row opened.
fn load(db: &Connection, key: &[u8; 32]) -> crate::Result<Rows> {
    #[cfg(test)]
    LOADS.with(|loads| loads.set(loads.get() + 1));
    let mut stmt = db.prepare_cached("SELECT kind, owner, entry, value FROM rows").map_err(sql)?;
    let mut found = stmt.query([]).map_err(sql)?;
    let mut rows = Rows::new();
    while let Some(row) = found.next().map_err(sql)? {
        let (kind, owner, entry): (String, String, String) =
            (row.get(0).map_err(sql)?, row.get(1).map_err(sql)?, row.get(2).map_err(sql)?);
        let sealed: Vec<u8> = row.get(3).map_err(sql)?;
        let value = super::cipher::unseal(key, aad(&kind, &owner, &entry), &sealed)?;
        rows.insert((kind, owner, entry), value);
    }
    Ok(rows)
}

// How often this thread read every row (tests).
#[cfg(test)]
thread_local! {
    pub(super) static LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Writes the rows that changed, and returns the rows as now written. `baseline`: the rows as the database has them
/// (the snapshot last read or written), so they need not be read back to compare.
pub(super) fn write(
    db: &Connection,
    key: &[u8; 32],
    state: &State,
    records: &Entries,
    baseline: Option<&Rows>,
) -> crate::Result<Rows> {
    let rows = encode(state, records);
    let loaded;
    let before = match baseline {
        Some(baseline) => baseline,
        None => {
            loaded = load(db, key)?;
            &loaded
        }
    };
    let mut remove =
        db.prepare_cached("DELETE FROM rows WHERE kind = ?1 AND owner = ?2 AND entry = ?3").map_err(sql)?;
    for (kind, owner, entry) in before.keys().filter(|key| !rows.contains_key(*key)) {
        remove.execute(params![kind, owner, entry]).map_err(sql)?;
    }
    let mut upsert = db
        .prepare_cached("INSERT OR REPLACE INTO rows (kind, owner, entry, value) VALUES (?1, ?2, ?3, ?4)")
        .map_err(sql)?;
    for (row, value) in &rows {
        if before.get(row) != Some(value) {
            let (kind, owner, entry) = row;
            let sealed = super::cipher::seal(key, aad(kind, owner, entry), value)?;
            upsert.execute(params![kind, owner, entry, sealed]).map_err(sql)?;
        }
    }
    Ok(rows)
}

/// The state and libsignal's records, with the rows they came from.
pub(super) fn read(db: &Connection, key: &[u8; 32]) -> crate::Result<(State, Entries, Rows)> {
    let rows = load(db, key)?;
    let (_, root) = under(&rows, ROOT, "").next().ok_or(Error::State)?;
    let mut root: Object = parse(root)?;
    join(&rows, "", "", &mut root, STATE)?;
    let state = serde_json::from_value(Value::Object(root)).map_err(|_| Error::State)?;
    let mut records: Entries = under(&rows, SIGNAL, "").map(|(k, v)| (k.to_owned(), v.to_vec())).collect();
    records.insert("seen".into(), under(&rows, REPLAYS, "").flat_map(|(_, v)| v.iter().copied()).collect());
    Ok((state, records, rows))
}

pub(super) fn clear(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch("DELETE FROM rows")
}
