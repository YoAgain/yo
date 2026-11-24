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
    /// The whole list in one row, under its own kind (`<field>.all`): a list that drops its first item as it takes
    /// one moved every per-position row, each save. A build that knows only those rows reads it as empty, which for
    /// the guards against copies that use it is harmless.
    Whole,
}
use Shape::*;

type Part = (&'static str, Shape);

/// How the state splits. What no part names stays in the root row.
const STATE: &[Part] = &[
    ("outbox", List),
    ("events", List),
    ("handed", List),
    ("group_held", List),
    ("seen", Whole),
    ("received", Whole),
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

fn whole(kind: &str) -> String {
    format!("{kind}.all")
}

fn split(rows: &mut Rows, kind: &str, owner: &str, fields: &mut Object, parts: &[Part]) {
    for &(field, shape) in parts {
        let kind = path(kind, field);
        match (fields.remove(field), shape) {
            (None, _) => {}
            (Some(value @ Value::Array(_)), Whole) => put(rows, &whole(&kind), owner, "", json(&value)),
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
            // Written by a build of before: one row per item.
            Whole if under(rows, &whole(&kind), owner).next().is_none() => {
                Value::Array(under(rows, &kind, owner).map(|(_, v)| parse(v)).collect::<crate::Result<_>>()?)
            }
            Whole => parse(under(rows, &whole(&kind), owner).next().map(|(_, v)| v).ok_or(Error::State)?)?,
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
    let mut upsert = db
        .prepare_cached("INSERT OR REPLACE INTO rows (kind, owner, entry, value) VALUES (?1, ?2, ?3, ?4)")
        .map_err(sql)?;
    // Both in key order: one walk side by side instead of a lookup per row (most of a save with thousands of rows).
    let (mut old, mut new) = (before.iter().peekable(), rows.iter().peekable());
    loop {
        let order = match (old.peek(), new.peek()) {
            (None, None) => break,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some((was, _)), Some((is, _))) => was.cmp(is),
        };
        let changed = match order {
            std::cmp::Ordering::Less => {
                let ((kind, owner, entry), _) = old.next().expect("peeked");
                remove.execute(params![kind, owner, entry]).map_err(sql)?;
                None
            }
            std::cmp::Ordering::Greater => new.next(),
            std::cmp::Ordering::Equal => {
                let ((_, was), is) = (old.next().expect("peeked"), new.next().expect("peeked"));
                (was != is.1).then_some(is)
            }
        };
        if let Some(((kind, owner, entry), value)) = changed {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Contact, Outgoing, Slot};
    use serde_json::json;

    #[test]
    fn reads_single_device_rows_with_their_sessions_and_queued_ciphertext() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(include_str!("../../migrations/V0001__initial_state.sql")).unwrap();
        let user = crate::state::hex(&[2; 16]);
        let slot = Slot { key: [3; 32], slot: 7 };
        let mut state = State::default();
        state
            .contacts
            .insert(user.clone(), Contact { devices: vec![slot], chat: true, accepted: true, ..Default::default() });
        let queued = Outgoing {
            user: user.clone(),
            seq: 9,
            plain: vec![1, 2],
            payloads: vec![crate::state::DevicePayload { device: 1, payload: vec![3, 4] }],
            made_for: vec![slot.key],
            ..Default::default()
        };
        state.outbox.push_back(queued.clone());
        let records = Entries::from([("session".into(), vec![8]), ("seen".into(), Vec::new())]);
        let key = [0; 32];
        let mut rows = encode(&state, &records);
        for ((kind, owner, entry), value) in &mut rows {
            let mut fields: Value = serde_json::from_slice(value).unwrap_or(Value::Null);
            match kind.as_str() {
                "state" => {
                    for name in ["siblings", "own_roster_at", "sibling_sent_at", "seen_through"] {
                        fields.as_object_mut().unwrap().remove(name);
                    }
                }
                "contacts" => {
                    fields.as_object_mut().unwrap().remove("devices");
                    fields["device"] = json!(slot);
                }
                "outbox" => fields["made_for"] = json!(slot.key),
                _ => {}
            }
            if matches!(kind.as_str(), "state" | "outbox" | "contacts") {
                *value = json(&fields);
            }
            let sealed = super::super::cipher::seal(&key, aad(kind, owner, entry), value).unwrap();
            db.execute("INSERT INTO rows VALUES (?1, ?2, ?3, ?4)", params![kind, owner, entry, sealed]).unwrap();
        }
        let (loaded, sessions, _) = read(&db, &key).unwrap();
        assert_eq!(loaded.contacts[&user], state.contacts[&user]);
        assert_eq!(loaded.outbox[0], queued);
        assert_eq!(sessions, records);
        assert!(loaded.siblings.is_empty());
    }

    const KEY: [u8; 32] = [9; 32];

    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(include_str!("../../migrations/V0001__initial_state.sql")).unwrap();
        db
    }

    fn full() -> State {
        let mut state = State::default();
        state.seen = (0..crate::SEEN as u32).map(|i| crate::sha256(&i.to_be_bytes())).collect();
        state.received = state.seen.iter().rev().copied().collect();
        state
    }

    /// A full guard list takes one hash and drops its first: one row changes, not one per position.
    #[test]
    fn a_full_guard_list_saves_as_one_row() {
        let mut state = full();
        let before = encode(&state, &Entries::new());
        state.seen.pop_front();
        state.seen.push_back(crate::sha256(b"new"));
        let after = encode(&state, &Entries::new());
        let changed: Vec<_> = after.iter().filter(|(row, value)| before.get(*row) != Some(*value)).collect();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0 .0, "seen.all");
        assert!(after.keys().all(|(kind, _, _)| kind != "seen" && kind != "received"));
    }

    /// A state written by a build of before (a row per hash) reads the same, and its next save leaves one row each.
    #[test]
    fn guard_lists_of_before_read_and_move_to_one_row() {
        let db = db();
        let state = full();
        let mut rows = encode(&state, &Entries::new());
        for (field, list) in [("seen", &state.seen), ("received", &state.received)] {
            rows.remove(&(whole(field), String::new(), String::new()));
            for (index, hash) in list.iter().enumerate() {
                put(&mut rows, field, "", &ordinal(index), json(&Value::String(yo_wire::b64(hash))));
            }
        }
        for ((kind, owner, entry), value) in &rows {
            let sealed = super::super::cipher::seal(&KEY, aad(kind, owner, entry), value).unwrap();
            db.execute(
                "INSERT INTO rows (kind, owner, entry, value) VALUES (?1, ?2, ?3, ?4)",
                params![kind, owner, entry, sealed],
            )
            .unwrap();
        }
        let (read_back, _, baseline) = read(&db, &KEY).unwrap();
        assert_eq!((&read_back.seen, &read_back.received), (&state.seen, &state.received));
        write(&db, &KEY, &read_back, &Entries::new(), Some(&baseline)).unwrap();
        let count = |kind: &str| -> i64 {
            db.query_row("SELECT count(*) FROM rows WHERE kind = ?1", [kind], |row| row.get(0)).unwrap()
        };
        assert_eq!((count("seen"), count("received"), count("seen.all"), count("received.all")), (0, 0, 1, 1));
        let (again, _, _) = read(&db, &KEY).unwrap();
        assert_eq!((&again.seen, &again.received), (&state.seen, &state.received));
    }
}
