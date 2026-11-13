//! The hub's store: SQLite, one connection, every method one transaction. What it holds is listed in spec 1: users
//! with their keys, devices and handles; envelopes waiting for devices; the order of each group's commits. No content
//! key, no sender, no membership.

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::sync::{Mutex, MutexGuard, PoisonError};
use yo_wire::handle::{KIND_ACCOUNT_KEY, KIND_USER, KIND_USERNAME};
use yo_wire::{DeviceId, GroupId, UserId, KEY_LEN};

pub type Key = [u8; KEY_LEN];

pub struct Db {
    conn: Mutex<Connection>,
}

/// A device that signs its requests.
pub struct Device {
    pub user_id: UserId,
    pub device_key: Key,
    pub auth_key: Key,
    /// When it last made a request, to the day (`touch`).
    pub last_seen_at: u64,
}

pub struct NewDevice {
    pub device_id: DeviceId,
    pub device_key: Key,
    pub auth_key: Key,
    pub push_platform: u8,
    pub push_token: Vec<u8>,
}

/// A KeyPackage the hub checked: its bytes and the hash it is remembered by.
pub struct StoredKeyPackage {
    pub hash: [u8; 32],
    pub data: Vec<u8>,
}

/// A user found by a handle.
pub struct Found {
    pub user_id: UserId,
    pub account_key: Key,
    pub device_keys: Vec<Key>,
    pub key_packages: Vec<Vec<u8>>,
}

/// A user's current account key and devices: what a delivery's device check compares with (spec 4.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roster {
    pub account_key: Key,
    pub device_keys: Vec<Key>,
}

/// A device to wake after a delivery.
pub struct Wake {
    pub device_id: DeviceId,
    pub push_platform: u8,
    pub push_token: Vec<u8>,
}

pub enum Delivered {
    Stored(Vec<Wake>),
    /// The sender's view of the user's devices is not the hub's: nothing stored.
    Mismatch(Roster),
    /// A device of the user holds as many envelopes as it may: later.
    Full,
    Unknown,
}

