//! The hub's store: SQLite, one connection, every method one transaction. What it holds is listed in spec 1: users
//! with their keys, devices and handles; envelopes waiting for devices; the order of each group's commits. No content
//! key, no sender, no membership.

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::sync::{Mutex, MutexGuard, PoisonError};
use yo_wire::content::BlobId;
use yo_wire::handle::{KIND_ACCOUNT_KEY, KIND_USER, KIND_USERNAME};
use yo_wire::{DeviceId, GroupId, UserId, KEY_LEN};

const DAY_MS: u64 = 24 * 3600 * 1000;
/// A blob not ready this long after it was stored belongs to no upload under way: its file never came into place.
const NOT_READY_KEPT_MS: u64 = 3600 * 1000;

pub type Key = [u8; KEY_LEN];

/// A user's storage (spec 6.9): its version and its sealed bytes.
pub type Sealed = (u64, Vec<u8>);

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
    /// The app it last named (`Yo-App`, "ios/202611140619"), if any.
    pub app: Option<String>,
}

pub struct NewDevice {
    pub device_id: DeviceId,
    pub device_key: Key,
    pub auth_key: Key,
    pub push_platform: u8,
    pub push_token: Vec<u8>,
    /// The app it names (`Yo-App`), if any.
    pub app: Option<String>,
}

/// The keys that may sign a backup record's fetch (spec 7): the passkey's, if it has one, and the code's. Neither: a
/// record stored before them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupAuth {
    pub passkey: Option<Key>,
    pub code: Option<Key>,
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
    /// The user's username as its owner writes it ("YoGPT"), if it has one.
    pub username: Option<String>,
}

/// A username nobody may claim (spec 3.3), as the operator's list writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reserved {
    pub spelling: String,
    pub note: Option<String>,
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

/// An envelope for the writer (`Db::deliver`): for a user, with the sender's device check and, if it came with one,
/// the token it spends (its key's row and its nonce).
pub struct Incoming<'a> {
    pub user: UserId,
    pub devices_hash: [u8; 32],
    pub envelope: &'a [u8],
    pub token: Option<(i64, [u8; 32])>,
}

