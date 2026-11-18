//! The V0001 row codecs. Keep these codecs readable by migration V0001 if the schema changes.
use crate::{error::Error, signal::Entries, state::State};
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

type Object = Map<String, Value>;
type Rows = BTreeMap<(String, String), Vec<u8>>;
type Tables = BTreeMap<&'static str, Rows>;

#[derive(Clone, Copy)]
enum Shape {
    Map,
    List,
}

use Shape::{List, Map as ByKey};

const ACCOUNT: &[&str] = &["user_id", "username", "account_secret", "name"];
const DEVICE: &[&str] = &[
    "auth_secret",
    "device_id",
    "device_key",
    "device_number",
    "certificate",
    "registration_id",
    "signed_until",
    "sender_certificate",
    "certificate_clock",
    "registering",
];
const PROFILE: &[&str] = &["profile_key", "profile_key_gen", "access_key_told"];
const STORAGE: &[&str] =
    &["storage_version", "storage_dirty", "storage_saved_at", "storage_fetched", "legacy_storage_key"];
const ROOT: &[(&str, &str, Shape)] = &[
    ("own", "devices", List),
    ("events", "inbox_events", List),
    ("handed", "inbox_events", List),
    ("group_held", "inbox_events", List),
    ("seen", "inbox_dedup", List),
    ("received", "inbox_dedup", List),
    ("deferred", "deferred_payloads", List),
    ("roster_asked", "roster_lookups", ByKey),
];
const CONTACT: &[(&str, &str, Shape)] = &[
    ("devices", "contact_devices", List),
    ("handles", "contact_handles", List),
    ("sent", "sent_cache", List),
    ("decryption_errors", "decryption_errors", List),
];
const GROUP: &[(&str, &str, Shape)] =
    &[("members", "group_members", List), ("endorsements", "group_endorsements", ByKey)];
const SYNCED: &[(&str, &str, Shape)] =
    &[("friends", "storage_friends", ByKey), ("g", "storage_groups", ByKey), ("k", "storage_keys", ByKey)];
const PREKEYS: &[(&str, &str, Shape)] = &[("retired", "retired_prekeys", List)];
const SIGNAL: &[(&str, &str)] = &[
    ("s", "sessions"),
    ("i", "identities"),
    ("p", "prekeys"),
    ("sp", "signed_prekeys"),
    ("k/k", "kyber_prekeys"),
    ("k/kl", "kyber_prekeys"),
];

fn ordinal(index: usize) -> String {
    format!("{index:020}")
}

fn json(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON value encodes")
}

fn object(value: Value) -> Object {
    match value {
        Value::Object(fields) => fields,
        _ => unreachable!("state fields serialize as objects"),
    }
}

fn remove(fields: &mut Object, keys: &[&str]) -> Object {
    keys.iter().filter_map(|key| fields.remove(*key).map(|value| (key.to_string(), value))).collect()
}

fn put(tables: &mut Tables, table: &'static str, owner: &str, entry: &str, value: Vec<u8>) {
    tables.entry(table).or_default().insert((owner.into(), entry.into()), value);
}

fn put_fields(tables: &mut Tables, table: &'static str, entry: &str, fields: Object) {
    put(tables, table, "", entry, json(&Value::Object(fields)));
}

