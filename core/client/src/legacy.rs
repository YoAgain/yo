//! Remove once every keychain item is v3.
use crate::keychain::{Item, ITEM_VERSION};
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

/// An old install's state file, at one place, with the state keys the platform could read there.
pub struct Source {
    pub path: std::path::PathBuf,
    pub format: u8,
    pub keys: Vec<[u8; 32]>,
    /// The keychain did not answer for this place's keys: its files are not readable now.
    pub keys_unavailable: bool,
}

/// The identity of an old install's local state, and whether some of it could not be read now.
pub struct Local {
    pub identity: Option<Identity>,
    pub not_now: bool,
}

/// The Ed25519 seed the old preferences kept (`accountKey`, base64), if any.
fn preferences_seed(saved: Option<&[u8]>) -> Option<[u8; 32]> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Preferences {
        account_key: Option<String>,
    }
    use base64::{engine::general_purpose::STANDARD, Engine};
    let key = serde_json::from_slice::<Preferences>(saved?).ok()?.account_key?;
    STANDARD.decode(key).ok()?.try_into().ok()
}

/// An old install not signed in again yet: old state (or the preferences' seed), and no state of today's that signed
/// in (`signed_in`: None while not readable).
pub fn pending(has_account: bool, saved: Option<&[u8]>, has_old_state: bool, signed_in: Option<bool>) -> bool {
    has_account && (has_old_state || preferences_seed(saved).is_some()) && signed_in != Some(true)
}

/// The old local identity: the preferences' seed first, else the first state file that opens under one of its keys.
pub fn local_identity(saved: Option<&[u8]>, sources: &[Source]) -> Local {
    if let Some(seed) = preferences_seed(saved) {
        return Local { identity: Some(Identity { secret: seed, ed25519: true, storage_key: None }), not_now: false };
    }
    let mut not_now = false;
    for source in sources {
        if source.keys_unavailable {
            not_now = true;
            continue;
        }
        if !source.path.exists() {
            continue;
        }
        for key in &source.keys {
            match read_identity(&source.path, key, source.format) {
                Ok(identity) => return Local { identity: Some(identity), not_now: false },
                Err(Error::State) => continue,
                Err(_) => not_now = true,
            }
        }
    }
    Local { identity: None, not_now }
}

/// The account shown while an old install signs in again.
pub struct Shown {
    pub username: String,
    pub user_id: Vec<u8>,
    pub by_passkey: bool,
    pub by_code: Option<bool>,
}

/// What the one-time sign-in does.
#[derive(Debug, PartialEq)]
pub enum Found {
    /// Sign in with the item; `storage_key`: what the hub's storage is still sealed under.
    SignIn { item: Item, storage_key: Option<[u8; 32]> },
    /// Signed out, or taken to another phone (`left`): never signed in again silently.
    SignedOut,
    /// No key anywhere.
    Missing,
    /// Not readable now: asked again later.
    NotNow,
}

/// The item to sign `shown` in again with: the iCloud Keychain's (`stored`), else one made from the old local state.
pub fn sign_in(
    stored: Option<Item>,
    local: Local,
    shown: &Shown,
    recovery_key: Option<String>,
    left: Option<&[u8]>,
) -> Found {
    let from_local = local.identity.as_ref().and_then(|identity| {
        let ed = identity.ed25519;
        crate::keychain::upgraded(&Item {
            username: shown.username.clone(),
            user_id: shown.user_id.clone(),
            account_key: identity.secret.to_vec(),
            by_passkey: !ed && shown.by_passkey,
            by_code: Some(!ed && shown.by_code == Some(true)),
            code_auth: None,
            passkey_auth: None,
            recovery_key: if ed { None } else { recovery_key },
            signed_out: Some(false),
            version: if ed { None } else { Some(ITEM_VERSION) },
            legacy_seed: None,
        })
    });
    let Some(mut item) = stored.or_else(|| from_local.clone()) else {
        return if local.not_now { Found::NotNow } else { Found::Missing };
    };
    let same = from_local.as_ref().is_some_and(|own| own.account_key == item.account_key);
    if item.legacy_seed.is_none() && same {
        item.legacy_seed = from_local.and_then(|own| own.legacy_seed);
    }
    if item.legacy_seed.is_none() && !same && local.not_now {
        return Found::NotNow;
    }
    if item.signed_out == Some(true) || left == Some(item.user_id.as_slice()) {
        return Found::SignedOut;
    }
    let storage_key = if same { local.identity.and_then(|identity| identity.storage_key) } else { None };
    Found::SignIn { item, storage_key }
}