pub enum Delivered {
    Stored(Vec<Wake>),
    /// Its token was spent before (by a delivery under way at the same time): nothing stored.
    Spent,
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

/// What one user may store in blobs (spec 6.6).
#[derive(Debug, Clone, Copy)]
pub struct BlobQuota {
    /// Blobs uploaded per day (UTC).
    pub per_day: u32,
    /// Their bytes per day.
    pub bytes_per_day: u64,
    /// Bytes of the user's blobs not yet fetched by all, at any time.
    pub stored: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BlobPut {
    Stored,
    /// A blob with this id is there.
    Taken,
    /// A blob with this id is on its way (another upload of it is not done): later.
    Unfinished,
    /// The user's quota is used up: later.
    Quota,
    /// The device was replaced, or its account removed, since it authenticated.
    DeviceGone,
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
-- How people find a user: a username by its skeleton (spec 3.3); kind 1, a phone number as its HMAC, is no longer
-- made. `spelling`: a username as its owner writes it (\"YoGPT\"); NULL for names from before spellings were kept,
-- shown as their skeleton (`migrate` adds the column to such a database).
CREATE TABLE IF NOT EXISTS handles (
    kind INTEGER NOT NULL,
    value BLOB NOT NULL,
    user_id BLOB NOT NULL REFERENCES users(id),
    spelling TEXT,
    PRIMARY KEY (kind, value)
) WITHOUT ROWID;
-- Usernames nobody may claim, by skeleton (spec 3.3): seeded from the operator's list at every start (INSERT OR
-- IGNORE), never deleted by the hub. A user who has such a name keeps it.
CREATE TABLE IF NOT EXISTS reserved (
    skeleton TEXT PRIMARY KEY,
    spelling TEXT NOT NULL,
    note TEXT,
    created_at INTEGER NOT NULL
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
    last_seen_at INTEGER NOT NULL,
    app TEXT
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
-- `first_epoch`: the epoch the group was registered at. Every epoch from it up to the current one had a commit
-- accepted (an epoch moves on by nothing else); any other never did. 1 for groups from before the column: the apps
-- register every group at epoch 1 (spec 4.2).
CREATE TABLE IF NOT EXISTS groups (
    id BLOB PRIMARY KEY,
    epoch INTEGER NOT NULL,
    token_hash BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    first_epoch INTEGER NOT NULL DEFAULT 1
) WITHOUT ROWID;
-- Every accepted commit's next-token hash, by the epoch it built on, kept for good (one small row per epoch): the
-- proof that a repeated commit was accepted (spec 4.3), also after its envelope is gone.
CREATE TABLE IF NOT EXISTS accepted (
    group_id BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    next_token_hash BLOB NOT NULL,
    PRIMARY KEY (group_id, epoch)
) WITHOUT ROWID;
-- Every accepted commit's envelope, by the epoch it built on, for 30 days: handed out to members who missed it.
CREATE TABLE IF NOT EXISTS commits (
    group_id BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    next_token_hash BLOB NOT NULL,
    envelope BLOB NOT NULL,
    accepted_at INTEGER NOT NULL,
    PRIMARY KEY (group_id, epoch)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS commits_by_age ON commits(accepted_at);
-- Each user's backup record (spec 7), as the client sent it: found by the user id or by its locator, handed out to a
-- request signed by one of its doors' auth keys (the passkey's NULL without one). A user with one has secured the
-- name, which is then never released (spec 6.4).
CREATE TABLE IF NOT EXISTS backups (
    user_id BLOB PRIMARY KEY REFERENCES users(id),
    locator BLOB UNIQUE,
    record BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    auth_passkey BLOB,
    auth_code BLOB
) WITHOUT ROWID;
-- Users reported by users (spec 6.3), for the operator to review: who reported whom, why (1 spam, 2 abuse, 3 other),
-- when. One row per pair, the latest report. No content: the hub has none.
CREATE TABLE IF NOT EXISTS reports (
    reporter_id BLOB NOT NULL,
    reported_id BLOB NOT NULL,
    reason INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (reporter_id, reported_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS reports_by_reported ON reports(reported_id);
-- Feedback on the app (spec 6.3), for the operator to read: who wrote it, the text as written (plain to the hub),
-- the app's version, when (ms). Kept when the account goes, with an all-zero user id (`NOBODY`).
CREATE TABLE IF NOT EXISTS feedback (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id BLOB NOT NULL,
    text TEXT NOT NULL,
    app_version TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS feedback_by_user ON feedback(user_id);
-- Each user's storage (spec 6.9), sealed by its devices under a key the hub never has; `version` counts its writes,
-- so one writer never overwrites another's unseen. Gone with the account.
CREATE TABLE IF NOT EXISTS storage (
    user_id BLOB PRIMARY KEY,
    version INTEGER NOT NULL,
    data BLOB NOT NULL,
    updated_at INTEGER NOT NULL
) WITHOUT ROWID;
-- Photos and videos (spec 6.8): the file is on disk, by its id; here what the hub needs to hand it out and delete it.
-- `user_id` is the uploader (for its quota), never a recipient. Gone after the last fetch, or after its time.
-- `fetching`: fetches under way, each taken from `fetches_left` as it starts and given back if it is cut off (all of
-- them at a start: none completed). `ready`: 0 until the file is in place; such a blob is not handed out, and one left
-- so (a crash in between) is removed at the next start, or within the hour.
CREATE TABLE IF NOT EXISTS blobs (
    id BLOB PRIMARY KEY,
    user_id BLOB NOT NULL,
    size INTEGER NOT NULL,
    fetches_left INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    fetching INTEGER NOT NULL DEFAULT 0,
    ready INTEGER NOT NULL DEFAULT 1
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS blobs_by_user ON blobs(user_id);
CREATE INDEX IF NOT EXISTS blobs_by_age ON blobs(created_at);
-- The Privacy Pass issuer's keys (spec 6.10), each as the seed it is derived from: the newest issues, the one before
-- still verifies what it issued. Older ones are deleted with their spent tokens.
CREATE TABLE IF NOT EXISTS token_keys (
    id INTEGER PRIMARY KEY,
    seed BLOB NOT NULL,
    created_at INTEGER NOT NULL
);
-- Tokens redeemed, by the nonce the client chose, until their key is deleted: a token is spent once. Nothing here, or
-- anywhere, ties a nonce to the device it was issued to: the hub never saw it before.
CREATE TABLE IF NOT EXISTS spent_tokens (
    nonce BLOB PRIMARY KEY,
    key_id INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS spent_tokens_by_key ON spent_tokens(key_id);
-- How many blobs, and bytes, each user uploaded on a day (days since the epoch, UTC); kept two days.
CREATE TABLE IF NOT EXISTS blob_quota (
    user_id BLOB NOT NULL,
    day INTEGER NOT NULL,
    count INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    PRIMARY KEY (user_id, day)
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

/// The user of a device, in a transaction: none for a device replaced since it authenticated.
fn user_of_device(tx: &Transaction<'_>, device: &DeviceId) -> rusqlite::Result<Option<UserId>> {
    tx.prepare_cached("SELECT user_id FROM devices WHERE id = ?1")?
        .query_row([device], |row| array(row.get(0)?))
        .optional()
}

/// The user id that feedback keeps once its sender's account is gone: no user has it (ids are random, and this one
/// is refused as a backup locator like any 16 bytes no user has).
pub const NOBODY: UserId = [0; 16];

/// Removes a user and everything about it: its devices (which answer 410 from then on), handles, account keys, backup
/// record, reports and storage. Its feedback stays, without the sender (`NOBODY`). The name and the key are free again.
fn remove_user(tx: &Transaction<'_>, user: &UserId, now: u64) -> rusqlite::Result<()> {
    retire_devices(tx, user, now)?;
    tx.execute("DELETE FROM backups WHERE user_id = ?1", [user])?;
    tx.execute("DELETE FROM handles WHERE user_id = ?1", [user])?;
    tx.execute("DELETE FROM account_keys WHERE user_id = ?1", [user])?;
    tx.execute("DELETE FROM reports WHERE reporter_id = ?1 OR reported_id = ?1", [user])?;
    tx.execute("DELETE FROM storage WHERE user_id = ?1", [user])?;
    tx.execute("UPDATE feedback SET user_id = ?2 WHERE user_id = ?1", params![user, NOBODY])?;
    // Its blobs' files go with the next sweep, or at once where the route knows them (`blobs_of`).
    tx.execute("DELETE FROM blobs WHERE user_id = ?1", [user])?;
    tx.execute("DELETE FROM blob_quota WHERE user_id = ?1", [user])?;
    tx.execute("DELETE FROM users WHERE id = ?1", [user])?;
    Ok(())
}

/// The schema this hub writes. A database of another (or a pre-release) schema is refused, never half migrated.
/// Within a version, changes are only additive (`migrate`): a column or table an older hub of the same version
/// ignores, so going back to it loses nothing that it knew.
const SCHEMA_VERSION: i64 = 1;

/// Brings a database of this version written by an older hub up to `SCHEMA`, in one transaction: the usernames'
/// spellings (`handles.spelling`, NULL for the names it holds: shown as their skeleton), the backups' auth keys and
/// the groups' first epochs (1, `SCHEMA`). Records from before the auth keys are deleted: no fetch could be signed
/// for them, and the apps make new ones (spec 7). The commits an older hub kept are proof of their acceptance
/// (`accepted`), at every start: also those of an older hub run meanwhile. The tables `SCHEMA` adds are made by
/// `CREATE TABLE IF NOT EXISTS` already.
fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (table, column, kind) in [
        ("handles", "spelling", "TEXT"),
        ("backups", "auth_passkey", "BLOB"),
        ("backups", "auth_code", "BLOB"),
        ("groups", "first_epoch", "INTEGER NOT NULL DEFAULT 1"),
        ("blobs", "fetching", "INTEGER NOT NULL DEFAULT 0"),
        ("blobs", "ready", "INTEGER NOT NULL DEFAULT 1"),
    ] {
        let has = tx.prepare("SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2")?.exists([table, column])?;
        if !has {
            tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"))?;
        }
    }
    let has_app = tx.prepare("SELECT 1 FROM pragma_table_info('devices') WHERE name = 'app'")?.exists([])?;
    if !has_app {
        tx.execute_batch("ALTER TABLE devices ADD COLUMN app TEXT")?;
    }
    tx.execute("DELETE FROM backups WHERE auth_code IS NULL", [])?;
    tx.execute(
        "INSERT OR IGNORE INTO accepted (group_id, epoch, next_token_hash)
         SELECT group_id, epoch, next_token_hash FROM commits",
        [],
    )?;
    tx.commit()
}

/// A user's username as it writes it (a name from before spellings: its skeleton), in a transaction.
fn username_of(tx: &Transaction<'_>, user: &UserId) -> rusqlite::Result<Option<String>> {
    tx.prepare_cached("SELECT coalesce(spelling, CAST(value AS TEXT)) FROM handles WHERE user_id = ?1 AND kind = ?2")?
        .query_row(params![user, KIND_USERNAME], |row| row.get(0))
        .optional()
}

/// Whether a skeleton is reserved, in a transaction.
fn is_reserved(tx: &Transaction<'_>, skeleton: &[u8]) -> rusqlite::Result<bool> {
    tx.prepare_cached("SELECT 1 FROM reserved WHERE skeleton = CAST(?1 AS TEXT)")?.exists([skeleton])
}

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

/// Whether a user may store one more blob of `size` bytes on the day of `now`.
fn blob_quota_left(
    tx: &Transaction<'_>,
    user: &UserId,
    size: u64,
    now: u64,
    quota: &BlobQuota,
) -> rusqlite::Result<bool> {
    let (count, bytes): (i64, i64) = tx
        .prepare_cached("SELECT count, bytes FROM blob_quota WHERE user_id = ?1 AND day = ?2")?
        .query_row(params![user, (now / DAY_MS) as i64], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?
        .unwrap_or((0, 0));
    if count as u64 >= quota.per_day as u64 || bytes as u64 + size > quota.bytes_per_day {
        return Ok(false);
    }
    let stored: i64 = tx
        .prepare_cached("SELECT coalesce(sum(size), 0) FROM blobs WHERE user_id = ?1")?
        .query_row([user], |row| row.get(0))?;
    Ok(stored as u64 + size <= quota.stored)
}

/// A blob's id and when it was stored, from a row of the two.
fn blob_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(BlobId, u64)> {
    Ok((array(row.get(0)?)?, row.get::<_, i64>(1)? as u64))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Db {
    /// WAL with `synchronous = FULL`: every commit is on the disk before it is answered, so what the hub acknowledged
    /// (a delivery, a commit's order, spec 4.7) survives a power loss, not only a crash of the process.
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        check_version(&conn)?;
        conn.execute_batch(SCHEMA)?;
        migrate(&mut conn)?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    /// Adds the operator's reserved names (spec 3.3) the table does not hold yet, by skeleton; names it holds stay
    /// as they are, and a name gone from the list stays reserved until deleted by hand. Returns how many were new.
    pub fn seed_reserved(&self, names: &[Reserved], now: u64) -> rusqlite::Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut added = 0;
        for name in names {
            added += tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO reserved (skeleton, spelling, note, created_at) VALUES (?1, ?2, ?3, ?4)",
                )?
                .execute(params![yo_wire::handle::skeleton(&name.spelling), name.spelling, name.note, now])?;
        }
        tx.commit()?;
        Ok(added)
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ---- users and devices ----

    pub fn device(&self, device: &DeviceId) -> rusqlite::Result<Option<Device>> {
        self.conn()
            .prepare_cached("SELECT user_id, device_key, auth_key, last_seen_at, app FROM devices WHERE id = ?1")?
            .query_row([device], |row| {
                Ok(Device {
                    user_id: array(row.get(0)?)?,
                    device_key: array(row.get(1)?)?,
                    auth_key: array(row.get(2)?)?,
                    last_seen_at: row.get::<_, i64>(3)? as u64,
                    app: row.get(4)?,
                })
            })
            .optional()
    }

    pub fn is_gone(&self, device: &DeviceId) -> rusqlite::Result<bool> {
        self.conn().prepare_cached("SELECT 1 FROM gone WHERE id = ?1")?.exists([device])
    }

    /// Seen at `now`, with the app it names (none: the one it had stays).
    pub fn touch(&self, device: &DeviceId, now: u64, app: Option<&str>) -> rusqlite::Result<()> {
        self.conn()
            .prepare_cached("UPDATE devices SET last_seen_at = ?2, app = coalesce(?3, app) WHERE id = ?1")?
            .execute(params![device, now, app])?;
        Ok(())
    }

    pub fn account_key(&self, user: &UserId) -> rusqlite::Result<Option<Key>> {
        self.conn()
            .prepare_cached("SELECT account_key FROM users WHERE id = ?1")?
            .query_row([user], |row| array(row.get(0)?))
            .optional()
    }

    /// Registers a device (spec 6.4). An account key that is a user's current key registers that user again (a new
    /// phone restored from the backup), whatever name the request has: the user keeps the name it has now. Any other
    /// key takes the name the request has (by its skeleton): a free name makes a new user, `fresh`; a name someone
    /// has, or a reserved one nobody has, is taken; and a key once another user's stays that user's forever. The
    /// user's earlier devices are replaced. `spelling`: a new name as written. Answers the user and its username as
    /// it writes it.
    #[allow(clippy::too_many_arguments)]
    pub fn register(
        &self,
        skeleton: &[u8],
        spelling: &str,
        fresh: &UserId,
        account_key: &Key,
        device: &NewDevice,
        key_packages: &[StoredKeyPackage],
        last_resort: &StoredKeyPackage,
        now: u64,
    ) -> rusqlite::Result<Write<(UserId, Option<String>)>> {
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
        let restored: Option<UserId> = tx
            .prepare_cached("SELECT id FROM users WHERE account_key = ?1")?
            .query_row([account_key], |row| array(row.get(0)?))
            .optional()?;
        let user = match restored {
            Some(user) => user,
            None => {
                let key_owner: Option<UserId> = tx
                    .prepare_cached("SELECT user_id FROM account_keys WHERE key = ?1")?
                    .query_row([account_key], |row| array(row.get(0)?))
                    .optional()?;
                if key_owner.is_some() {
                    return Ok(Write::Refused(Conflict::KeyTaken));
                }
                let holder = tx
                    .prepare_cached("SELECT 1 FROM handles WHERE kind = ?1 AND value = ?2")?
                    .exists(params![KIND_USERNAME, skeleton])?;
                if holder || is_reserved(&tx, skeleton)? {
                    return Ok(Write::Refused(Conflict::UsernameTaken));
                }
                tx.execute(
                    "INSERT INTO users (id, account_key, created_at) VALUES (?1, ?2, ?3)",
                    params![fresh, account_key, now],
                )?;
                tx.execute("INSERT INTO account_keys (key, user_id) VALUES (?1, ?2)", params![account_key, fresh])?;
                tx.execute(
                    "INSERT INTO handles (kind, value, user_id, spelling) VALUES (?1, ?2, ?3, ?4)",
                    params![KIND_USERNAME, skeleton, fresh, spelling],
                )?;
                *fresh
            }
        };
        retire_devices(&tx, &user, now)?;
        tx.execute(
            "INSERT INTO devices
                 (id, user_id, device_key, auth_key, push_platform, push_token, created_at, last_seen_at, app)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)",
            params![
                device.device_id,
                user,
                device.device_key,
                device.auth_key,
                device.push_platform,
                device.push_token,
                now,
                device.app
            ],
        )?;
        if let Some(conflict) = insert_key_packages(&tx, &device.device_id, key_packages, Some(last_resort))? {
            return Ok(Write::Refused(conflict));
        }
        let username = username_of(&tx, &user)?;
        tx.commit()?;
        Ok(Write::Done((user, username)))
    }

    /// Claims a username (its skeleton and its spelling) for a device's user, giving up its old one; `None` only gives
    /// it up. The user's own name in another spelling is the user's still, also when it has been reserved since. The
    /// device is checked in this transaction, as `put_backup` checks it.
    pub fn set_username(&self, device: &DeviceId, name: Option<(&[u8], &str)>) -> rusqlite::Result<Write<()>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(user) = user_of_device(&tx, device)? else { return Ok(Write::Refused(Conflict::DeviceGone)) };
        let user = &user;
        if let Some((skeleton, _)) = name {
            let owner: Option<UserId> = tx
                .prepare_cached("SELECT user_id FROM handles WHERE kind = ?1 AND value = ?2")?
                .query_row(params![KIND_USERNAME, skeleton], |row| array(row.get(0)?))
                .optional()?;
            let taken = match owner {
                Some(owner) => owner != *user,
                None => is_reserved(&tx, skeleton)?,
            };
            if taken {
                return Ok(Write::Refused(Conflict::UsernameTaken));
            }
        }
        tx.execute("DELETE FROM handles WHERE user_id = ?1 AND kind = ?2", params![user, KIND_USERNAME])?;
        if let Some((skeleton, spelling)) = name {
            tx.execute(
                "INSERT INTO handles (kind, value, user_id, spelling) VALUES (?1, ?2, ?3, ?4)",
                params![KIND_USERNAME, skeleton, user, spelling],
            )?;
        }
        tx.commit()?;
        Ok(Write::Done(()))
    }

    /// Whether a username (by its skeleton) belongs to anyone or is reserved: what registration would find taken.
    pub fn username_taken(&self, skeleton: &[u8]) -> rusqlite::Result<bool> {
        self.conn()
            .prepare_cached(
                "SELECT 1 FROM handles WHERE kind = ?1 AND value = ?2
                 UNION ALL SELECT 1 FROM reserved WHERE skeleton = CAST(?2 AS TEXT)",
            )?
            .exists(params![KIND_USERNAME, skeleton])
    }

    /// A user by a handle (`value` as stored: a username by its skeleton, a user id, an account key, which must be
    /// the user's current one): id, account key, devices and, if asked, one KeyPackage for each device not in
    /// `except`, a single-use one while there is one, else the last resort.
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
        let username = username_of(&tx, &user)?;
        tx.commit()?;
        Ok(Some(Found { user_id: user, account_key, device_keys, key_packages, username }))
    }

    /// Deletes a device's account (spec 6.3): its user with everything about it (`remove_user`). The device is
    /// checked in this transaction, as `put_backup` checks it.
    pub fn delete_account(&self, device: &DeviceId, now: u64) -> rusqlite::Result<Write<()>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(user) = user_of_device(&tx, device)? else { return Ok(Write::Refused(Conflict::DeviceGone)) };
        remove_user(&tx, &user, now)?;
        tx.commit()?;
        Ok(Write::Done(()))
    }

    /// Stores the report of `reported` by a device's user, replacing the pair's earlier one; false when there is no
    /// such user. The device is checked in this transaction: an account deleted meanwhile reports nobody.
    pub fn report(&self, device: &DeviceId, reported: &UserId, reason: u8, now: u64) -> rusqlite::Result<Write<bool>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(reporter) = user_of_device(&tx, device)? else { return Ok(Write::Refused(Conflict::DeviceGone)) };
        let stored = tx
            .prepare_cached(
                "INSERT INTO reports (reporter_id, reported_id, reason, created_at)
                 SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM users WHERE id = ?2)
                 ON CONFLICT (reporter_id, reported_id) DO UPDATE SET
                     reason = excluded.reason, created_at = excluded.created_at",
            )?
            .execute(params![reporter, reported, reason, now])?;
        tx.commit()?;
        Ok(Write::Done(stored == 1))
    }

    /// Stores feedback from a device's user. The device is checked in this transaction: an account deleted meanwhile
    /// leaves none behind.
    pub fn feedback(&self, device: &DeviceId, text: &str, app_version: &str, now: u64) -> rusqlite::Result<Write<()>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(user) = user_of_device(&tx, device)? else { return Ok(Write::Refused(Conflict::DeviceGone)) };
        tx.prepare_cached("INSERT INTO feedback (user_id, text, app_version, created_at) VALUES (?1, ?2, ?3, ?4)")?
            .execute(params![user, text, app_version, now])?;
        tx.commit()?;
        Ok(Write::Done(()))
    }

    // ---- storage (spec 6.9) ----

    /// The storage of a user: its version and sealed bytes.
    pub fn storage(&self, user: &UserId) -> rusqlite::Result<Option<Sealed>> {
        self.conn()
            .prepare_cached("SELECT version, data FROM storage WHERE user_id = ?1")?
            .query_row([user], |row| Ok((row.get::<_, i64>(0)? as u64, row.get(1)?)))
            .optional()
    }

    /// Replaces a device's user's storage if it is at version `replaces` (0: there is none), as version `replaces + 1`.
    /// Otherwise `Err` with the current one (none: version 0, empty). The device is checked in this transaction.
    pub fn put_storage(
        &self,
        device: &DeviceId,
        replaces: u64,
        data: &[u8],
        now: u64,
    ) -> rusqlite::Result<Write<Result<(), Sealed>>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(user) = user_of_device(&tx, device)? else { return Ok(Write::Refused(Conflict::DeviceGone)) };
        let current: Sealed = tx
            .prepare_cached("SELECT version, data FROM storage WHERE user_id = ?1")?
            .query_row([&user], |row| Ok((row.get::<_, i64>(0)? as u64, row.get(1)?)))
            .optional()?
            .unwrap_or_default();
        if current.0 != replaces {
            return Ok(Write::Done(Err(current)));
        }
        tx.prepare_cached(
            "INSERT INTO storage (user_id, version, data, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (user_id) DO UPDATE SET
                 version = excluded.version, data = excluded.data, updated_at = excluded.updated_at",
        )?
        .execute(params![user, (replaces + 1) as i64, data, now])?;
        tx.commit()?;
        Ok(Write::Done(Ok(())))
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
        auth: &BackupAuth,
        now: u64,
    ) -> rusqlite::Result<Write<()>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(user) = user_of_device(&tx, device)? else { return Ok(Write::Refused(Conflict::DeviceGone)) };
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
            "INSERT INTO backups (user_id, locator, record, updated_at, auth_passkey, auth_code)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (user_id) DO UPDATE SET
                 locator = excluded.locator, record = excluded.record, updated_at = excluded.updated_at,
                 auth_passkey = excluded.auth_passkey, auth_code = excluded.auth_code",
        )?
        .execute(params![user, locator, record, now, auth.passkey, auth.code])?;
        tx.commit()?;
        Ok(Write::Done(()))
    }