fn split(tables: &mut Tables, fields: &mut Object, layout: &[(&str, &'static str, Shape)], parent: Option<&str>) {
    for &(field, table, shape) in layout {
        let rows = tables.entry(table).or_default();
        let owner = parent.unwrap_or(field);
        match (fields.remove(field), shape) {
            (None, _) => (),
            (Some(Value::Array(values)), List) => {
                rows.extend(
                    values.into_iter().enumerate().map(|(index, value)| ((owner.into(), ordinal(index)), json(&value))),
                );
            }
            (Some(Value::Object(values)), ByKey) => {
                rows.extend(values.into_iter().map(|(key, value)| ((owner.into(), key), json(&value))));
            }
            _ => unreachable!("state field matches its row layout"),
        }
    }
}

fn encode(state: &State, records: &Entries) -> Tables {
    let mut rows = Tables::new();
    let mut fields = object(serde_json::to_value(state).expect("state encodes"));
    put_fields(&mut rows, "accounts", "local", remove(&mut fields, ACCOUNT));
    put_fields(&mut rows, "devices", "local", remove(&mut fields, DEVICE));
    put_fields(&mut rows, "profile_keys", "local", remove(&mut fields, PROFILE));
    put_fields(&mut rows, "account_storage", "metadata", remove(&mut fields, STORAGE));
    let Value::Array(outbox) = fields.remove("outbox").expect("outbox") else {
        unreachable!("outbox serializes as an array");
    };
    let outbox_rows = rows.entry("outbox").or_default();
    for (index, value) in outbox.into_iter().enumerate() {
        outbox_rows.insert((ordinal(index), value["seq"].as_u64().expect("sequence").to_string()), json(&value));
    }
    split(&mut rows, &mut fields, ROOT, None);
    for (field, table, layout) in [("contacts", "contacts", CONTACT), ("hub_groups", "groups", GROUP)] {
        rows.entry(table).or_default();
        // Initialize child tables even when there are no parents, so stale rows are removed.
        for &(_, child, _) in layout {
            rows.entry(child).or_default();
        }
        for (key, value) in object(fields.remove(field).expect("state map")) {
            let mut values = object(value);
            split(&mut rows, &mut values, layout, Some(&key));
            put_fields(&mut rows, table, &key, values);
        }
    }
    let mut synced = object(fields.remove("storage").expect("account storage"));
    split(&mut rows, &mut synced, SYNCED, None);
    put_fields(&mut rows, "account_storage", "synced", synced);
    let mut prekeys = object(fields.remove("prekeys").expect("prekey state"));
    put_fields(&mut rows, "prekey_uploads", "pending", remove(&mut prekeys, &["upload"]));
    split(&mut rows, &mut prekeys, PREKEYS, None);
    put_fields(&mut rows, "prekey_state", "local", prekeys);
    put_fields(&mut rows, "settings", "local", fields);
    rows.extend(protocol_rows(records));
    rows
}

fn protocol_rows(records: &Entries) -> Tables {
    let mut rows = Tables::new();
    for &(prefix, table) in SIGNAL {
        let prefix_with_slash = format!("{prefix}/");
        let items = rows.entry(table).or_default();
        for (key, value) in records.range(prefix_with_slash.clone()..) {
            let Some(key) = key.strip_prefix(&prefix_with_slash) else { break };
            let (owner, entry) = match table {
                "sessions" => key.rsplit_once('.').unwrap_or(("", key)),
                "kyber_prekeys" => (prefix, key),
                _ => ("", key),
            };
            items.insert((owner.into(), entry.into()), value.clone());
        }
    }
    rows.entry("kyber_replays").or_default();
    if let Some(seen) = records.get("seen") {
        for (index, replay) in seen.as_chunks::<37>().0.iter().enumerate() {
            put(&mut rows, "kyber_replays", "", &ordinal(index), replay.to_vec());
        }
    }
    rows
}

#[derive(Clone, Copy)]
enum Column {
    Text,
    Integer,
    Unsigned,
    Bool,
    Secret,
    Json,
}

use Column::*;

fn columns(table: &str) -> &'static [(&'static str, Column)] {
    match table {
        "accounts" => &[("user_id", Text), ("username", Text), ("name", Text), ("account_secret", Secret)],
        "devices" => &[
            ("auth_secret", Secret),
            ("device_id", Json),
            ("device_key", Json),
            ("device_number", Integer),
            ("certificate", Json),
            ("registration_id", Integer),
            ("signed_until", Unsigned),
            ("sender_certificate", Json),
            ("certificate_clock", Unsigned),
            ("registering", Json),
        ],
        "profile_keys" => &[("profile_key", Secret), ("profile_key_gen", Integer), ("access_key_told", Bool)],
        "account_storage" => &[
            ("storage_version", Unsigned),
            ("storage_dirty", Bool),
            ("storage_saved_at", Unsigned),
            ("storage_fetched", Bool),
            ("legacy_storage_key", Secret),
            ("version", Unsigned),
            ("icon", Json),
            ("left_handed", Json),
            ("pk", Secret),
            ("pg", Json),
        ],
        "contacts" => &[
            ("probe_service", Bool),
            ("probe_reply_tag", Json),
            ("probe_pending", Json),
            ("account_key", Json),
            ("name", Text),
            ("verified", Bool),
            ("unconfirmed", Bool),
            ("blocked", Bool),
            ("gone", Bool),
            ("chat", Bool),
            ("profile_due", Bool),
            ("receipt_due", Bool),
            ("outdated_at", Unsigned),
            ("accepted", Bool),
            ("profile_key", Secret),
            ("key_due", Bool),
            ("key_given", Bool),
            ("key_resent_at", Unsigned),
            ("key_gen", Integer),
        ],
        "groups" => &[
            ("key", Secret),
            ("name", Text),
            ("revision", Integer),
            ("left", Bool),
            ("accepted", Bool),
            ("keyed", Bool),
            ("named", Integer),
            ("endorsed_until", Unsigned),
        ],
        "outbox" => &[
            ("queued_at", Unsigned),
            ("seq", Unsigned),
            ("user", Text),
            ("plain", Secret),
            ("device", Json),
            ("non_urgent", Bool),
            ("payload", Json),
            ("made_for", Json),
            ("refused", Bool),
        ],
        "settings" => {
            &[("next_seq", Unsigned), ("last_sent_ms", Unsigned), ("requests_open", Bool), ("recovery_lookups", Json)]
        }
        "prekey_state" => &[("next_id", Integer), ("signed_at", Unsigned), ("left", Integer)],
        "prekey_uploads" => &[("upload", Json)],
        "contact_devices" => &[("key", Json), ("slot", Integer)],
        _ => &[],
    }
}

