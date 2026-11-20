//! The hub's store (spec 1): SQLite, one connection, one transaction per method; no content key, sender or contacts.

mod schema;

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
// The in-file tests use these through `super::*`.
#[cfg(test)]
use schema::{
    migrate_diagnostics, with_feedback_type, with_support, with_support_done, with_todos, DIAGNOSTIC_DAYS,
    LEGACY_FEEDBACK, SCHEMA,
};
use std::sync::{Mutex, MutexGuard, PoisonError};
use yo_wire::api;
use yo_wire::content::BlobId;
use yo_wire::handle::{KIND_ACCOUNT_KEY, KIND_USER, KIND_USERNAME};
use yo_wire::{DeviceId, UserId, KEY_LEN};

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
    pub registration_id: u32,
    pub push_platform: u8,
    pub push_token: Vec<u8>,
    /// The app it names (`Yo-App`), if any.
    pub app: Option<String>,
}

/// The keys that may sign a backup record's fetch (spec 7): the passkey's and the code's; neither for older records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupAuth {
    pub passkey: Option<Key>,
    pub code: Option<Key>,
}

/// Prekey kinds (spec 4.1): signed and last resort, replaced by an upload; one-time ones, handed out once.
const SIGNED: i64 = 0;
const LAST_RESORT: i64 = 1;
const ONE_TIME: i64 = 2;
const KYBER: i64 = 3;

/// A user found by a handle.
pub struct Found {
    pub user_id: UserId,
    pub account_key: Key,
    pub devices: Vec<api::Device>,
    pub bundles: Vec<api::Bundle>,
    /// The user's username as its owner writes it ("YoGPT"), if it has one.
    pub username: Option<String>,
}

/// A username nobody may claim (spec 3.3), as the operator's list writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reserved {
    pub spelling: String,
    pub note: Option<String>,
}

/// A user's current account key and devices: what a delivery's device check compares with (spec 4.4).
pub type Roster = api::Devices;

/// A device to wake after a delivery.
pub struct Wake {
    pub device_id: DeviceId,
    pub push_platform: u8,
    pub push_token: Vec<u8>,
    /// A bot's device (`is_bot_app`): its Yos are counted apart (`yo-in-bot`, 6.6).
    pub bot: bool,
}

/// A bot's app as its device names it (`Yo-App`): YoGPT, or the CLI the bots (STATUS, PING, …) run on. The operator's
/// counts keep bots apart by it; a person on the CLI counts as a bot too.
pub fn is_bot_app(app: Option<&str>) -> bool {
    app.is_some_and(|app| app.starts_with("yogpt/") || app.starts_with("cli/"))
}

/// An envelope for the writer (`Db::deliver`): for a user, with the sender's device check.
pub struct Incoming<'a> {
    pub user: UserId,
    pub devices_hash: [u8; 32],
    pub envelope: &'a [u8],
}