    /// A backup record by its user id or, where no user has that id, by its locator: what is asked for by a user's
    /// id is that user's record or none, never one whose locator happens to be the same.
    pub fn backup(&self, id: &[u8]) -> rusqlite::Result<Option<(Vec<u8>, BackupAuth)>> {
        let key = |value: Option<Vec<u8>>| value.and_then(|v| v.try_into().ok());
        self.conn()
            .prepare_cached(
                "SELECT record, auth_passkey, auth_code FROM backups WHERE user_id = ?1
                 UNION ALL SELECT record, auth_passkey, auth_code FROM backups
                     WHERE locator = ?1 AND NOT EXISTS (SELECT 1 FROM users WHERE id = ?1)
                 LIMIT 1",
            )?
            .query_row([id], |row| Ok((row.get(0)?, BackupAuth { passkey: key(row.get(1)?), code: key(row.get(2)?) })))
            .optional()
    }

    // ---- blobs (spec 6.8) ----

    /// Whether a user's quota has room for a blob of `size` bytes now: asked before the upload is taken; `put_blob`
    /// asks again as it stores.
    pub fn blob_quota_left(&self, user: &UserId, size: u64, now: u64, quota: &BlobQuota) -> rusqlite::Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        blob_quota_left(&tx, user, size, now, quota)
    }

    /// The bytes of all blobs stored.
    pub fn blobs_stored(&self) -> rusqlite::Result<u64> {
        self.conn()
            .prepare_cached("SELECT coalesce(sum(size), 0) FROM blobs")?
            .query_row([], |row| row.get::<_, i64>(0).map(|n| n as u64))
    }

    pub fn blob_exists(&self, id: &BlobId) -> rusqlite::Result<bool> {
        self.conn().prepare_cached("SELECT 1 FROM blobs WHERE id = ?1")?.exists([id])
    }

    /// Whether a blob with this id is there and ready (`Some(true)`), there with its file still on its way
    /// (`Some(false)`), or not there.
    pub fn blob_ready_state(&self, id: &BlobId) -> rusqlite::Result<Option<bool>> {
        self.conn()
            .prepare_cached("SELECT ready FROM blobs WHERE id = ?1")?
            .query_row([id], |row| row.get(0))
            .optional()
    }

    /// Stores a blob's row for `fetches` fetches and counts it against its uploader's quota, if the id is free and
    /// the quota has room. Not ready: its file is moved into place after, then `blob_ready`.
    pub fn put_blob(
        &self,
        device: &DeviceId,
        id: &BlobId,
        size: u64,
        fetches: u32,
        now: u64,
        quota: &BlobQuota,
    ) -> rusqlite::Result<BlobPut> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(user) = user_of_device(&tx, device)? else { return Ok(BlobPut::DeviceGone) };
        let ready: Option<bool> =
            tx.prepare_cached("SELECT ready FROM blobs WHERE id = ?1")?.query_row([id], |row| row.get(0)).optional()?;
        match ready {
            Some(true) => return Ok(BlobPut::Taken),
            Some(false) => return Ok(BlobPut::Unfinished),
            None => {}
        }
        if !blob_quota_left(&tx, &user, size, now, quota)? {
            return Ok(BlobPut::Quota);
        }
        tx.prepare_cached(
            "INSERT INTO blobs (id, user_id, size, fetches_left, created_at, ready) VALUES (?1, ?2, ?3, ?4, ?5, 0)",
        )?
        .execute(params![id, user, size as i64, fetches, now])?;
        tx.prepare_cached(
            "INSERT INTO blob_quota (user_id, day, count, bytes) VALUES (?1, ?2, 1, ?3)
             ON CONFLICT (user_id, day) DO UPDATE SET count = count + 1, bytes = bytes + excluded.bytes",
        )?
        .execute(params![user, (now / DAY_MS) as i64, size as i64])?;
        tx.commit()?;
        Ok(BlobPut::Stored)
    }

    /// A blob's file is in place: it is handed out from now on.
    pub fn blob_ready(&self, id: &BlobId, created_at: u64) -> rusqlite::Result<()> {
        self.conn()
            .prepare_cached("UPDATE blobs SET ready = 1 WHERE id = ?1 AND created_at = ?2")?
            .execute(params![id, created_at as i64])?;
        Ok(())
    }

    /// Takes one of a blob's fetches as a fetch starts, if it is ready, has one left and is within its time: its size
    /// and when it was stored (which upload under this id it is). In one statement, so fetches at once never pass
    /// the blob's count; given back if the fetch is cut off (`blob_fetched`).
    pub fn take_fetch(&self, id: &BlobId, now: u64, ttl_ms: u64) -> rusqlite::Result<Option<(u64, u64)>> {
        self.conn()
            .prepare_cached(
                "UPDATE blobs SET fetches_left = fetches_left - 1, fetching = fetching + 1
                 WHERE id = ?1 AND ready = 1 AND fetches_left > 0 AND created_at >= ?2
                 RETURNING size, created_at",
            )?
            .query_row(params![id, now.saturating_sub(ttl_ms) as i64], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64))
            })
            .optional()
    }

    /// A fetch taken of the blob stored at `created_at` is over: counted if `complete`, else given back. True when it
    /// was the last and the row is gone (the file must go too). A fetch of a blob gone meanwhile counts for none
    /// stored again under its id.
    pub fn blob_fetched(&self, id: &BlobId, created_at: u64, complete: bool) -> rusqlite::Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let left: Option<(i64, i64)> = tx
            .prepare_cached(
                "UPDATE blobs SET fetching = fetching - 1, fetches_left = fetches_left + ?3
                 WHERE id = ?1 AND created_at = ?2 AND fetching > 0 RETURNING fetches_left, fetching",
            )?
            .query_row(params![id, created_at as i64, !complete as i64], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        let last = left == Some((0, 0));
        if last {
            tx.execute("DELETE FROM blobs WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(last)
    }

    pub fn delete_blob(&self, id: &BlobId) -> rusqlite::Result<()> {
        self.conn().execute("DELETE FROM blobs WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Deletes the rows of blobs past their time, and of those not ready for an hour, and returns their ids (their
    /// files must go too); forgets quotas of days before yesterday.
    pub fn prune_blobs(&self, now: u64, ttl_ms: u64) -> rusqlite::Result<Vec<(BlobId, u64)>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = tx
            .prepare_cached(
                "DELETE FROM blobs WHERE created_at < ?1 OR (ready = 0 AND created_at < ?2) RETURNING id, created_at",
            )?
            .query_map(
                params![now.saturating_sub(ttl_ms) as i64, now.saturating_sub(NOT_READY_KEPT_MS) as i64],
                blob_row,
            )?
            .collect::<Result<_, _>>()?;
        tx.execute("DELETE FROM blob_quota WHERE day < ?1", [(now / DAY_MS) as i64 - 1])?;
        tx.commit()?;
        Ok(ids)
    }

    /// At a start: the fetches under way before are given back (none completed), and blobs not ready (a crash while
    /// the file was moved into place) are deleted; their ids, whose files must go too.
    pub fn recover_blobs(&self) -> rusqlite::Result<Vec<(BlobId, u64)>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE blobs SET fetches_left = fetches_left + fetching, fetching = 0 WHERE fetching > 0", [])?;
        let ids = tx
            .prepare_cached("DELETE FROM blobs WHERE ready = 0 RETURNING id, created_at")?
            .query_map([], blob_row)?
            .collect::<Result<_, _>>()?;
        tx.commit()?;
        Ok(ids)
    }

    /// Every blob, by id and when it was stored: what the sweep keeps.
    pub fn blob_ids(&self) -> rusqlite::Result<Vec<(BlobId, u64)>> {
        self.conn().prepare_cached("SELECT id, created_at FROM blobs")?.query_map([], blob_row)?.collect()
    }

    /// The blobs a device's user uploaded: whose files go when the account is deleted.
    pub fn blobs_of(&self, device: &DeviceId) -> rusqlite::Result<Vec<(BlobId, u64)>> {
        self.conn()
            .prepare_cached(
                "SELECT b.id, b.created_at FROM blobs b JOIN devices d ON d.user_id = b.user_id WHERE d.id = ?1",
            )?
            .query_map([device], blob_row)?
            .collect()
    }

    // ---- KeyPackages and push ----

    /// Adds a device's KeyPackages, checked against its account key. The device is checked in this transaction: one
    /// replaced since (its account key may be another now) adds none.
    pub fn add_key_packages(
        &self,
        device: &DeviceId,
        key_packages: &[StoredKeyPackage],
        last_resort: Option<&StoredKeyPackage>,
        most: u32,
    ) -> rusqlite::Result<Write<()>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if user_of_device(&tx, device)?.is_none() {
            return Ok(Write::Refused(Conflict::DeviceGone));
        }
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

    /// Sets a device's push token; refused for a device replaced since it authenticated.
    pub fn set_push(&self, device: &DeviceId, platform: u8, token: &[u8]) -> rusqlite::Result<Write<()>> {
        let changed = self
            .conn()
            .prepare_cached("UPDATE devices SET push_platform = ?2, push_token = ?3 WHERE id = ?1")?
            .execute(params![device, platform, token])?;
        Ok(if changed == 1 { Write::Done(()) } else { Write::Refused(Conflict::DeviceGone) })
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
    pub fn deliver(&self, items: &[Incoming<'_>], now: u64, most: u32) -> rusqlite::Result<Vec<Delivered>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut outcomes = Vec::with_capacity(items.len());
        for Incoming { user, devices_hash, envelope, token } in items {
            // Spent whatever becomes of the delivery: the token paid for the request.
            if let Some((key, nonce)) = token {
                let spent = tx
                    .prepare_cached("INSERT OR IGNORE INTO spent_tokens (nonce, key_id) VALUES (?1, ?2)")?
                    .execute(params![nonce, key])?;
                if spent == 0 {
                    outcomes.push(Delivered::Spent);
                    continue;
                }
            }
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

    /// The issuer's keys (spec 6.10), newest first: row, seed, when made.
    pub fn token_keys(&self) -> rusqlite::Result<Vec<(i64, Vec<u8>, u64)>> {
        self.conn()
            .prepare("SELECT id, seed, created_at FROM token_keys ORDER BY id DESC")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? as u64)))?
            .collect()
    }

    /// Adds an issuer key, the newest, and deletes all but the `keep` newest with the tokens they spent. Its row.
    pub fn add_token_key(&self, seed: &[u8], now: u64, keep: usize) -> rusqlite::Result<i64> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO token_keys (seed, created_at) VALUES (?1, ?2)", params![seed, now as i64])?;
        let id = tx.last_insert_rowid();
        let old = "SELECT id FROM token_keys ORDER BY id DESC LIMIT -1 OFFSET ?1";
        tx.execute(&format!("DELETE FROM spent_tokens WHERE key_id IN ({old})"), [keep as i64])?;
        tx.execute(&format!("DELETE FROM token_keys WHERE id IN ({old})"), [keep as i64])?;
        tx.commit()?;
        Ok(id)
    }

    /// Whether a token's nonce was spent already.
    pub fn token_spent(&self, nonce: &[u8; 32]) -> rusqlite::Result<bool> {
        self.conn().prepare_cached("SELECT 1 FROM spent_tokens WHERE nonce = ?1")?.exists([nonce])
    }

    pub fn fetch(&self, device: &DeviceId, most: usize) -> rusqlite::Result<Vec<(u64, Vec<u8>)>> {
        self.conn()
            .prepare_cached("SELECT id, envelope FROM messages WHERE device_id = ?1 ORDER BY id LIMIT ?2")?
            .query_map(params![device, most as i64], |row| Ok((row.get::<_, i64>(0)? as u64, row.get(1)?)))?
            .collect()
    }

    /// Deletes a device's envelopes by id. Committed without waiting for the disk (`synchronous = NORMAL` for this
    /// transaction alone; the next commit's sync takes it along): an ack lost to a power cut costs a repeat, which the
    /// device skips (spec 4.7), and an ack of its own `fsync` would cost one per inbox read.
    pub fn ack(&self, device: &DeviceId, ids: &[u64]) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        conn.execute_batch("PRAGMA synchronous = NORMAL")?;
        let acked = (|| {
            let tx = conn.transaction()?;
            {
                let mut delete = tx.prepare_cached("DELETE FROM messages WHERE device_id = ?1 AND id = ?2")?;
                for id in ids {
                    delete.execute(params![device, *id as i64])?;
                }
            }
            tx.commit()
        })();
        conn.execute_batch("PRAGMA synchronous = FULL")?;
        acked
    }

    // ---- groups ----

    /// Registers a group at an epoch; false if the id exists.
    pub fn create_group(&self, group: &GroupId, epoch: u64, token_hash: &[u8; 32], now: u64) -> rusqlite::Result<bool> {
        let changed = self
            .conn()
            .prepare_cached(
                "INSERT INTO groups (id, epoch, token_hash, updated_at, first_epoch) VALUES (?1, ?2, ?3, ?4, ?2)
                 ON CONFLICT (id) DO NOTHING",
            )?
            .execute(params![group, epoch as i64, token_hash, now])?;
        if changed == 1 {
            return Ok(true);
        }
        // The same registration again (its answer was lost): created, as the first time. Only its maker knows the
        // token, and the group has not moved on since.
        let same: Option<(i64, i64, Vec<u8>)> = self
            .conn()
            .prepare_cached("SELECT epoch, first_epoch, token_hash FROM groups WHERE id = ?1")?
            .query_row(params![group], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()?;
        Ok(same.is_some_and(|(now_at, first, hash)| {
            now_at == epoch as i64 && first == epoch as i64 && constant_time_eq(&hash, token_hash)
        }))
    }

    /// Orders a commit (spec 4.3): accepted only on the group's current epoch with that epoch's token, and kept. A
    /// commit accepted before is accepted again: the same epoch and next-token hash, which only it can produce, also
    /// after its envelope is gone.
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
            .prepare_cached("SELECT next_token_hash FROM accepted WHERE group_id = ?1 AND epoch = ?2")?
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
            "INSERT INTO accepted (group_id, epoch, next_token_hash) VALUES (?1, ?2, ?3)",
            params![group, epoch as i64, next_token_hash],
        )?;
        tx.execute(
            "INSERT INTO commits (group_id, epoch, next_token_hash, envelope, accepted_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![group, epoch as i64, next_token_hash, envelope, now],
        )?;
        tx.commit()?;
        Ok(Commit::Accepted)
    }

    /// The envelope of the commit accepted on `epoch`; else whether one was accepted there and is kept no more: the
    /// epoch is the group's first or later, and before its current one (`groups.first_epoch`). Read under one lock, so
    /// a commit accepted meanwhile is never taken for one pruned.
    pub fn commit_envelope(&self, group: &GroupId, epoch: u64) -> rusqlite::Result<Result<Vec<u8>, bool>> {
        let conn = self.conn();
        let envelope: Option<Vec<u8>> = conn
            .prepare_cached("SELECT envelope FROM commits WHERE group_id = ?1 AND epoch = ?2")?
            .query_row(params![group, epoch as i64], |row| row.get(0))
            .optional()?;
        if let Some(envelope) = envelope {
            return Ok(Ok(envelope));
        }
        let epochs: Option<(i64, i64)> = conn
            .prepare_cached("SELECT first_epoch, epoch FROM groups WHERE id = ?1")?
            .query_row([group], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        Ok(Err(epochs.is_some_and(|(first, current)| (first as u64..current as u64).contains(&epoch))))
    }

    /// Deletes envelopes and commits' envelopes older than the TTL (their acceptance stays, `accepted`), and forgets
    /// replaced devices after a year.
    pub fn prune(&self, now: u64, ttl_ms: u64) -> rusqlite::Result<usize> {
        let conn = self.conn();
        let before = now.saturating_sub(ttl_ms) as i64;
        let messages = conn.execute("DELETE FROM messages WHERE received_at < ?1", [before])?;
        conn.execute("DELETE FROM commits WHERE accepted_at < ?1", [before])?;
        conn.execute("DELETE FROM gone WHERE gone_at < ?1", [now.saturating_sub(365 * 24 * 3600 * 1000) as i64])?;
        Ok(messages)
    }

    /// What should never be: rows that belong to nobody (a user, a device or a blob gone without them), each check
    /// with how many rows fail it. Empty when the store is consistent. For tests and the operator; reads only.
    pub fn apps(&self) -> rusqlite::Result<Vec<(String, i64)>> {
        let conn = self.conn();
        let mut statement = conn.prepare("SELECT coalesce(app, ''), count(*) FROM devices GROUP BY 1 ORDER BY 1")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    pub fn audit(&self) -> rusqlite::Result<Vec<(&'static str, i64)>> {
        const CHECKS: &[(&str, &str)] = &[
            ("devices without their user", "SELECT count(*) FROM devices WHERE user_id NOT IN (SELECT id FROM users)"),
            ("users without a device", "SELECT count(*) FROM users WHERE id NOT IN (SELECT user_id FROM devices)"),
            (
                "users whose key is not theirs",
                "SELECT count(*) FROM users u WHERE NOT EXISTS
                 (SELECT 1 FROM account_keys k WHERE k.key = u.account_key AND k.user_id = u.id)",
            ),
            (
                "account keys without their user",
                "SELECT count(*) FROM account_keys WHERE user_id NOT IN (SELECT id FROM users)",
            ),
            ("handles without their user", "SELECT count(*) FROM handles WHERE user_id NOT IN (SELECT id FROM users)"),
            ("backups without their user", "SELECT count(*) FROM backups WHERE user_id NOT IN (SELECT id FROM users)"),
            (
                "reports by or about nobody",
                "SELECT count(*) FROM reports WHERE reporter_id NOT IN (SELECT id FROM users)
                 OR reported_id NOT IN (SELECT id FROM users)",
            ),
            (
                "feedback of a user gone, not anonymised",
                "SELECT count(*) FROM feedback WHERE user_id != zeroblob(16) AND user_id NOT IN (SELECT id FROM users)",
            ),
            ("storage without its user", "SELECT count(*) FROM storage WHERE user_id NOT IN (SELECT id FROM users)"),
            ("blobs without their user", "SELECT count(*) FROM blobs WHERE user_id NOT IN (SELECT id FROM users)"),
            (
                "blob quotas without their user",
                "SELECT count(*) FROM blob_quota WHERE user_id NOT IN (SELECT id FROM users)",
            ),
            (
                "key packages without their device",
                "SELECT count(*) FROM key_packages WHERE device_id NOT IN (SELECT id FROM devices)",
            ),
            (
                "envelopes without their device",
                "SELECT count(*) FROM messages WHERE device_id NOT IN (SELECT id FROM devices)",
            ),
            ("devices both there and gone", "SELECT count(*) FROM devices WHERE id IN (SELECT id FROM gone)"),
            (
                "spent tokens of a key gone",
                "SELECT count(*) FROM spent_tokens WHERE key_id NOT IN (SELECT id FROM token_keys)",
            ),
        ];
        let conn = self.conn();
        let mut failed = Vec::new();
        for (what, sql) in CHECKS {
            let n: i64 = conn.query_row(sql, [], |row| row.get(0))?;
            if n > 0 {
                failed.push((*what, n));
            }
        }
        Ok(failed)
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
            remove_user(&tx, user, now)?;
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
            app: None,
        }
    }

    /// Registers device `n` for a username with account key `key`.
    fn register(db: &Db, name: &[u8], key: u8, n: u8, now: u64) -> Write<(UserId, Option<String>)> {
        let last_resort = StoredKeyPackage { hash: [n; 32], data: vec![n] };
        let spelling = std::str::from_utf8(name).unwrap();
        let skeleton = yo_wire::handle::skeleton(spelling);
        db.register(skeleton.as_bytes(), spelling, &[n; 16], &[key; 32], &device(n), &[], &last_resort, now).unwrap()
    }

    fn reserve(db: &Db, names: &[&str]) -> usize {
        let names: Vec<Reserved> = names.iter().map(|n| Reserved { spelling: n.to_string(), note: None }).collect();
        db.seed_reserved(&names, 0).unwrap()
    }

    /// The spelling a lookup by any spelling of the name answers.
    fn spelling(db: &Db, name: &str) -> Option<String> {
        let skeleton = yo_wire::handle::skeleton(name);
        db.lookup(KIND_USERNAME, skeleton.as_bytes(), false, &[]).unwrap().and_then(|found| found.username)
    }

    #[test]
    fn a_username_keeps_its_spelling_and_is_one_name_in_any() {
        let db = Db::open(":memory:").unwrap();
        let gpt = user(register(&db, b"YoGPT", 1, 1, 0));
        for other in ["yogpt", "YOGPT", "Y0GPT", "yoGpt"] {
            assert!(matches!(register(&db, other.as_bytes(), 2, 2, 0), Write::Refused(Conflict::UsernameTaken)));
            assert_eq!(spelling(&db, other).as_deref(), Some("YoGPT"), "{other}");
            assert!(db.username_taken(yo_wire::handle::skeleton(other).as_bytes()).unwrap());
        }
        assert_eq!(db.lookup(KIND_USER, &gpt, false, &[]).unwrap().unwrap().username.as_deref(), Some("YoGPT"));
        // A new phone registering the name in another spelling keeps the spelling the user has.
        assert_eq!(user(register(&db, b"yogpt", 1, 3, 0)), gpt);
        assert_eq!(spelling(&db, "YoGPT").as_deref(), Some("YoGPT"));
        // Its owner may write it otherwise; nobody else may take it.
        assert!(matches!(db.set_username(&[3; 16], Some((b"YOGPT", "YOgpt"))).unwrap(), Write::Done(())));
        assert_eq!(spelling(&db, "yogpt").as_deref(), Some("YOgpt"));
        let mia = user(register(&db, b"Mia", 4, 4, 0));
        assert!(matches!(db.set_username(&mia, Some((b"YOGPT", "Y0GPT"))).unwrap(), Write::Refused(_)));
    }

    #[test]
    fn reserved_names_are_taken_in_any_spelling_but_stay_their_holders() {
        let db = Db::open(":memory:").unwrap();
        let mod_user = user(register(&db, b"Moderator", 1, 1, 0));
        assert_eq!(reserve(&db, &["Admin", "Moderator", "Support"]), 3);
        assert_eq!(reserve(&db, &["ADMIN", "Admin", "Help"]), 1, "seeding again adds only what is new");
        let count: i64 = db.conn().query_row("SELECT count(*) FROM reserved", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 4);
        for name in ["admin", "ADMIN", "Adm1n", "ÄDMIN", "help"] {
            assert!(
                matches!(register(&db, name.as_bytes(), 2, 2, 0), Write::Refused(Conflict::UsernameTaken)),
                "{name}"
            );
            assert!(db.username_taken(yo_wire::handle::skeleton(name).as_bytes()).unwrap(), "{name}");
        }
        let mia = user(register(&db, b"Mia", 3, 3, 0));
        assert!(matches!(db.set_username(&mia, Some((b"SUPPORT", "support"))).unwrap(), Write::Refused(_)));
        // Who had the name before it was reserved keeps it: found, registered again on a new phone, written anew.
        assert_eq!(spelling(&db, "moderator").as_deref(), Some("Moderator"));
        assert_eq!(user(register(&db, b"Moderator", 1, 4, 0)), mod_user);
        assert!(matches!(db.set_username(&[4; 16], Some((b"MODERATOR", "MODERATOR"))).unwrap(), Write::Done(())));
    }

    /// A database the hub before spellings made (schema 1 without `handles.spelling` and the backups' auth keys):
    /// opened, it gets the columns, its names are found and shown as their skeleton, its backup from before the auth
    /// keys is gone, and nothing in it is refused.
    #[test]
    fn a_database_from_before_spellings_is_migrated() {
        let path = std::env::temp_dir().join(format!("yo-hub-migrate-{}.sqlite", std::process::id()));
        let path = path.to_str().unwrap().to_owned();
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}")).unwrap();
            conn.execute_batch(&SCHEMA.replace("    spelling TEXT,\n", "")).unwrap();
            conn.execute_batch("DROP TABLE reserved").unwrap();
            conn.execute(
                "INSERT INTO users (id, account_key, created_at) VALUES (?1, ?2, 0)",
                params![[1u8; 16], [1u8; 32]],
            )
            .unwrap();
            conn.execute("INSERT INTO account_keys (key, user_id) VALUES (?1, ?2)", params![[1u8; 32], [1u8; 16]])
                .unwrap();
            conn.execute(
                "INSERT INTO handles (kind, value, user_id) VALUES (?1, ?2, ?3)",
                params![KIND_USERNAME, b"CHRISTOPHER".to_vec(), [1u8; 16]],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO devices (id, user_id, device_key, auth_key, push_platform, push_token, created_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, 0, x'', 0, 0)",
                params![[1u8; 16], [1u8; 16], [1u8; 32], [1u8; 32]],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO backups (user_id, locator, record, updated_at) VALUES (?1, NULL, x'01', 0)",
                [[1u8; 16]],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.backup(&[1; 16]).unwrap(), None);
        assert_eq!(spelling(&db, "Christopher").as_deref(), Some("CHRISTOPHER"));
        assert!(matches!(register(&db, b"christopher", 2, 2, 0), Write::Refused(Conflict::UsernameTaken)));
        drop(db);
        // Opened again: nothing more to migrate. The user's own key registers it again (a new phone).
        let db = Db::open(&path).unwrap();
        assert_eq!(user(register(&db, b"Christopher", 1, 9, 0)), [1; 16]);
        assert_eq!(spelling(&db, "christopher").as_deref(), Some("CHRISTOPHER"));
        // The operator's one-off: the spelling the user chose.
        db.conn()
            .execute(
                "UPDATE handles SET spelling = 'Christopher' WHERE kind = 2 AND value = CAST('CHRISTOPHER' AS BLOB)",
                [],
            )
            .unwrap();
        assert_eq!(spelling(&db, "CHRISTOPHER").as_deref(), Some("Christopher"));
        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{path}-wal"));
        let _ = std::fs::remove_file(format!("{path}-shm"));
    }

    fn user(written: Write<(UserId, Option<String>)>) -> UserId {
        match written {
            Write::Done((user, _)) => user,
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
        // The key cannot take a free name as another user: it registers its own user, with the name it has.
        assert!(
            matches!(register(&db, b"MIA", 1, 4, 0), Write::Done((user, Some(name))) if user == first && name == "LEO")
        );
        assert!(db.username_taken(b"LEO").unwrap() && !db.username_taken(b"MIA").unwrap());
    }

    /// A backup knows the name the account had when it was made. Renamed since, its name taken by someone else or
    /// free again: the account key brings back the same user, with the name it has now (spec 6.4, 7).
    #[test]
    fn a_restore_finds_its_user_by_the_account_key_whatever_name_it_had() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        assert!(matches!(db.set_username(&[1; 16], Some((b"LEON", "Leon"))).unwrap(), Write::Done(())));
        // The old name free: the restore does not take it back.
        assert!(
            matches!(register(&db, b"LEO", 1, 2, 0), Write::Done((user, Some(name))) if user == leo && name == "Leon")
        );
        assert!(!db.username_taken(b"LEO").unwrap());
        // The old name someone else's now: still Leon.
        let mia = user(register(&db, b"LEO", 3, 3, 0));
        assert_ne!(mia, leo);
        assert!(
            matches!(register(&db, b"LEO", 1, 4, 0), Write::Done((user, Some(name))) if user == leo && name == "Leon")
        );
        assert_eq!(db.device(&[3; 16]).unwrap().unwrap().user_id, mia, "the stranger keeps the name and the device");
        assert!(db.is_gone(&[2; 16]).unwrap());
        // No name at all: the same user, without one.
        assert!(matches!(db.set_username(&[4; 16], None).unwrap(), Write::Done(())));
        assert!(matches!(register(&db, b"LEON", 1, 5, 0), Write::Done((user, None)) if user == leo));
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
        assert!(matches!(
            db.put_backup(ana, Some(&[9; 16]), b"one", &BackupAuth::default(), 0).unwrap(),
            Write::Done(())
        ));
        assert_eq!(found(&a).map(|f| f.0).as_deref(), Some(&b"one"[..]));
        assert_eq!(found(&[9; 16]).map(|f| f.0).as_deref(), Some(&b"one"[..]));
        // Another user cannot take the locator.
        assert!(refused(
            db.put_backup(ben, Some(&[9; 16]), b"two", &BackupAuth::default(), 0).unwrap(),
            Conflict::LocatorTaken
        ));
        // A new record with a new locator: the old one finds nothing.
        db.put_backup(ana, Some(&[8; 16]), b"three", &BackupAuth::default(), 0).unwrap();
        assert!(found(&[9; 16]).is_none());
        assert_eq!(found(&[8; 16]).map(|f| f.0).as_deref(), Some(&b"three"[..]));
        // No locator may be a user's id.
        assert!(refused(
            db.put_backup(ben, Some(&a), b"four", &BackupAuth::default(), 0).unwrap(),
            Conflict::LocatorTaken
        ));
        // Without a locator: by the user id only. With its auth keys.
        let auth = BackupAuth { passkey: None, code: Some([5; 32]) };
        db.put_backup(ana, None, b"five", &auth, 0).unwrap();
        assert!(found(&[8; 16]).is_none());
        assert_eq!(found(&a), Some((b"five".to_vec(), auth)));
    }

    #[test]
    fn a_user_id_never_finds_another_users_record_by_locator() {
        let db = Db::open(":memory:").unwrap();
        user(register(&db, b"BEN", 2, 2, 0));
        // BEN's locator is [7; 16]; then a user comes to have that id. Asked by its id, the hub has no record for it.
        db.put_backup(&[2; 16], Some(&[7; 16]), b"ben", &BackupAuth::default(), 0).unwrap();
        assert_eq!(user(register(&db, b"SEVEN", 7, 7, 0)), [7; 16]);
        assert!(db.backup(&[7; 16]).unwrap().is_none());
    }

    #[test]
    fn a_replaced_device_cannot_store_a_backup() {
        let db = Db::open(":memory:").unwrap();
        let first = user(register(&db, b"LEO", 1, 1, 0));
        // A new phone restores the account and stores its record; the old phone's late request must not replace it.
        user(register(&db, b"LEO", 1, 3, 0));
        db.put_backup(&[3; 16], None, b"new", &BackupAuth::default(), 0).unwrap();
        assert!(matches!(
            db.put_backup(&[1; 16], None, b"old", &BackupAuth::default(), 0).unwrap(),
            Write::Refused(Conflict::DeviceGone)
        ));
        assert_eq!(db.backup(&first).unwrap().map(|f| f.0).as_deref(), Some(&b"new"[..]));
    }

    /// What a device writes for its user is checked against the device in the write's own transaction: one replaced
    /// after it authenticated (a new phone restored the account, or the account was deleted) changes nothing.
    #[test]
    fn a_replaced_device_writes_nothing_for_its_user() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        user(register(&db, b"LEO", 1, 2, 0));
        let gone = |w: Write<()>| matches!(w, Write::Refused(Conflict::DeviceGone));
        assert!(gone(db.set_username(&[1; 16], Some((b"OLD", "OLD"))).unwrap()));
        assert_eq!(spelling(&db, "LEO").as_deref(), Some("LEO"), "the name stays the new phone's");
        let key_package = StoredKeyPackage { hash: [7; 32], data: vec![7] };
        assert!(gone(db.add_key_packages(&[1; 16], std::slice::from_ref(&key_package), None, 100).unwrap()));
        assert!(gone(db.set_push(&[1; 16], 1, b"ab").unwrap()));
        // Deleted: nothing is written for a user that is no more.
        assert!(matches!(db.delete_account(&[2; 16], 1).unwrap(), Write::Done(())));
        assert!(gone(db.set_username(&[2; 16], Some((b"LEO", "LEO"))).unwrap()));
        assert!(gone(db.add_key_packages(&[2; 16], &[key_package], None, 100).unwrap()));
        assert!(db.lookup(KIND_USER, &leo, false, &[]).unwrap().is_none());
        assert!(!db.username_taken(b"LEO").unwrap());
        assert_eq!(db.audit().unwrap(), []);
    }

    /// A database from before `groups.first_epoch` and `accepted`, whose older hub pruned a commit: every epoch from 1
    /// up to the group's current one counts as accepted (`410` once pruned), and the commits it still keeps are
    /// accepted again when repeated.
    #[test]
    fn a_database_from_before_first_epochs_is_migrated() {
        let path = std::env::temp_dir().join(format!("yo-hub-epochs-{}.db", std::process::id()));
        let path = path.to_str().unwrap();
        let (group, token) = ([5; 32], [6; 32]);
        {
            let db = Db::open(path).unwrap();
            assert!(db.create_group(&group, 1, &token, 0).unwrap());
            assert!(matches!(db.commit(&group, 1, &token, &[1; 32], b"one", 0).unwrap(), Commit::Accepted));
            assert!(matches!(db.commit(&group, 2, &[1; 32], &[2; 32], b"two", 0).unwrap(), Commit::Accepted));
            db.conn()
                .execute_batch(
                    "ALTER TABLE groups DROP COLUMN first_epoch; DROP TABLE accepted;
                     DELETE FROM commits WHERE epoch = 1;",
                )
                .unwrap();
        }
        let db = Db::open(path).unwrap();
        assert_eq!(db.commit_envelope(&group, 1).unwrap(), Err(true), "accepted, and pruned by the older hub");
        assert_eq!(db.commit_envelope(&group, 2).unwrap(), Ok(b"two".to_vec()));
        assert_eq!(db.commit_envelope(&group, 0).unwrap(), Err(false), "never");
        assert_eq!(db.commit_envelope(&group, 3).unwrap(), Err(false), "not yet");
        assert!(matches!(db.commit(&group, 2, &[1; 32], &[2; 32], b"two", 0).unwrap(), Commit::Accepted));
        drop(db);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{path}{suffix}"));
        }
    }

    /// A blob's fetches are taken as they start: fetches at once never pass its count, one cut off gives its fetch
    /// back, and so does a restart. A blob is handed out only once it is ready; one never ready goes at the next start.
    #[test]
    fn blob_fetches_are_taken_as_they_start_and_given_back_when_cut_off() {
        let db = Db::open(":memory:").unwrap();
        user(register(&db, b"LEO", 1, 1, 0));
        let quota = BlobQuota { per_day: 10, bytes_per_day: 1 << 20, stored: 1 << 20 };
        let (id, ttl) = ([9; 16], 30 * DAY);
        assert_eq!(db.put_blob(&[1; 16], &id, 100, 2, 5, &quota).unwrap(), BlobPut::Stored);
        assert_eq!(db.take_fetch(&id, 5, ttl).unwrap(), None, "not ready: its file is not in place");
        assert_eq!(db.put_blob(&[1; 16], &id, 100, 2, 5, &quota).unwrap(), BlobPut::Unfinished, "another try: later");
        assert_eq!(db.blob_ready_state(&id).unwrap(), Some(false));
        db.blob_ready(&id, 5).unwrap();
        assert_eq!(db.put_blob(&[1; 16], &id, 100, 2, 5, &quota).unwrap(), BlobPut::Taken, "done: it is there");
        assert_eq!(db.take_fetch(&id, 5, ttl).unwrap(), Some((100, 5)));
        assert_eq!(db.take_fetch(&id, 5, ttl).unwrap(), Some((100, 5)));
        assert_eq!(db.take_fetch(&id, 5, ttl).unwrap(), None, "two fetches under way, none left");
        assert!(!db.blob_fetched(&id, 5, false).unwrap(), "one cut off");
        assert!(!db.blob_fetched(&id, 5, true).unwrap(), "one complete, the other given back");
        assert_eq!(db.take_fetch(&id, 5, ttl).unwrap(), Some((100, 5)));
        assert!(db.recover_blobs().unwrap().is_empty(), "a restart: the fetch under way given back");
        assert_eq!(db.take_fetch(&id, 5, ttl).unwrap(), Some((100, 5)));
        assert!(db.blob_fetched(&id, 5, true).unwrap(), "the last: the blob goes");
        assert!(!db.blob_exists(&id).unwrap());
        // Stored, never ready (a crash before its file was in place): gone at the next start, its id free again.
        assert_eq!(db.put_blob(&[1; 16], &id, 100, 1, 6, &quota).unwrap(), BlobPut::Stored);
        assert_eq!(db.recover_blobs().unwrap(), [(id, 6)]);
        assert_eq!(db.put_blob(&[1; 16], &id, 100, 1, 7, &quota).unwrap(), BlobPut::Stored);
        // Or within the hour, without a restart.
        assert!(db.prune_blobs(7 + NOT_READY_KEPT_MS - 1, ttl).unwrap().is_empty());
        assert_eq!(db.prune_blobs(7 + NOT_READY_KEPT_MS + 1, ttl).unwrap(), [(id, 7)]);
    }

    /// Commits wait for the disk (`FULL`, 2); an ack alone does not, and leaves the connection as it was.
    #[test]
    fn only_an_ack_commits_without_waiting_for_the_disk() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        let devices_hash = yo_wire::devices::hash(&[1; 32], &[[1; 32]]);
        db.deliver(&[Incoming { user: leo, devices_hash, envelope: b"envelope", token: None }], 0, 10).unwrap();
        let synchronous = || -> i64 { db.conn().query_row("PRAGMA synchronous", [], |row| row.get(0)).unwrap() };
        assert_eq!(synchronous(), 2);
        let (id, _) = db.fetch(&[1; 16], 10).unwrap().remove(0);
        db.ack(&[1; 16], &[id]).unwrap();
        assert!(db.fetch(&[1; 16], 10).unwrap().is_empty());
        assert_eq!(synchronous(), 2);
    }

    #[test]
    fn a_deleted_account_leaves_nothing_behind() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        let mia = user(register(&db, b"MIA", 2, 2, 0));
        db.put_backup(&[1; 16], Some(&[9; 16]), b"record", &BackupAuth::default(), 0).unwrap();
        let incoming = Incoming {
            user: leo,
            devices_hash: yo_wire::devices::hash(&[1; 32], &[[1; 32]]),
            envelope: b"envelope",
            token: None,
        };
        db.deliver(&[incoming], 0, 10).unwrap();
        // The test's users have the ids of their devices.
        let reported = |device: &DeviceId, whom: &UserId| match db.report(device, whom, 1, 0).unwrap() {
            Write::Done(stored) => Some(stored),
            Write::Refused(_) => None,
        };
        assert_eq!((reported(&leo, &mia), reported(&mia, &leo)), (Some(true), Some(true)));
        assert!(matches!(db.feedback(&[1; 16], "more colors", "1.0 (1)", 0).unwrap(), Write::Done(())));
        assert!(matches!(db.feedback(&[2; 16], "fine", "1.0 (1)", 0).unwrap(), Write::Done(())));
        assert!(matches!(db.delete_account(&[1; 16], 1).unwrap(), Write::Done(())));
        assert!(db.is_gone(&[1; 16]).unwrap(), "its device learns that the account is gone");
        assert!(db.lookup(KIND_USER, &leo, false, &[]).unwrap().is_none());
        assert!(db.backup(&leo).unwrap().is_none() && db.backup(&[9; 16]).unwrap().is_none());
        assert!(db.fetch(&[1; 16], 10).unwrap().is_empty());
        let left = |table: &str| -> i64 {
            db.conn().query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0)).unwrap()
        };
        assert_eq!((left("reports"), left("key_packages"), left("account_keys")), (0, 1, 1), "only MIA's are left");
        let feedback: Vec<Vec<u8>> = db
            .conn()
            .prepare("SELECT user_id FROM feedback ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(feedback, [NOBODY.to_vec(), mia.to_vec()], "LEO's feedback stays without LEO, MIA's as it was");
        assert_eq!(db.audit().unwrap(), [], "nothing of LEO's is left behind");
        assert!(matches!(db.feedback(&[1; 16], "late", "1.0 (1)", 1).unwrap(), Write::Refused(Conflict::DeviceGone)));
        assert!(matches!(db.delete_account(&[1; 16], 1).unwrap(), Write::Refused(Conflict::DeviceGone)));
        // The name and the key are free again.
        user(register(&db, b"LEO", 1, 3, 2));
        assert_eq!(reported(&mia, &leo), Some(false), "no such user any more");
        assert_eq!(reported(&leo, &mia), None, "a deleted account reports nobody");
    }

    #[test]
    fn feedback_is_kept_as_written_with_who_and_when() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        assert!(matches!(db.feedback(&[1; 16], "Mehr Farben, bitte 🎨", "1.2 (34)", 5).unwrap(), Write::Done(())));
        assert!(matches!(db.feedback(&[1; 16], "again", "1.2 (35)", 6).unwrap(), Write::Done(())));
        let rows: Vec<(i64, Vec<u8>, String, String, i64)> = db
            .conn()
            .prepare("SELECT id, user_id, text, app_version, created_at FROM feedback ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                (1, leo.to_vec(), "Mehr Farben, bitte 🎨".to_owned(), "1.2 (34)".to_owned(), 5),
                (2, leo.to_vec(), "again".to_owned(), "1.2 (35)".to_owned(), 6),
            ]
        );
        assert!(matches!(db.feedback(&[9; 16], "nobody", "1", 7).unwrap(), Write::Refused(Conflict::DeviceGone)));
    }

    #[test]
    fn a_database_from_before_feedback_gains_its_table() {
        let path = std::env::temp_dir().join(format!("yo-hub-feedback-{}.db", std::process::id()));
        let path = path.to_str().unwrap();
        {
            let db = Db::open(path).unwrap();
            user(register(&db, b"LEO", 1, 1, 0));
            db.conn().execute_batch("DROP INDEX feedback_by_user; DROP TABLE feedback").unwrap();
        }
        let db = Db::open(path).unwrap();
        assert!(matches!(db.feedback(&[1; 16], "hi", "1", 0).unwrap(), Write::Done(())));
        drop(db);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{path}{suffix}"));
        }
    }

    #[test]
    fn names_not_secured_are_released_after_the_idle_time() {
        let db = Db::open(":memory:").unwrap();
        let idle = user(register(&db, b"IDLE", 1, 1, 0));
        let secured = user(register(&db, b"SAFE", 2, 2, 0));
        let active = user(register(&db, b"BUSY", 3, 3, 0));
        db.put_backup(&[2; 16], None, b"record", &BackupAuth::default(), 0).unwrap();
        db.touch(&[3; 16], 50 * DAY, None).unwrap();
        assert!(matches!(db.feedback(&[1; 16], "where are my friends", "1.0 (1)", 0).unwrap(), Write::Done(())));
        let exists = |user: &UserId| db.lookup(KIND_USER, user, false, &[]).unwrap().is_some();
        assert_eq!(db.release_unsecured(89 * DAY, 90 * DAY).unwrap(), 0);
        assert_eq!(db.release_unsecured(91 * DAY, 90 * DAY).unwrap(), 1);
        assert!(!exists(&idle));
        let feedback: (Vec<u8>, String) = db
            .conn()
            .query_row("SELECT user_id, text FROM feedback", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        assert_eq!(feedback, (NOBODY.to_vec(), "where are my friends".into()), "the feedback stays, without IDLE");
        assert!(db.is_gone(&[1; 16]).unwrap(), "its device learns that it was removed");
        assert!(exists(&secured) && exists(&active));
        // The name is free for anyone now, and so is the key.
        user(register(&db, b"IDLE", 4, 4, 91 * DAY));
        user(register(&db, b"OTHER", 1, 5, 91 * DAY));
        assert_eq!(db.release_unsecured(200 * DAY, 90 * DAY).unwrap(), 3, "BUSY and the two new ones, never SAFE");
        assert!(exists(&secured));
    }

    #[test]
    fn a_group_registered_again_is_created_again_until_it_moves_on() {
        let db = Db::open(":memory:").unwrap();
        let group: GroupId = [7; 32];
        assert!(db.create_group(&group, 1, &[1; 32], 0).unwrap());
        assert!(db.create_group(&group, 1, &[1; 32], 0).unwrap(), "the same registration: created");
        assert!(!db.create_group(&group, 1, &[2; 32], 0).unwrap(), "another token: taken");
        assert!(matches!(db.commit(&group, 1, &[1; 32], &[3; 32], b"c", 0).unwrap(), Commit::Accepted));
        assert!(!db.create_group(&group, 1, &[1; 32], 0).unwrap(), "moved on: taken");
    }
}