fn sealed_record(table: &str) -> bool {
    // Synced group entries contain another copy of the master key. Pending events can contain media keys.
    matches!(
        table,
        "sessions"
            | "identities"
            | "prekeys"
            | "signed_prekeys"
            | "kyber_prekeys"
            | "sender_keys"
            | "storage_groups"
            | "storage_keys"
            | "sent_cache"
            | "inbox_events"
    )
}

fn select_columns(table: &str) -> String {
    let mut names = "owner, entry, value".to_owned();
    for (name, _) in columns(table) {
        names.push_str(", ");
        names.push_str(name);
    }
    names
}

fn read_value(
    row: &rusqlite::Row<'_>,
    key: &[u8; 32],
    table: &str,
    owner: &str,
    entry: &str,
) -> crate::Result<Vec<u8>> {
    let value: Vec<u8> = row.get(2).map_err(sql)?;
    if sealed_record(table) {
        return super::cipher::unseal(key, [table, owner, entry, "value"], &value);
    }
    let columns = columns(table);
    if columns.is_empty() {
        return Ok(value);
    }
    let mut fields: Object = parse(&value)?;
    for (index, &(name, kind)) in columns.iter().enumerate() {
        if fields.contains_key(name) {
            return Err(Error::State);
        }
        let value: SqlValue = row.get(3 + index).map_err(sql)?;
        let decoded = match (kind, value) {
            (Text, SqlValue::Null) => Value::Null,
            (_, SqlValue::Null) => continue,
            (Text, SqlValue::Text(value)) => Value::String(value),
            (Integer, SqlValue::Integer(value)) => Value::from(value),
            (Unsigned, SqlValue::Text(value)) => Value::from(value.parse::<u64>().map_err(|_| Error::State)?),
            (Bool, SqlValue::Integer(value @ (0 | 1))) => Value::Bool(value != 0),
            (Secret, SqlValue::Blob(value)) => {
                parse(&super::cipher::unseal(key, [table, owner, entry, name], &value)?)?
            }
            (Json, SqlValue::Blob(value)) => parse(&value)?,
            _ => return Err(Error::State),
        };
        fields.insert(name.into(), decoded);
    }
    Ok(json(&Value::Object(fields)))
}

fn load_rows(db: &Connection, key: &[u8; 32], table: &str, owner: Option<&str>) -> crate::Result<Rows> {
    let condition = if owner.is_some() { " WHERE owner = ?1" } else { "" };
    let mut stmt =
        db.prepare_cached(&format!("SELECT {} FROM {table}{condition}", select_columns(table))).map_err(sql)?;
    let mut rows = stmt.query(params_from_iter(owner)).map_err(sql)?;
    let mut result = Rows::new();
    while let Some(row) = rows.next().map_err(sql)? {
        let owner: String = row.get(0).map_err(sql)?;
        let entry: String = row.get(1).map_err(sql)?;
        let value = read_value(row, key, table, &owner, &entry)?;
        result.insert((owner, entry), value);
    }
    Ok(result)
}