pub enum Delivered {
    Stored(Vec<Wake>),
    /// The sender's view of the user's devices is not the hub's: nothing stored.
    Mismatch(Roster),
    /// A device of the user holds as many envelopes as it may: later.
    Full,
    Unknown,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Conflict {
    SupportFull,
    DeviceTaken,
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

impl<T> Write<T> {
    pub fn done(self) -> Result<T, Conflict> {
        match self {
            Write::Done(done) => Ok(done),
            Write::Refused(conflict) => Err(conflict),
        }
    }
}

/// What a delivery is to its recipient (spec 4.10, `Db::admit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// It shows the recipient's profile key, or the recipient has none.
    Contact,
    /// A request of this sender, taken.
    Request,
    /// No contact's (no key, or not the current one) and unsigned.
    Closed,
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

/// A row, if there is one.
fn row<T, P: rusqlite::Params>(
    conn: &Connection,
    sql: &str,
    params: P,
    f: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<Option<T>> {
    conn.prepare_cached(sql)?.query_row(params, f).optional()
}

/// `f` in one immediate transaction, committed when it returns `Ok`.
fn immediate<T>(conn: &mut Connection, f: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let done = f(&tx)?;
    tx.commit()?;
    Ok(done)
}

fn array<const N: usize>(bytes: Vec<u8>) -> rusqlite::Result<[u8; N]> {
    bytes.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn roster(tx: &Transaction<'_>, user: &UserId) -> rusqlite::Result<Option<Roster>> {
    let Some(account_key) = row(tx, "SELECT account_key FROM users WHERE id = ?1", [user], |row| array(row.get(0)?))?
    else {
        return Ok(None);
    };
    let devices = tx
        .prepare_cached("SELECT device_key, number FROM devices WHERE user_id = ?1 ORDER BY device_key")?
        .query_map([user], |row| Ok(api::Device { key: array(row.get(0)?)?, id: row.get(1)? }))?
        .collect::<Result<_, _>>()?;
    Ok(Some(Roster { account_key, devices }))
}

/// Stores an upload of prekeys, up to `most` one-time ones of each kind, none with an id used before: never twice.
fn insert_prekeys(tx: &Transaction<'_>, device: &DeviceId, prekeys: &api::Prekeys, most: u32) -> rusqlite::Result<()> {
    let mut insert = tx.prepare_cached(
        "INSERT OR IGNORE INTO prekeys (device_id, kind, id, key, signature) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (kind, signed) in [(SIGNED, &prekeys.signed), (LAST_RESORT, &prekeys.last_resort)] {
        if let Some(signed) = signed {
            tx.execute("DELETE FROM prekeys WHERE device_id = ?1 AND kind = ?2", params![device, kind])?;
            insert.execute(params![device, kind, signed.id, signed.key.as_slice(), signed.signature.as_slice()])?;
        }
    }
    let count = |kind: i64| -> rusqlite::Result<u32> {
        tx.prepare_cached("SELECT count(*) FROM prekeys WHERE device_id = ?1 AND kind = ?2")?
            .query_row(params![device, kind], |row| row.get(0))
    };
    let max: u32 = row(tx, "SELECT prekey_max FROM devices WHERE id = ?1", [device], |row| row.get(0))?.unwrap_or(0);
    let room = most.saturating_sub(count(ONE_TIME)?) as usize;
    for one in prekeys.one_time.iter().filter(|p| p.id > max).take(room) {
        insert.execute(params![device, ONE_TIME, one.id, one.key.as_slice(), &[] as &[u8]])?;
    }
    let room = most.saturating_sub(count(KYBER)?) as usize;
    for kyber in prekeys.kyber.iter().filter(|p| p.id > max).take(room) {
        insert.execute(params![device, KYBER, kyber.id, kyber.key.as_slice(), kyber.signature.as_slice()])?;
    }
    let newest = prekeys.one_time.iter().map(|p| p.id).chain(prekeys.kyber.iter().map(|p| p.id)).max().unwrap_or(0);
    tx.execute("UPDATE devices SET prekey_max = max(prekey_max, ?2) WHERE id = ?1", params![device, newest])?;
    Ok(())
}

/// A prekey of a kind for a device, the oldest first; with `take`, gone from here.
fn prekey(tx: &Transaction<'_>, device: &[u8], kind: i64, take: bool) -> rusqlite::Result<Option<api::SignedPrekey>> {
    let found: Option<(u32, Vec<u8>, Vec<u8>)> = row(
        tx,
        "SELECT id, key, signature FROM prekeys WHERE device_id = ?1 AND kind = ?2 ORDER BY id LIMIT 1",
        params![device, kind],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if let (Some((id, _, _)), true) = (&found, take) {
        tx.execute("DELETE FROM prekeys WHERE device_id = ?1 AND kind = ?2 AND id = ?3", params![device, kind, id])?;
    }
    Ok(found.map(|(id, key, signature)| api::SignedPrekey { id, key: key.into(), signature: signature.into() }))
}

/// Removes a user's devices with their prekeys and envelopes. Their own requests answer 410 from then on.
fn retire_devices(tx: &Transaction<'_>, user: &UserId, now: u64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO gone (id, gone_at) SELECT id, ?2 FROM devices WHERE user_id = ?1",
        params![user, now],
    )?;
    for table in ["prekeys", "messages"] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE device_id IN (SELECT id FROM devices WHERE user_id = ?1)"),
            [user],
        )?;
    }
    tx.execute("DELETE FROM devices WHERE user_id = ?1", [user])?;
    Ok(())
}

/// The user of a device, in a transaction: none for a device replaced since it authenticated.
fn user_of_device(tx: &Transaction<'_>, device: &DeviceId) -> rusqlite::Result<Option<UserId>> {
    row(tx, "SELECT user_id FROM devices WHERE id = ?1", [device], |row| array(row.get(0)?))
}

/// The user id a support message keeps once its sender's account is gone: no user has it.
pub const NOBODY: UserId = [0; 16];

/// Removes a user and everything about it; its devices answer 410, its support messages stay as `NOBODY`'s.
fn remove_user(tx: &Transaction<'_>, user: &UserId, now: u64) -> rusqlite::Result<()> {
    retire_devices(tx, user, now)?;
    // Its blobs' files go with the next sweep, or at once where the route knows them (`blobs_of`).
    for table in
        ["backups", "handles", "account_keys", "storage", "contact_keys", "profile_commitments", "blobs", "blob_quota"]
    {
        tx.execute(&format!("DELETE FROM {table} WHERE user_id = ?1"), [user])?;
    }
    tx.execute("DELETE FROM reports WHERE reporter_id = ?1 OR reported_id = ?1", [user])?;
    tx.execute("UPDATE support_messages SET user_id = ?2 WHERE user_id = ?1", params![user, NOBODY])?;
    tx.execute("DELETE FROM users WHERE id = ?1", [user])?;
    Ok(())
}

/// A user's username as it writes it (a name from before spellings: its skeleton), in a transaction.
fn username_of(tx: &Transaction<'_>, user: &UserId) -> rusqlite::Result<Option<String>> {
    row(
        tx,
        "SELECT coalesce(spelling, CAST(value AS TEXT)) FROM handles WHERE user_id = ?1 AND kind = ?2",
        params![user, KIND_USERNAME],
        |row| row.get(0),
    )
}

/// Whether a skeleton is reserved, in a transaction.
fn is_reserved(tx: &Transaction<'_>, skeleton: &[u8]) -> rusqlite::Result<bool> {
    tx.prepare_cached(
        "SELECT 1 FROM reserved WHERE skeleton = CAST(?1 AS TEXT)
         AND (SELECT value FROM settings WHERE key='reserved_enabled') = 1",
    )?
    .exists([skeleton])
}

fn diagnostic_write(conn: &Connection, kind: &str, payload: &serde_json::Value, now: u64) -> rusqlite::Result<i64> {
    let field = |name| payload[name].as_str().unwrap_or("");
    let identity = if kind == "error" {
        serde_json::json!([kind, field("domain"), field("code"), field("place"), field("app")])
    } else {
        serde_json::json!([kind, field("exception"), field("frames"), field("termination"), field("app")])
    };
    let signature = yo_wire::b64(&yo_wire::sha256(identity.to_string().as_bytes()));
    // One report may stand for several of the same, counted on the phone (checked on receipt).
    let count = field("count").parse::<i64>().unwrap_or(1).max(1);
    conn.execute(
        "INSERT INTO diagnostics(kind, signature, count, first_seen, last_seen, app, os, model, payload)
        VALUES (?1,?2,?8,?3,?3,?4,?5,?6,?7)
        ON CONFLICT(signature) DO UPDATE SET count=count+excluded.count, last_seen=excluded.last_seen,
        app=excluded.app, os=excluded.os, model=excluded.model, payload=excluded.payload",
        params![kind, signature, now, field("app"), field("os"), field("model"), payload.to_string(), count],
    )?;
    conn.execute(
        "INSERT INTO diagnostic_days(day,kind,count) VALUES (?1,?2,?3)
        ON CONFLICT(day,kind) DO UPDATE SET count=count+excluded.count",
        params![now / 86_400_000, kind, count],
    )?;
    conn.execute("DELETE FROM diagnostic_days WHERE day < ?1", [now.saturating_sub(14 * 86_400_000) / 86_400_000])?;
    conn.query_row("SELECT id FROM diagnostics WHERE signature=?1", [signature], |r| r.get(0))
}

fn legacy_crash(text: &str, app: &str) -> serde_json::Value {
    let lines: Vec<_> = text.lines().collect();
    let meta: Vec<_> = lines.first().unwrap_or(&"").split(" · ").collect();
    serde_json::json!({"app": app, "os": meta.get(1).unwrap_or(&""), "model": meta.get(2).unwrap_or(&""),
        "exception": lines.get(1).unwrap_or(&""), "frames": lines.iter().skip(2).copied().collect::<Vec<_>>().join("\n"),
        "termination": "", "details": text})
}

/// Whether a user may store one more blob of `size` bytes on the day of `now`.
fn blob_quota_left(tx: &Connection, user: &UserId, size: u64, now: u64, quota: &BlobQuota) -> rusqlite::Result<bool> {
    let (count, bytes): (i64, i64) = row(
        tx,
        "SELECT count, bytes FROM blob_quota WHERE user_id = ?1 AND day = ?2",
        params![user, (now / DAY_MS) as i64],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?
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

impl Db {
    /// WAL, `synchronous = NORMAL`: a power loss may roll back recent commits (SECURITY.md).
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let mut conn = Connection::open(path)?;
        // 16K WAL pages amortize checkpoint syncs; the default 1000 stall too often under continuous deliveries.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA wal_autocheckpoint = 16384;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        schema::upgrade(&mut conn)?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a statement changed exactly one row.
    fn one_changed<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<bool> {
        Ok(self.conn().prepare_cached(sql)?.execute(params)? == 1)
    }

    fn run<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<()> {
        self.conn().prepare_cached(sql)?.execute(params)?;
        Ok(())
    }

    fn write<T>(&self, f: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        immediate(&mut self.conn(), f)
    }

    /// `write` for a device's user, checked in the same transaction: a device replaced since it authenticated is gone.
    fn as_device<T>(
        &self,
        device: &DeviceId,
        f: impl FnOnce(&Transaction<'_>, UserId) -> rusqlite::Result<Write<T>>,
    ) -> rusqlite::Result<Write<T>> {
        self.write(|tx| match user_of_device(tx, device)? {
            Some(user) => f(tx, user),
            None => Ok(Write::Refused(Conflict::DeviceGone)),
        })
    }

    // ---- reserved names (spec 3.3) ----

    /// Stored names and the independent enforcement switch for the admin API.
    pub fn reserved_state(&self) -> rusqlite::Result<serde_json::Value> {
        let conn = self.conn();
        let enabled: bool =
            conn.query_row("SELECT value FROM settings WHERE key='reserved_enabled'", [], |r| r.get(0))?;
        let mut stmt = conn.prepare("SELECT spelling, note FROM reserved ORDER BY skeleton")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let notes: serde_json::Map<_, _> =
            rows.iter().filter_map(|(name, note)| Some((name.clone(), note.clone()?.into()))).collect();
        let names: Vec<_> = rows.into_iter().map(|(name, _)| name).collect();
        Ok(serde_json::json!({"enabled": enabled, "names": names, "notes": notes}))
    }

    pub fn set_reserved_enabled(&self, enabled: bool) -> rusqlite::Result<()> {
        self.run("UPDATE settings SET value=?1 WHERE key='reserved_enabled'", [enabled])
    }

    pub fn remove_reserved(&self, name: &str) -> rusqlite::Result<()> {
        self.run("DELETE FROM reserved WHERE skeleton=?1", [yo_wire::handle::skeleton(name)])
    }

    /// Add names by skeleton, preserving existing spellings and notes. Returns how many were new.
    pub fn add_reserved(&self, names: &[Reserved], now: u64) -> rusqlite::Result<usize> {
        self.reserve(names, false, now)
    }

    /// Add names by skeleton or, for names already reserved, replace their note (`None` clears it); existing
    /// spellings stay. Returns how many were new.
    pub fn set_reserved_notes(&self, names: &[Reserved], now: u64) -> rusqlite::Result<usize> {
        self.reserve(names, true, now)
    }

    fn reserve(&self, names: &[Reserved], notes: bool, now: u64) -> rusqlite::Result<usize> {
        self.write(|tx| {
            let mut insert = tx.prepare_cached(
                "INSERT OR IGNORE INTO reserved (skeleton, spelling, note, created_at) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut note = tx.prepare_cached("UPDATE reserved SET note=?2 WHERE skeleton=?1")?;
            let mut added = 0;
            for name in names {
                let skeleton = yo_wire::handle::skeleton(&name.spelling);
                let new = insert.execute(params![skeleton, name.spelling, name.note, now])?;
                if new == 0 && notes {
                    note.execute(params![skeleton, name.note])?;
                }
                added += new;
            }
            Ok(added)
        })
    }

    // ---- users and devices ----

    pub fn device(&self, device: &DeviceId) -> rusqlite::Result<Option<Device>> {
        row(&self.conn(), "SELECT user_id, auth_key, last_seen_at, app FROM devices WHERE id = ?1", [device], |row| {
            Ok(Device {
                user_id: array(row.get(0)?)?,
                auth_key: array(row.get(1)?)?,
                last_seen_at: row.get::<_, i64>(2)? as u64,
                app: row.get(3)?,
            })
        })
    }

    pub fn is_gone(&self, device: &DeviceId) -> rusqlite::Result<bool> {
        self.conn().prepare_cached("SELECT 1 FROM gone WHERE id = ?1")?.exists([device])
    }

    /// Seen at `now`, with the app it names (none: the one it had stays).
    pub fn touch(&self, device: &DeviceId, now: u64, app: Option<&str>) -> rusqlite::Result<()> {
        self.run(
            "UPDATE devices SET last_seen_at = ?2, app = coalesce(?3, app) WHERE id = ?1",
            params![device, now, app],
        )
    }

    /// The user an account key belongs to now: a registration with it restores that user (spec 6.4).
    pub fn user_of_account_key(&self, account_key: &Key) -> rusqlite::Result<Option<UserId>> {
        row(&self.conn(), "SELECT id FROM users WHERE account_key = ?1", [account_key], |row| array(row.get(0)?))
    }

    /// Who has a username (by its skeleton), if anyone.
    pub fn username_owner(&self, skeleton: &[u8]) -> rusqlite::Result<Option<UserId>> {
        row(
            &self.conn(),
            "SELECT user_id FROM handles WHERE kind = ?1 AND value = ?2",
            params![KIND_USERNAME, skeleton],
            |row| array(row.get(0)?),
        )
    }

    pub fn account_key(&self, user: &UserId) -> rusqlite::Result<Option<Key>> {
        row(&self.conn(), "SELECT account_key FROM users WHERE id = ?1", [user], |row| array(row.get(0)?))
    }

    /// Registers a device (spec 6.4): the current account key's user again, else a new user by a free name.
    #[allow(clippy::too_many_arguments)]
    pub fn register(
        &self,
        skeleton: &[u8],
        spelling: &str,
        fresh: &UserId,
        account_key: &Key,
        device: &NewDevice,
        prekeys: &api::Prekeys,
        most: u32,
        now: u64,
    ) -> rusqlite::Result<Write<(UserId, Option<String>, u8)>> {
        self.write(|tx| {
            let taken = tx
                .prepare_cached(
                    "SELECT 1 FROM devices WHERE id = ?1 OR device_key = ?2 UNION ALL SELECT 1 FROM gone WHERE id = ?1",
                )?
                .exists(params![device.device_id, device.device_key])?;
            if taken {
                return Ok(Write::Refused(Conflict::DeviceTaken));
            }
            let restored: Option<UserId> =
                row(tx, "SELECT id FROM users WHERE account_key = ?1", [account_key], |row| array(row.get(0)?))?;
            let user = match restored {
                Some(user) => user,
                None => {
                    if tx.prepare_cached("SELECT 1 FROM account_keys WHERE key = ?1")?.exists([account_key])? {
                        return Ok(Write::Refused(Conflict::KeyTaken));
                    }
                    let holder = tx
                        .prepare_cached("SELECT 1 FROM handles WHERE kind = ?1 AND value = ?2")?
                        .exists(params![KIND_USERNAME, skeleton])?;
                    if holder || is_reserved(tx, skeleton)? {
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
            retire_devices(tx, &user, now)?;
            // The lowest number free among the user's devices (all of them replaced: 1).
            let taken: Vec<u8> = tx
                .prepare_cached("SELECT number FROM devices WHERE user_id = ?1")?
                .query_map([&user], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            let number = (1..=127u8).find(|n| !taken.contains(n)).ok_or(rusqlite::Error::InvalidQuery)?;
            tx.execute(
                "INSERT INTO devices
                     (id, user_id, device_key, auth_key, push_platform, push_token, created_at, last_seen_at, app,
                      registration_id, number)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9, ?10)",
                params![
                    device.device_id,
                    user,
                    device.device_key,
                    device.auth_key,
                    device.push_platform,
                    device.push_token,
                    now,
                    device.app,
                    device.registration_id,
                    number
                ],
            )?;
            insert_prekeys(tx, &device.device_id, prekeys, most)?;
            let username = username_of(tx, &user)?;
            Ok(Write::Done((user, username, number)))
        })
    }

    /// Claims a username for a device's user, giving up its old one; `None` only gives it up.
    pub fn set_username(&self, device: &DeviceId, name: Option<(&[u8], &str)>) -> rusqlite::Result<Write<()>> {
        self.as_device(device, |tx, user| {
            let user = &user;
            if let Some((skeleton, _)) = name {
                let owner: Option<UserId> = row(
                    tx,
                    "SELECT user_id FROM handles WHERE kind = ?1 AND value = ?2",
                    params![KIND_USERNAME, skeleton],
                    |row| array(row.get(0)?),
                )?;
                let taken = match owner {
                    Some(owner) => owner != *user,
                    None => is_reserved(tx, skeleton)?,
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
            Ok(Write::Done(()))
        })
    }

    /// A user's profile key check (spec 4.10), at once: the one before does no more.
    pub fn set_access_key(
        &self,
        user: &UserId,
        check: &[u8; 32],
        requests_open: bool,
        commitment: &[u8],
    ) -> rusqlite::Result<()> {
        self.write(|tx| {
            tx.prepare_cached(
                "INSERT INTO contact_keys (user_id, current, requests_open) VALUES (?1, ?2, ?3)
                 ON CONFLICT(user_id) DO UPDATE SET current = ?2, requests_open = ?3",
            )?
            .execute(params![user, check.as_slice(), requests_open])?;
            tx.prepare_cached(
                "INSERT INTO profile_commitments (user_id, commitment) VALUES (?1, ?2)
                 ON CONFLICT(user_id) DO UPDATE SET commitment = ?2",
            )?
            .execute(params![user, commitment])?;
            Ok(())
        })
    }

    /// For tests: a commitment without an account.
    #[cfg(test)]
    pub fn set_commitment_for_tests(&self, user: &UserId, commitment: &[u8]) {
        self.conn()
            .execute(
                "INSERT OR REPLACE INTO profile_commitments (user_id, commitment) VALUES (?1, ?2)",
                params![user, commitment],
            )
            .unwrap();
    }

    /// A user's profile key commitment (spec 4.9), if it told one.
    pub fn commitment(&self, user: &UserId) -> rusqlite::Result<Option<Vec<u8>>> {
        row(&self.conn(), "SELECT commitment FROM profile_commitments WHERE user_id = ?1", [user], |row| row.get(0))
    }

    /// What a delivery to `user` is (spec 4.10): a contact's (its key shown, or none set), a signed request, or closed.
    pub fn admit(
        &self,
        user: &UserId,
        shown: Option<[u8; 32]>,
        sender: Option<&UserId>,
    ) -> rusqlite::Result<Admission> {
        let current: Option<Vec<u8>> =
            row(&self.conn(), "SELECT current FROM contact_keys WHERE user_id = ?1", [user], |row| row.get(0))?;
        let Some(current) = current else { return Ok(Admission::Contact) };
        // In constant time, as Signal compares an access key.
        if shown.is_some_and(|shown| bool::from(subtle::ConstantTimeEq::ct_eq(&current[..], &shown[..]))) {
            return Ok(Admission::Contact);
        }
        // Signed: a request, never refused for being one; the device drops strangers' itself (as Signal's).
        Ok(if sender.is_some() { Admission::Request } else { Admission::Closed })
    }

    /// Whether a username (by its skeleton) belongs to anyone or is reserved: what registration would find taken.
    pub fn username_taken(&self, skeleton: &[u8]) -> rusqlite::Result<bool> {
        self.conn()
            .prepare_cached(
                "SELECT 1 FROM handles WHERE kind = ?1 AND value = ?2
                 UNION ALL SELECT 1 FROM reserved WHERE skeleton = CAST(?2 AS TEXT) AND (SELECT value FROM settings WHERE key='reserved_enabled') = 1",
            )?
            .exists(params![KIND_USERNAME, skeleton])
    }

    /// A user by a handle as stored: id, account key, devices and, if `take`, a bundle per device (spec 4.1).
    pub fn lookup(&self, kind: u8, value: &[u8], take: bool, except: &[Key]) -> rusqlite::Result<Option<Found>> {
        self.write(|tx| {
            let id = |row: &rusqlite::Row<'_>| array(row.get(0)?);
            let user: Option<UserId> = match kind {
                KIND_USER => row(tx, "SELECT id FROM users WHERE id = ?1", [value], id)?,
                KIND_ACCOUNT_KEY => row(tx, "SELECT id FROM users WHERE account_key = ?1", [value], id)?,
                _ => row(tx, "SELECT user_id FROM handles WHERE kind = ?1 AND value = ?2", params![kind, value], id)?,
            };
            let Some(user) = user else { return Ok(None) };
            let Some(Roster { account_key, devices: roster_devices }) = roster(tx, &user)? else { return Ok(None) };
            if roster_devices.is_empty() {
                return Ok(None);
            }
            let mut bundles = Vec::new();
            if take {
                let devices: Vec<(Vec<u8>, Key, u32)> = tx
                    .prepare_cached(
                        "SELECT id, device_key, registration_id FROM devices WHERE user_id = ?1 ORDER BY device_key",
                    )?
                    .query_map([&user], |row| Ok((row.get(0)?, array(row.get(1)?)?, row.get(2)?)))?
                    .collect::<Result<_, _>>()?;
                for (device, device_key, registration_id) in devices.iter().filter(|(_, key, _)| !except.contains(key))
                {
                    let (Some(signed), Some(last_resort)) =
                        (prekey(tx, device, SIGNED, false)?, prekey(tx, device, LAST_RESORT, false)?)
                    else {
                        // A device without prekeys: an app from before sessions.
                        continue;
                    };
                    let kyber = prekey(tx, device, KYBER, true)?.unwrap_or(last_resort);
                    let one_time = prekey(tx, device, ONE_TIME, true)?.map(|p| api::Prekey { id: p.id, key: p.key });
                    bundles.push(api::Bundle {
                        device_key: *device_key,
                        registration_id: *registration_id,
                        signed,
                        kyber,
                        one_time,
                    });
                }
            }
            let username = username_of(tx, &user)?;
            Ok(Some(Found { user_id: user, account_key, devices: roster_devices, bundles, username }))
        })
    }

    /// Deletes a device's account (spec 6.3) with everything about it (`remove_user`).
    pub fn delete_account(&self, device: &DeviceId, now: u64) -> rusqlite::Result<Write<()>> {
        self.as_device(device, |tx, user| {
            remove_user(tx, &user, now)?;
            Ok(Write::Done(()))
        })
    }

    /// Operator deletion, using the same cleanup as account deletion; unknown users change nothing.
    pub fn admin_delete_user(&self, user: &UserId, now: u64) -> rusqlite::Result<Option<Vec<(BlobId, u64)>>> {
        self.write(|tx| {
            if !tx.prepare_cached("SELECT 1 FROM users WHERE id = ?1")?.exists([user])? {
                return Ok(None);
            }
            let blobs = tx
                .prepare_cached("SELECT id, created_at FROM blobs WHERE user_id = ?1")?
                .query_map([user], blob_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            remove_user(tx, user, now)?;
            Ok(Some(blobs))
        })
    }

    // ---- groups (spec 4.9) ----

    /// A new group at revision 1; false if its id is taken.
    pub fn create_group(&self, id: &[u8], params: &[u8], state: &[u8], now: u64) -> rusqlite::Result<bool> {
        self.one_changed(
            "INSERT OR IGNORE INTO zk_groups (id, params, revision, state, updated_at, created_at)
             VALUES (?1, ?2, 1, ?3, ?4, ?4)",
            params![id, params, state, now as i64],
        )
    }

    /// A group's params and state.
    pub fn group(&self, id: &[u8]) -> rusqlite::Result<Option<(Vec<u8>, Vec<u8>)>> {
        row(&self.conn(), "SELECT params, state FROM zk_groups WHERE id = ?1", [id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
    }

    /// A group's next revision, only if it is still at `revision` (compare and swap).
    pub fn update_group(&self, id: &[u8], revision: u32, state: &[u8], now: u64) -> rusqlite::Result<bool> {
        self.one_changed(
            "UPDATE zk_groups SET revision = revision + 1, state = ?3, updated_at = ?4 WHERE id = ?1 AND revision = ?2",
            params![id, revision, state, now as i64],
        )
    }

    /// The group ends (its last member left), if it is still at `revision`.
    pub fn end_group(&self, id: &[u8], revision: u32) -> rusqlite::Result<bool> {
        self.one_changed("DELETE FROM zk_groups WHERE id = ?1 AND revision = ?2", params![id, revision])
    }

    // ---- reports, support and diagnostics ----

    /// Stores the report of `reported` by a device's user, the pair's latest; false when there is no such user.
    pub fn report(&self, device: &DeviceId, reported: &UserId, reason: u8, now: u64) -> rusqlite::Result<Write<bool>> {
        self.as_device(device, |tx, reporter| {
            let stored = tx
                .prepare_cached(
                    "INSERT INTO reports (reporter_id, reported_id, reason, created_at)
                     SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM users WHERE id = ?2)
                     ON CONFLICT (reporter_id, reported_id) DO UPDATE SET
                         reason = excluded.reason, created_at = excluded.created_at",
                )?
                .execute(params![reporter, reported, reason, now])?;
            Ok(Write::Done(stored == 1))
        })
    }

    /// Stores feedback from a device's user, up to 1000 messages a thread.
    pub fn feedback(&self, device: &DeviceId, text: &str, app_version: &str, now: u64) -> rusqlite::Result<Write<()>> {
        self.as_device(device, |tx, user| {
            // An atomic cap shared by old and new app routes. A full thread is retained, never silently trimmed.
            let count: i64 =
                tx.query_row("SELECT count(*) FROM support_messages WHERE user_id = ?1", [user], |r| r.get(0))?;
            if count >= 1000 {
                return Ok(Write::Refused(Conflict::SupportFull));
            }
            tx.execute(
                "INSERT INTO support_messages(user_id,from_team,text,created_at,app_version) VALUES (?1,0,?2,?3,?4)",
                params![user, text, now, app_version],
            )?;
            Ok(Write::Done(()))
        })
    }

    pub fn support_thread(&self, user: &UserId) -> rusqlite::Result<yo_wire::api::SupportThread> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id,from_team,text,created_at,coalesce(read_at,0) FROM support_messages WHERE user_id=?1 ORDER BY created_at,id")?;
        let messages = stmt
            .query_map([user], |r| {
                Ok(yo_wire::api::SupportMessage {
                    id: r.get(0)?,
                    from_team: r.get(1)?,
                    text: r.get::<_, String>(2)?.into_bytes().into(),
                    created_at: r.get(3)?,
                    read_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let unread = messages.iter().filter(|m| m.from_team == 1 && m.read_at == 0).count() as u32;
        Ok(yo_wire::api::SupportThread { messages, unread })
    }

    pub fn support_read(&self, user: &UserId, from_team: bool, through: u64, now: u64) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE support_messages SET read_at=CASE WHEN ?3 THEN coalesce(read_at,?1) ELSE read_at END, done_at=CASE WHEN ?3 THEN done_at ELSE ?1 END WHERE user_id=?2 AND from_team=?3 AND id<=?4",
            params![now, user, from_team, through],
        )?;
        Ok(())
    }

    pub fn support_reply(&self, user: &UserId, text: &str, now: u64) -> rusqlite::Result<bool> {
        let conn = self.conn();
        Ok(conn.execute(
            "INSERT INTO support_messages(user_id,from_team,text,created_at)
            SELECT ?1,1,?2,?3 WHERE EXISTS (SELECT 1 FROM users WHERE id=?1)
            AND EXISTS (SELECT 1 FROM support_messages WHERE user_id=?1)
            AND (SELECT count(*) FROM support_messages WHERE user_id=?1)<1000",
            params![user, text, now],
        )? == 1)
    }

    pub fn support_threads(&self) -> rusqlite::Result<serde_json::Value> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT hex(s.user_id),max(s.created_at),sum(s.from_team=0 AND s.done_at IS NULL),
            (SELECT coalesce(spelling,value) FROM handles WHERE user_id=s.user_id AND kind=2 LIMIT 1)
            FROM support_messages s WHERE length(s.user_id)>0 GROUP BY s.user_id
            ORDER BY (sum(s.from_team=0 AND s.done_at IS NULL)>0) DESC,max(s.created_at) DESC LIMIT 999",
        )?;
        let rows: Vec<_> = stmt.query_map([], |r| Ok(serde_json::json!({"user_id":r.get::<_,String>(0)?,"at":r.get::<_,i64>(1)?,"unread":r.get::<_,i64>(2)?,"name":r.get::<_,Option<String>>(3)?})))?.collect::<rusqlite::Result<_>>()?;
        Ok(serde_json::json!(rows))
    }

    pub fn admin_support_thread(&self, user: &[u8]) -> rusqlite::Result<serde_json::Value> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id,from_team,text,created_at,read_at,done_at FROM support_messages WHERE user_id=?1 ORDER BY created_at,id")?;
        let rows = stmt.query_map([user], |r| Ok(serde_json::json!({"id":r.get::<_,i64>(0)?,"from_team":r.get::<_,bool>(1)?,"text":r.get::<_,String>(2)?,"created_at":r.get::<_,i64>(3)?,"read_at":r.get::<_,Option<i64>>(4)?,"done_at":r.get::<_,Option<i64>>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(serde_json::json!({"messages": rows}))
    }

    /// Deliveries taken in the hour of `now` (`n` more); hours older than 8 days go.
    pub fn count_deliveries(&self, now: u64, n: u64) -> rusqlite::Result<()> {
        let hour = (now / 3_600_000) as i64;
        let conn = self.conn();
        conn.prepare_cached(
            "INSERT INTO deliveries_hourly (hour, n) VALUES (?1, ?2) ON CONFLICT(hour) DO UPDATE SET n = n + ?2",
        )?
        .execute(params![hour, n as i64])?;
        conn.prepare_cached("DELETE FROM deliveries_hourly WHERE hour < ?1")?.execute([hour - 8 * 24])?;
        Ok(())
    }

    /// Events counted in the minute of `now`, `(kind, n)`: counts only, nothing of who or what (6.6).
    pub fn count_events(&self, now: u64, counts: &[(&str, u64)]) -> rusqlite::Result<()> {
        let minute = (now / 60_000) as i64;
        let conn = self.conn();
        let mut add = conn.prepare_cached(
            "INSERT INTO events (kind, minute, n) VALUES (?1, ?2, ?3)
             ON CONFLICT(kind, minute) DO UPDATE SET n = n + ?3",
        )?;
        for (kind, n) in counts {
            add.execute(params![kind, minute, *n as i64])?;
        }
        Ok(())
    }

    /// Old apps' crash encoding, mapped into anonymous diagnostics.
    pub fn crash(&self, text: &str, app_version: &str, now: u64) -> rusqlite::Result<()> {
        self.diagnostic("crash", &legacy_crash(text, app_version), now)
    }

    pub fn diagnostic(&self, kind: &str, payload: &serde_json::Value, now: u64) -> rusqlite::Result<()> {
        self.write(|tx| {
            let uuid = payload["uuid"].as_str().map(str::to_ascii_lowercase);
            if let Some(uuid) = &uuid {
                if tx.prepare("SELECT 1 FROM diagnostic_seen WHERE uuid=?1")?.exists([uuid])? {
                    return Ok(());
                }
            }
            let id = diagnostic_write(tx, kind, payload, now)?;
            if let Some(uuid) = uuid {
                tx.execute("INSERT INTO diagnostic_seen(uuid, diagnostic_id) VALUES (?1, ?2)", params![uuid, id])?;
            }
            Ok(())
        })
    }

    pub fn diagnostics(&self) -> rusqlite::Result<serde_json::Value> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id,kind,count,first_seen,last_seen,app,os,model,payload,done FROM diagnostics ORDER BY last_seen DESC")?;
        let rows = stmt.query_map([], |r| Ok(serde_json::json!({
            "id":r.get::<_,i64>(0)?, "kind":r.get::<_,String>(1)?, "count":r.get::<_,i64>(2)?,
            "first_seen":r.get::<_,i64>(3)?, "last_seen":r.get::<_,i64>(4)?, "app":r.get::<_,String>(5)?,
            "os":r.get::<_,String>(6)?, "model":r.get::<_,String>(7)?, "payload":r.get::<_,String>(8)?, "done":r.get::<_,bool>(9)?
        })))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(serde_json::json!(rows))
    }

    pub fn diagnostic_done(&self, id: i64, done: bool) -> rusqlite::Result<()> {
        self.run("UPDATE diagnostics SET done=?2 WHERE id=?1", params![id, done])
    }

    /// A support message done (`Some(at)`) or open again.
    pub fn message_done(&self, id: i64, done: Option<u64>) -> rusqlite::Result<bool> {
        self.one_changed("UPDATE support_messages SET done_at=?1 WHERE id=?2", params![done, id])
    }

    /// The team's to-dos (`todos`, schema 7), oldest first.
    pub fn todos(&self) -> rusqlite::Result<serde_json::Value> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id,text,created_at,done_at FROM todos ORDER BY created_at,id")?;
        let rows: Vec<_> = stmt
            .query_map([], |r| {
                Ok(serde_json::json!({"id":r.get::<_,i64>(0)?,"text":r.get::<_,String>(1)?,
                    "created_at":r.get::<_,i64>(2)?,"done_at":r.get::<_,Option<i64>>(3)?}))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(serde_json::json!(rows))
    }

    pub fn todo_done(&self, id: i64, done: Option<u64>) -> rusqlite::Result<bool> {
        self.one_changed("UPDATE todos SET done_at=?1 WHERE id=?2", params![done, id])
    }

    pub fn todo_delete(&self, id: i64) -> rusqlite::Result<bool> {
        self.one_changed("DELETE FROM todos WHERE id=?1", [id])
    }

    /// An operator's to-do, with no account attached.
    pub fn todo(&self, text: &str, now: u64) -> rusqlite::Result<i64> {
        let conn = self.conn();
        conn.execute("INSERT INTO todos (text,created_at) VALUES (?1,?2)", params![text, now])?;
        Ok(conn.last_insert_rowid())
    }

    // ---- storage (spec 6.9) ----

    /// The storage of a user: its version and sealed bytes.
    pub fn storage(&self, user: &UserId) -> rusqlite::Result<Option<Sealed>> {
        row(&self.conn(), "SELECT version, data FROM storage WHERE user_id = ?1", [user], |row| {
            Ok((row.get::<_, i64>(0)? as u64, row.get(1)?))
        })
    }

    /// Replaces a user's storage if it is at version `replaces` (0: none); else `Err` with the current one.
    pub fn put_storage(
        &self,
        device: &DeviceId,
        replaces: u64,
        data: &[u8],
        now: u64,
    ) -> rusqlite::Result<Write<Result<(), Sealed>>> {
        self.as_device(device, |tx, user| {
            let current: Sealed = row(tx, "SELECT version, data FROM storage WHERE user_id = ?1", [&user], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get(1)?))
            })?
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
            Ok(Write::Done(Ok(())))
        })
    }

    // ---- backups (spec 7) ----

    /// Stores a user's backup record; a locator that is another user's, or any user's id, is refused.
    pub fn put_backup(
        &self,
        device: &DeviceId,
        locator: Option<&[u8]>,
        record: &[u8],
        auth: &BackupAuth,
        now: u64,
    ) -> rusqlite::Result<Write<()>> {
        self.as_device(device, |tx, user| {
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
            Ok(Write::Done(()))
        })
    }

    /// A backup record by its user id or, where no user has that id, by its locator.
    pub fn backup(&self, id: &[u8]) -> rusqlite::Result<Option<(Vec<u8>, BackupAuth)>> {
        let key = |value: Option<Vec<u8>>| value.and_then(|v| v.try_into().ok());
        row(
            &self.conn(),
            "SELECT record, auth_passkey, auth_code FROM backups WHERE user_id = ?1
             UNION ALL SELECT record, auth_passkey, auth_code FROM backups
                 WHERE locator = ?1 AND NOT EXISTS (SELECT 1 FROM users WHERE id = ?1)
             LIMIT 1",
            [id],
            |row| Ok((row.get(0)?, BackupAuth { passkey: key(row.get(1)?), code: key(row.get(2)?) })),
        )
    }

    // ---- blobs (spec 6.8) ----

    /// Whether a user's quota has room for `size` bytes now; `put_blob` asks again as it stores.
    pub fn blob_quota_left(&self, user: &UserId, size: u64, now: u64, quota: &BlobQuota) -> rusqlite::Result<bool> {
        blob_quota_left(&self.conn(), user, size, now, quota)
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

    /// A blob there and ready (`Some(true)`), there with its file on its way (`Some(false)`), or none.
    pub fn blob_ready_state(&self, id: &BlobId) -> rusqlite::Result<Option<bool>> {
        row(&self.conn(), "SELECT ready FROM blobs WHERE id = ?1", [id], |row| row.get(0))
    }

    /// Stores a blob's row, not ready, if the id is free and the quota has room.
    pub fn put_blob(
        &self,
        device: &DeviceId,
        id: &BlobId,
        size: u64,
        fetches: u32,
        now: u64,
        quota: &BlobQuota,
    ) -> rusqlite::Result<BlobPut> {
        self.write(|tx| {
            let Some(user) = user_of_device(tx, device)? else { return Ok(BlobPut::DeviceGone) };
            let ready: Option<bool> = row(tx, "SELECT ready FROM blobs WHERE id = ?1", [id], |row| row.get(0))?;
            match ready {
                Some(true) => return Ok(BlobPut::Taken),
                Some(false) => return Ok(BlobPut::Unfinished),
                None => {}
            }
            if !blob_quota_left(tx, &user, size, now, quota)? {
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
            Ok(BlobPut::Stored)
        })
    }

    /// The file is in place, the blob handed out from now on; `false`: its row went meanwhile, the upload failed.
    pub fn blob_ready(&self, id: &BlobId, created_at: u64) -> rusqlite::Result<bool> {
        self.one_changed(
            "UPDATE blobs SET ready = 1, gets_left = max(3, 3 * fetches_left) WHERE id = ?1 AND created_at = ?2 AND ready = 0",
            params![id, created_at as i64],
        )
    }

    /// A GET reserves only an abuse-budget slot. Cut-off streams return it.
    pub fn take_blob_get(&self, id: &BlobId, now: u64, ttl_ms: u64) -> rusqlite::Result<Option<(u64, u64)>> {
        row(
            &self.conn(),
            "UPDATE blobs SET gets_left = gets_left - 1 WHERE id = ?1 AND ready = 1
             AND fetches_left > 0 AND gets_left > 0 AND created_at >= ?2 RETURNING size, created_at",
            params![id, now.saturating_sub(ttl_ms) as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
    }

    pub fn return_blob_get(&self, id: &BlobId, created_at: u64) -> rusqlite::Result<()> {
        self.run(
            "UPDATE blobs SET gets_left = gets_left + 1 WHERE id = ?1 AND created_at = ?2",
            params![id, created_at as i64],
        )
    }

    /// One anonymous acknowledgement is one fetch; the last deletes the row immediately.
    pub fn ack_blob(&self, id: &BlobId, now: u64, ttl_ms: u64) -> rusqlite::Result<Option<u64>> {
        self.write(|tx| {
            let last: Option<u64> = tx.prepare_cached(
                "UPDATE blobs SET fetches_left = fetches_left - 1 WHERE id = ?1 AND ready = 1
                 AND fetches_left > 0 AND created_at >= ?2 RETURNING CASE WHEN fetches_left = 0 THEN created_at ELSE NULL END"
            )?.query_row(params![id, now.saturating_sub(ttl_ms) as i64], |row| row.get::<_, Option<u64>>(0)).optional()?.flatten();
            if last.is_some() {
                tx.execute("DELETE FROM blobs WHERE id = ?1", [id])?;
            }
            Ok(last)
        })
    }

    pub fn delete_blob(&self, id: &BlobId) -> rusqlite::Result<()> {
        self.run("DELETE FROM blobs WHERE id = ?1", [id])
    }

    /// Deletes blobs past their time, or not ready for an hour, and returns them (their files go too).
    pub fn prune_blobs(&self, now: u64, ttl_ms: u64) -> rusqlite::Result<Vec<(BlobId, u64)>> {
        self.write(|tx| {
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
            Ok(ids)
        })
    }

    /// At a start: fetches under way given back, blobs not ready (a crash mid-upload) deleted and returned.
    pub fn recover_blobs(&self) -> rusqlite::Result<Vec<(BlobId, u64)>> {
        self.write(|tx| {
            tx.execute("UPDATE blobs SET fetches_left = fetches_left + fetching, fetching = 0 WHERE fetching > 0", [])?;
            let ids = tx
                .prepare_cached("DELETE FROM blobs WHERE ready = 0 RETURNING id, created_at")?
                .query_map([], blob_row)?
                .collect::<Result<_, _>>()?;
            Ok(ids)
        })
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

    // ---- prekeys and push ----

    /// Stores a device's upload of prekeys (`insert_prekeys`).
    pub fn add_prekeys(&self, device: &DeviceId, prekeys: &api::Prekeys, most: u32) -> rusqlite::Result<Write<()>> {
        self.as_device(device, |tx, _| {
            insert_prekeys(tx, device, prekeys, most)?;
            Ok(Write::Done(()))
        })
    }

    /// The one-time prekeys a device has left here: of the scarcer kind.
    pub fn prekeys_left(&self, device: &DeviceId) -> rusqlite::Result<u32> {
        self.conn()
            .prepare_cached(
                "SELECT min(
                     (SELECT count(*) FROM prekeys WHERE device_id = ?1 AND kind = ?2),
                     (SELECT count(*) FROM prekeys WHERE device_id = ?1 AND kind = ?3))",
            )?
            .query_row(params![device, ONE_TIME, KYBER], |row| row.get(0))
    }

    /// Sets a device's push token; refused for a device replaced since it authenticated.
    pub fn set_push(&self, device: &DeviceId, platform: u8, token: &[u8]) -> rusqlite::Result<Write<()>> {
        let changed = self.one_changed(
            "UPDATE devices SET push_platform = ?2, push_token = ?3 WHERE id = ?1",
            params![device, platform, token],
        )?;
        Ok(if changed { Write::Done(()) } else { Write::Refused(Conflict::DeviceGone) })
    }

    /// Clears a push token the push service refused, unless the device set another meanwhile.
    pub fn clear_push(&self, device: &DeviceId, token: &[u8]) -> rusqlite::Result<()> {
        self.run(
            "UPDATE devices SET push_platform = 0, push_token = x'' WHERE id = ?1 AND push_token = ?2",
            params![device, token],
        )
    }

    // ---- delivery ----

    /// Delivers envelopes in one transaction (group commit): each to every device of its user, or to none (spec 4.4).
    /// Immediate, as every write here: a deferred one that read first and then wrote failed at once ("database is
    /// locked", a 500) when another connection (litestream's) committed meanwhile, busy timeout or not.
    pub fn deliver(&self, items: &[Incoming<'_>], now: u64, most: u32) -> rusqlite::Result<Vec<Delivered>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut outcomes = Vec::with_capacity(items.len());
        for Incoming { user, devices_hash, envelope } in items {
            let Some(current) = roster(&tx, user)? else {
                outcomes.push(Delivered::Unknown);
                continue;
            };
            if current.devices.is_empty() {
                outcomes.push(Delivered::Unknown);
                continue;
            }
            if yo_wire::devices::hash(&current.account_key, &current.keys()) != *devices_hash {
                outcomes.push(Delivered::Mismatch(current));
                continue;
            }
            let devices: Vec<Wake> = tx
                .prepare_cached("SELECT id, push_platform, push_token, app FROM devices WHERE user_id = ?1")?
                .query_map([user], |row| {
                    Ok(Wake {
                        device_id: array(row.get(0)?)?,
                        push_platform: row.get(1)?,
                        push_token: row.get(2)?,
                        bot: is_bot_app(row.get::<_, Option<String>>(3)?.as_deref()),
                    })
                })?
                .collect::<Result<_, _>>()?;
            // Only the last allowed slot matters: an index probe, no count.
            let mut slot = tx.prepare_cached("SELECT 1 FROM messages WHERE device_id = ?1 LIMIT 1 OFFSET ?2")?;
            let mut full = most == 0;
            for device in &devices {
                if full {
                    break;
                }
                full = slot.exists(params![device.device_id, most - 1])?;
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

    /// A device's envelopes, oldest first: id, envelope, when it came (ms).
    pub fn fetch(&self, device: &DeviceId, most: usize) -> rusqlite::Result<Vec<(u64, Vec<u8>, u64)>> {
        self.conn()
            .prepare_cached("SELECT id, envelope, received_at FROM messages WHERE device_id = ?1 ORDER BY id LIMIT ?2")?
            .query_map(params![device, most as i64], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get(1)?, row.get::<_, i64>(2)? as u64))
            })?
            .collect()
    }

    /// A device's number among its user's devices (spec 4).
    pub fn device_number(&self, device: &DeviceId) -> rusqlite::Result<Option<u8>> {
        row(&self.conn(), "SELECT number FROM devices WHERE id = ?1", [device], |row| row.get(0))
    }

    /// Deletes a device's envelopes by id (WAL NORMAL: one undone by a power loss comes again, spec 4.7).
    /// Returns how many there were.
    /// Deletes the device's envelopes of `ids`; when each one deleted was accepted (ms), for the latency counts.
    pub fn ack(&self, device: &DeviceId, ids: &[u64]) -> rusqlite::Result<Vec<u64>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut acked = Vec::new();
        {
            let mut delete =
                tx.prepare_cached("DELETE FROM messages WHERE device_id = ?1 AND id = ?2 RETURNING received_at")?;
            for id in ids {
                if let Some(at) = delete.query_row(params![device, *id as i64], |r| r.get::<_, i64>(0)).optional()? {
                    acked.push(at as u64);
                }
            }
        }
        tx.commit()?;
        Ok(acked)
    }

    // ---- upkeep and audit ----

    /// Deletes envelopes older than the TTL, and forgets replaced devices after a year.
    pub fn prune(&self, now: u64, ttl_ms: u64) -> rusqlite::Result<usize> {
        let conn = self.conn();
        let before = now.saturating_sub(ttl_ms) as i64;
        let messages = conn.execute("DELETE FROM messages WHERE received_at < ?1", [before])?;
        conn.execute("DELETE FROM gone WHERE gone_at < ?1", [now.saturating_sub(365 * 24 * 3600 * 1000) as i64])?;
        Ok(messages)
    }

    /// The apps the devices named last (`Yo-App`), with how many devices each. For the operator; reads only.
    pub fn apps(&self) -> rusqlite::Result<Vec<(String, i64)>> {
        let conn = self.conn();
        let mut statement = conn.prepare("SELECT coalesce(app, ''), count(*) FROM devices GROUP BY 1 ORDER BY 1")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// Rows that belong to nobody, with how many fail each check; empty when consistent.
    pub fn audit(&self) -> rusqlite::Result<Vec<(&'static str, i64)>> {
        // Each: what fails, and the rows that fail it (`SELECT count(*) FROM …`).
        const CHECKS: &[(&str, &str)] = &[
            ("devices without their user", "devices WHERE user_id NOT IN (SELECT id FROM users)"),
            ("users without a device", "users WHERE id NOT IN (SELECT user_id FROM devices)"),
            ("users whose key is not theirs", "users u WHERE NOT EXISTS (SELECT 1 FROM account_keys k WHERE k.key = u.account_key AND k.user_id = u.id)"),
            ("account keys without their user", "account_keys WHERE user_id NOT IN (SELECT id FROM users)"),
            ("handles without their user", "handles WHERE user_id NOT IN (SELECT id FROM users)"),
            ("backups without their user", "backups WHERE user_id NOT IN (SELECT id FROM users)"),
            ("reports by or about nobody", "reports WHERE reporter_id NOT IN (SELECT id FROM users) OR reported_id NOT IN (SELECT id FROM users)"),
            ("support of a user gone, not anonymised", "support_messages WHERE user_id != zeroblob(16) AND user_id NOT IN (SELECT id FROM users)"),
            ("storage without its user", "storage WHERE user_id NOT IN (SELECT id FROM users)"),
            ("blobs without their user", "blobs WHERE user_id NOT IN (SELECT id FROM users)"),
            ("blob quotas without their user", "blob_quota WHERE user_id NOT IN (SELECT id FROM users)"),
            ("prekeys without their device", "prekeys WHERE device_id NOT IN (SELECT id FROM devices)"),
            ("envelopes without their device", "messages WHERE device_id NOT IN (SELECT id FROM devices)"),
            ("devices both there and gone", "devices WHERE id IN (SELECT id FROM gone)"),
        ];
        let conn = self.conn();
        let mut failed = Vec::new();
        for (what, sql) in CHECKS {
            let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {sql}"), [], |row| row.get(0))?;
            if n > 0 {
                failed.push((*what, n));
            }
        }
        Ok(failed)
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
            registration_id: n as u32,
            push_platform: 0,
            push_token: Vec::new(),
            app: None,
        }
    }

    fn signed(id: u32) -> api::SignedPrekey {
        api::SignedPrekey { id, key: vec![id as u8; 33].into(), signature: vec![1; 64].into() }
    }

    /// A device's first prekeys: its signed prekey and last resort, and `n` one-time ones of each kind.
    fn prekeys(n: u32) -> api::Prekeys {
        api::Prekeys {
            signed: Some(signed(1)),
            last_resort: Some(signed(2)),
            one_time: (10..10 + n).map(|id| api::Prekey { id, key: vec![id as u8; 33].into() }).collect(),
            kyber: (100..100 + n).map(signed).collect(),
        }
    }

    /// Registers device `n` for a username with account key `key`.
    fn register(db: &Db, name: &[u8], key: u8, n: u8, now: u64) -> Write<(UserId, Option<String>, u8)> {
        let spelling = std::str::from_utf8(name).unwrap();
        let skeleton = yo_wire::handle::skeleton(spelling);
        db.register(skeleton.as_bytes(), spelling, &[n; 16], &[key; 32], &device(n), &prekeys(0), 100, now).unwrap()
    }

    /// A lookup hands out a bundle per device: its one-time prekeys once each, the last resort Kyber prekey when
    /// none is left; a device without prekeys (an app from before sessions) has no bundle. A repeated upload changes
    /// nothing, and one-time prekeys beyond the most are not kept.
    #[test]
    fn feedback_type_migration_and_writes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE feedback (id INTEGER PRIMARY KEY, user_id BLOB NOT NULL,
            text TEXT NOT NULL, app_version TEXT NOT NULL, created_at INTEGER NOT NULL, auto INTEGER NOT NULL DEFAULT 0);
            INSERT INTO feedback VALUES (1, X'', 'plain', '1', 1, 0),
                (2, X'', 'crash', 'admin', 2, 1), (3, X'', 'todo', 'admin', 3, 0);").unwrap();
        with_feedback_type(&conn).unwrap();
        with_feedback_type(&conn).unwrap();
        let kinds: Vec<String> = conn
            .prepare("SELECT type FROM feedback ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(kinds, ["feedback", "crash", "todo"]);
        let db = Db::open(":memory:").unwrap();
        db.crash("trace", "1", 7).unwrap();
        let id = db.todo("fix", 8).unwrap();
        let row: (String, u64, Option<u64>) = db
            .conn()
            .query_row("SELECT text,created_at,done_at FROM todos WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(row, ("fix".into(), 8, None));
        with_support(&conn, 0).unwrap();
        with_support(&conn, 0).unwrap();
        conn.execute("INSERT INTO support_messages(user_id,from_team,text,created_at) VALUES(X'',0,'new',4)", [])
            .unwrap();
        assert!(conn.last_insert_rowid() > 3, "new support IDs cannot reuse old crash or todo done flags");
        assert_eq!(
            conn.query_row("SELECT text FROM support_messages WHERE id=1", [], |r| r.get::<_, String>(0)).unwrap(),
            "plain"
        );
        assert_eq!(conn.query_row("SELECT text FROM todos WHERE id=3", [], |r| r.get::<_, String>(0)).unwrap(), "todo");
        assert_eq!(conn.query_row("SELECT count(*) FROM feedback", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        assert_eq!(
            db.conn().query_row("SELECT kind FROM diagnostics", [], |r| r.get::<_, String>(0)).unwrap(),
            "crash"
        );
    }

    #[test]
    fn schemas_3_and_4_reach_8_without_losing_support_or_diagnostics() {
        for version in [3, 4] {
            let path = std::env::temp_dir().join(format!("yo-support-{version}-{}.sqlite", std::process::id()));
            let _ = std::fs::remove_file(&path);
            {
                let db = Db::open(path.to_str().unwrap()).unwrap();
                let mut conn = db.conn();
                conn.execute_batch(LEGACY_FEEDBACK).unwrap();
                conn.execute_batch(
                    "DROP TABLE support_messages;
                    DROP TABLE diagnostic_seen; DROP TABLE diagnostics; DROP TABLE diagnostic_days;
                    INSERT INTO feedback(id,user_id,text,app_version,created_at,auto,type) VALUES
                    (10,zeroblob(16),'Help with notifications 🎨','ios/123',1700000000000,0,'feedback'),
                    (11,zeroblob(16),'ios/123 · 26 · iPhone\nSIGSEGV\nYo +0x10','ios/123',1700000001000,1,'crash'),
                    (12,zeroblob(16),'Fix notifications','admin',1700000002000,0,'todo'),
                    (13,zeroblob(16),'ios/123 · 26 · iPhone\nSIGSEGV\nYo +0x10','ios/123',1700000003000,1,'crash');
                    PRAGMA user_version=3;",
                )
                .unwrap();
                if version == 4 {
                    conn.execute_batch(DIAGNOSTIC_DAYS).unwrap();
                    migrate_diagnostics(&mut conn).unwrap();
                    conn.execute_batch("UPDATE diagnostics SET done=1;
                        CREATE TABLE diagnostic_seen(uuid TEXT PRIMARY KEY, diagnostic_id INTEGER NOT NULL REFERENCES diagnostics(id));
                        INSERT INTO diagnostic_seen SELECT '550e8400-e29b-41d4-a716-446655440000',id FROM diagnostics;
                        DROP TABLE diagnostic_days;").unwrap();
                }
            }
            for _ in 0..2 {
                let db = Db::open(path.to_str().unwrap()).unwrap();
                let conn = db.conn();
                assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 8);
                let message: (String, String, i64) = conn
                    .query_row("SELECT text,app_version,created_at FROM support_messages WHERE id=10", [], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })
                    .unwrap();
                assert_eq!(message, ("Help with notifications 🎨".into(), "ios/123".into(), 1700000000000));
                assert_eq!(
                    conn.query_row("SELECT text FROM todos", [], |r| r.get::<_, String>(0)).unwrap(),
                    "Fix notifications"
                );
                assert!(!conn.prepare("SELECT 1 FROM support_messages WHERE user_id=X''").unwrap().exists([]).unwrap());
                assert!(!conn
                    .prepare("SELECT 1 FROM sqlite_master WHERE name='feedback'")
                    .unwrap()
                    .exists([])
                    .unwrap());
                let diagnostic: (i64, i64, i64, bool) = conn
                    .query_row("SELECT count,first_seen,last_seen,done FROM diagnostics", [], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })
                    .unwrap();
                assert_eq!(diagnostic, (2, 1700000001000, 1700000003000, version == 4));
                if version == 3 {
                    assert!(
                        conn.query_row("SELECT seq FROM sqlite_sequence WHERE name='support_messages'", [], |r| r
                            .get::<_, i64>(0))
                            .unwrap()
                            >= 13
                    );
                }
                if version == 4 {
                    assert_eq!(
                        conn.query_row("SELECT count(*) FROM diagnostic_seen", [], |r| r.get::<_, i64>(0)).unwrap(),
                        1
                    );
                }
            }
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn schema_5_done_migration_preserves_colliding_todos_and_receipts() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE support_messages(id INTEGER PRIMARY KEY AUTOINCREMENT,user_id BLOB,from_team INTEGER,text TEXT,created_at INTEGER,read_at INTEGER);
            CREATE TABLE todos(id INTEGER PRIMARY KEY,text TEXT,created_at INTEGER,done_at INTEGER);
            INSERT INTO support_messages VALUES(1,zeroblob(16),0,'handled',100,200),
                (2,zeroblob(16),0,'open',101,NULL),(3,zeroblob(16),1,'reply',102,210);
            INSERT INTO todos VALUES(1,'open task 🎨',90,NULL),(3,'completed task',95,205);
            PRAGMA user_version=5;").unwrap();
        for _ in 0..2 {
            with_support_done(&mut conn).unwrap();
            assert_eq!(conn.query_row("SELECT count(*) FROM support_messages", [], |r| r.get::<_, i64>(0)).unwrap(), 5);
            assert_eq!(
                conn.query_row("SELECT done_at FROM support_messages WHERE id=1", [], |r| r.get::<_, i64>(0)).unwrap(),
                200
            );
            assert_eq!(conn.query_row("SELECT count(*) FROM support_messages WHERE user_id=X'' AND from_team=1 AND done_at IS NULL AND text='open task 🎨'", [], |r| r.get::<_,i64>(0)).unwrap(),1);
            assert_eq!(
                conn.query_row("SELECT done_at FROM support_messages WHERE text='completed task'", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                205
            );
            assert_eq!(
                conn.query_row("SELECT read_at FROM support_messages WHERE id=3", [], |r| r.get::<_, i64>(0)).unwrap(),
                210
            );
        }
        conn.execute("UPDATE support_messages SET done_at=NULL WHERE id=1", []).unwrap();
        with_support_done(&mut conn).unwrap();
        assert!(conn
            .query_row("SELECT done_at FROM support_messages WHERE id=1", [], |r| r.get::<_, Option<i64>>(0))
            .unwrap()
            .is_none());
    }

    #[test]
    fn support_cap_and_read_direction() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        db.feedback(&[1; 16], "hi", "1", 1).unwrap();
        assert!(db.support_reply(&leo, "reply", 2).unwrap());
        db.support_read(&leo, false, 100, 3).unwrap();
        assert_eq!(db.support_thread(&leo).unwrap().unread, 1);
        db.support_read(&leo, true, 2, 4).unwrap();
        assert_eq!(db.support_thread(&leo).unwrap().unread, 0);
        for _ in 2..1000 {
            db.feedback(&[1; 16], "hi", "1", 5).unwrap();
        }
        assert!(matches!(db.feedback(&[1; 16], "full", "1", 6).unwrap(), Write::Refused(Conflict::SupportFull)));
        assert!(!db.support_reply(&leo, "full", 6).unwrap());
        assert_eq!(db.support_thread(&leo).unwrap().messages.len(), 1000);
        let id = db.todo("task", 7).unwrap();
        assert!(db.todo_done(id, Some(8)).unwrap());
        assert_eq!(db.todos().unwrap()[0]["done_at"], 8);
        assert!(db.todo_done(id, None).unwrap());
        assert!(!db.todo_done(id + 1, None).unwrap());
        assert!(db.support_threads().unwrap().as_array().unwrap().iter().all(|t| t["user_id"] != "team"));
        assert!(db.todo_delete(id).unwrap() && !db.todo_delete(id).unwrap());
        assert_eq!(db.todos().unwrap(), serde_json::json!([]));
    }

    #[test]
    fn schema_7_moves_the_team_thread_into_todos_once() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE support_messages(id INTEGER PRIMARY KEY AUTOINCREMENT,user_id BLOB,from_team INTEGER,
                text TEXT,created_at INTEGER,read_at INTEGER,app_version TEXT NOT NULL DEFAULT '',done_at INTEGER);
            INSERT INTO support_messages(user_id,from_team,text,created_at,done_at) VALUES
                (zeroblob(16),0,'hi',100,NULL),(X'',1,'open task 🎨',90,NULL),(X'',1,'done task',95,205);
            PRAGMA user_version=6;",
        )
        .unwrap();
        for _ in 0..2 {
            with_support_done(&mut conn).unwrap();
            with_todos(&mut conn).unwrap();
            let todos: Vec<(String, i64, Option<i64>)> = conn
                .prepare("SELECT text,created_at,done_at FROM todos ORDER BY id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert_eq!(todos, [("open task 🎨".into(), 90, None), ("done task".into(), 95, Some(205))]);
            assert_eq!(conn.query_row("SELECT count(*) FROM support_messages", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
            assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 7);
        }
    }

    #[test]
    fn diagnostics_uuid_is_idempotent() {
        let db = Db::open(":memory:").unwrap();
        let mut payload = serde_json::json!({"uuid": "550e8400-e29b-41d4-a716-446655440000",
            "app": "ios/test", "domain": "test", "code": "1", "place": "test"});
        db.diagnostic("error", &payload, 1).unwrap();
        db.diagnostic("error", &payload, 2).unwrap();
        assert_eq!(db.diagnostics().unwrap()[0]["count"], 1);
        payload["uuid"] = "550e8400-e29b-41d4-a716-446655440001".into();
        db.diagnostic("error", &payload, 3).unwrap();
        assert_eq!(db.diagnostics().unwrap()[0]["count"], 2);
        payload.as_object_mut().unwrap().remove("uuid");
        db.diagnostic("error", &payload, 4).unwrap();
        db.diagnostic("error", &payload, 5).unwrap();
        assert_eq!(db.diagnostics().unwrap()[0]["count"], 4);
    }

    #[test]
    fn diagnostics_migrate_once_and_count_every_occurrence() {
        let db = Db::open(":memory:").unwrap();
        {
            let mut conn = db.conn();
            conn.execute_batch("DROP TABLE diagnostics; PRAGMA user_version = 3").unwrap();
            conn.execute_batch(LEGACY_FEEDBACK).unwrap();
            conn.execute("INSERT INTO feedback(user_id,text,app_version,created_at,auto,type) VALUES(X'', 'trace','1',2,1,'crash'), (X'', 'trace','1',3,1,'crash')", []).unwrap();
            migrate_diagnostics(&mut conn).unwrap();
            migrate_diagnostics(&mut conn).unwrap();
            assert_eq!(conn.query_row("SELECT count(*) FROM feedback", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        }
        let rows = db.diagnostics().unwrap();
        assert_eq!(rows[0]["count"], 2);
        assert_eq!(rows[0]["first_seen"], 2);
        assert_eq!(rows[0]["last_seen"], 3);
        db.diagnostic_done(rows[0]["id"].as_i64().unwrap(), true).unwrap();
        db.crash("trace", "1", 4).unwrap();
        assert_eq!(db.diagnostics().unwrap()[0]["done"], true);
        let mut error = serde_json::json!({"domain":"test", "code":"1", "place":"sync", "app":"ios/2", "os":"26", "model":"iPhone"});
        for at in 10..110 {
            db.diagnostic("error", &error, at).unwrap();
        }
        error["os"] = serde_json::json!("27");
        db.diagnostic("error", &error, 110).unwrap();
        let rows = db.diagnostics().unwrap();
        assert_eq!(rows[0]["count"], 101);
        assert_eq!(rows[0]["os"], "27");
        assert_eq!(rows[0]["first_seen"], 10);
        error["app"] = serde_json::json!("ios/3");
        db.diagnostic("error", &error, 111).unwrap();
        assert_eq!(db.diagnostics().unwrap().as_array().unwrap().len(), 3);
    }

    #[test]
    fn prekeys_go_once_and_the_last_resort_after() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        let mut more = prekeys(2);
        more.signed = None;
        more.last_resort = None;
        assert!(matches!(db.add_prekeys(&[1; 16], &more, 3).unwrap(), Write::Done(())));
        assert!(matches!(db.add_prekeys(&[1; 16], &more, 3).unwrap(), Write::Done(())));
        assert_eq!(db.prekeys_left(&[1; 16]).unwrap(), 2);
        let bundle = |db: &Db| db.lookup(KIND_USER, &leo, true, &[]).unwrap().unwrap().bundles.remove(0);
        let first = bundle(&db);
        assert_eq!((first.registration_id, first.signed.id, first.kyber.id), (1, 1, 100));
        assert_eq!(first.one_time.map(|p| p.id), Some(10));
        let second = bundle(&db);
        assert_eq!((second.kyber.id, second.one_time.map(|p| p.id)), (101, Some(11)));
        assert_eq!(db.prekeys_left(&[1; 16]).unwrap(), 0);
        assert!(matches!(db.add_prekeys(&[1; 16], &more, 3).unwrap(), Write::Done(())));
        assert_eq!(db.prekeys_left(&[1; 16]).unwrap(), 0, "handed out once: a repeated upload brings none back");
        let last = bundle(&db);
        assert_eq!((last.kyber.id, last.one_time), (2, None), "the last resort, no one-time key left");
        // Of a device asked not to hand out, nothing; and nothing taken by a lookup without prekeys.
        assert!(db.lookup(KIND_USER, &leo, true, &[[1; 32]]).unwrap().unwrap().bundles.is_empty());
        db.conn().execute("DELETE FROM prekeys", []).unwrap();
        let found = db.lookup(KIND_USER, &leo, true, &[]).unwrap().unwrap();
        assert_eq!((found.devices.len(), found.bundles.len()), (1, 0), "a device without prekeys: no bundle");
    }

    fn reserve(db: &Db, names: &[&str]) -> usize {
        let names: Vec<Reserved> = names.iter().map(|n| Reserved { spelling: n.to_string(), note: None }).collect();
        db.add_reserved(&names, 0).unwrap()
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
    fn reservation_switch_preserves_names_and_controls_all_claims() {
        let db = Db::open(":memory:").unwrap();
        assert_eq!(db.reserved_state().unwrap()["enabled"], true);
        reserve(&db, &["Admin", "Support"]);
        db.set_reserved_enabled(false).unwrap();
        assert_eq!(db.reserved_state().unwrap()["names"].as_array().unwrap().len(), 2);
        assert!(!db.username_taken(b"ADMIN").unwrap());
        let owner = user(register(&db, b"Admin", 1, 1, 0));
        assert!(db.username_taken(b"ADMIN").unwrap(), "a holder still takes the name while off");
        assert!(matches!(db.set_username(&owner, Some((b"SUPPORT", "Support"))).unwrap(), Write::Done(())));
        db.set_reserved_enabled(true).unwrap();
        assert!(db.username_taken(b"ADMIN").unwrap());
        assert!(matches!(register(&db, b"Admin", 2, 2, 0), Write::Refused(Conflict::UsernameTaken)));
        db.remove_reserved("Adm1n").unwrap();
        assert!(!db.username_taken(b"ADMIN").unwrap());
        assert_eq!(db.reserved_state().unwrap()["names"], serde_json::json!(["Support"]));
    }

    #[test]
    fn events_are_counts_per_kind_and_minute_kept_for_good() {
        let db = Db::open(":memory:").unwrap();
        let minute = 60_000u64;
        db.count_events(10 * minute + 5, &[("yo-in", 2), ("media-in", 1)]).unwrap();
        db.count_events(10 * minute + 59_999, &[("yo-in", 3)]).unwrap();
        db.count_events(11 * minute, &[("yo-out", 4)]).unwrap();
        let rows = |db: &Db| -> Vec<(String, i64, i64)> {
            db.conn()
                .prepare("SELECT kind, minute, n FROM events ORDER BY minute, kind")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let row = |kind: &str, minute, n| (kind.to_owned(), minute, n);
        assert_eq!(rows(&db), [row("media-in", 10, 1), row("yo-in", 10, 5), row("yo-out", 11, 4)]);
        let columns: Vec<String> = db
            .conn()
            .prepare("SELECT name FROM pragma_table_info('events')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(columns, ["kind", "minute", "n"], "no identifier of anyone or anything");
        db.prune(1000 * DAY_MS, DAY_MS).unwrap();
        assert_eq!(rows(&db).len(), 3, "kept for good: counts of no one");
    }

    #[test]
    fn groups_made_before_created_at_keep_null() {
        let path = std::env::temp_dir().join(format!("yo-groups-created-{}.sqlite", std::process::id()));
        let path = path.to_str().unwrap();
        let _ = std::fs::remove_file(path);
        {
            let db = Db::open(path).unwrap();
            db.conn().execute_batch("ALTER TABLE zk_groups DROP COLUMN created_at").unwrap();
            db.conn()
                .execute(
                    "INSERT INTO zk_groups (id, params, revision, state, updated_at) VALUES (X'01', X'', 1, X'', 5)",
                    [],
                )
                .unwrap();
        }
        let db = Db::open(path).unwrap();
        assert!(db.create_group(&[2], &[], &[], 7).unwrap());
        let made: Vec<Option<i64>> = db
            .conn()
            .prepare("SELECT created_at FROM zk_groups ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(made, [None, Some(7)]);
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reserved_notes_are_set_replaced_and_cleared_without_touching_spelling() {
        let db = Db::open(":memory:").unwrap();
        let noted = |name: &str, note: Option<&str>| Reserved { spelling: name.into(), note: note.map(str::to_owned) };
        reserve(&db, &["Admin"]);
        assert_eq!(db.set_reserved_notes(&[noted("ADM1N", Some("role")), noted("Open", Some("app"))], 0).unwrap(), 1);
        assert_eq!(db.reserved_state().unwrap()["notes"], serde_json::json!({"Admin": "role", "Open": "app"}));
        assert_eq!(db.add_reserved(&[noted("admin", Some("ignored"))], 0).unwrap(), 0, "plain adds keep notes");
        db.set_reserved_notes(&[noted("open", None)], 0).unwrap();
        assert_eq!(db.reserved_state().unwrap()["names"], serde_json::json!(["Admin", "Open"]));
        assert_eq!(db.reserved_state().unwrap()["notes"], serde_json::json!({"Admin": "role"}));
        assert!(db.username_taken(yo_wire::handle::skeleton("open").as_bytes()).unwrap());
    }

    #[test]
    fn reservation_migration_keeps_the_old_state_once_and_persists_switch() {
        for populated in [false, true] {
            let path = std::env::temp_dir().join(format!("yo-reserved-{}-{populated}.sqlite", std::process::id()));
            let path = path.to_str().unwrap();
            let _ = std::fs::remove_file(path);
            {
                let db = Db::open(path).unwrap();
                if populated {
                    reserve(&db, &["Admin"]);
                }
                db.conn().execute_batch("DROP TABLE settings").unwrap();
            }
            {
                let db = Db::open(path).unwrap();
                assert_eq!(db.reserved_state().unwrap()["enabled"], populated);
                db.set_reserved_enabled(!populated).unwrap();
            }
            {
                let db = Db::open(path).unwrap();
                assert_eq!(db.reserved_state().unwrap()["enabled"], !populated);
                assert_eq!(db.reserved_state().unwrap()["names"].as_array().unwrap().len(), usize::from(populated));
            }
            std::fs::remove_file(path).unwrap();
        }
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

    /// A database of schema 1 (the hubs of MLS chats, also one from before spellings and the backups' auth keys):
    /// opened, its accounts stay (the user, its name shown as its skeleton, its device without prekeys), its MLS tables,
    /// the envelopes waiting and the backups of then go, the account key is its Curve25519 form (schema 3), and the
    /// user's own key registers it again.
    #[test]
    fn a_database_of_schema_1_keeps_its_accounts() {
        let path = std::env::temp_dir().join(format!("yo-hub-schema-1-{}.sqlite", std::process::id()));
        let path = path.to_str().unwrap().to_owned();
        let _ = std::fs::remove_file(&path);
        let point = curve25519_dalek::constants::ED25519_BASEPOINT_POINT * curve25519_dalek::Scalar::from(7u64);
        let (ed, curve) = (point.compress(), point.to_montgomery().to_bytes());
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA user_version = 1").unwrap();
            let schema = SCHEMA
                .replace("    spelling TEXT,\n", "")
                .replace("    app TEXT,\n    registration_id INTEGER NOT NULL DEFAULT 0\n", "    app TEXT\n");
            conn.execute_batch(&schema).unwrap();
            conn.execute_batch(
                "DROP TABLE prekeys;
                 CREATE TABLE key_packages (id INTEGER PRIMARY KEY, device_id BLOB, data BLOB, last_resort INTEGER);
                 CREATE TABLE groups (id BLOB PRIMARY KEY, epoch INTEGER, token_hash BLOB, updated_at INTEGER);
                 CREATE TABLE commits (group_id BLOB, epoch INTEGER, envelope BLOB);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO users (id, account_key, created_at) VALUES (?1, ?2, 0)",
                params![[1u8; 16], ed.to_bytes()],
            )
            .unwrap();
            conn.execute("INSERT INTO account_keys (key, user_id) VALUES (?1, ?2)", params![ed.to_bytes(), [1u8; 16]])
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
            conn.execute("INSERT INTO messages (device_id, envelope, received_at) VALUES (?1, x'02', 0)", [[1u8; 16]])
                .unwrap();
            conn.execute("INSERT INTO key_packages (device_id, data, last_resort) VALUES (?1, x'03', 1)", [[1u8; 16]])
                .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert!(db.backup(&[1; 16]).unwrap().is_none(), "the words' backups are gone");
        assert_eq!(db.account_key(&[1; 16]).unwrap(), Some(curve), "its Curve25519 form");
        assert_eq!(spelling(&db, "Christopher").as_deref(), Some("CHRISTOPHER"));
        let found = db.lookup(KIND_USER, &[1; 16], true, &[]).unwrap().unwrap();
        assert_eq!(found.devices, vec![api::Device { key: [1; 32], id: 1 }], "the device, number 1");
        assert!(found.bundles.is_empty(), "without prekeys");
        assert!(db.fetch(&[1; 16], 10).unwrap().is_empty(), "the envelopes of then are gone");
        let tables: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN ('key_packages', 'groups', 'commits')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
        assert!(matches!(register(&db, b"christopher", 2, 2, 0), Write::Refused(Conflict::UsernameTaken)));
        drop(db);
        // Opened again: schema 3 now. The user's own key registers it again (an app updated, a new phone).
        let db = Db::open(&path).unwrap();
        let skeleton = yo_wire::handle::skeleton("Christopher");
        let again = db.register(skeleton.as_bytes(), "Christopher", &[9; 16], &curve, &device(9), &prekeys(0), 100, 0);
        assert_eq!(user(again.unwrap()), [1; 16]);
        assert_eq!(db.lookup(KIND_USER, &[1; 16], true, &[]).unwrap().unwrap().bundles.len(), 1);
        assert_eq!(db.audit().unwrap(), []);
        drop(db);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{path}{suffix}"));
        }
    }

    fn user(written: Write<(UserId, Option<String>, u8)>) -> UserId {
        match written {
            Write::Done((user, _, _)) => user,
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
            matches!(register(&db, b"MIA", 1, 4, 0), Write::Done((user, Some(name), _)) if user == first && name == "LEO")
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
            matches!(register(&db, b"LEO", 1, 2, 0), Write::Done((user, Some(name), _)) if user == leo && name == "Leon")
        );
        assert!(!db.username_taken(b"LEO").unwrap());
        // The old name someone else's now: still Leon.
        let mia = user(register(&db, b"LEO", 3, 3, 0));
        assert_ne!(mia, leo);
        assert!(
            matches!(register(&db, b"LEO", 1, 4, 0), Write::Done((user, Some(name), _)) if user == leo && name == "Leon")
        );
        assert_eq!(db.device(&[3; 16]).unwrap().unwrap().user_id, mia, "the stranger keeps the name and the device");
        assert!(db.is_gone(&[2; 16]).unwrap());
        // No name at all: the same user, without one.
        assert!(matches!(db.set_username(&[4; 16], None).unwrap(), Write::Done(())));
        assert!(matches!(register(&db, b"LEON", 1, 5, 0), Write::Done((user, None, _)) if user == leo));
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
        assert!(gone(db.add_prekeys(&[1; 16], &prekeys(1), 100).unwrap()));
        assert!(gone(db.set_push(&[1; 16], 1, b"ab").unwrap()));
        // Deleted: nothing is written for a user that is no more.
        assert!(matches!(db.delete_account(&[2; 16], 1).unwrap(), Write::Done(())));
        assert!(gone(db.set_username(&[2; 16], Some((b"LEO", "LEO"))).unwrap()));
        assert!(gone(db.add_prekeys(&[2; 16], &prekeys(1), 100).unwrap()));
        assert!(db.lookup(KIND_USER, &leo, false, &[]).unwrap().is_none());
        assert!(!db.username_taken(b"LEO").unwrap());
        assert_eq!(db.audit().unwrap(), []);
    }

    #[test]
    fn existing_blob_rows_gain_a_persisted_get_budget() {
        let path = std::env::temp_dir().join(format!("yo-blob-ack-migration-{}.sqlite", std::process::id()));
        let path = path.to_str().unwrap();
        let _ = std::fs::remove_file(path);
        {
            let db = Db::open(path).unwrap();
            user(register(&db, b"LEO", 1, 1, 0));
            let quota = BlobQuota { per_day: 10, bytes_per_day: 1 << 20, stored: 1 << 20 };
            db.put_blob(&[1; 16], &[9; 16], 100, 2, 5, &quota).unwrap();
            db.blob_ready(&[9; 16], 5).unwrap();
            db.conn().execute_batch("ALTER TABLE blobs DROP COLUMN gets_left").unwrap();
        }
        {
            let db = Db::open(path).unwrap();
            for _ in 0..5 {
                assert_eq!(db.take_blob_get(&[9; 16], 5, 100).unwrap(), Some((100, 5)));
            }
        }
        {
            let db = Db::open(path).unwrap();
            assert_eq!(db.take_blob_get(&[9; 16], 5, 100).unwrap(), Some((100, 5)));
            assert_eq!(db.take_blob_get(&[9; 16], 5, 100).unwrap(), None);
            assert!(!db.blob_ready(&[9; 16], 5).unwrap(), "cannot reset an existing budget");
            assert_eq!(db.ack_blob(&[9; 16], 5, 100).unwrap(), None);
            assert_eq!(db.ack_blob(&[9; 16], 5, 100).unwrap(), Some(5));
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn blob_gets_are_bounded_across_recovery_and_acks_expire() {
        let db = Db::open(":memory:").unwrap();
        user(register(&db, b"LEO", 1, 1, 0));
        let quota = BlobQuota { per_day: 10, bytes_per_day: 1 << 20, stored: 1 << 20 };
        let id = [9; 16];
        db.put_blob(&[1; 16], &id, 100, 2, 5, &quota).unwrap();
        assert_eq!(db.take_blob_get(&id, 5, 100).unwrap(), None);
        assert_eq!(db.ack_blob(&id, 5, 100).unwrap(), None, "not ready");
        db.blob_ready(&id, 5).unwrap();
        for _ in 0..6 {
            assert_eq!(db.take_blob_get(&id, 5, 100).unwrap(), Some((100, 5)));
        }
        assert_eq!(db.take_blob_get(&id, 5, 100).unwrap(), None);
        db.recover_blobs().unwrap();
        assert_eq!(db.take_blob_get(&id, 5, 100).unwrap(), None, "recovery never resets budget");
        db.return_blob_get(&id, 5).unwrap();
        assert_eq!(db.take_blob_get(&id, 5, 100).unwrap(), Some((100, 5)), "cut-off slot returned");
        assert_eq!(db.ack_blob(&id, 5, 100).unwrap(), None, "one fetch remains despite six GETs");
        assert_eq!(db.ack_blob(&id, 106, 100).unwrap(), None, "expired ack never consumes");
        assert_eq!(db.take_blob_get(&id, 106, 100).unwrap(), None);
        assert_eq!(db.ack_blob(&id, 5, 100).unwrap(), Some(5), "last ack deletes immediately");
        assert!(!db.blob_exists(&id).unwrap());
        assert_eq!(db.ack_blob(&id, 5, 100).unwrap(), None);
    }

    #[test]
    fn blob_get_budget_returns_on_cutoff_and_only_ack_deletes() {
        let db = Db::open(":memory:").unwrap();
        user(register(&db, b"LEO", 1, 1, 0));
        let quota = BlobQuota { per_day: 10, bytes_per_day: 1 << 20, stored: 1 << 20 };
        let (id, ttl) = ([9; 16], 30 * DAY);
        assert_eq!(db.put_blob(&[1; 16], &id, 100, 2, 5, &quota).unwrap(), BlobPut::Stored);
        assert_eq!(db.take_blob_get(&id, 5, ttl).unwrap(), None, "not ready: its file is not in place");
        assert_eq!(db.put_blob(&[1; 16], &id, 100, 2, 5, &quota).unwrap(), BlobPut::Unfinished, "another try: later");
        assert_eq!(db.blob_ready_state(&id).unwrap(), Some(false));
        assert!(db.blob_ready(&id, 5).unwrap());
        assert_eq!(db.put_blob(&[1; 16], &id, 100, 2, 5, &quota).unwrap(), BlobPut::Taken, "done: it is there");
        for _ in 0..6 {
            assert_eq!(db.take_blob_get(&id, 5, ttl).unwrap(), Some((100, 5)));
        }
        assert_eq!(db.take_blob_get(&id, 5, ttl).unwrap(), None);
        db.return_blob_get(&id, 5).unwrap();
        assert_eq!(db.take_blob_get(&id, 5, ttl).unwrap(), Some((100, 5)));
        assert!(db.recover_blobs().unwrap().is_empty());
        assert_eq!(db.ack_blob(&id, 5, ttl).unwrap(), None);
        assert_eq!(db.ack_blob(&id, 5, ttl).unwrap(), Some(5));
        assert!(!db.blob_exists(&id).unwrap());
        // Stored, never ready (a crash before its file was in place): gone at the next start, its id free again.
        assert_eq!(db.put_blob(&[1; 16], &id, 100, 1, 6, &quota).unwrap(), BlobPut::Stored);
        assert_eq!(db.recover_blobs().unwrap(), [(id, 6)]);
        assert_eq!(db.put_blob(&[1; 16], &id, 100, 1, 7, &quota).unwrap(), BlobPut::Stored);
        // Or within the hour, without a restart.
        assert!(db.prune_blobs(7 + NOT_READY_KEPT_MS - 1, ttl).unwrap().is_empty());
        assert_eq!(db.prune_blobs(7 + NOT_READY_KEPT_MS + 1, ttl).unwrap(), [(id, 7)]);
    }

    /// NORMAL (1) avoids fsync per commit; an inbox ack must not restore FULL for subsequent deliveries.
    #[test]
    fn deliveries_and_acks_keep_normal_synchronization() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        let devices_hash = yo_wire::devices::hash(&[1; 32], &[[1; 32]]);
        db.deliver(&[Incoming { user: leo, devices_hash, envelope: b"envelope" }], 0, 10).unwrap();
        let synchronous = || -> i64 { db.conn().query_row("PRAGMA synchronous", [], |row| row.get(0)).unwrap() };
        assert_eq!(synchronous(), 1);
        assert_eq!(db.conn().query_row("PRAGMA wal_autocheckpoint", [], |row| row.get::<_, i64>(0)).unwrap(), 16384);
        let (id, _, _) = db.fetch(&[1; 16], 10).unwrap().remove(0);
        assert_eq!(db.ack(&[1; 16], &[id]).unwrap(), [0], "when it was accepted");
        assert!(db.fetch(&[1; 16], 10).unwrap().is_empty());
        assert_eq!(synchronous(), 1);
    }

    /// Another connection (litestream's) commits while a delivery or an ack waits for the lock: it waits and goes
    /// through, never "database is locked" for a snapshot read before that commit.
    #[test]
    fn deliveries_and_acks_wait_out_another_connections_commit() {
        let path = std::env::temp_dir().join(format!("yo-db-busy-{}.sqlite", std::process::id()));
        let path = path.to_str().unwrap().to_owned();
        let db = Db::open(&path).unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        let devices_hash = yo_wire::devices::hash(&[1; 32], &[[1; 32]]);
        let other = Connection::open(&path).unwrap();
        other.execute_batch("CREATE TABLE other (n INTEGER)").unwrap();
        let commit_later = |other: Connection| {
            other.execute_batch("BEGIN IMMEDIATE; INSERT INTO other VALUES (1);").unwrap();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                other.execute_batch("COMMIT").unwrap();
                other
            })
        };
        let committer = commit_later(other);
        let delivered = db.deliver(&[Incoming { user: leo, devices_hash, envelope: b"envelope" }], 0, 10).unwrap();
        assert!(matches!(&delivered[..], [Delivered::Stored(_)]));
        let ids: Vec<u64> = db.fetch(&[1; 16], 10).unwrap().into_iter().map(|(id, _, _)| id).collect();
        let committer = commit_later(committer.join().unwrap());
        assert_eq!(db.ack(&[1; 16], &ids).unwrap().len(), 1);
        drop(committer.join().unwrap());
        drop(db);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{path}{suffix}"));
        }
    }

    #[test]
    fn a_bots_devices_are_marked_for_the_counts() {
        assert!(is_bot_app(Some("yogpt/202611190000")) && is_bot_app(Some("cli/7")));
        assert!(!is_bot_app(Some("ios/202611191257")) && !is_bot_app(Some("android/3")) && !is_bot_app(None));
        let db = Db::open(":memory:").unwrap();
        let bot = user(register(&db, b"BOT", 1, 1, 0));
        db.touch(&[1; 16], 0, Some("cli/7")).unwrap();
        let devices_hash = yo_wire::devices::hash(&[1; 32], &[[1; 32]]);
        let delivered = db.deliver(&[Incoming { user: bot, devices_hash, envelope: b"envelope" }], 0, 10).unwrap();
        assert!(matches!(&delivered[..], [Delivered::Stored(devices)] if devices.iter().all(|w| w.bot)));
    }

    /// Exit without dropping SQLite (no close-time checkpoint): the committed WAL survives a process crash.
    #[test]
    fn committed_delivery_survives_process_exit() {
        const CHILD_DB: &str = "YO_DB_CRASH_TEST";
        if let Ok(path) = std::env::var(CHILD_DB) {
            let db = Db::open(&path).unwrap();
            let leo = user(register(&db, b"LEO", 1, 1, 0));
            let devices_hash = yo_wire::devices::hash(&[1; 32], &[[1; 32]]);
            db.deliver(&[Incoming { user: leo, devices_hash, envelope: b"committed" }], 0, 10).unwrap();
            std::process::exit(0);
        }
        let path = std::env::temp_dir().join(format!("yo-db-crash-{}.sqlite", std::process::id()));
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "db::tests::committed_delivery_survives_process_exit", "--test-threads=4"])
            .env(CHILD_DB, &path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(std::fs::metadata(format!("{}-wal", path.display())).unwrap().len() > 0);
        let db = Db::open(path.to_str().unwrap()).unwrap();
        let messages = db.fetch(&[1; 16], 10).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].1, b"committed");
        drop(db);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn a_deleted_account_leaves_nothing_behind() {
        for admin in [false, true] {
            let db = Db::open(":memory:").unwrap();
            let leo = user(register(&db, b"LEO", 1, 1, 0));
            let mia = user(register(&db, b"MIA", 2, 2, 0));
            db.put_backup(&[1; 16], Some(&[9; 16]), b"record", &BackupAuth::default(), 0).unwrap();
            let incoming = Incoming {
                user: leo,
                devices_hash: yo_wire::devices::hash(&[1; 32], &[[1; 32]]),
                envelope: b"envelope",
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
            if admin {
                assert_eq!(db.admin_delete_user(&leo, 1).unwrap(), Some(Vec::new()));
                assert_eq!(db.admin_delete_user(&leo, 1).unwrap(), None);
            } else {
                assert!(matches!(db.delete_account(&[1; 16], 1).unwrap(), Write::Done(())));
            }
            assert!(db.is_gone(&[1; 16]).unwrap(), "its device learns that the account is gone");
            assert!(db.lookup(KIND_USER, &leo, false, &[]).unwrap().is_none());
            assert!(db.backup(&leo).unwrap().is_none() && db.backup(&[9; 16]).unwrap().is_none());
            assert!(db.fetch(&[1; 16], 10).unwrap().is_empty());
            let left = |table: &str| -> i64 {
                db.conn().query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0)).unwrap()
            };
            assert_eq!((left("reports"), left("prekeys"), left("account_keys")), (0, 2, 1), "only MIA's are left");
            let feedback: Vec<Vec<u8>> = db
                .conn()
                .prepare("SELECT user_id FROM support_messages ORDER BY id")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(feedback, [NOBODY.to_vec(), mia.to_vec()], "LEO's feedback stays without LEO, MIA's as it was");
            assert_eq!(db.audit().unwrap(), [], "nothing of LEO's is left behind");
            assert!(matches!(
                db.feedback(&[1; 16], "late", "1.0 (1)", 1).unwrap(),
                Write::Refused(Conflict::DeviceGone)
            ));
            assert!(matches!(db.delete_account(&[1; 16], 1).unwrap(), Write::Refused(Conflict::DeviceGone)));
            // The name and the key are free again.
            user(register(&db, b"LEO", 1, 3, 2));
            assert_eq!(reported(&mia, &leo), Some(false), "no such user any more");
            assert_eq!(reported(&leo, &mia), None, "a deleted account reports nobody");
        }
    }

    #[test]
    fn feedback_is_kept_as_written_with_who_and_when() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        assert!(matches!(db.feedback(&[1; 16], "Mehr Farben, bitte 🎨", "1.2 (34)", 5).unwrap(), Write::Done(())));
        assert!(matches!(db.feedback(&[1; 16], "again", "1.2 (35)", 6).unwrap(), Write::Done(())));
        assert_eq!(
            db.conn()
                .query_row("SELECT count(*) FROM support_messages WHERE from_team = 0", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let rows: Vec<(i64, Vec<u8>, String, String, i64)> = db
            .conn()
            .prepare("SELECT id, user_id, text, app_version, created_at FROM support_messages ORDER BY id")
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
    fn schema_7_with_feedback_reaches_8_with_its_rows_moved_and_the_table_gone() {
        let path = std::env::temp_dir().join(format!("yo-hub-feedback-{}.db", std::process::id()));
        let path = path.to_str().unwrap();
        let leo = {
            let db = Db::open(path).unwrap();
            let leo = user(register(&db, b"LEO", 1, 1, 0));
            let conn = db.conn();
            conn.execute_batch(LEGACY_FEEDBACK).unwrap();
            conn.execute(
                "INSERT INTO feedback(user_id,text,app_version,created_at,auto,type) VALUES
                (?1,'Mehr Farben 🎨','ios/7',100,0,'feedback'),(X'','trace','ios/7',101,1,'crash'),
                (X'','a task','admin',102,0,'todo')",
                [leo],
            )
            .unwrap();
            conn.execute_batch("PRAGMA user_version=7").unwrap();
            leo
        };
        for _ in 0..2 {
            let db = Db::open(path).unwrap();
            let conn = db.conn();
            assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 8);
            assert!(!conn
                .prepare("SELECT 1 FROM sqlite_master WHERE name LIKE 'feedback%'")
                .unwrap()
                .exists([])
                .unwrap());
            let message: (Vec<u8>, String, String, i64) = conn
                .query_row("SELECT user_id,text,app_version,created_at FROM support_messages", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .unwrap();
            assert_eq!(message, (leo.to_vec(), "Mehr Farben 🎨".into(), "ios/7".into(), 100));
            assert_eq!(conn.query_row("SELECT text FROM todos", [], |r| r.get::<_, String>(0)).unwrap(), "a task");
            assert_eq!(conn.query_row("SELECT first_seen FROM diagnostics", [], |r| r.get::<_, i64>(0)).unwrap(), 101);
        }
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{path}{suffix}"));
        }
    }

    /// Profile keys of before signed requests (spec 4.10): the check before a change and the count per recipient go;
    /// the current check stays and counts alone.
    #[test]
    fn a_database_with_the_old_contact_keys_loses_the_grace() {
        let path = std::env::temp_dir().join(format!("yo-hub-grace-{}.db", std::process::id()));
        let path = path.to_str().unwrap();
        let leo = {
            let db = Db::open(path).unwrap();
            let leo = user(register(&db, b"LEO", 1, 1, 0));
            db.conn()
                .execute_batch(
                    "DROP TABLE contact_keys;
                     CREATE TABLE requests (sender BLOB, recipient BLOB, day INTEGER, n INTEGER);
                     CREATE TABLE contact_keys (user_id BLOB PRIMARY KEY REFERENCES users(id), current BLOB NOT NULL,
                       previous BLOB, previous_until INTEGER NOT NULL DEFAULT 0,
                       requests_open INTEGER NOT NULL DEFAULT 1) WITHOUT ROWID;
                     CREATE TABLE requests_day (user_id BLOB NOT NULL, day INTEGER NOT NULL, n INTEGER NOT NULL,
                       PRIMARY KEY (user_id, day)) WITHOUT ROWID;",
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO contact_keys VALUES (?1, ?2, ?3, ?4, 1)",
                    params![leo, [2u8; 32].as_slice(), [1u8; 32].as_slice(), i64::MAX],
                )
                .unwrap();
            leo
        };
        let db = Db::open(path).unwrap();
        assert_eq!(db.admit(&leo, Some([2; 32]), None).unwrap(), Admission::Contact);
        assert_eq!(db.admit(&leo, Some([1; 32]), None).unwrap(), Admission::Closed, "no grace");
        let gone: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('requests_day', 'requests')", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(gone, 0);
        drop(db);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{path}{suffix}"));
        }
    }

    /// A delivery without the key is a request when signed, whatever the recipient's switch (its limits are the
    /// hub's buckets, `limiter::Requests`); unsigned, closed.
    #[test]
    fn a_signed_delivery_without_the_key_is_a_request() {
        let db = Db::open(":memory:").unwrap();
        let users: Vec<UserId> = (1..=2u8).map(|i| user(register(&db, &[b'A' + i; 3], i, i, 0))).collect();
        let (spam, leo) = (&users[0], &users[1]);
        db.set_access_key(leo, &[9; 32], true, &[1]).unwrap();
        assert_eq!(db.admit(leo, None, Some(spam)).unwrap(), Admission::Request);
        assert_eq!(db.admit(leo, None, None).unwrap(), Admission::Closed, "unsigned");
        db.set_access_key(leo, &[9; 32], false, &[1]).unwrap();
        assert_eq!(db.admit(leo, None, Some(spam)).unwrap(), Admission::Request, "the switch is the device's");
    }

    /// An upload whose row went while it came (its account deleted meanwhile) is not marked done: its file is removed
    /// and it is no 201.
    #[test]
    fn an_upload_whose_row_went_is_not_done() {
        let db = Db::open(":memory:").unwrap();
        register(&db, b"GONE", 1, 1, 0);
        let quota = BlobQuota { per_day: 10, bytes_per_day: 1 << 20, stored: 1 << 20 };
        assert_eq!(db.put_blob(&[1; 16], &[7; 16], 100, 2, 5, &quota).unwrap(), BlobPut::Stored);
        db.delete_blob(&[7; 16]).unwrap();
        assert!(!db.blob_ready(&[7; 16], 5).unwrap());
    }
}
