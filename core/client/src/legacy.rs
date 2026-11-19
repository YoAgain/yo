//! Remove once every keychain item is v3.
use crate::{signal::Entries, state::State, Error, Result};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::Path;

/// Not readable now (the file protected until the phone is unlocked, another process writing): `Network`, read again
/// later. Anything else (no such row, damaged): `State`, never there.
fn later(error: impl std::fmt::Display) -> Error {
    Error::Network(format!("old state is unavailable now: {error}"))
}

fn io_error(error: std::io::Error) -> Error {
    match error.kind() {
        std::io::ErrorKind::NotFound => Error::State,
        _ => later(error),
    }
}

fn read_error(error: rusqlite::Error) -> Error {
    use rusqlite::ErrorCode::*;
    match error.sqlite_error_code() {
        Some(DatabaseBusy | DatabaseLocked | CannotOpen | PermissionDenied | SystemIoFailure | OutOfMemory) => {
            later(error)
        }
        _ => Error::State,
    }
}

/// Surely not there: no such row, or a table or column of a build that did not have it.
fn absent(error: &rusqlite::Error) -> bool {
    let text = error.to_string();
    matches!(error, rusqlite::Error::QueryReturnedNoRows)
        || text.starts_with("no such table")
        || text.starts_with("no such column")
}

/// What an old install's local state gives the one-time automatic sign-in.
pub struct Identity {
    pub secret: [u8; 32],
    /// `secret` is an Ed25519 seed, to convert with `legacy_account_secret`.
    pub ed25519: bool,
    /// The key the hub's storage is still sealed under (spec 6.9): from the seed, or kept by a converted state.
    pub storage_key: Option<[u8; 32]>,
}

/// Identity material for the main app's one-time automatic sign-in.
pub fn read_identity(path: &Path, key: &[u8; 32], format: u8) -> Result<Identity> {
    match format {
        0 | 1 => {
            let bytes = zeroize::Zeroizing::new(std::fs::read(path).map_err(io_error)?);
            let offset = if format == 1 { 36 } else { 0 };
            crate::state::legacy_identity(bytes.get(offset..).ok_or(Error::State)?, key)
        }
        2 | 3 => {
            let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(read_error)?;
            conn.busy_timeout(std::time::Duration::from_secs(3)).map_err(read_error)?;
            if format == 2 {
                let blob: Vec<u8> =
                    conn.query_row("SELECT blob FROM state WHERE id = 1", [], |row| row.get(0)).map_err(read_error)?;
                return crate::state::legacy_identity(&blob, key);
            }
            let secret: Vec<u8> = conn
                .query_row("SELECT account_secret FROM accounts WHERE owner = '' AND entry = 'local'", [], |row| {
                    row.get(0)
                })
                .map_err(read_error)?;
            let column = |table: &str, owner: &str, entry: &str, name: &str, sealed: &[u8]| {
                let plain = crate::statedb::cipher::unseal(key, [table, owner, entry, name], sealed)?;
                Ok::<_, Error>(zeroize::Zeroizing::new(plain))
            };
            let secret = column("accounts", "", "local", "account_secret", &secret)?;
            // Only a missing table, column or row means no storage key kept; anything else is no answer.
            let storage_key: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT legacy_storage_key FROM account_storage WHERE owner = '' AND entry = 'metadata'",
                    [],
                    |row| row.get(0),
                )
                .or_else(|error| if absent(&error) { Ok(None) } else { Err(read_error(error)) })?;
            let storage_key = match storage_key {
                Some(sealed) => {
                    serde_json::from_slice(&column("account_storage", "", "metadata", "legacy_storage_key", &sealed)?)
                        .map_err(|_| Error::State)?
                }
                None => None,
            };
            let secret = serde_json::from_slice(&secret).map_err(|_| Error::State)?;
            Ok(Identity { secret, ed25519: false, storage_key })
        }
        _ => Err(Error::State),
    }
}

/// A state file of the build before the database (YoGPT's `state`), in today's form: the same device, its sessions
/// and what waits, so the bot goes on without registering again (app/yogpt/README.md).
pub fn read_state(blob: &[u8], key: &[u8; 32]) -> Result<(State, Entries)> {
    let (version, mut json, records) = crate::state::open_legacy(blob, key)?;
    if version == 1 {
        return Err(Error::State);
    }
    // Lists of devices then, one device now.
    let first = |object: &mut Value, from: &str, to: &str| {
        if let Some(fields) = object.as_object_mut() {
            let list = fields.remove(from).unwrap_or(Value::Null);
            fields.insert(to.into(), list.get(0).cloned().unwrap_or(Value::Null));
        }
    };
    let list = |json: &mut Value, name: &str| json.get_mut(name).and_then(Value::as_array_mut).map(std::mem::take);
    for contact in json.get_mut("contacts").and_then(Value::as_object_mut).into_iter().flat_map(|c| c.values_mut()) {
        first(contact, "devices", "device");
    }
    for item in json.get_mut("outbox").and_then(Value::as_array_mut).into_iter().flatten() {
        first(item, "made_for", "made_for");
    }
    // A name as `Profile { name }`; copies of what this device sent (`own`), text, links and places are gone.
    let event = |event: &mut Value| {
        let Some(fields) = event.as_object_mut().and_then(|e| e.values_mut().next()).and_then(Value::as_object_mut)
        else {
            return true;
        };
        if fields.remove("own") == Some(Value::Bool(true)) {
            return false;
        }
        match fields.get_mut("content") {
            Some(Value::Object(content)) if content.contains_key("Profile") => {
                let name = content.remove("Profile").unwrap_or(Value::Null);
                content.insert("Profile".into(), serde_json::json!({ "name": name }));
                true
            }
            Some(Value::Object(content)) => !["Text", "Link", "Location"].iter().any(|k| content.contains_key(*k)),
            _ => true,
        }
    };
    for name in ["events", "handed"] {
        if let Some(mut events) = list(&mut json, name) {
            events.retain_mut(event);
            json[name] = Value::Array(events);
        }
    }
    if let Some(mut held) = list(&mut json, "group_held") {
        held.retain_mut(|pair| pair.get_mut(1).is_none_or(event));
        json["group_held"] = Value::Array(held);
    }
    Ok((serde_json::from_value(json).map_err(|_| Error::State)?, records))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An I/O error is read again later, never taken as a missing storage key (which would cost its last copy).
    #[test]
    fn an_io_error_is_no_answer_and_only_a_missing_row_is_absent() {
        let io = || rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR_READ), None);
        assert!(!absent(&io()));
        assert!(matches!(read_error(io()), Error::Network(_)));
        let corrupt = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT), None);
        assert!(!absent(&corrupt));
        let conn = Connection::open_in_memory().unwrap();
        let query = |sql: &str| conn.query_row(sql, [], |row| row.get::<_, Vec<u8>>(0)).unwrap_err();
        assert!(absent(&query("SELECT legacy_storage_key FROM account_storage")));
        conn.execute_batch("CREATE TABLE account_storage (owner TEXT, entry TEXT)").unwrap();
        assert!(absent(&query("SELECT legacy_storage_key FROM account_storage")));
        conn.execute_batch("ALTER TABLE account_storage ADD COLUMN legacy_storage_key BLOB").unwrap();
        assert!(absent(&query("SELECT legacy_storage_key FROM account_storage")));
    }
}