fn sql(error: rusqlite::Error) -> Error {
    super::cipher::sqlite_error(error)
}

pub(super) fn write(db: &Connection, key: &[u8; 32], state: &State, records: &Entries) -> crate::Result<()> {
    write_with_baseline(db, key, state, records, None)
}

// A baseline is trusted only after the caller checked its matching CAS word in this transaction.
pub(super) fn write_with_baseline(
    db: &Connection,
    key: &[u8; 32],
    state: &State,
    records: &Entries,
    baseline: Option<&Entries>,
) -> crate::Result<()> {
    let mut tables = encode(state, records);
    let mut baseline = baseline.map(protocol_rows).unwrap_or_default();
    for parent in ["contacts", "groups"] {
        replace(db, key, parent, tables.remove(parent).expect("parent rows"), None)?;
    }
    for (table, rows) in tables {
        replace(db, key, table, rows, baseline.remove(table))?;
    }
    Ok(())
}

fn write_values(
    key: &[u8; 32],
    table: &str,
    owner: String,
    entry: String,
    value: Vec<u8>,
) -> crate::Result<Vec<SqlValue>> {
    let mut values = Vec::new();
    let columns = columns(table);
    let value = if sealed_record(table) {
        super::cipher::seal(key, [table, &owner, &entry, "value"], &value)?
    } else if columns.is_empty() {
        value
    } else {
        let mut fields: Object = parse(&value)?;
        for &(name, kind) in columns {
            let field = fields.remove(name);
            let value = match (kind, field) {
                (_, None) => SqlValue::Null,
                (Text, Some(Value::Null)) => SqlValue::Null,
                (Text, Some(Value::String(value))) => SqlValue::Text(value),
                (Integer, Some(value)) => SqlValue::Integer(value.as_i64().ok_or(Error::State)?),
                (Unsigned, Some(value)) => SqlValue::Text(value.as_u64().ok_or(Error::State)?.to_string()),
                (Bool, Some(Value::Bool(value))) => SqlValue::Integer(i64::from(value)),
                (Json, Some(value)) => SqlValue::Blob(json(&value)),
                (Secret, Some(value)) => {
                    SqlValue::Blob(super::cipher::seal(key, [table, &owner, &entry, name], &json(&value))?)
                }
                _ => return Err(Error::State),
            };
            values.push(value);
        }
        json(&Value::Object(fields))
    };
    let mut row = vec![SqlValue::Text(owner), SqlValue::Text(entry), SqlValue::Blob(value)];
    row.extend(values);
    Ok(row)
}