pub enum Commit {
    Accepted,
    Stale(u64),
    BadToken,
    Unknown,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Conflict {
    DeviceTaken,
    KeyPackageSeen,
    /// The account key is, or was, another user's.
    KeyTaken,
    UsernameTaken,
    /// Another user's backup has this locator, or a user this id.
    LocatorTaken,
    /// The device was replaced, or its account removed.
    DeviceGone,
}

pub enum Write<T> {
    Done(T),
    Refused(Conflict),
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS users (
    id BLOB PRIMARY KEY,
    account_key BLOB NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
) WITHOUT ROWID;
-- Every account key a user ever had: a key belongs to one user forever.
CREATE TABLE IF NOT EXISTS account_keys (
    key BLOB PRIMARY KEY,
    user_id BLOB NOT NULL REFERENCES users(id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS account_keys_by_user ON account_keys(user_id);
-- How people find a user: a phone number as its HMAC, a username by its skeleton (spec 3.3).
CREATE TABLE IF NOT EXISTS handles (
    kind INTEGER NOT NULL,
    value BLOB NOT NULL,
    user_id BLOB NOT NULL REFERENCES users(id),
    PRIMARY KEY (kind, value)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS handles_by_user ON handles(user_id, kind);
CREATE TABLE IF NOT EXISTS devices (
    id BLOB PRIMARY KEY,
    user_id BLOB NOT NULL REFERENCES users(id),
    device_key BLOB NOT NULL UNIQUE,
    auth_key BLOB NOT NULL,
    push_platform INTEGER NOT NULL,
    push_token BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS devices_by_user ON devices(user_id);
-- Devices that were replaced: their own requests answer 410.
CREATE TABLE IF NOT EXISTS gone (
    id BLOB PRIMARY KEY,
    gone_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS key_packages (
    id INTEGER PRIMARY KEY,
    device_id BLOB NOT NULL,
    data BLOB NOT NULL,
    last_resort INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS key_packages_by_device ON key_packages(device_id, last_resort, id);
CREATE TABLE IF NOT EXISTS seen_key_packages (
    hash BLOB PRIMARY KEY
) WITHOUT ROWID;
-- AUTOINCREMENT: an id is never handed out twice, so an ack can never delete an envelope it did not see.
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id BLOB NOT NULL,
    envelope BLOB NOT NULL,
    received_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_by_device ON messages(device_id, id);
CREATE INDEX IF NOT EXISTS messages_by_age ON messages(received_at);
CREATE TABLE IF NOT EXISTS groups (
    id BLOB PRIMARY KEY,
    epoch INTEGER NOT NULL,
    token_hash BLOB NOT NULL,
    updated_at INTEGER NOT NULL
) WITHOUT ROWID;
-- Every accepted commit, by the epoch it built on: handed out to members who missed it, and the proof that a
-- repeated commit was accepted.
CREATE TABLE IF NOT EXISTS commits (
    group_id BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    next_token_hash BLOB NOT NULL,
    envelope BLOB NOT NULL,
    accepted_at INTEGER NOT NULL,
    PRIMARY KEY (group_id, epoch)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS commits_by_age ON commits(accepted_at);
-- Each user's backup record (spec 7), as the client sent it: found by the user id or by its locator. A user with one
-- has secured the name, which is then never released (spec 6.4).
CREATE TABLE IF NOT EXISTS backups (
    user_id BLOB PRIMARY KEY REFERENCES users(id),
    locator BLOB UNIQUE,
    record BLOB NOT NULL,
    updated_at INTEGER NOT NULL
) WITHOUT ROWID;
";

fn array<const N: usize>(bytes: Vec<u8>) -> rusqlite::Result<[u8; N]> {
    bytes.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn roster(tx: &Transaction<'_>, user: &UserId) -> rusqlite::Result<Option<Roster>> {
    let Some(account_key) = tx
        .prepare_cached("SELECT account_key FROM users WHERE id = ?1")?
        .query_row([user], |row| array(row.get(0)?))
        .optional()?
    else {
        return Ok(None);
    };
    let device_keys = tx
        .prepare_cached("SELECT device_key FROM devices WHERE user_id = ?1 ORDER BY device_key")?
        .query_map([user], |row| array(row.get(0)?))?
        .collect::<Result<_, _>>()?;
    Ok(Some(Roster { account_key, device_keys }))
}

fn insert_key_packages(
    tx: &Transaction<'_>,
    device: &DeviceId,
    key_packages: &[StoredKeyPackage],
    last_resort: Option<&StoredKeyPackage>,
) -> rusqlite::Result<Option<Conflict>> {
    let mut seen = tx.prepare_cached("INSERT INTO seen_key_packages (hash) VALUES (?1) ON CONFLICT DO NOTHING")?;
    let mut insert =
        tx.prepare_cached("INSERT INTO key_packages (device_id, data, last_resort) VALUES (?1, ?2, ?3)")?;
    for (key_package, last) in key_packages.iter().map(|k| (k, false)).chain(last_resort.map(|k| (k, true))) {
        if seen.execute([&key_package.hash])? == 0 {
            return Ok(Some(Conflict::KeyPackageSeen));
        }
        if last {
            tx.execute("DELETE FROM key_packages WHERE device_id = ?1 AND last_resort = 1", [device])?;
        }
        insert.execute(params![device, key_package.data, last])?;
    }
    Ok(None)
}

/// Removes a user's devices with their KeyPackages and envelopes. Their own requests answer 410 from then on.
fn retire_devices(tx: &Transaction<'_>, user: &UserId, now: u64) -> rusqlite::Result<()> {
    let devices: Vec<Vec<u8>> = tx
        .prepare_cached("SELECT id FROM devices WHERE user_id = ?1")?
        .query_map([user], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    for id in &devices {
        tx.execute("INSERT OR REPLACE INTO gone (id, gone_at) VALUES (?1, ?2)", params![id, now])?;
        tx.execute("DELETE FROM key_packages WHERE device_id = ?1", [id])?;
        tx.execute("DELETE FROM messages WHERE device_id = ?1", [id])?;
        tx.execute("DELETE FROM devices WHERE id = ?1", [id])?;
    }
    Ok(())
}

/// The schema this hub writes. A database of another (or a pre-release) schema is refused, never half migrated.
const SCHEMA_VERSION: i64 = 1;

fn check_version(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let tables: i64 =
        conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table'", [], |row| row.get(0))?;
    match version {
        SCHEMA_VERSION => Ok(()),
        0 if tables == 0 => conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}")),
        _ => Err(rusqlite::Error::InvalidParameterName(format!(
            "database schema {version}, this hub writes {SCHEMA_VERSION}: start from an empty database"
        ))),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        check_version(&conn)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ---- users and devices ----

    pub fn device(&self, device: &DeviceId) -> rusqlite::Result<Option<Device>> {
        self.conn()
            .prepare_cached("SELECT user_id, device_key, auth_key, last_seen_at FROM devices WHERE id = ?1")?
            .query_row([device], |row| {
                Ok(Device {
                    user_id: array(row.get(0)?)?,
                    device_key: array(row.get(1)?)?,
                    auth_key: array(row.get(2)?)?,
                    last_seen_at: row.get::<_, i64>(3)? as u64,
                })
            })
            .optional()
    }

    pub fn is_gone(&self, device: &DeviceId) -> rusqlite::Result<bool> {
        self.conn().prepare_cached("SELECT 1 FROM gone WHERE id = ?1")?.exists([device])
    }

    pub fn touch(&self, device: &DeviceId, now: u64) -> rusqlite::Result<()> {
        self.conn()
            .prepare_cached("UPDATE devices SET last_seen_at = ?2 WHERE id = ?1")?
            .execute(params![device, now])?;
        Ok(())
    }

    pub fn account_key(&self, user: &UserId) -> rusqlite::Result<Option<Key>> {
        self.conn()
            .prepare_cached("SELECT account_key FROM users WHERE id = ?1")?
            .query_row([user], |row| array(row.get(0)?))
            .optional()
    }

    /// Registers a device for the user a handle names (spec 6.4). A free handle makes a new user, `fresh`. A phone
    /// number keeps its user (a new phone keeps the user id). A username (`by_key`) is its user's again only with the
    /// user's current account key (a new phone restored from the backup); with any other it is taken. The account key
    /// becomes the user's current key and stays the user's forever. The user's earlier devices are replaced.
    #[allow(clippy::too_many_arguments)]
    pub fn register(
        &self,
        handle_kind: u8,
        handle_value: &[u8],
        by_key: bool,
        fresh: &UserId,
        account_key: &Key,
        device: &NewDevice,
        key_packages: &[StoredKeyPackage],
        last_resort: &StoredKeyPackage,
        now: u64,
    ) -> rusqlite::Result<Write<UserId>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let taken = tx
            .prepare_cached(
                "SELECT 1 FROM devices WHERE id = ?1 OR device_key = ?2 UNION ALL SELECT 1 FROM gone WHERE id = ?1",
            )?
            .exists(params![device.device_id, device.device_key])?;
        if taken {
            return Ok(Write::Refused(Conflict::DeviceTaken));
        }
        let holder: Option<UserId> = tx
            .prepare_cached("SELECT user_id FROM handles WHERE kind = ?1 AND value = ?2")?
            .query_row(params![handle_kind, handle_value], |row| array(row.get(0)?))
            .optional()?;
        let user = match holder {
            Some(user) if by_key => {
                let current: Option<Key> = tx
                    .prepare_cached("SELECT account_key FROM users WHERE id = ?1")?
                    .query_row([&user], |row| array(row.get(0)?))
                    .optional()?;
                if current != Some(*account_key) {
                    return Ok(Write::Refused(Conflict::UsernameTaken));
                }
                user
            }
            Some(user) => user,
            None => *fresh,
        };
        let key_owner: Option<UserId> = tx
            .prepare_cached("SELECT user_id FROM account_keys WHERE key = ?1")?
            .query_row([account_key], |row| array(row.get(0)?))
            .optional()?;
        if key_owner.is_some_and(|owner| owner != user) {
            return Ok(Write::Refused(Conflict::KeyTaken));
        }
        retire_devices(&tx, &user, now)?;
        tx.execute(
            "INSERT INTO users (id, account_key, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (id) DO UPDATE SET account_key = excluded.account_key",
            params![user, account_key, now],
        )?;
        tx.execute("INSERT OR IGNORE INTO account_keys (key, user_id) VALUES (?1, ?2)", params![account_key, user])?;
        tx.execute(
            "INSERT OR IGNORE INTO handles (kind, value, user_id) VALUES (?1, ?2, ?3)",
            params![handle_kind, handle_value, user],
        )?;
        tx.execute(
            "INSERT INTO devices (id, user_id, device_key, auth_key, push_platform, push_token, created_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                device.device_id,
                user,
                device.device_key,
                device.auth_key,
                device.push_platform,
                device.push_token,
                now
            ],
        )?;
        if let Some(conflict) = insert_key_packages(&tx, &device.device_id, key_packages, Some(last_resort))? {
            return Ok(Write::Refused(conflict));
        }
        tx.commit()?;
        Ok(Write::Done(user))
    }

    /// Claims a username for a user, giving up its old one; `None` only gives it up.
    pub fn set_username(&self, user: &UserId, name: Option<&[u8]>) -> rusqlite::Result<Write<()>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(name) = name {
            let owner: Option<UserId> = tx
                .prepare_cached("SELECT user_id FROM handles WHERE kind = ?1 AND value = ?2")?
                .query_row(params![KIND_USERNAME, name], |row| array(row.get(0)?))
                .optional()?;
            if owner.is_some_and(|owner| owner != *user) {
                return Ok(Write::Refused(Conflict::UsernameTaken));
            }
        }
        tx.execute("DELETE FROM handles WHERE user_id = ?1 AND kind = ?2", params![user, KIND_USERNAME])?;
        if let Some(name) = name {
            tx.execute(
                "INSERT INTO handles (kind, value, user_id) VALUES (?1, ?2, ?3)",
                params![KIND_USERNAME, name, user],
            )?;
        }
        tx.commit()?;
        Ok(Write::Done(()))
    }

    /// Whether a handle (`value` as stored) belongs to anyone: what registration would find taken.
    pub fn handle_taken(&self, kind: u8, value: &[u8]) -> rusqlite::Result<bool> {
        self.conn().prepare_cached("SELECT 1 FROM handles WHERE kind = ?1 AND value = ?2")?.exists(params![kind, value])
    }

    /// A user by a handle (`value` as stored: a phone number's HMAC, a username in upper case, a user id, an account
    /// key, which must be the user's current one): id, account key, devices and, if asked, one KeyPackage for each
    /// device not in `except`, a single-use one while there is one, else the last resort.
    pub fn lookup(&self, kind: u8, value: &[u8], take: bool, except: &[Key]) -> rusqlite::Result<Option<Found>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let user: Option<UserId> = match kind {
            KIND_USER => {
                tx.prepare_cached("SELECT id FROM users WHERE id = ?1")?.query_row([value], |row| array(row.get(0)?))
            }
            KIND_ACCOUNT_KEY => tx
                .prepare_cached("SELECT id FROM users WHERE account_key = ?1")?
                .query_row([value], |row| array(row.get(0)?)),
            _ => tx
                .prepare_cached("SELECT user_id FROM handles WHERE kind = ?1 AND value = ?2")?
                .query_row(params![kind, value], |row| array(row.get(0)?)),
        }
        .optional()?;
        let Some(user) = user else { return Ok(None) };
        let Some(Roster { account_key, device_keys }) = roster(&tx, &user)? else { return Ok(None) };
        if device_keys.is_empty() {
            return Ok(None);
        }
        let mut key_packages = Vec::new();
        if take {
            let devices: Vec<(Vec<u8>, Key)> = tx
                .prepare_cached("SELECT id, device_key FROM devices WHERE user_id = ?1 ORDER BY device_key")?
                .query_map([&user], |row| Ok((row.get(0)?, array(row.get(1)?)?)))?
                .collect::<Result<_, _>>()?;
            for (device, _) in devices.iter().filter(|(_, key)| !except.contains(key)) {
                let found: Option<(i64, Vec<u8>, bool)> = tx
                    .prepare_cached(
                        "SELECT id, data, last_resort FROM key_packages WHERE device_id = ?1
                         ORDER BY last_resort, id LIMIT 1",
                    )?
                    .query_row([device], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .optional()?;
                if let Some((id, data, last_resort)) = found {
                    if !last_resort {
                        tx.execute("DELETE FROM key_packages WHERE id = ?1", [id])?;
                    }
                    key_packages.push(data);
                }
            }
        }
        tx.commit()?;
        Ok(Some(Found { user_id: user, account_key, device_keys, key_packages }))
    }

    // ---- backups ----

    /// Stores the backup record of a device's user, replacing the one before: the old locator finds nothing from then
    /// on. The device is checked in this transaction: one replaced since it authenticated (a new phone restored the
    /// account) must not overwrite the new phone's record. A locator that is another user's, or any user's id, is
    /// refused: a locator never stands in for a user id.
    pub fn put_backup(
        &self,
        device: &DeviceId,
        locator: Option<&[u8]>,
        record: &[u8],
        now: u64,
    ) -> rusqlite::Result<Write<()>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let user: Option<UserId> = tx
            .prepare_cached("SELECT user_id FROM devices WHERE id = ?1")?
            .query_row([device], |row| array(row.get(0)?))
            .optional()?;
        let Some(user) = user else { return Ok(Write::Refused(Conflict::DeviceGone)) };
        if let Some(locator) = locator {
            let taken = tx
                .prepare_cached(
                    "SELECT 1 FROM backups WHERE locator = ?1 AND user_id != ?2
                     UNION ALL SELECT 1 FROM users WHERE id = ?1",
                )?
                .exists(params![locator, user])?;
            if taken {
                return Ok(Write::Refused(Conflict::LocatorTaken));
            }
        }
        tx.prepare_cached(
            "INSERT INTO backups (user_id, locator, record, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (user_id) DO UPDATE SET
                 locator = excluded.locator, record = excluded.record, updated_at = excluded.updated_at",
        )?
        .execute(params![user, locator, record, now])?;
        tx.commit()?;
        Ok(Write::Done(()))
    }

    /// A backup record by its user id or, where no user has that id, by its locator: what is asked for by a user's
    /// id is that user's record or none, never one whose locator happens to be the same.
    pub fn backup(&self, id: &[u8]) -> rusqlite::Result<Option<Vec<u8>>> {
        self.conn()
            .prepare_cached(
                "SELECT record FROM backups WHERE user_id = ?1
                 UNION ALL SELECT record FROM backups WHERE locator = ?1 AND NOT EXISTS (SELECT 1 FROM users WHERE id = ?1)
                 LIMIT 1",
            )?
            .query_row([id], |row| row.get(0))
            .optional()
    }

    // ---- KeyPackages and push ----

    pub fn add_key_packages(
        &self,
        device: &DeviceId,
        key_packages: &[StoredKeyPackage],
        last_resort: Option<&StoredKeyPackage>,
        most: u32,
    ) -> rusqlite::Result<Write<()>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let held: u32 = tx
            .prepare_cached("SELECT count(*) FROM key_packages WHERE device_id = ?1 AND last_resort = 0")?
            .query_row([device], |row| row.get(0))?;
        if held as usize + key_packages.len() > most as usize {
            // More than a device needs: refused rather than stored without end.
            return Ok(Write::Refused(Conflict::KeyPackageSeen));
        }
        if let Some(conflict) = insert_key_packages(&tx, device, key_packages, last_resort)? {
            return Ok(Write::Refused(conflict));
        }
        tx.commit()?;
        Ok(Write::Done(()))
    }

    pub fn key_packages_left(&self, device: &DeviceId) -> rusqlite::Result<u32> {
        self.conn()
            .prepare_cached("SELECT count(*) FROM key_packages WHERE device_id = ?1 AND last_resort = 0")?
            .query_row([device], |row| row.get(0))
    }

    pub fn set_push(&self, device: &DeviceId, platform: u8, token: &[u8]) -> rusqlite::Result<()> {
        self.conn()
            .prepare_cached("UPDATE devices SET push_platform = ?2, push_token = ?3 WHERE id = ?1")?
            .execute(params![device, platform, token])?;
        Ok(())
    }

    /// Clears a push token the push service refused, unless the device set another meanwhile.
    pub fn clear_push(&self, device: &DeviceId, token: &[u8]) -> rusqlite::Result<()> {
        self.conn()
            .prepare_cached("UPDATE devices SET push_platform = 0, push_token = x'' WHERE id = ?1 AND push_token = ?2")?
            .execute(params![device, token])?;
        Ok(())
    }

    // ---- delivery ----

    /// Delivers envelopes to users, all in one transaction (group commit). For each: the sender's device hash is
    /// compared with the user's current roster, and the envelope is stored for every device, or for none.
    pub fn deliver(
        &self,
        items: &[(UserId, [u8; 32], &[u8])],
        now: u64,
        most: u32,
    ) -> rusqlite::Result<Vec<Delivered>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut outcomes = Vec::with_capacity(items.len());
        for (user, devices_hash, envelope) in items {
            let Some(current) = roster(&tx, user)? else {
                outcomes.push(Delivered::Unknown);
                continue;
            };
            if current.device_keys.is_empty() {
                outcomes.push(Delivered::Unknown);
                continue;
            }
            if yo_wire::devices::hash(&current.account_key, &current.device_keys) != *devices_hash {
                outcomes.push(Delivered::Mismatch(current));
                continue;
            }
            let devices: Vec<Wake> = tx
                .prepare_cached("SELECT id, push_platform, push_token FROM devices WHERE user_id = ?1")?
                .query_map([user], |row| {
                    Ok(Wake { device_id: array(row.get(0)?)?, push_platform: row.get(1)?, push_token: row.get(2)? })
                })?
                .collect::<Result<_, _>>()?;
            // Bounded: counts at most `most` rows per device.
            let mut held =
                tx.prepare_cached("SELECT count(*) FROM (SELECT 1 FROM messages WHERE device_id = ?1 LIMIT ?2)")?;
            let mut full = false;
            for device in &devices {
                full |= held.query_row(params![device.device_id, most], |row| row.get::<_, u32>(0))? >= most;
            }
            if full {
                outcomes.push(Delivered::Full);
                continue;
            }
            let mut insert =
                tx.prepare_cached("INSERT INTO messages (device_id, envelope, received_at) VALUES (?1, ?2, ?3)")?;
            for device in &devices {
                insert.execute(params![device.device_id, envelope, now])?;
            }
            outcomes.push(Delivered::Stored(devices));
        }
        tx.commit()?;
        Ok(outcomes)
    }

    pub fn fetch(&self, device: &DeviceId, most: usize) -> rusqlite::Result<Vec<(u64, Vec<u8>)>> {
        self.conn()
            .prepare_cached("SELECT id, envelope FROM messages WHERE device_id = ?1 ORDER BY id LIMIT ?2")?
            .query_map(params![device, most as i64], |row| Ok((row.get::<_, i64>(0)? as u64, row.get(1)?)))?
            .collect()
    }

    pub fn ack(&self, device: &DeviceId, ids: &[u64]) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut delete = tx.prepare_cached("DELETE FROM messages WHERE device_id = ?1 AND id = ?2")?;
            for id in ids {
                delete.execute(params![device, *id as i64])?;
            }
        }
        tx.commit()
    }

    // ---- groups ----

    /// Registers a group at an epoch; false if the id exists.
    pub fn create_group(&self, group: &GroupId, epoch: u64, token_hash: &[u8; 32], now: u64) -> rusqlite::Result<bool> {
        let changed = self
            .conn()
            .prepare_cached(
                "INSERT INTO groups (id, epoch, token_hash, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (id) DO NOTHING",
            )?
            .execute(params![group, epoch as i64, token_hash, now])?;
        Ok(changed == 1)
    }

    /// Orders a commit (spec 4.3): accepted only on the group's current epoch with that epoch's token, and kept. A
    /// commit accepted before is accepted again: the same epoch and next-token hash, which only it can produce.
    pub fn commit(
        &self,
        group: &GroupId,
        epoch: u64,
        token_hash: &[u8; 32],
        next_token_hash: &[u8; 32],
        envelope: &[u8],
        now: u64,
    ) -> rusqlite::Result<Commit> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let accepted: Option<Vec<u8>> = tx
            .prepare_cached("SELECT next_token_hash FROM commits WHERE group_id = ?1 AND epoch = ?2")?
            .query_row(params![group, epoch as i64], |row| row.get(0))
            .optional()?;
        if accepted.as_deref().is_some_and(|hash| constant_time_eq(hash, next_token_hash)) {
            return Ok(Commit::Accepted);
        }
        let current: Option<(i64, Vec<u8>)> = tx
            .prepare_cached("SELECT epoch, token_hash FROM groups WHERE id = ?1")?
            .query_row([group], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        let Some((current, stored)) = current else {
            return Ok(Commit::Unknown);
        };
        if current as u64 != epoch {
            return Ok(Commit::Stale(current as u64));
        }
        if !constant_time_eq(&stored, token_hash) {
            return Ok(Commit::BadToken);
        }
        tx.execute(
            "UPDATE groups SET epoch = ?2, token_hash = ?3, updated_at = ?4 WHERE id = ?1",
            params![group, (epoch + 1) as i64, next_token_hash, now],
        )?;
        tx.execute(
            "INSERT INTO commits (group_id, epoch, next_token_hash, envelope, accepted_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![group, epoch as i64, next_token_hash, envelope, now],
        )?;
        tx.commit()?;
        Ok(Commit::Accepted)
    }

    /// The envelope of the commit accepted on `epoch`.
    pub fn commit_envelope(&self, group: &GroupId, epoch: u64) -> rusqlite::Result<Option<Vec<u8>>> {
        self.conn()
            .prepare_cached("SELECT envelope FROM commits WHERE group_id = ?1 AND epoch = ?2")?
            .query_row(params![group, epoch as i64], |row| row.get(0))
            .optional()
    }

    /// Deletes envelopes and kept commits older than the TTL, and forgets replaced devices after a year.
    pub fn prune(&self, now: u64, ttl_ms: u64) -> rusqlite::Result<usize> {
        let conn = self.conn();
        let before = now.saturating_sub(ttl_ms) as i64;
        let messages = conn.execute("DELETE FROM messages WHERE received_at < ?1", [before])?;
        conn.execute("DELETE FROM commits WHERE accepted_at < ?1", [before])?;
        conn.execute("DELETE FROM gone WHERE gone_at < ?1", [now.saturating_sub(365 * 24 * 3600 * 1000) as i64])?;
        Ok(messages)
    }

    /// Releases names not secured (spec 6.4): removes every user without a backup record none of whose devices made
    /// a request for `idle_ms`, with its handles and account keys. Its devices answer 410 from then on. Returns how
    /// many users were removed.
    pub fn release_unsecured(&self, now: u64, idle_ms: u64) -> rusqlite::Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before = now.saturating_sub(idle_ms) as i64;
        let idle: Vec<UserId> = tx
            .prepare_cached(
                "SELECT id FROM users WHERE created_at < ?1
                 AND NOT EXISTS (SELECT 1 FROM backups WHERE user_id = users.id)
                 AND NOT EXISTS (SELECT 1 FROM devices WHERE user_id = users.id AND last_seen_at >= ?1)",
            )?
            .query_map([before], |row| array(row.get(0)?))?
            .collect::<Result<_, _>>()?;
        for user in &idle {
            retire_devices(&tx, user, now)?;
            tx.execute("DELETE FROM handles WHERE user_id = ?1", [user])?;
            tx.execute("DELETE FROM account_keys WHERE user_id = ?1", [user])?;
            tx.execute("DELETE FROM users WHERE id = ?1", [user])?;
        }
        tx.commit()?;
        Ok(idle.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 3600 * 1000;

    fn device(n: u8) -> NewDevice {
        NewDevice {
            device_id: [n; 16],
            device_key: [n; 32],
            auth_key: [n; 32],
            push_platform: 0,
            push_token: Vec::new(),
        }
    }

    /// Registers device `n` for a username with account key `key`.
    fn register(db: &Db, name: &[u8], key: u8, n: u8, now: u64) -> Write<UserId> {
        let last_resort = StoredKeyPackage { hash: [n; 32], data: vec![n] };
        db.register(KIND_USERNAME, name, true, &[n; 16], &[key; 32], &device(n), &[], &last_resort, now).unwrap()
    }

    fn user(written: Write<UserId>) -> UserId {
        match written {
            Write::Done(user) => user,
            Write::Refused(conflict) => panic!("{conflict:?}"),
        }
    }

    #[test]
    fn a_username_is_its_users_again_only_with_the_current_key() {
        let db = Db::open(":memory:").unwrap();
        let first = user(register(&db, b"LEO", 1, 1, 0));
        assert!(matches!(register(&db, b"LEO", 2, 2, 0), Write::Refused(Conflict::UsernameTaken)));
        // A new phone with the account key from the backup: the same user, the old device gone.
        assert_eq!(user(register(&db, b"LEO", 1, 3, 0)), first);
        assert!(db.device(&[1; 16]).unwrap().is_none());
        assert!(db.is_gone(&[1; 16]).unwrap());
        assert_eq!(db.device(&[3; 16]).unwrap().unwrap().user_id, first);
        // The key cannot take a free name as another user.
        assert!(matches!(register(&db, b"MIA", 1, 4, 0), Write::Refused(Conflict::KeyTaken)));
    }

    #[test]
    fn backups_are_found_by_user_id_or_current_locator() {
        let db = Db::open(":memory:").unwrap();
        // The test's users get the ids of their first devices: ANA [1; 16], device [1; 16]; BEN [2; 16], device [2; 16].
        let (ana, ben) = (&[1; 16], &[2; 16]);
        let a = user(register(&db, b"ANA", 1, 1, 0));
        user(register(&db, b"BEN", 2, 2, 0));
        let found = |id: &[u8]| db.backup(id).unwrap();
        let refused = |w: Write<()>, want: Conflict| matches!(w, Write::Refused(c) if c == want);
        assert!(found(&a).is_none());
        assert!(matches!(db.put_backup(ana, Some(&[9; 16]), b"one", 0).unwrap(), Write::Done(())));
        assert_eq!(found(&a).as_deref(), Some(&b"one"[..]));
        assert_eq!(found(&[9; 16]).as_deref(), Some(&b"one"[..]));
        // Another user cannot take the locator.
        assert!(refused(db.put_backup(ben, Some(&[9; 16]), b"two", 0).unwrap(), Conflict::LocatorTaken));
        // A new record with a new locator: the old one finds nothing.
        db.put_backup(ana, Some(&[8; 16]), b"three", 0).unwrap();
        assert!(found(&[9; 16]).is_none());
        assert_eq!(found(&[8; 16]).as_deref(), Some(&b"three"[..]));
        // No locator may be a user's id.
        assert!(refused(db.put_backup(ben, Some(&a), b"four", 0).unwrap(), Conflict::LocatorTaken));
        // Without a locator: by the user id only.
        db.put_backup(ana, None, b"five", 0).unwrap();
        assert!(found(&[8; 16]).is_none());
        assert_eq!(found(&a).as_deref(), Some(&b"five"[..]));
    }

    #[test]
    fn a_user_id_never_finds_another_users_record_by_locator() {
        let db = Db::open(":memory:").unwrap();
        user(register(&db, b"BEN", 2, 2, 0));
        // BEN's locator is [7; 16]; then a user comes to have that id. Asked by its id, the hub has no record for it.
        db.put_backup(&[2; 16], Some(&[7; 16]), b"ben", 0).unwrap();
        assert_eq!(user(register(&db, b"SEVEN", 7, 7, 0)), [7; 16]);
        assert!(db.backup(&[7; 16]).unwrap().is_none());
    }

    #[test]
    fn a_replaced_device_cannot_store_a_backup() {
        let db = Db::open(":memory:").unwrap();
        let first = user(register(&db, b"LEO", 1, 1, 0));
        // A new phone restores the account and stores its record; the old phone's late request must not replace it.
        user(register(&db, b"LEO", 1, 3, 0));
        db.put_backup(&[3; 16], None, b"new", 0).unwrap();
        assert!(matches!(db.put_backup(&[1; 16], None, b"old", 0).unwrap(), Write::Refused(Conflict::DeviceGone)));
        assert_eq!(db.backup(&first).unwrap().as_deref(), Some(&b"new"[..]));
    }

    #[test]
    fn names_not_secured_are_released_after_the_idle_time() {
        let db = Db::open(":memory:").unwrap();
        let idle = user(register(&db, b"IDLE", 1, 1, 0));
        let secured = user(register(&db, b"SAFE", 2, 2, 0));
        let active = user(register(&db, b"BUSY", 3, 3, 0));
        db.put_backup(&[2; 16], None, b"record", 0).unwrap();
        db.touch(&[3; 16], 50 * DAY).unwrap();
        let exists = |user: &UserId| db.lookup(KIND_USER, user, false, &[]).unwrap().is_some();
        assert_eq!(db.release_unsecured(89 * DAY, 90 * DAY).unwrap(), 0);
        assert_eq!(db.release_unsecured(91 * DAY, 90 * DAY).unwrap(), 1);
        assert!(!exists(&idle));
        assert!(db.is_gone(&[1; 16]).unwrap(), "its device learns that it was removed");
        assert!(exists(&secured) && exists(&active));
        // The name is free for anyone now, and so is the key.
        user(register(&db, b"IDLE", 4, 4, 91 * DAY));
        user(register(&db, b"OTHER", 1, 5, 91 * DAY));
        assert_eq!(db.release_unsecured(200 * DAY, 90 * DAY).unwrap(), 3, "BUSY and the two new ones, never SAFE");
        assert!(exists(&secured));
    }
}