/// A state file of the build before the database (YoGPT's `state`), in today's form: the same device, its sessions
/// and what waits, so the bot goes on without registering again (app/cli/README.md).
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

    fn shown() -> Shown {
        Shown { username: "Jo".into(), user_id: vec![1; 16], by_passkey: true, by_code: Some(true) }
    }

    fn local(secret: u8, ed25519: bool, not_now: bool) -> Local {
        Local { identity: Some(Identity { secret: [secret; 32], ed25519, storage_key: Some([9; 32]) }), not_now }
    }

    fn stored(secret: u8) -> Item {
        let mut item =
            crate::keychain::decode(include_bytes!("../../../tests/fixtures/keychain-v1/item-v3.json")).unwrap();
        item.user_id = vec![1; 16];
        item.account_key = vec![secret; 32];
        item
    }

    #[test]
    fn the_preferences_seed_counts_as_old_state_until_signed_in() {
        let saved = br#"{"accountKey":"AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE="}"#;
        assert!(pending(true, Some(saved), false, None));
        assert!(!pending(true, Some(saved), false, Some(true)));
        assert!(!pending(false, Some(saved), true, Some(false)));
        assert!(!pending(true, Some(b"{}"), false, Some(false)));
        assert!(pending(true, None, true, Some(false)));
        let found = local_identity(Some(saved), &[]);
        assert!(found.identity.is_some_and(|identity| identity.ed25519 && identity.secret == [1; 32]));
    }

    #[test]
    fn unreadable_keys_are_not_now_and_missing_files_are_nothing() {
        let source = |keys_unavailable| Source {
            path: "/nonexistent/yo-state".into(),
            format: 0,
            keys: vec![[0; 32]],
            keys_unavailable,
        };
        assert!(local_identity(None, &[source(true)]).not_now);
        let none = local_identity(None, &[source(false)]);
        assert!(none.identity.is_none() && !none.not_now);
    }

    #[test]
    fn the_keychain_item_wins_and_the_local_state_fills_in_its_storage_key() {
        // Nothing anywhere: missing, unless something was not readable.
        assert_eq!(sign_in(None, Local { identity: None, not_now: false }, &shown(), None, None), Found::Missing);
        assert_eq!(sign_in(None, Local { identity: None, not_now: true }, &shown(), None, None), Found::NotNow);
        // Only the local state: an item made from it, with its storage key.
        let Found::SignIn { item, storage_key } = sign_in(None, local(2, false, false), &shown(), None, None) else {
            panic!()
        };
        assert_eq!((item.account_key, item.by_passkey, storage_key), (vec![2; 32], true, Some([9; 32])));
        // A seed: converted, its seed kept, the ways in not taken over.
        let Found::SignIn { item, .. } = sign_in(None, local(2, true, false), &shown(), Some("k".into()), None) else {
            panic!()
        };
        assert_eq!((item.legacy_seed, item.by_passkey, item.recovery_key), (Some(vec![2; 32]), false, None));
        // The keychain's item of another key wins, without the local storage key; unread local state waits.
        let Found::SignIn { item, storage_key } =
            sign_in(Some(stored(5)), local(2, false, false), &shown(), None, None)
        else {
            panic!()
        };
        assert_eq!((item.account_key, storage_key), (vec![5; 32], None));
        assert_eq!(
            sign_in(Some(stored(5)), Local { identity: None, not_now: true }, &shown(), None, None),
            Found::NotNow
        );
        // Signed out or left: never again silently.
        let mut out = stored(5);
        out.signed_out = Some(true);
        assert_eq!(sign_in(Some(out), local(5, false, false), &shown(), None, None), Found::SignedOut);
        assert_eq!(sign_in(Some(stored(5)), local(5, false, false), &shown(), None, Some(&[1; 16])), Found::SignedOut);
    }

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