fn replace(db: &Connection, key: &[u8; 32], table: &str, rows: Rows, baseline: Option<Rows>) -> crate::Result<()> {
    let before = match baseline {
        Some(rows) => rows,
        None => load_rows(db, key, table, None)?,
    };
    let mut remove = db.prepare_cached(&format!("DELETE FROM {table} WHERE owner = ?1 AND entry = ?2")).map_err(sql)?;
    for (owner, entry) in before.keys().filter(|key| !rows.contains_key(*key)) {
        remove.execute(params![owner, entry]).map_err(sql)?;
    }
    let names = select_columns(table);
    let placeholders = vec!["?"; 3 + columns(table).len()].join(", ");
    let updates =
        names.split(", ").skip(2).map(|name| format!("{name} = excluded.{name}")).collect::<Vec<_>>().join(", ");
    let mut upsert = db
        .prepare_cached(&format!(
            "INSERT INTO {table} ({names}) VALUES ({placeholders})
         ON CONFLICT (owner, entry) DO UPDATE SET {updates}"
        ))
        .map_err(sql)?;
    for ((owner, entry), value) in rows {
        if before.get(&(owner.clone(), entry.clone())) != Some(&value) {
            upsert.execute(params_from_iter(write_values(key, table, owner, entry, value)?)).map_err(sql)?;
        }
    }
    Ok(())
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> crate::Result<T> {
    serde_json::from_slice(bytes).map_err(|_| Error::State)
}

fn fields(db: &Connection, key: &[u8; 32], table: &str, entry: &str) -> crate::Result<Object> {
    let mut stmt = db
        .prepare_cached(&format!("SELECT {} FROM {table} WHERE owner = '' AND entry = ?1", select_columns(table)))
        .map_err(sql)?;
    let mut rows = stmt.query([entry]).map_err(sql)?;
    parse(&read_value(rows.next().map_err(sql)?.ok_or(Error::State)?, key, table, "", entry)?)
}

fn join(
    db: &Connection,
    key: &[u8; 32],
    fields: &mut Object,
    layout: &[(&str, &str, Shape)],
    parent: Option<&str>,
) -> crate::Result<()> {
    for &(field, table, shape) in layout {
        let rows = load_rows(db, key, table, Some(parent.unwrap_or(field)))?;
        let value = match shape {
            List => Value::Array(rows.into_values().map(|bytes| parse(&bytes)).collect::<crate::Result<_>>()?),
            ByKey => Value::Object(
                rows.into_iter()
                    .map(|((_, entry), bytes)| Ok((entry, parse(&bytes)?)))
                    .collect::<crate::Result<_>>()?,
            ),
        };
        fields.insert(field.into(), value);
    }
    Ok(())
}

pub(super) fn read(db: &Connection, key: &[u8; 32]) -> crate::Result<(State, Entries)> {
    let mut root = fields(db, key, "settings", "local")?;
    for table in ["accounts", "devices", "profile_keys"] {
        root.extend(fields(db, key, table, "local")?);
    }
    root.extend(fields(db, key, "account_storage", "metadata")?);
    let outbox = load_rows(db, key, "outbox", None)?
        .into_values()
        .map(|bytes| parse(&bytes))
        .collect::<crate::Result<Vec<_>>>()?;
    root.insert("outbox".into(), Value::Array(outbox));
    join(db, key, &mut root, ROOT, None)?;
    for (field, table, layout) in [("contacts", "contacts", CONTACT), ("hub_groups", "groups", GROUP)] {
        let mut parents = Object::new();
        for ((_, entry), bytes) in load_rows(db, key, table, None)? {
            let mut parent = parse(&bytes)?;
            join(db, key, &mut parent, layout, Some(&entry))?;
            parents.insert(entry, Value::Object(parent));
        }
        root.insert(field.into(), Value::Object(parents));
    }
    let mut synced = fields(db, key, "account_storage", "synced")?;
    join(db, key, &mut synced, SYNCED, None)?;
    root.insert("storage".into(), Value::Object(synced));
    let mut prekeys = fields(db, key, "prekey_state", "local")?;
    prekeys.extend(fields(db, key, "prekey_uploads", "pending")?);
    join(db, key, &mut prekeys, PREKEYS, None)?;
    root.insert("prekeys".into(), Value::Object(prekeys));
    let state = serde_json::from_value(Value::Object(root)).map_err(|_| Error::State)?;
    let mut records = Entries::new();
    for (table, prefix) in
        [("sessions", "s"), ("identities", "i"), ("prekeys", "p"), ("signed_prekeys", "sp"), ("kyber_prekeys", "k")]
    {
        for ((owner, id), value) in load_rows(db, key, table, None)? {
            let key = match table {
                "sessions" if !owner.is_empty() => format!("s/{owner}.{id}"),
                "kyber_prekeys" => format!("{owner}/{id}"),
                _ => format!("{prefix}/{id}"),
            };
            records.insert(key, value);
        }
    }
    records.insert("seen".into(), load_rows(db, key, "kyber_replays", None)?.into_values().flatten().collect());
    Ok((state, records))
}

pub(super) fn clear(db: &Connection) -> rusqlite::Result<()> {
    // Contacts and groups cascade to their child rows.
    db.execute_batch(
        "DELETE FROM accounts; DELETE FROM devices; DELETE FROM settings; DELETE FROM profile_keys;
         DELETE FROM account_storage; DELETE FROM storage_friends; DELETE FROM storage_groups; DELETE FROM storage_keys;
         DELETE FROM contacts; DELETE FROM groups; DELETE FROM outbox; DELETE FROM inbox_events;
         DELETE FROM inbox_dedup; DELETE FROM deferred_payloads; DELETE FROM roster_lookups;
         DELETE FROM prekey_uploads; DELETE FROM retired_prekeys; DELETE FROM prekey_state;
         DELETE FROM sessions; DELETE FROM identities; DELETE FROM prekeys; DELETE FROM signed_prekeys;
         DELETE FROM kyber_prekeys; DELETE FROM kyber_replays; DELETE FROM sender_keys;",
    )
}
