//! The hub's store (spec 1): SQLite, one connection, one transaction per method; no content key, sender or contacts.

mod schema;

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
// The in-file tests use these through `super::*`.
#[cfg(test)]
use schema::{
    as_schema_10, migrate_diagnostics, with_feedback_type, with_support, with_support_done, with_todos,
    LEGACY_FEEDBACK, SCHEMA_10,
};
use std::sync::{Mutex, MutexGuard, PoisonError};
use yoip::net::api;
use yoip::net::handle::{KIND_ACCOUNT_KEY, KIND_USER, KIND_USERNAME};
use yoip::protocol::content::BlobId;
use yoip::{DeviceId, UserId, KEY_LEN};

const DAY_MS: u64 = 24 * 3600 * 1000;
const HOUR_MS: u64 = 3600 * 1000;

/// A time as the hub keeps when a device or an install was last heard from (spec 1): the start of its hour (UTC),
/// fine enough for the hourly levels' 24- and 48-hour windows (`phones-24h`, `build:`), no finer.
pub fn to_the_hour(ms: u64) -> u64 {
    ms - ms % HOUR_MS
}
/// How long a crash or error group is kept after it was last seen, and an install's occurrence of one (spec 4).
const DIAGNOSTICS_KEPT_DAYS: u64 = 90;
/// A blob not ready this long after it was stored belongs to no upload under way: its file never came into place.
const NOT_READY_KEPT_MS: u64 = 3600 * 1000;

pub type Key = [u8; KEY_LEN];

/// An install id (spec 4): 16 random bytes a phone makes for help, never tied to an account.
pub type Install = [u8; 16];

/// A user's storage (spec 4.3): its version and its sealed bytes.
pub type Sealed = (u64, Vec<u8>);

pub struct Db {
    pub(crate) agent: Option<crate::corrosion::Agent>,
    pub(crate) add_devices: bool,
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
    /// The version that app named with it ("1.0.0-RC36"), if any.
    pub version: Option<String>,
    /// When it last made a request inside OHTTP, to the day (`touch`); none before it did.
    pub ohttp_at: Option<u64>,
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
    /// That app's version, if it names one.
    pub version: Option<String>,
}

/// The keys that may sign a backup record's fetch (spec 2.4): the passkey's and the code's; neither for older records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupAuth {
    pub passkey: Option<Key>,
    pub code: Option<Key>,
}

/// Prekey kinds (spec 3.1): signed and last resort, replaced by an upload; one-time ones, handed out once.
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
    /// It declares itself a bot.
    pub bot: bool,
}

/// A username nobody may claim (spec 2.2), as the operator's list writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reserved {
    pub spelling: String,
    pub note: Option<String>,
}

/// A user's current account key and devices: what a delivery's device check compares with (spec 3.2).
pub type Roster = api::Devices;

/// A device to wake after a delivery.
pub struct Wake {
    pub device_id: DeviceId,
    pub push_platform: u8,
    pub push_token: Vec<u8>,
    /// A bot's device (`is_bot_app`): its Yos are counted apart (`yo-in-bot`, 1).
    pub bot: bool,
    /// Envelopes waiting for the device (unfetched) after this delivery: past `push_backlog` its pushes slow down.
    pub waiting: u32,
}

/// A bot's app as its device names it (`Yo-App`): YoGPT, or the CLI the bots (STATUS, PING, …) run on. The operator's
/// counts keep bots apart by it; a person on the CLI counts as a bot too.
pub fn is_bot_app(app: Option<&str>) -> bool {
    app.is_some_and(|app| app.starts_with("yogpt/") || app.starts_with("cli/"))
}

/// What a delivery brings for the writer (spec 3.2).
pub enum Copies<'a> {
    /// One envelope for each device, by its number (`api::Copies`).
    Each(Vec<(u8, &'a [u8])>),
    /// One envelope for every device: an app before multi-device, while each account has one device (phase 1).
    Legacy(&'a [u8]),
}

/// An envelope for the writer (`Db::deliver`): for a user, with the sender's device check.
pub struct Incoming<'a> {
    pub user: UserId,
    pub devices_hash: [u8; 32],
    /// The sending device, when it writes to its own user: neither checked for nor delivered to (Signal's sync).
    pub from: Option<DeviceId>,
    pub copies: Copies<'a>,
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
    /// A hub that adds devices: the account has 16 already (`409 device_limit`); one goes only by the owner's choice.
    DeviceLimit,
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

/// What a delivery is to its recipient (spec 3.3, `Db::admit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// It shows the recipient's profile key, or the recipient has none.
    Contact,
    /// A request of this sender, taken.
    Request,
    /// No contact's (no key, or not the current one) and unsigned.
    Closed,
    /// No such user: `404` before anything is counted for it.
    Unknown,
}

/// What one user may store in blobs (spec 4.1).
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
    let Some(account_key) = row(tx, "SELECT account_key FROM account WHERE id = ?1", [user], |row| array(row.get(0)?))?
    else {
        return Ok(None);
    };
    let devices = tx
        .prepare_cached("SELECT device_key, number FROM account_device WHERE user_id = ?1 ORDER BY device_key")?
        .query_map([user], |row| Ok(api::Device { key: array(row.get(0)?)?, id: row.get(1)? }))?
        .collect::<Result<_, _>>()?;
    Ok(Some(Roster { account_key, devices }))
}

/// Stores an upload of prekeys, up to `most` one-time ones of each kind, none with an id used before: never twice.
fn insert_prekeys(tx: &Transaction<'_>, device: &DeviceId, prekeys: &api::Prekeys, most: u32) -> rusqlite::Result<()> {
    let mut insert = tx.prepare_cached(
        "INSERT OR IGNORE INTO account_prekey (device_id, kind, id, key, signature) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (kind, signed) in [(SIGNED, &prekeys.signed), (LAST_RESORT, &prekeys.last_resort)] {
        if let Some(signed) = signed {
            tx.execute("DELETE FROM account_prekey WHERE device_id = ?1 AND kind = ?2", params![device, kind])?;
            insert.execute(params![device, kind, signed.id, signed.key.as_slice(), signed.signature.as_slice()])?;
        }
    }
    let count = |kind: i64| -> rusqlite::Result<u32> {
        tx.prepare_cached("SELECT count(*) FROM account_prekey WHERE device_id = ?1 AND kind = ?2")?
            .query_row(params![device, kind], |row| row.get(0))
    };
    let max: u32 =
        row(tx, "SELECT prekey_max FROM account_device WHERE id = ?1", [device], |row| row.get(0))?.unwrap_or(0);
    let room = most.saturating_sub(count(ONE_TIME)?) as usize;
    for one in prekeys.one_time.iter().filter(|p| p.id > max).take(room) {
        insert.execute(params![device, ONE_TIME, one.id, one.key.as_slice(), &[] as &[u8]])?;
    }
    let room = most.saturating_sub(count(KYBER)?) as usize;
    for kyber in prekeys.kyber.iter().filter(|p| p.id > max).take(room) {
        insert.execute(params![device, KYBER, kyber.id, kyber.key.as_slice(), kyber.signature.as_slice()])?;
    }
    let newest = prekeys.one_time.iter().map(|p| p.id).chain(prekeys.kyber.iter().map(|p| p.id)).max().unwrap_or(0);
    tx.execute("UPDATE account_device SET prekey_max = max(prekey_max, ?2) WHERE id = ?1", params![device, newest])?;
    Ok(())
}

/// A prekey of a kind for a device, the oldest first; with `take`, gone from here.
fn prekey(tx: &Transaction<'_>, device: &[u8], kind: i64, take: bool) -> rusqlite::Result<Option<api::SignedPrekey>> {
    let found: Option<(u32, Vec<u8>, Vec<u8>)> = row(
        tx,
        "SELECT id, key, signature FROM account_prekey WHERE device_id = ?1 AND kind = ?2 ORDER BY id LIMIT 1",
        params![device, kind],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if let (Some((id, _, _)), true) = (&found, take) {
        tx.execute(
            "DELETE FROM account_prekey WHERE device_id = ?1 AND kind = ?2 AND id = ?3",
            params![device, kind, id],
        )?;
    }
    Ok(found.map(|(id, key, signature)| api::SignedPrekey { id, key: key.into(), signature: signature.into() }))
}

/// A push token is one app on one phone: another device still holding it (an account signed out of the phone or left
/// for another one there) is woken with it no more. Else its Yos woke the phone's next account, or none ("You may have
/// received a Yo" for an account the phone no longer has: NotificationFallback noAccount).
fn release_push_token(tx: &Transaction<'_>, device: &DeviceId, platform: u8, token: &[u8]) -> rusqlite::Result<()> {
    if !token.is_empty() {
        tx.prepare_cached(
            "UPDATE account_device SET push_platform = 0, push_token = x''
             WHERE push_platform = ?2 AND push_token = ?3 AND id != ?1",
        )?
        .execute(params![device, platform, token])?;
    }
    Ok(())
}

/// Removes a user's devices with their prekeys and envelopes. Their own requests answer 410 from then on.
fn retire_devices(tx: &Transaction<'_>, user: &UserId, now: u64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO account_device_gone (id, gone_at) SELECT id, ?2 FROM account_device WHERE user_id = ?1",
        params![user, now],
    )?;
    for table in ["account_prekey", "message"] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE device_id IN (SELECT id FROM account_device WHERE user_id = ?1)"),
            [user],
        )?;
    }
    tx.execute("DELETE FROM account_device WHERE user_id = ?1", [user])?;
    Ok(())
}

/// A user's devices by number: registered, last seen, the app it named.
fn devices_of(conn: &Connection, user: &UserId) -> rusqlite::Result<Vec<api::OwnDevice>> {
    conn.prepare_cached(
        "SELECT number, created_at, last_seen_at, coalesce(app, '') FROM account_device WHERE user_id = ?1 ORDER BY number",
    )?
    .query_map([user], |row| {
        Ok(api::OwnDevice {
            number: row.get(0)?,
            created_at: row.get::<_, i64>(1)? as u64,
            last_seen_at: row.get::<_, i64>(2)? as u64,
            app: row.get::<_, String>(3)?.into_bytes().into(),
        })
    })?
    .collect()
}

/// Removes one device of a user, by its number, with its prekeys and envelopes; its own requests answer 410 from
/// then on. `false`: none of that number.
fn retire_device(tx: &Transaction<'_>, user: &UserId, number: u8, now: u64) -> rusqlite::Result<bool> {
    let Some(id): Option<Vec<u8>> =
        row(tx, "SELECT id FROM account_device WHERE user_id = ?1 AND number = ?2", params![user, number], |row| {
            row.get(0)
        })?
    else {
        return Ok(false);
    };
    tx.execute("INSERT OR REPLACE INTO account_device_gone (id, gone_at) VALUES (?1, ?2)", params![id, now])?;
    for table in ["account_prekey", "message"] {
        tx.execute(&format!("DELETE FROM {table} WHERE device_id = ?1"), [&id])?;
    }
    tx.execute("DELETE FROM account_device WHERE id = ?1", [&id])?;
    Ok(true)
}

/// The user of a device, in a transaction: none for a device replaced since it authenticated.
fn user_of_device(tx: &Transaction<'_>, device: &DeviceId) -> rusqlite::Result<Option<UserId>> {
    row(tx, "SELECT user_id FROM account_device WHERE id = ?1", [device], |row| array(row.get(0)?))
}

/// Removes a user and everything about it; its devices answer 410. Reports about it go; help by install ids (spec
/// 4) names no account and stays.
fn remove_user(tx: &Transaction<'_>, user: &UserId, now: u64) -> rusqlite::Result<()> {
    retire_devices(tx, user, now)?;
    // Its blobs' files go with the next sweep, or at once where the route knows them (`blobs_of`).
    for table in [
        "account_backup",
        "account_name",
        "account_key",
        "account_storage",
        "account_contact_key",
        "account_profile",
        "message_blob",
        "message_blob_quota",
        "stat_request",
    ] {
        tx.execute(&format!("DELETE FROM {table} WHERE user_id = ?1"), [user])?;
    }
    tx.execute("DELETE FROM support_report WHERE reported_id = ?1", [user])?;
    tx.execute("DELETE FROM account WHERE id = ?1", [user])?;
    Ok(())
}

/// A user's username as it writes it (a name from before spellings: its skeleton), in a transaction.
fn username_of(tx: &Transaction<'_>, user: &UserId) -> rusqlite::Result<Option<String>> {
    row(
        tx,
        "SELECT coalesce(spelling, CAST(value AS TEXT)) FROM account_name WHERE user_id = ?1 AND kind = ?2",
        params![user, KIND_USERNAME],
        |row| row.get(0),
    )
}

/// Whether a skeleton is reserved, in a transaction.
fn is_reserved(tx: &Transaction<'_>, skeleton: &[u8]) -> rusqlite::Result<bool> {
    tx.prepare_cached("SELECT 1 FROM name_reserved WHERE skeleton = CAST(?1 AS TEXT)")?.exists([skeleton])
}

/// The network's own errors (domain, code), which are not the app's: a lost, timed-out or refused connection, a
/// busy or failing server, a slow request. They never count against a phone in the error-free share (levels
/// `problem-installs-24h`); the operator page keeps the same list (`NETWORK_ERRORS` in server/admin/users.py).
const NETWORK_ERRORS: [(&str, &str); 11] = [
    ("NSURLErrorDomain", "-1001"),
    ("NSURLErrorDomain", "-1004"),
    ("NSURLErrorDomain", "-1005"),
    ("NSURLErrorDomain", "-1009"),
    ("NSURLErrorDomain", "-1020"),
    ("HTTP", "429"),
    ("HTTP", "502"),
    ("HTTP", "503"),
    ("HTTP", "504"),
    ("NotificationFallback", "network"),
    ("SharedAccess", "a slow request"),
];

/// SQL (on `diagnostics d`) true for a report that is not one of `NETWORK_ERRORS`.
fn network_errors_sql() -> String {
    let each: Vec<String> = NETWORK_ERRORS
        .iter()
        .map(|(domain, code)| {
            format!(
                "(coalesce(json_extract(d.payload, '$.domain'), '') = '{domain}' AND coalesce(json_extract(d.payload, '$.code'), '') = '{code}')"
            )
        })
        .collect();
    format!("NOT ({})", each.join(" OR "))
}

/// A problem's identity (spec 4), without the build: kind and domain, code and place (errors), or exception, top
/// frames and termination (crashes). The builds it came in are counted apart (`diagnostic_builds`).
fn diagnostic_signature(kind: &str, payload: &serde_json::Value) -> String {
    let field = |name| payload[name].as_str().unwrap_or("");
    let identity = if kind == "error" {
        serde_json::json!([kind, field("domain"), field("code"), field("place")])
    } else {
        serde_json::json!([kind, field("exception"), field("frames"), field("termination")])
    };
    yoip::b64(&yoip::sha256(identity.to_string().as_bytes()))
}

/// The newest build per platform among `apps` ("ios/202611241344,android/202611241341"), as `YO_MIN_APP` is written.
fn newest_builds<'a>(apps: impl IntoIterator<Item = &'a str>) -> String {
    let mut newest: Vec<(String, u64)> = Vec::new();
    for (platform, build) in apps.into_iter().filter_map(crate::config::app) {
        match newest.iter_mut().find(|(p, _)| *p == platform) {
            Some(known) => known.1 = known.1.max(build),
            None => newest.push((platform, build)),
        }
    }
    newest.sort();
    newest.iter().map(|(p, b)| format!("{p}/{b}")).collect::<Vec<_>>().join(",")
}

/// Whether `app` is newer than every build a problem was marked done in (`done_builds`): a build of its platform
/// above that platform's, or a platform it was not seen on then.
fn newer_than(app: &str, done_builds: &str) -> bool {
    let Some((platform, build)) = crate::config::app(app) else { return false };
    crate::config::parse_min_app(done_builds)
        .ok()
        .and_then(|done| done.into_iter().find(|(p, _)| *p == platform))
        .is_none_or(|(_, done)| build > done)
}

fn diagnostic_write(conn: &Connection, kind: &str, payload: &serde_json::Value, now: u64) -> rusqlite::Result<i64> {
    let field = |name| payload[name].as_str().unwrap_or("");
    let signature = diagnostic_signature(kind, payload);
    // One report may stand for several of the same, counted on the phone (checked on receipt).
    let count = field("count").parse::<i64>().unwrap_or(1).max(1);
    // "Expected" (the network, an answer the app takes in, iOS taking time back): counts to learn from, never an open
    // problem, so done from the first report on.
    let expected = field("severity") == "expected";
    conn.execute(
        "INSERT INTO diag(kind, signature, count, first_seen, last_seen, app, os, model, payload, done)
        VALUES (?1,?2,?8,?3,?3,?4,?5,?6,?7,?9)
        ON CONFLICT(signature) DO UPDATE SET count=count+excluded.count, last_seen=excluded.last_seen,
        app=excluded.app, os=excluded.os, model=excluded.model, payload=excluded.payload",
        params![kind, signature, now, field("app"), field("os"), field("model"), payload.to_string(), count, expected],
    )?;
    conn.execute(
        "INSERT INTO stat(metric, hour, value) VALUES ('diagnostics:' || ?2, ?1, ?3)
         ON CONFLICT(metric, key, hour) DO UPDATE SET value = value + excluded.value",
        params![now / HOUR_MS, kind, count],
    )?;
    let (id, done_builds): (i64, Option<String>) = conn.query_row(
        "SELECT id, CASE WHEN done = 1 THEN coalesce(done_builds, '') END FROM diag WHERE signature=?1",
        [&signature],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    // The builds it came in, and reports per hour: "still reported" on the operator's page.
    conn.execute(
        "INSERT INTO diag_build(diagnostic_id, app, count, last_seen) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(diagnostic_id, app) DO UPDATE SET count = count + excluded.count, last_seen = excluded.last_seen",
        params![id, field("app"), count, now],
    )?;
    conn.execute(
        "INSERT INTO stat(metric, key, hour, value) VALUES ('diag', CAST(?1 AS TEXT), ?2, ?3)
         ON CONFLICT(metric, key, hour) DO UPDATE SET value = value + excluded.value",
        params![id, (now / 3_600_000) as i64, count],
    )?;
    // Done, and now reported by a newer build than any it was marked done in: open again. Expected ones stay counts.
    if let Some(done_builds) = done_builds {
        if !expected && newer_than(field("app"), &done_builds) {
            conn.execute("UPDATE diag SET done = 0, done_at = NULL, done_builds = NULL WHERE id = ?1", [id])?;
        }
    }
    Ok(id)
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
        "SELECT count, bytes FROM message_blob_quota WHERE user_id = ?1 AND day = ?2",
        params![user, (now / DAY_MS) as i64],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?
    .unwrap_or((0, 0));
    if count as u64 >= quota.per_day as u64 || bytes as u64 + size > quota.bytes_per_day {
        return Ok(false);
    }
    let stored: i64 = tx
        .prepare_cached("SELECT coalesce(sum(size), 0) FROM message_blob WHERE user_id = ?1")?
        .query_row([user], |row| row.get(0))?;
    Ok(stored as u64 + size <= quota.stored)
}

/// A blob's id and when it was stored, from a row of the two.
fn blob_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(BlobId, u64)> {
    Ok((array(row.get(0)?)?, row.get::<_, i64>(1)? as u64))
}

impl Db {
    /// WAL, `synchronous = NORMAL`: a power loss may roll back recent commits (SECURITY.md, Hub).
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
        conn.execute_batch("CREATE TEMP VIEW inbox_message AS SELECT * FROM main.message")?;
        // Expected groups are counts, never open problems (`diagnostic_write`): those kept open before are done too.
        conn.execute(
            "UPDATE diag SET done = 1 WHERE done = 0 AND json_extract(payload, '$.severity') = 'expected'",
            [],
        )?;
        Ok(Db { agent: None, conn: Mutex::new(conn), add_devices: false })
    }

    pub(crate) fn configured(config: &crate::config::Config) -> rusqlite::Result<Self> {
        let mut db = Self::open(&config.db_path)?;
        if let Some(config) = &config.corrosion {
            let agent = crate::corrosion::Agent::open(&mut db.conn(), config)?;
            db.agent = Some(agent);
        } else {
            crate::corrosion::detach_mirror(&mut db.conn())?;
        }
        Ok(db)
    }

    /// Restores the replicated inboxes before restarting in standalone mode.
    pub fn corrosion_back(config: &crate::config::Config) -> rusqlite::Result<()> {
        let cfg = config
            .corrosion
            .as_ref()
            .filter(|c| c.upstream.is_none())
            .ok_or(rusqlite::Error::InvalidParameterName("central Corrosion configuration required".into()))?;
        let db = Self::open(&config.db_path)?;
        let conn = db.conn();
        crate::corrosion::attach(&conn, &cfg.db)?;
        conn.execute(
            "INSERT OR IGNORE INTO main.message(id,device_id,envelope,received_at)
            SELECT id,device_id,envelope,received_at FROM corro.message",
            [],
        )?;
        Ok(())
    }

    fn edge(&self) -> bool {
        self.agent.as_ref().is_some_and(|a| a.config.upstream.is_some())
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

    // ---- reserved names (spec 2.2) ----

    /// Stored names and their notes for the admin API.
    pub fn reserved_state(&self) -> rusqlite::Result<serde_json::Value> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT spelling, note FROM name_reserved ORDER BY skeleton")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let notes: serde_json::Map<_, _> =
            rows.iter().filter_map(|(name, note)| Some((name.clone(), note.clone()?.into()))).collect();
        let names: Vec<_> = rows.into_iter().map(|(name, _)| name).collect();
        Ok(serde_json::json!({"names": names, "notes": notes}))
    }

    pub fn remove_reserved(&self, name: &str) -> rusqlite::Result<()> {
        self.run("DELETE FROM name_reserved WHERE skeleton=?1", [yoip::net::handle::skeleton(name)])
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
                "INSERT OR IGNORE INTO name_reserved (skeleton, spelling, note, created_at) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut note = tx.prepare_cached("UPDATE name_reserved SET note=?2 WHERE skeleton=?1")?;
            let mut added = 0;
            for name in names {
                let skeleton = yoip::net::handle::skeleton(&name.spelling);
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
        let sql = "SELECT user_id, auth_key, last_seen_at, app, version, ohttp_at FROM account_device WHERE id = ?1";
        row(&self.conn(), sql, [device], |row| {
            Ok(Device {
                user_id: array(row.get(0)?)?,
                auth_key: array(row.get(1)?)?,
                last_seen_at: row.get::<_, i64>(2)? as u64,
                app: row.get(3)?,
                version: row.get(4)?,
                ohttp_at: row.get::<_, Option<i64>>(5)?.map(|at| at as u64),
            })
        })
    }

    /// The signing device's account's devices, by number (`GET /v1/devices`).
    pub fn own_devices(&self, device: &DeviceId) -> rusqlite::Result<Write<Vec<api::OwnDevice>>> {
        self.as_device(device, |tx, user| Ok(Write::Done(devices_of(tx, &user)?)))
    }

    /// A user's devices as `own_devices` lists them (none for no user): for `409 device_limit`.
    pub fn devices_of_user(&self, user: &UserId) -> rusqlite::Result<Vec<api::OwnDevice>> {
        devices_of(&self.conn(), user)
    }

    /// Removes another device of the signing device's account (`DELETE /v1/devices/{n}`): `None` none of that
    /// number, `Some(false)` the signing device itself.
    pub fn remove_own_device(&self, device: &DeviceId, number: u8, now: u64) -> rusqlite::Result<Write<Option<bool>>> {
        self.as_device(device, |tx, user| {
            let own: u8 = tx.query_row("SELECT number FROM account_device WHERE id = ?1", [device], |r| r.get(0))?;
            if own == number {
                return Ok(Write::Done(Some(false)));
            }
            Ok(Write::Done(retire_device(tx, &user, number, now)?.then_some(true)))
        })
    }

    pub fn is_gone(&self, device: &DeviceId) -> rusqlite::Result<bool> {
        self.conn().prepare_cached("SELECT 1 FROM account_device_gone WHERE id = ?1")?.exists([device])
    }

    /// Seen at `now` (kept to the hour), with the app it names and that app's version (no app: the one it had stays,
    /// with its version); `inside` OHTTP, seen there at `now` too.
    pub fn touch(
        &self,
        device: &DeviceId,
        now: u64,
        app: Option<(&str, Option<&str>)>,
        inside: bool,
    ) -> rusqlite::Result<()> {
        if self.edge() {
            return Ok(());
        }
        let (app, version) = (app.map(|(a, _)| a), app.and_then(|(_, v)| v));
        self.run(
            "UPDATE account_device SET last_seen_at = ?2, app = coalesce(?3, app),
                 version = CASE WHEN ?3 IS NULL THEN version ELSE ?4 END,
                 ohttp_at = CASE WHEN ?5 THEN ?2 ELSE ohttp_at END WHERE id = ?1",
            params![device, to_the_hour(now), app, version, inside],
        )
    }

    /// The user an account key belongs to now: a registration with it restores that user (spec 4.2).
    pub fn user_of_account_key(&self, account_key: &Key) -> rusqlite::Result<Option<UserId>> {
        row(&self.conn(), "SELECT id FROM account WHERE account_key = ?1", [account_key], |row| array(row.get(0)?))
    }

    /// Who has a username (by its skeleton), if anyone.
    pub fn username_owner(&self, skeleton: &[u8]) -> rusqlite::Result<Option<UserId>> {
        row(
            &self.conn(),
            "SELECT user_id FROM account_name WHERE kind = ?1 AND value = ?2",
            params![KIND_USERNAME, skeleton],
            |row| array(row.get(0)?),
        )
    }

    pub fn account_key(&self, user: &UserId) -> rusqlite::Result<Option<Key>> {
        row(&self.conn(), "SELECT account_key FROM account WHERE id = ?1", [user], |row| array(row.get(0)?))
    }

    /// Registers a device (spec 4.2): the current account key's user again, else a new user by a free name.
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
        replace: Option<u8>,
        now: u64,
    ) -> rusqlite::Result<Write<(UserId, Option<String>, u8)>> {
        self.write(|tx| {
            let taken = tx
                .prepare_cached(
                    "SELECT 1 FROM account_device WHERE id = ?1 OR device_key = ?2 UNION ALL SELECT 1 FROM account_device_gone WHERE id = ?1",
                )?
                .exists(params![device.device_id, device.device_key])?;
            if taken {
                return Ok(Write::Refused(Conflict::DeviceTaken));
            }
            let restored: Option<UserId> =
                row(tx, "SELECT id FROM account WHERE account_key = ?1", [account_key], |row| array(row.get(0)?))?;
            let user = match restored {
                Some(user) => user,
                None => {
                    if tx.prepare_cached("SELECT 1 FROM account_key WHERE key = ?1")?.exists([account_key])? {
                        return Ok(Write::Refused(Conflict::KeyTaken));
                    }
                    let holder = tx
                        .prepare_cached("SELECT 1 FROM account_name WHERE kind = ?1 AND value = ?2")?
                        .exists(params![KIND_USERNAME, skeleton])?;
                    if holder || is_reserved(tx, skeleton)? {
                        return Ok(Write::Refused(Conflict::UsernameTaken));
                    }
                    tx.execute(
                        "INSERT INTO account (id, account_key, created_at) VALUES (?1, ?2, ?3)",
                        params![fresh, account_key, now],
                    )?;
                    tx.execute("INSERT INTO account_key (key, user_id) VALUES (?1, ?2)", params![account_key, fresh])?;
                    tx.execute(
                        "INSERT INTO account_name (kind, value, user_id, spelling) VALUES (?1, ?2, ?3, ?4)",
                        params![KIND_USERNAME, skeleton, fresh, spelling],
                    )?;
                    *fresh
                }
            };
            if !self.add_devices {
                retire_devices(tx, &user, now)?;
            } else {
                // The one the owner chose to go for this one (`RegisterReplacing`), if it is there.
                if let Some(number) = replace {
                    retire_device(tx, &user, number, now)?;
                }
                let count: usize =
                    tx.query_row("SELECT count(*) FROM account_device WHERE user_id = ?1", [&user], |r| r.get(0))?;
                // Never one removed by itself: the new device asks which one may go (multi-device phase 2).
                if count >= yoip::net::limits::MAX_DEVICES {
                    return Ok(Write::Refused(Conflict::DeviceLimit));
                }
            }
            let taken: Vec<u8> = tx
                .prepare_cached("SELECT number FROM account_device WHERE user_id = ?1")?
                .query_map([&user], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            let number = (1..=127u8).find(|n| !taken.contains(n)).ok_or(rusqlite::Error::InvalidQuery)?;
            tx.execute(
                "INSERT INTO account_device
                     (id, user_id, device_key, auth_key, push_platform, push_token, created_at, last_seen_at, app,
                      registration_id, number, version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?11, ?8, ?9, ?10, ?12)",
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
                    number,
                    to_the_hour(now),
                    device.version
                ],
            )?;
            release_push_token(tx, &device.device_id, device.push_platform, &device.push_token)?;
            insert_prekeys(tx, &device.device_id, prekeys, most)?;
            let username = username_of(tx, &user)?;
            Ok(Write::Done((user, username, number)))
        })
    }

    /// Marks the account a bot (spec 2.1), one-way: false only if it is none; false for a bot gives `false` (`409`).
    /// Written as `device`, which must still be there (a replaced one is `410`).
    pub fn set_bot(&self, device: &DeviceId, bot: bool) -> rusqlite::Result<Write<bool>> {
        self.as_device(device, |tx, user| {
            if bot {
                tx.execute("UPDATE account SET bot = 1 WHERE id = ?1", params![user])?;
                return Ok(Write::Done(true));
            }
            let is: bool = tx.query_row("SELECT bot FROM account WHERE id = ?1", params![user], |r| r.get(0))?;
            Ok(Write::Done(!is))
        })
    }

    pub fn set_access_key(
        &self,
        device: &DeviceId,
        check: &[u8; 32],
        requests_open: bool,
        commitment: &[u8],
    ) -> rusqlite::Result<Write<()>> {
        self.as_device(device, |tx, user| {
            tx.prepare_cached(
                "INSERT INTO account_contact_key (user_id, current, requests_open) VALUES (?1, ?2, ?3)
                 ON CONFLICT(user_id) DO UPDATE SET current = ?2, requests_open = ?3",
            )?
            .execute(params![user, check.as_slice(), requests_open])?;
            tx.prepare_cached(
                "INSERT INTO account_profile (user_id, commitment) VALUES (?1, ?2)
                 ON CONFLICT(user_id) DO UPDATE SET commitment = ?2",
            )?
            .execute(params![user, commitment])?;
            Ok(Write::Done(()))
        })
    }

    /// For tests: a commitment without an account.
    #[cfg(test)]
    pub fn set_commitment_for_tests(&self, user: &UserId, commitment: &[u8]) {
        self.conn()
            .execute(
                "INSERT OR REPLACE INTO account_profile (user_id, commitment) VALUES (?1, ?2)",
                params![user, commitment],
            )
            .unwrap();
    }

    /// A user's profile key commitment (spec 3.4), if it told one.
    pub fn commitment(&self, user: &UserId) -> rusqlite::Result<Option<Vec<u8>>> {
        row(&self.conn(), "SELECT commitment FROM account_profile WHERE user_id = ?1", [user], |row| row.get(0))
    }

    /// What a delivery to `user` is (spec 3.3): a contact's (its key shown, or none set), a signed request, or closed.
    pub fn admit(
        &self,
        user: &UserId,
        shown: Option<[u8; 32]>,
        sender: Option<&UserId>,
    ) -> rusqlite::Result<Admission> {
        let current: Option<Vec<u8>> =
            row(&self.conn(), "SELECT current FROM account_contact_key WHERE user_id = ?1", [user], |row| row.get(0))?;
        let Some(current) = current else {
            // A made-up id must not leave a recipient's bucket behind (`inbound:<user>`, kept a day).
            let known = self.conn().prepare_cached("SELECT 1 FROM account WHERE id = ?1")?.exists([user])?;
            return Ok(if known { Admission::Contact } else { Admission::Unknown });
        };
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
                "SELECT 1 FROM account_name WHERE kind = ?1 AND value = ?2
                 UNION ALL SELECT 1 FROM name_reserved WHERE skeleton = CAST(?2 AS TEXT)",
            )?
            .exists(params![KIND_USERNAME, skeleton])
    }

    /// A user by a handle as stored: id, account key, devices and, if `take`, a bundle per device (spec 3.1).
    pub fn lookup(&self, kind: u8, value: &[u8], take: bool, except: &[Key]) -> rusqlite::Result<Option<Found>> {
        self.write(|tx| {
            let id = |row: &rusqlite::Row<'_>| array(row.get(0)?);
            let user: Option<UserId> = match kind {
                KIND_USER => row(tx, "SELECT id FROM account WHERE id = ?1", [value], id)?,
                KIND_ACCOUNT_KEY => row(tx, "SELECT id FROM account WHERE account_key = ?1", [value], id)?,
                _ => row(tx, "SELECT user_id FROM account_name WHERE kind = ?1 AND value = ?2", params![kind, value], id)?,
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
                        "SELECT id, device_key, registration_id FROM account_device WHERE user_id = ?1 ORDER BY device_key",
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
            let bot = tx.query_row("SELECT bot FROM account WHERE id = ?1", [&user], |row| row.get(0))?;
            Ok(Some(Found { user_id: user, account_key, devices: roster_devices, bundles, username, bot }))
        })
    }

    /// Deletes a device's account (spec 4.2) with everything about it (`remove_user`).
    pub fn delete_account(&self, device: &DeviceId, now: u64) -> rusqlite::Result<Write<()>> {
        self.as_device(device, |tx, user| {
            remove_user(tx, &user, now)?;
            Ok(Write::Done(()))
        })
    }

    /// Operator deletion, using the same cleanup as account deletion; unknown users change nothing.
    pub fn admin_delete_user(&self, user: &UserId, now: u64) -> rusqlite::Result<Option<Vec<(BlobId, u64)>>> {
        self.write(|tx| {
            if !tx.prepare_cached("SELECT 1 FROM account WHERE id = ?1")?.exists([user])? {
                return Ok(None);
            }
            let blobs = tx
                .prepare_cached("SELECT id, created_at FROM message_blob WHERE user_id = ?1")?
                .query_map([user], blob_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            remove_user(tx, user, now)?;
            Ok(Some(blobs))
        })
    }

    // ---- groups (spec 3.4) ----

    /// A new group at revision 1; false if its id is taken, or was a group that ended.
    pub fn create_group(&self, id: &[u8], params: &[u8], state: &[u8], now: u64) -> rusqlite::Result<bool> {
        self.one_changed(
            "INSERT OR IGNORE INTO grp (id, params, revision, state, updated_at, created_at)
             SELECT ?1, ?2, 1, ?3, ?4, ?4 WHERE NOT EXISTS (SELECT 1 FROM grp_gone WHERE id = ?1)",
            params![id, params, state, now as i64],
        )
    }

    /// A group's params and state.
    pub fn group(&self, id: &[u8]) -> rusqlite::Result<Option<(Vec<u8>, Vec<u8>)>> {
        row(&self.conn(), "SELECT params, state FROM grp WHERE id = ?1", [id], |row| Ok((row.get(0)?, row.get(1)?)))
    }

    /// A group's next revision, only if it is still at `revision` (compare and swap).
    pub fn update_group(&self, id: &[u8], revision: u32, state: &[u8], now: u64) -> rusqlite::Result<bool> {
        self.one_changed(
            "UPDATE grp SET revision = revision + 1, state = ?3, updated_at = ?4 WHERE id = ?1 AND revision = ?2",
            params![id, revision, state, now as i64],
        )
    }

    /// The group ends (its last member left), if it is still at `revision`; its id is gone for good.
    pub fn end_group(&self, id: &[u8], revision: u32, now: u64) -> rusqlite::Result<bool> {
        self.write(|tx| {
            let ended = tx.execute("DELETE FROM grp WHERE id = ?1 AND revision = ?2", params![id, revision])? == 1;
            if ended {
                tx.execute("INSERT OR IGNORE INTO grp_gone (id, at) VALUES (?1, ?2)", params![id, now as i64])?;
            }
            Ok(ended)
        })
    }

    // ---- reports, support and diagnostics ----

    /// Stores the report of `reported` by an install, the pair's latest; false when there is no such user.
    pub fn report(&self, install: &Install, reported: &UserId, reason: u8, now: u64) -> rusqlite::Result<bool> {
        let stored = self
            .conn()
            .prepare_cached(
                "INSERT INTO support_report (reporter, reported_id, reason, created_at)
                 SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM account WHERE id = ?2)
                 ON CONFLICT (reporter, reported_id) DO UPDATE SET
                     reason = excluded.reason, created_at = excluded.created_at",
            )?
            .execute(params![install, reported, reason, now])?;
        Ok(stored == 1)
    }

    /// Stores an install's message to the operator, up to 1000 messages a thread.
    pub fn support_message(
        &self,
        install: &Install,
        text: &str,
        app_version: &str,
        now: u64,
    ) -> rusqlite::Result<Write<()>> {
        self.write(|tx| {
            // A full thread is retained, never silently trimmed.
            let count: i64 =
                tx.query_row("SELECT count(*) FROM support WHERE install = ?1", [install], |r| r.get(0))?;
            if count >= 1000 {
                return Ok(Write::Refused(Conflict::SupportFull));
            }
            tx.execute(
                "INSERT INTO support(install,from_team,text,created_at,app_version) VALUES (?1,0,?2,?3,?4)",
                params![install, text, now, app_version],
            )?;
            Ok(Write::Done(()))
        })
    }

    pub fn support_thread(&self, install: &Install) -> rusqlite::Result<yoip::net::api::SupportThread> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id,from_team,text,created_at,coalesce(read_at,0) FROM support WHERE install=?1 ORDER BY created_at,id")?;
        let messages = stmt
            .query_map([install], |r| {
                Ok(yoip::net::api::SupportMessage {
                    id: r.get(0)?,
                    from_team: r.get(1)?,
                    text: r.get::<_, String>(2)?.into_bytes().into(),
                    created_at: r.get(3)?,
                    read_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let unread = messages.iter().filter(|m| m.from_team == 1 && m.read_at == 0).count() as u32;
        Ok(yoip::net::api::SupportThread { messages, unread })
    }

    /// The team's answers read by the install (`from_team`), or the install's messages handled by the operator.
    pub fn support_read(&self, install: &Install, from_team: bool, through: u64, now: u64) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE support SET read_at=CASE WHEN ?3 THEN coalesce(read_at,?1) ELSE read_at END, done_at=CASE WHEN ?3 THEN done_at ELSE ?1 END WHERE install=?2 AND from_team=?3 AND id<=?4
            ",
            params![now, install, from_team, through],
        )?;
        Ok(())
    }

    /// The operator's answer, to a thread the install began; false for none, or a full one.
    pub fn support_reply(&self, install: &Install, text: &str, now: u64) -> rusqlite::Result<bool> {
        let conn = self.conn();
        Ok(conn.execute(
            "INSERT INTO support(install,from_team,text,created_at)
            SELECT ?1,1,?2,?3 WHERE EXISTS (SELECT 1 FROM support WHERE install=?1)
            AND (SELECT count(*) FROM support WHERE install=?1)<1000",
            params![install, text, now],
        )? == 1)
    }

    /// Every thread, by install id (lowercase hex), open ones first.
    pub fn support_threads(&self) -> rusqlite::Result<serde_json::Value> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT lower(hex(s.install)),max(s.created_at),sum(s.from_team=0 AND s.done_at IS NULL)
            FROM support s GROUP BY s.install
            ORDER BY (sum(s.from_team=0 AND s.done_at IS NULL)>0) DESC,max(s.created_at) DESC LIMIT 999",
        )?;
        let rows: Vec<_> = stmt.query_map([], |r| Ok(serde_json::json!({"install":r.get::<_,String>(0)?,"at":r.get::<_,i64>(1)?,"unread":r.get::<_,i64>(2)?})))?.collect::<rusqlite::Result<_>>()?;
        Ok(serde_json::json!(rows))
    }

    pub fn admin_support_thread(&self, install: &Install) -> rusqlite::Result<serde_json::Value> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id,from_team,text,created_at,read_at,done_at FROM support WHERE install=?1 ORDER BY created_at,id",
        )?;
        let rows = stmt.query_map([install], |r| Ok(serde_json::json!({"id":r.get::<_,i64>(0)?,"from_team":r.get::<_,bool>(1)?,"text":r.get::<_,String>(2)?,"created_at":r.get::<_,i64>(3)?,"read_at":r.get::<_,Option<i64>>(4)?,"done_at":r.get::<_,Option<i64>>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(serde_json::json!({"messages": rows}))
    }

    /// Deliveries taken in the hour of `now` (`n` more).
    pub fn count_deliveries(&self, now: u64, n: u64) -> rusqlite::Result<()> {
        self.run(
            "INSERT INTO stat(metric, hour, value) VALUES ('deliveries', ?1, ?2)
             ON CONFLICT(metric, key, hour) DO UPDATE SET value = value + excluded.value",
            params![now / HOUR_MS, n],
        )
    }

    /// One signed request (3.3) a sender sent, counted in its hour; kept 25 hours, for the operator's Users page
    /// ("Requests 24h"). Only requests: a contact's delivery is sealed and never counted by sender.
    pub fn count_request(&self, sender: &UserId, now: u64) -> rusqlite::Result<()> {
        if self.edge() {
            return Ok(());
        }
        let hour = (now / 3_600_000) as i64;
        let conn = self.conn();
        conn.prepare_cached(
            "INSERT INTO stat_request (user_id, hour, n) VALUES (?1, ?2, 1)
             ON CONFLICT(user_id, hour) DO UPDATE SET n = n + 1",
        )?
        .execute(params![sender, hour])?;
        conn.prepare_cached("DELETE FROM stat_request WHERE hour < ?1")?.execute([hour - 25])?;
        Ok(())
    }

    /// Events counted in the minute of `now`, `(kind, n)`: counts only, nothing of who or what (1).
    pub fn count_events(&self, now: u64, counts: &[(&str, u64)]) -> rusqlite::Result<()> {
        let minute = (now / 60_000) as i64;
        self.write(|tx| {
            let mut add = tx.prepare_cached(
                "INSERT INTO stat_minute (kind, minute, n) VALUES (?1, ?2, ?3)
                 ON CONFLICT(kind, minute) DO UPDATE SET n = n + excluded.n",
            )?;
            let mut hourly = tx.prepare_cached(
                "INSERT INTO stat(metric, hour, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(metric, key, hour) DO UPDATE SET value = value + excluded.value",
            )?;
            for (kind, n) in counts {
                add.execute(params![kind, minute, n])?;
                hourly.execute(params![kind, minute / 60, n])?;
            }
            tx.execute("DELETE FROM stat_minute WHERE minute < ?1", [minute - 120])?;
            Ok(())
        })
    }

    /// The levels at `now` into its hour, counts only, nothing of who or what (1): the hourly sample
    /// keeps an hour's first (a restart within the hour takes none); `problems_only` the open problems alone, right
    /// after one is marked done, replacing the hour's.
    pub fn sample_levels(&self, now: u64, problems_only: bool) -> rusqlite::Result<()> {
        const PROBLEMS: [(&str, &str); 2] = [
            ("open-errors", "SELECT coalesce(sum(count), 0) FROM diag WHERE done = 0 AND kind = 'error'"),
            ("open-crashes", "SELECT coalesce(sum(count), 0) FROM diag WHERE done = 0 AND kind = 'crash'"),
        ];
        const OTHERS: [(&str, &str); 5] = [
            ("queue-envelopes", "SELECT count(*) FROM inbox_message"),
            ("queue-media-bytes", "SELECT coalesce(sum(size), 0) FROM message_blob"),
            ("active-24h", "SELECT count(DISTINCT user_id) FROM account_device WHERE last_seen_at > ?1 - 86400000"),
            // People, not bots (`is_bot_app`): an account with a device of a person's app heard from in 48 hours.
            (
                "active-48h",
                "SELECT count(DISTINCT user_id) FROM account_device WHERE last_seen_at > ?1 - 172800000 \
                 AND (app IS NULL OR (app NOT LIKE 'yogpt/%' AND app NOT LIKE 'cli/%'))",
            ),
            (
                "active-30d",
                "SELECT count(DISTINCT user_id) FROM account_device WHERE last_seen_at > ?1 - 30 * 86400000",
            ),
        ];
        let others: &[_] = if problems_only { &[] } else { &OTHERS };
        let hour = (now / 3_600_000) as i64;
        self.write(|tx| {
            let mut put = tx.prepare_cached(if problems_only {
                "INSERT OR REPLACE INTO stat (metric, hour, value) VALUES (?1, ?2, ?3)"
            } else {
                "INSERT OR IGNORE INTO stat (metric, hour, value) VALUES (?1, ?2, ?3)"
            })?;
            for (kind, sql) in PROBLEMS.iter().chain(others) {
                let mut query = tx.prepare_cached(sql)?;
                let value: i64 = if query.parameter_count() == 1 {
                    query.query_row([now as i64], |r| r.get(0))
                } else {
                    query.query_row([], |r| r.get(0))
                }?;
                put.execute(params![kind, hour, value])?;
            }
            if !problems_only {
                // The phones (iOS, Android) seen within 24 hours, and of them the installs that sent an error or crash
                // in those 24 hours (a row without `at`: its day's start; the network's errors left out,
                // `NETWORK_ERRORS`): the error-free share of who is still here, in all and per platform.
                let net = network_errors_sql();
                for (suffix, like) in [("", "%"), (":ios", "ios/%"), (":android", "android/%")] {
                    let phones = "SELECT count(*) FROM account_device WHERE last_seen_at > ?1 - 86400000 AND app LIKE ?2 \
                         AND (app LIKE 'ios/%' OR app LIKE 'android/%')";
                    let hit = format!(
                        "SELECT count(DISTINCT i.install) FROM diag_install i JOIN diag d ON d.id = i.diagnostic_id \
                         WHERE coalesce(i.at, i.day * 86400000) > ?1 - 86400000 AND d.os NOT LIKE 'mac%' \
                         AND d.app LIKE ?2 AND (d.app LIKE 'ios/%' OR d.app LIKE 'android/%') AND {net}"
                    );
                    for (kind, sql) in [("phones-24h", phones.to_owned()), ("problem-installs-24h", hit)] {
                        let value: i64 = tx.prepare_cached(&sql)?.query_row(params![now as i64, like], |r| r.get(0))?;
                        put.execute(params![format!("{kind}{suffix}"), hour, value])?;
                    }
                }
                // The devices per build (`build:ios/202611191626`, `build:unknown` for none named) seen within 48
                // hours, of the apps' (bots and the CLI left out): the operator's releases over time. Counts only.
                // 48 hours, not 24: last seen is written once a day, so a phone used every day stays in the window
                // and the hourly shares move smoothly instead of stepping each time a day-old touch ages out.
                let mut builds = tx.prepare_cached(
                    "SELECT 'build:' || coalesce(app, 'unknown'), count(*) FROM account_device WHERE last_seen_at > ?1 - 172800000 \
                     AND (app IS NULL OR app LIKE 'ios/%' OR app LIKE 'android/%') GROUP BY 1",
                )?;
                let rows = builds
                    .query_map([now as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for (kind, value) in rows {
                    put.execute(params![kind, hour, value])?;
                }
            }
            Ok(())
        })
    }

    /// A crash or error report, with the install that sent it (none: the hub's own), by the day.
    pub fn diagnostic(
        &self,
        kind: &str,
        payload: &serde_json::Value,
        install: Option<&Install>,
        now: u64,
    ) -> rusqlite::Result<()> {
        if self.edge() {
            return Ok(());
        }
        self.write(|tx| {
            let uuid = payload["uuid"].as_str().map(str::to_ascii_lowercase);
            if let Some(uuid) = &uuid {
                if tx.prepare("SELECT 1 FROM diag_seen WHERE uuid=?1")?.exists([uuid])? {
                    return Ok(());
                }
            }
            let id = diagnostic_write(tx, kind, payload, now)?;
            if let Some(uuid) = uuid {
                tx.execute("INSERT INTO diag_seen(uuid, diagnostic_id) VALUES (?1, ?2)", params![uuid, id])?;
            }
            if let Some(install) = install {
                tx.execute(
                    "INSERT INTO diag_install(diagnostic_id, install, day, at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(diagnostic_id, install, day) DO UPDATE SET at = ?4",
                    params![id, install, now / DAY_MS, to_the_hour(now) as i64],
                )?;
            }
            Ok(())
        })
    }

    pub fn diagnostics(&self) -> rusqlite::Result<serde_json::Value> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id,kind,count,first_seen,last_seen,app,os,model,payload,done,
            (SELECT count(DISTINCT install) FROM diag_install WHERE diagnostic_id=d.id), done_builds, done_at,
            (SELECT json_group_array(json_array(app, count, last_seen)) FROM diag_build WHERE diagnostic_id=d.id),
            (SELECT json_group_object(hour, value) FROM stat WHERE metric = 'diag' AND key = CAST(d.id AS TEXT)
             AND hour >= unixepoch() / 3600 - 8 * 24)
            FROM diag d ORDER BY last_seen DESC",
        )?;
        let json =
            |text: Option<String>| text.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::Value::Null);
        let rows = stmt.query_map([], |r| Ok(serde_json::json!({
            "id":r.get::<_,i64>(0)?, "kind":r.get::<_,String>(1)?, "count":r.get::<_,i64>(2)?,
            "first_seen":r.get::<_,i64>(3)?, "last_seen":r.get::<_,i64>(4)?, "app":r.get::<_,String>(5)?,
            "os":r.get::<_,String>(6)?, "model":r.get::<_,String>(7)?, "payload":r.get::<_,String>(8)?, "done":r.get::<_,bool>(9)?,
            "installs":r.get::<_,i64>(10)?, "done_builds":r.get::<_,Option<String>>(11)?, "done_at":r.get::<_,Option<i64>>(12)?,
            // [[app, reports, last seen]] and {hour: reports} (hours since 1970, the last 8 days).
            "builds":json(r.get(13)?), "hours":json(r.get(14)?)
        })))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(serde_json::json!(rows))
    }

    /// Marked done (with the newest build per platform it came in so far, and when) or open again.
    pub fn diagnostic_done(&self, id: i64, done: bool, now: u64) -> rusqlite::Result<()> {
        self.write(|tx| {
            let apps: Vec<String> = tx
                .prepare("SELECT app FROM diag_build WHERE diagnostic_id = ?1")?
                .query_map([id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let builds = done.then(|| newest_builds(apps.iter().map(String::as_str)));
            tx.execute(
                "UPDATE diag SET done = ?2, done_builds = ?3, done_at = CASE WHEN ?2 THEN ?4 END WHERE id = ?1",
                params![id, done, builds, now as i64],
            )?;
            Ok(())
        })
    }

    /// A support message done (`Some(at)`) or open again.
    pub fn message_done(&self, id: i64, done: Option<u64>) -> rusqlite::Result<bool> {
        self.one_changed("UPDATE support SET done_at=?1 WHERE id=?2", params![done, id])
    }

    // ---- storage (spec 4.3) ----

    /// The storage of a user: its version and sealed bytes.
    pub fn storage(&self, user: &UserId) -> rusqlite::Result<Option<Sealed>> {
        row(&self.conn(), "SELECT version, data FROM account_storage WHERE user_id = ?1", [user], |row| {
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
            let current: Sealed =
                row(tx, "SELECT version, data FROM account_storage WHERE user_id = ?1", [&user], |row| {
                    Ok((row.get::<_, i64>(0)? as u64, row.get(1)?))
                })?
                .unwrap_or_default();
            if current.0 != replaces {
                return Ok(Write::Done(Err(current)));
            }
            tx.prepare_cached(
                "INSERT INTO account_storage (user_id, version, data, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (user_id) DO UPDATE SET
                     version = excluded.version, data = excluded.data, updated_at = excluded.updated_at",
            )?
            .execute(params![user, (replaces + 1) as i64, data, now])?;
            Ok(Write::Done(Ok(())))
        })
    }

    // ---- backups (spec 2.4) ----

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
                        "SELECT 1 FROM account_backup WHERE locator = ?1 AND user_id != ?2
                         UNION ALL SELECT 1 FROM account WHERE id = ?1",
                    )?
                    .exists(params![locator, user])?;
                if taken {
                    return Ok(Write::Refused(Conflict::LocatorTaken));
                }
            }
            tx.prepare_cached(
                "INSERT INTO account_backup (user_id, locator, record, updated_at, auth_passkey, auth_code)
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
            "SELECT record, auth_passkey, auth_code FROM account_backup WHERE user_id = ?1
             UNION ALL SELECT record, auth_passkey, auth_code FROM account_backup
                 WHERE locator = ?1 AND NOT EXISTS (SELECT 1 FROM account WHERE id = ?1)
             LIMIT 1",
            [id],
            |row| Ok((row.get(0)?, BackupAuth { passkey: key(row.get(1)?), code: key(row.get(2)?) })),
        )
    }

    // ---- blobs (spec 4.3) ----

    /// Whether a user's quota has room for `size` bytes now; `put_blob` asks again as it stores.
    pub fn blob_quota_left(&self, user: &UserId, size: u64, now: u64, quota: &BlobQuota) -> rusqlite::Result<bool> {
        blob_quota_left(&self.conn(), user, size, now, quota)
    }

    /// The bytes of all blobs stored.
    pub fn blobs_stored(&self) -> rusqlite::Result<u64> {
        self.conn()
            .prepare_cached("SELECT coalesce(sum(size), 0) FROM message_blob")?
            .query_row([], |row| row.get::<_, i64>(0).map(|n| n as u64))
    }

    pub fn blob_exists(&self, id: &BlobId) -> rusqlite::Result<bool> {
        self.conn().prepare_cached("SELECT 1 FROM message_blob WHERE id = ?1")?.exists([id])
    }

    /// A blob there and ready (`Some(true)`), there with its file on its way (`Some(false)`), or none.
    pub fn blob_ready_state(&self, id: &BlobId) -> rusqlite::Result<Option<bool>> {
        row(&self.conn(), "SELECT ready FROM message_blob WHERE id = ?1", [id], |row| row.get(0))
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
            let ready: Option<bool> = row(tx, "SELECT ready FROM message_blob WHERE id = ?1", [id], |row| row.get(0))?;
            match ready {
                Some(true) => return Ok(BlobPut::Taken),
                Some(false) => return Ok(BlobPut::Unfinished),
                None => {}
            }
            if !blob_quota_left(tx, &user, size, now, quota)? {
                return Ok(BlobPut::Quota);
            }
            tx.prepare_cached(
                "INSERT INTO message_blob (id, user_id, size, fetches_left, created_at, ready) VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            )?
            .execute(params![id, user, size as i64, fetches, now])?;
            tx.prepare_cached(
                "INSERT INTO message_blob_quota (user_id, day, count, bytes) VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT (user_id, day) DO UPDATE SET count = count + 1, bytes = bytes + excluded.bytes",
            )?
            .execute(params![user, (now / DAY_MS) as i64, size as i64])?;
            Ok(BlobPut::Stored)
        })
    }

    /// The file is in place, the blob handed out from now on; `false`: its row went meanwhile, the upload failed.
    pub fn blob_ready(&self, id: &BlobId, created_at: u64) -> rusqlite::Result<bool> {
        self.one_changed(
            "UPDATE message_blob SET ready = 1, gets_left = fetches_left + 3 WHERE id = ?1 AND created_at = ?2 AND ready = 0",
            params![id, created_at as i64],
        )
    }

    /// A GET reserves only an abuse-budget slot. Cut-off streams return it.
    pub fn take_blob_get(&self, id: &BlobId, now: u64, ttl_ms: u64) -> rusqlite::Result<Option<(u64, u64)>> {
        row(
            &self.conn(),
            "UPDATE message_blob SET gets_left = gets_left - 1 WHERE id = ?1 AND ready = 1
             AND fetches_left > 0 AND gets_left > 0 AND created_at >= ?2 RETURNING size, created_at",
            params![id, now.saturating_sub(ttl_ms) as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
    }

    pub fn return_blob_get(&self, id: &BlobId, created_at: u64) -> rusqlite::Result<()> {
        self.run(
            "UPDATE message_blob SET gets_left = gets_left + 1 WHERE id = ?1 AND created_at = ?2",
            params![id, created_at as i64],
        )
    }

    /// One anonymous acknowledgement is one fetch; the last deletes the row immediately.
    pub fn ack_blob(&self, id: &BlobId, now: u64, ttl_ms: u64) -> rusqlite::Result<Option<u64>> {
        self.write(|tx| {
            let last: Option<u64> = tx.prepare_cached(
                "UPDATE message_blob SET fetches_left = fetches_left - 1 WHERE id = ?1 AND ready = 1
                 AND fetches_left > 0 AND created_at >= ?2 RETURNING CASE WHEN fetches_left = 0 THEN created_at ELSE NULL END"
            )?.query_row(params![id, now.saturating_sub(ttl_ms) as i64], |row| row.get::<_, Option<u64>>(0)).optional()?.flatten();
            if last.is_some() {
                tx.execute("DELETE FROM message_blob WHERE id = ?1", [id])?;
            }
            Ok(last)
        })
    }

    pub fn delete_blob(&self, id: &BlobId) -> rusqlite::Result<()> {
        self.run("DELETE FROM message_blob WHERE id = ?1", [id])
    }

    /// Deletes blobs past their time, or not ready for an hour, and returns them (their files go too).
    pub fn prune_blobs(&self, now: u64, ttl_ms: u64) -> rusqlite::Result<Vec<(BlobId, u64)>> {
        self.write(|tx| {
            let ids = tx
                .prepare_cached(
                    "DELETE FROM message_blob WHERE created_at < ?1 OR (ready = 0 AND created_at < ?2) RETURNING id, created_at",
                )?
                .query_map(
                    params![now.saturating_sub(ttl_ms) as i64, now.saturating_sub(NOT_READY_KEPT_MS) as i64],
                    blob_row,
                )?
                .collect::<Result<_, _>>()?;
            tx.execute("DELETE FROM message_blob_quota WHERE day < ?1", [(now / DAY_MS) as i64 - 1])?;
            Ok(ids)
        })
    }

    /// At a start: fetches under way given back, blobs not ready (a crash mid-upload) deleted and returned.
    pub fn recover_blobs(&self) -> rusqlite::Result<Vec<(BlobId, u64)>> {
        self.write(|tx| {
            tx.execute(
                "UPDATE message_blob SET fetches_left = fetches_left + fetching, fetching = 0 WHERE fetching > 0",
                [],
            )?;
            let ids = tx
                .prepare_cached("DELETE FROM message_blob WHERE ready = 0 RETURNING id, created_at")?
                .query_map([], blob_row)?
                .collect::<Result<_, _>>()?;
            Ok(ids)
        })
    }

    /// Every blob, by id and when it was stored: what the sweep keeps.
    pub fn blob_ids(&self) -> rusqlite::Result<Vec<(BlobId, u64)>> {
        self.conn().prepare_cached("SELECT id, created_at FROM message_blob")?.query_map([], blob_row)?.collect()
    }

    /// The blobs a device's user uploaded: whose files go when the account is deleted.
    pub fn blobs_of(&self, device: &DeviceId) -> rusqlite::Result<Vec<(BlobId, u64)>> {
        self.conn()
            .prepare_cached(
                "SELECT b.id, b.created_at FROM message_blob b JOIN account_device d ON d.user_id = b.user_id WHERE d.id = ?1",
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
        if self.edge() {
            return Ok(row(
                &self.conn(),
                "SELECT n FROM corro.account_prekey_left WHERE device_id=?",
                [device],
                |r| r.get(0),
            )?
            .unwrap_or(0));
        }
        self.conn()
            .prepare_cached(
                "SELECT min(
                     (SELECT count(*) FROM account_prekey WHERE device_id = ?1 AND kind = ?2),
                     (SELECT count(*) FROM account_prekey WHERE device_id = ?1 AND kind = ?3))",
            )?
            .query_row(params![device, ONE_TIME, KYBER], |row| row.get(0))
    }

    /// A success sticks until the account is deleted. Only a current device can record a verdict.
    pub(crate) fn record_attestation(
        &self,
        device: &DeviceId,
        platform: &str,
        reason: Option<&str>,
        level: Option<&str>,
        now: u64,
    ) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO account_attestation(user_id,state,platform,reason,level,at_ms)
             SELECT user_id,?2,?3,?4,?5,?6 FROM account_device WHERE id=?1
             ON CONFLICT(user_id) DO UPDATE SET state=excluded.state, platform=excluded.platform,
               reason=excluded.reason, level=excluded.level, at_ms=excluded.at_ms
             WHERE (excluded.state='attested' AND account_attestation.state='failed')
                OR (excluded.state=account_attestation.state AND excluded.at_ms>=account_attestation.at_ms)",
            params![device, if reason.is_some() { "failed" } else { "attested" }, platform, reason, level, now],
        )?;
        Ok(())
    }

    /// Sets a device's push token, taken from any other device that held it; refused for a device replaced since it
    /// authenticated.
    pub fn set_push(&self, device: &DeviceId, platform: u8, token: &[u8]) -> rusqlite::Result<Write<()>> {
        self.write(|tx| {
            let changed = tx
                .prepare_cached("UPDATE account_device SET push_platform = ?2, push_token = ?3 WHERE id = ?1")?
                .execute(params![device, platform, token])?
                == 1;
            if !changed {
                return Ok(Write::Refused(Conflict::DeviceGone));
            }
            release_push_token(tx, device, platform, token)?;
            Ok(Write::Done(()))
        })
    }

    /// Clears a push token the push service refused, unless the device set another meanwhile.
    pub fn clear_push(&self, device: &DeviceId, token: &[u8]) -> rusqlite::Result<()> {
        if self.edge() {
            return Ok(());
        }
        self.run(
            "UPDATE account_device SET push_platform = 0, push_token = x'' WHERE id = ?1 AND push_token = ?2",
            params![device, token],
        )
    }

    // ---- delivery ----

    /// Delivers envelopes in one transaction (group commit): each to every device of its user, or to none (spec 3.2).
    /// Immediate, as every write here: a deferred one that read first and then wrote failed at once ("database is
    /// locked", a 500) when another connection (litestream's) committed meanwhile, busy timeout or not.
    pub fn deliver(&self, items: &[Incoming<'_>], now: u64, most: u32) -> rusqlite::Result<Vec<Delivered>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut batch = Vec::new();
        let mut pending = std::collections::HashMap::<DeviceId, u32>::new();
        let mut outcomes = Vec::with_capacity(items.len());
        for Incoming { user, devices_hash, from, copies } in items {
            let Some(current) = roster(&tx, user)? else {
                outcomes.push(Delivered::Unknown);
                continue;
            };
            if current.devices.is_empty() {
                outcomes.push(Delivered::Unknown);
                continue;
            }
            let mut devices: Vec<(u8, Wake)> = tx
                .prepare_cached(
                    "SELECT id, number, push_platform, push_token, app FROM account_device WHERE user_id = ?1",
                )?
                .query_map([user], |row| {
                    Ok((
                        row.get(1)?,
                        Wake {
                            device_id: array(row.get(0)?)?,
                            push_platform: row.get(2)?,
                            push_token: row.get(3)?,
                            bot: is_bot_app(row.get::<_, Option<String>>(4)?.as_deref()),
                            waiting: 0,
                        },
                    ))
                })?
                .collect::<Result<_, _>>()?;
            let sender = from.and_then(|from| devices.iter().find(|(_, d)| d.device_id == from).map(|(n, _)| *n));
            devices.retain(|(number, _)| Some(*number) != sender);
            let addressed: Vec<_> = current.devices.iter().filter(|d| Some(d.id) != sender).map(|d| d.key).collect();
            // The sender knows the roster as it is (the hash); it may write to some of its devices only (a
            // decryption error's answer, a message sent again to one device, a sync to devices it trusts).
            let listed: Vec<u8> = match copies {
                Copies::Each(each) => each.iter().map(|(n, _)| *n).collect(),
                Copies::Legacy(_) => devices.iter().map(|(n, _)| *n).collect(),
            };
            if addressed.is_empty()
                || yoip::net::devices::hash(&current.account_key, &addressed) != *devices_hash
                || listed.is_empty()
                || !listed.iter().all(|n| devices.iter().any(|(d, _)| d == n))
            {
                outcomes.push(Delivered::Mismatch(current));
                continue;
            }
            devices.retain(|(number, _)| listed.contains(number));
            // Only the last allowed slot matters: an index probe, no count.
            let mut slot = tx.prepare_cached("SELECT 1 FROM inbox_message WHERE device_id = ?1 LIMIT 1 OFFSET ?2")?;
            let mut full = most == 0;
            for (_, device) in &devices {
                if full {
                    break;
                }
                let extra = pending.get(&device.device_id).copied().unwrap_or(0);
                full = extra >= most || slot.exists(params![device.device_id, most - extra - 1])?;
            }
            if full {
                outcomes.push(Delivered::Full);
                continue;
            }
            let mut waiting = tx.prepare_cached("SELECT COUNT(*) FROM inbox_message WHERE device_id = ?1")?;
            // Each device only its own copy (Signal's per-device message): nothing stored N times.
            for (number, device) in &mut devices {
                let envelope = match copies {
                    Copies::Each(each) => each.iter().find(|(n, _)| n == number).map_or(&[][..], |(_, e)| *e),
                    Copies::Legacy(envelope) => *envelope,
                };
                let extra = if let Some(agent) = &self.agent {
                    batch.push(agent.message(&device.device_id, envelope, now));
                    let n = pending.entry(device.device_id).or_default();
                    *n += 1;
                    *n
                } else {
                    tx.prepare_cached("INSERT INTO message (device_id,envelope,received_at) VALUES (?,?,?)")?
                        .execute(params![device.device_id, envelope, now])?;
                    0
                };
                device.waiting = waiting.query_row([device.device_id], |row| row.get::<_, u32>(0))? + extra;
            }
            let devices = devices.into_iter().map(|(_, wake)| wake).collect();
            outcomes.push(Delivered::Stored(devices));
        }
        if let Some(agent) = &self.agent {
            agent.write(batch)?;
        }
        tx.commit()?;
        Ok(outcomes)
    }

    /// A device's envelopes, oldest first: id, envelope, when it came (ms).
    pub fn fetch(&self, device: &DeviceId, most: usize) -> rusqlite::Result<Vec<(u64, Vec<u8>, u64)>> {
        self.conn()
            .prepare_cached(
                "SELECT id, envelope, received_at FROM inbox_message WHERE device_id = ?1 ORDER BY id LIMIT ?2",
            )?
            .query_map(params![device, most as i64], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get(1)?, row.get::<_, i64>(2)? as u64))
            })?
            .collect()
    }

    /// Remember which remote ids this device can acknowledge before replication arrives.
    pub(crate) fn peer_items(
        &self,
        device: &DeviceId,
        items: Vec<api::InboxItem>,
        only_missing: bool,
        now: u64,
    ) -> rusqlite::Result<Vec<api::InboxItem>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut found = Vec::new();
        {
            let mut local = tx.prepare_cached("SELECT 1 FROM corro.message WHERE id=? AND device_id=?")?;
            let mut receipt = tx.prepare_cached("INSERT OR REPLACE INTO peer_inbox_receipt VALUES(?,?,?)")?;
            let recent = self.agent.as_ref().unwrap().recent_acks.lock().unwrap();
            for item in items {
                if recent.contains(device, item.id) {
                    continue;
                }
                if local.exists(params![item.id, device])? {
                    if only_missing {
                        continue;
                    }
                } else {
                    receipt.execute(params![device, item.id, now])?;
                }
                found.push(item);
            }
        }
        tx.commit()?;
        Ok(found)
    }

    /// A device's number among its user's devices (spec 3).
    pub fn device_number(&self, device: &DeviceId) -> rusqlite::Result<Option<u8>> {
        row(&self.conn(), "SELECT number FROM account_device WHERE id = ?1", [device], |row| row.get(0))
    }

    /// Deletes a device's envelopes by id (WAL NORMAL: one undone by a power loss comes again, spec 3.2).
    /// Returns how many there were.
    /// Deletes the device's envelopes of `ids`; when each one deleted was accepted (ms), for the latency counts.
    /// Deletes the acknowledged envelopes; returns when each was accepted, and whether the device is a bot's.
    pub fn ack(&self, device: &DeviceId, ids: &[u64]) -> rusqlite::Result<(Vec<u64>, bool)> {
        let (times, _, bot) = self.ack_inbox(device, ids)?;
        Ok((times, bot))
    }

    pub(crate) fn ack_inbox(&self, device: &DeviceId, ids: &[u64]) -> rusqlite::Result<(Vec<u64>, usize, bool)> {
        let mut remote = 0;
        let mut acknowledged = Vec::new();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut acked = Vec::new();
        if let Some(agent) = &self.agent {
            if !ids.is_empty() {
                let slots = std::iter::repeat_n("?", ids.len()).collect::<Vec<_>>().join(",");
                let filter = format!("device_id=unhex(?) AND id IN ({slots})");
                let mut args = vec![rusqlite::types::Value::Text(crate::blobs::hex(device))];
                args.extend(ids.iter().map(|&id| rusqlite::types::Value::Integer(id as i64)));
                let mut received: std::collections::HashMap<u64, u64> = tx
                    .prepare(&format!("SELECT id, received_at FROM corro.message WHERE {filter}"))?
                    .query_map(rusqlite::params_from_iter(&args), |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                let mut batch = Vec::new();
                let device_hex = crate::blobs::hex(device);
                for &id in ids {
                    let offered =
                        tx.execute("DELETE FROM peer_inbox_receipt WHERE device_id=? AND id=?", params![device, id])?;
                    if let Some(time) = received.remove(&id) {
                        acked.push(time);
                        acknowledged.push(id);
                    } else if offered != 0 {
                        batch.push(serde_json::json!([
                            "INSERT OR IGNORE INTO message(id, device_id) VALUES (?, unhex(?))",
                            [id, device_hex]
                        ]));
                        remote += 1;
                        acknowledged.push(id);
                    }
                }
                let mut args = vec![serde_json::json!(crate::blobs::hex(device))];
                args.extend(ids.iter().map(|&id| serde_json::json!(id as i64)));
                batch.push(serde_json::json!([format!("DELETE FROM message WHERE {filter}"), args]));
                agent.write(batch)?;
            }
        } else {
            let mut delete =
                tx.prepare_cached("DELETE FROM message WHERE device_id = ?1 AND id = ?2 RETURNING received_at")?;
            for id in ids {
                if let Some(at) = delete.query_row(params![device, *id as i64], |r| r.get::<_, i64>(0)).optional()? {
                    acked.push(at as u64);
                }
            }
        }
        let app: Option<String> =
            tx.query_row("SELECT app FROM account_device WHERE id = ?1", [device], |r| r.get(0)).optional()?.flatten();
        tx.commit()?;
        if let Some(agent) = &self.agent {
            agent.recent_acks.lock().unwrap().remember(*device, &acknowledged);
        }
        Ok((acked, remote, is_bot_app(app.as_deref())))
    }

    // ---- upkeep and audit ----

    /// Deletes envelopes older than the TTL. Replaced devices stay in `gone` for good (about 40 bytes each): a
    /// registration carries no time, so its body played back after any while must still find its device id taken.
    pub fn prune(&self, now: u64, ttl_ms: u64) -> rusqlite::Result<usize> {
        let conn = self.conn();
        let before = now.saturating_sub(ttl_ms) as i64;
        if self.agent.is_some() {
            conn.execute("DELETE FROM peer_inbox_receipt WHERE seen_at < ?", [before])?;
        }
        if self.edge() {
            return Ok(0);
        }
        let messages = if let Some(agent) = &self.agent {
            agent.write(vec![serde_json::json!(["DELETE FROM message WHERE received_at < ?", [before]])])?
        } else {
            conn.execute("DELETE FROM message WHERE received_at < ?1", [before])?
        };
        // Crash and error groups not seen for 90 days go, with their uuids and installs (spec 4).
        let kept = now.saturating_sub(DIAGNOSTICS_KEPT_DAYS * DAY_MS);
        conn.execute("DELETE FROM diag WHERE last_seen < ?1", [kept as i64])?;
        conn.execute("DELETE FROM diag_install WHERE day < ?1", [(kept / DAY_MS) as i64])?;
        Ok(messages)
    }

    /// The apps the devices named last (`Yo-App`), with how many devices each. For the operator; reads only.
    pub fn apps(&self) -> rusqlite::Result<Vec<(String, i64)>> {
        let conn = self.conn();
        let mut statement =
            conn.prepare("SELECT coalesce(app, ''), count(*) FROM account_device GROUP BY 1 ORDER BY 1")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// Rows that belong to nobody, with how many fail each check; empty when consistent.
    pub fn audit(&self) -> rusqlite::Result<Vec<(&'static str, i64)>> {
        // Each: what fails, and the rows that fail it (`SELECT count(*) FROM …`).
        const CHECKS: &[(&str, &str)] = &[
            ("devices without their user", "account_device WHERE user_id NOT IN (SELECT id FROM account)"),
            ("users without a device", "account WHERE id NOT IN (SELECT user_id FROM account_device)"),
            ("users whose key is not theirs", "account u WHERE NOT EXISTS (SELECT 1 FROM account_key k WHERE k.key = u.account_key AND k.user_id = u.id)"),
            ("account keys without their user", "account_key WHERE user_id NOT IN (SELECT id FROM account)"),
            ("handles without their user", "account_name WHERE user_id NOT IN (SELECT id FROM account)"),
            ("backups without their user", "account_backup WHERE user_id NOT IN (SELECT id FROM account)"),
            ("reports about nobody", "support_report WHERE reported_id NOT IN (SELECT id FROM account)"),
            ("storage without its user", "account_storage WHERE user_id NOT IN (SELECT id FROM account)"),
            ("blobs without their user", "message_blob WHERE user_id NOT IN (SELECT id FROM account)"),
            ("blob quotas without their user", "message_blob_quota WHERE user_id NOT IN (SELECT id FROM account)"),
            ("prekeys without their device", "account_prekey WHERE device_id NOT IN (SELECT id FROM account_device)"),
            ("envelopes without their device", "inbox_message WHERE device_id NOT IN (SELECT id FROM account_device)"),
            ("devices both there and gone", "account_device WHERE id IN (SELECT id FROM account_device_gone)"),
        ];
        let conn = self.conn();
        let mut failed = Vec::new();
        for (what, sql) in CHECKS {
            if self.edge() && (sql.contains("account_key") || sql.contains("account_prekey")) {
                continue;
            }
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
            version: None,
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
        let skeleton = yoip::net::handle::skeleton(spelling);
        db.register(skeleton.as_bytes(), spelling, &[n; 16], &[key; 32], &device(n), &prekeys(0), 100, None, now)
            .unwrap()
    }

    #[test]
    fn attestation_sticks_until_account_deletion() {
        let db = Db::open(":memory:").unwrap();
        register(&db, b"test", 1, 2, 1).done().unwrap();
        let id = [2; 16];
        let row = || {
            db.conn().query_row("SELECT platform,reason,level,at_ms FROM account_attestation", [], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, u64>(3)?,
                ))
            })
        };
        db.record_attestation(&id, "ios", Some("nonce"), None, 10).unwrap();
        db.record_attestation(&id, "ios", Some("chain"), None, 11).unwrap();
        assert_eq!(row().unwrap().1.as_deref(), Some("chain"));
        db.record_attestation(&id, "android", None, Some("device"), 12).unwrap();
        db.record_attestation(&id, "ios", Some("format"), None, 13).unwrap();
        assert_eq!(row().unwrap(), ("android".into(), None, Some("device".into()), 12));
        db.delete_account(&id, 14).unwrap().done().unwrap();
        assert!(matches!(row(), Err(rusqlite::Error::QueryReturnedNoRows)));
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
        db.diagnostic("crash", &legacy_crash("trace", "1"), None, 7).unwrap();
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
        assert_eq!(db.conn().query_row("SELECT kind FROM diag", [], |r| r.get::<_, String>(0)).unwrap(), "crash");
    }

    #[test]
    fn schemas_3_and_4_reach_10_without_losing_support_or_diagnostics() {
        for version in [3, 4] {
            let path = std::env::temp_dir().join(format!("yo-support-{version}-{}.sqlite", std::process::id()));
            let _ = std::fs::remove_file(&path);
            {
                let db = Db::open(path.to_str().unwrap()).unwrap();
                let mut conn = db.conn();
                as_schema_10(&conn);
                conn.execute_batch(LEGACY_FEEDBACK).unwrap();
                conn.execute_batch(
                    "DROP TABLE support_messages;
                    DROP TABLE diagnostic_seen; DROP TABLE diagnostics;
                    INSERT INTO feedback(id,user_id,text,app_version,created_at,auto,type) VALUES
                    (10,zeroblob(16),'Help with notifications 🎨','ios/123',1700000000000,0,'feedback'),
                    (11,zeroblob(16),'ios/123 · 26 · iPhone\nSIGSEGV\nYo +0x10','ios/123',1700000001000,1,'crash'),
                    (12,zeroblob(16),'Fix notifications','admin',1700000002000,0,'todo'),
                    (13,zeroblob(16),'ios/123 · 26 · iPhone\nSIGSEGV\nYo +0x10','ios/123',1700000003000,1,'crash');
                    PRAGMA user_version=3;",
                )
                .unwrap();
                if version == 4 {
                    migrate_diagnostics(&mut conn).unwrap();
                    conn.execute_batch("UPDATE diagnostics SET done=1;
                        CREATE TABLE diagnostic_seen(uuid TEXT PRIMARY KEY, diagnostic_id INTEGER NOT NULL REFERENCES diagnostics(id));
                        INSERT INTO diagnostic_seen SELECT '550e8400-e29b-41d4-a716-446655440000',id FROM diagnostics;").unwrap();
                }
            }
            for _ in 0..2 {
                let db = Db::open(path.to_str().unwrap()).unwrap();
                let conn = db.conn();
                assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 11);
                let message: (String, String, i64) = conn
                    .query_row("SELECT text,app_version,created_at FROM support WHERE id=10", [], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })
                    .unwrap();
                assert_eq!(message, ("Help with notifications 🎨".into(), "ios/123".into(), 1700000000000));
                assert_eq!(
                    conn.query_row("SELECT text FROM todos", [], |r| r.get::<_, String>(0)).unwrap(),
                    "Fix notifications"
                );
                assert!(!conn.prepare("SELECT 1 FROM support WHERE install=X''").unwrap().exists([]).unwrap());
                assert!(!conn
                    .prepare("SELECT 1 FROM sqlite_master WHERE name='feedback'")
                    .unwrap()
                    .exists([])
                    .unwrap());
                let diagnostic: (i64, i64, i64, bool) = conn
                    .query_row("SELECT count,first_seen,last_seen,done FROM diag", [], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })
                    .unwrap();
                assert_eq!(diagnostic, (2, 1700000001000, 1700000003000, version == 4));
                if version == 3 {
                    assert!(
                        conn.query_row("SELECT seq FROM sqlite_sequence WHERE name='support'", [], |r| r
                            .get::<_, i64>(0))
                            .unwrap()
                            >= 13
                    );
                }
                if version == 4 {
                    assert_eq!(
                        conn.query_row("SELECT count(*) FROM diag_seen", [], |r| r.get::<_, i64>(0)).unwrap(),
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
        let phone = [7; 16];
        assert!(!db.support_reply(&phone, "nobody wrote", 1).unwrap(), "no thread, no answer");
        db.support_message(&phone, "hi", "1", 1).unwrap();
        assert!(db.support_reply(&phone, "reply", 2).unwrap());
        db.support_read(&phone, false, 100, 3).unwrap();
        assert_eq!(db.support_thread(&phone).unwrap().unread, 1);
        assert!(db.support_thread(&[8; 16]).unwrap().messages.is_empty(), "another install sees nothing of it");
        db.support_read(&phone, true, 2, 4).unwrap();
        assert_eq!(db.support_thread(&phone).unwrap().unread, 0);
        for _ in 2..1000 {
            db.support_message(&phone, "hi", "1", 5).unwrap();
        }
        assert!(matches!(db.support_message(&phone, "full", "1", 6).unwrap(), Write::Refused(Conflict::SupportFull)));
        assert!(!db.support_reply(&phone, "full", 6).unwrap());
        assert_eq!(db.support_thread(&phone).unwrap().messages.len(), 1000);
        let threads = db.support_threads().unwrap();
        assert_eq!(threads[0]["install"], "07".repeat(16));
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
    fn expected_diagnostics_are_counts_not_open_problems() {
        let path = std::env::temp_dir().join(format!("yo-db-expected-{}.sqlite", std::process::id()));
        let path = path.to_str().unwrap();
        let expected = serde_json::json!({"domain": "HTTP", "code": "429", "place": "x", "app": "ios/1",
            "severity": "expected"});
        let error = serde_json::json!({"domain": "HTTP", "code": "500", "place": "x", "app": "ios/1"});
        {
            let db = Db::open(path).unwrap();
            db.diagnostic("error", &expected, None, 1).unwrap();
            db.diagnostic("error", &error, None, 1).unwrap();
            // As an older hub kept it: open.
            db.conn().execute("UPDATE diag SET done = 0", []).unwrap();
        }
        let db = Db::open(path).unwrap();
        let done = |code: &str| {
            db.diagnostics()
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["payload"].as_str().unwrap().contains(code))
                .unwrap()["done"]
                .clone()
        };
        assert_eq!((done("429"), done("500")), (true.into(), false.into()));
        db.diagnostic(
            "error",
            &serde_json::json!({"domain": "a", "code": "1", "place": "x", "app": "ios/1",
            "severity": "expected"}),
            None,
            2,
        )
        .unwrap();
        assert_eq!(done(r#""code":"1""#), true);
        drop(db);
        for suffix in ["", "-wal", "-shm"] {
            _ = std::fs::remove_file(format!("{path}{suffix}"));
        }
    }

    #[test]
    fn diagnostics_uuid_is_idempotent() {
        let db = Db::open(":memory:").unwrap();
        let mut payload = serde_json::json!({"uuid": "550e8400-e29b-41d4-a716-446655440000",
            "app": "ios/test", "domain": "test", "code": "1", "place": "test"});
        db.diagnostic("error", &payload, None, 1).unwrap();
        db.diagnostic("error", &payload, None, 2).unwrap();
        assert_eq!(db.diagnostics().unwrap()[0]["count"], 1);
        payload["uuid"] = "550e8400-e29b-41d4-a716-446655440001".into();
        db.diagnostic("error", &payload, None, 3).unwrap();
        assert_eq!(db.diagnostics().unwrap()[0]["count"], 2);
        payload.as_object_mut().unwrap().remove("uuid");
        db.diagnostic("error", &payload, None, 4).unwrap();
        db.diagnostic("error", &payload, None, 5).unwrap();
        assert_eq!(db.diagnostics().unwrap()[0]["count"], 4);
    }

    #[test]
    fn diagnostics_migrate_once_and_count_every_occurrence() {
        let db = Db::open(":memory:").unwrap();
        {
            let mut conn = db.conn();
            as_schema_10(&conn);
            conn.execute_batch("DROP TABLE diagnostics; PRAGMA user_version = 3").unwrap();
            conn.execute_batch(LEGACY_FEEDBACK).unwrap();
            conn.execute("INSERT INTO feedback(user_id,text,app_version,created_at,auto,type) VALUES(X'', 'trace','1',2,1,'crash'), (X'', 'trace','1',3,1,'crash')", []).unwrap();
            migrate_diagnostics(&mut conn).unwrap();
            migrate_diagnostics(&mut conn).unwrap();
            assert_eq!(conn.query_row("SELECT count(*) FROM feedback", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
            conn.execute_batch("PRAGMA user_version = 10").unwrap();
            schema::upgrade(&mut conn).unwrap();
        }
        let rows = db.diagnostics().unwrap();
        assert_eq!(rows[0]["count"], 2);
        assert_eq!(rows[0]["first_seen"], 2);
        assert_eq!(rows[0]["last_seen"], 3);
        db.diagnostic_done(rows[0]["id"].as_i64().unwrap(), true, 0).unwrap();
        db.diagnostic("crash", &legacy_crash("trace", "1"), None, 4).unwrap();
        assert_eq!(db.diagnostics().unwrap()[0]["done"], true);
        let mut error = serde_json::json!({"domain":"test", "code":"1", "place":"sync", "app":"ios/2", "os":"26", "model":"iPhone"});
        for at in 10..110 {
            db.diagnostic("error", &error, None, at).unwrap();
        }
        error["os"] = serde_json::json!("27");
        db.diagnostic("error", &error, None, 110).unwrap();
        let rows = db.diagnostics().unwrap();
        assert_eq!(rows[0]["count"], 101);
        assert_eq!(rows[0]["os"], "27");
        assert_eq!(rows[0]["first_seen"], 10);
        error["app"] = serde_json::json!("ios/3");
        db.diagnostic("error", &error, None, 111).unwrap();
        let rows = db.diagnostics().unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 2, "another build: the same problem");
        assert_eq!(rows[0]["count"], 102);
        assert_eq!(rows[0]["builds"], serde_json::json!([["ios/2", 101, 110], ["ios/3", 1, 111]]));
    }

    /// A problem marked done comes back with a report of a newer build than any it was marked done in (spec 4);
    /// the builds it had stay done, an expected one stays a count.
    #[test]
    fn a_done_problem_reopens_in_a_newer_build() {
        let db = Db::open(":memory:").unwrap();
        let report = |app: &str, at: u64| {
            let error = serde_json::json!({"domain":"d", "code":"1", "place":"p", "app":app, "os":"26", "model":"x"});
            db.diagnostic("error", &error, None, at).unwrap();
            let row = db.diagnostics().unwrap()[0].clone();
            (row["done"].as_bool().unwrap(), row["done_builds"].clone())
        };
        report("ios/200", 1);
        report("android/7", 2);
        report("ios/100", 3);
        db.diagnostic_done(1, true, 4).unwrap();
        assert_eq!(db.diagnostics().unwrap()[0]["done_builds"], "android/7,ios/200");
        assert_eq!(db.diagnostics().unwrap()[0]["done_at"], 4);
        assert!(report("ios/100", 5).0, "an older build still reporting: done, still counted");
        assert!(report("ios/200", 6).0);
        assert!(report("android/7", 7).0);
        assert_eq!(report("ios/201", 8), (false, serde_json::Value::Null), "a newer build: open again");
        db.diagnostic_done(1, true, 9).unwrap();
        assert!(!report("cli/1", 10).0, "a platform it was not seen on");
        assert_eq!(db.diagnostics().unwrap()[0]["hours"], serde_json::json!({}), "old hours stay out of the list");
        let hour = crate::now_ms() / HOUR_MS;
        report("ios/201", hour * HOUR_MS);
        assert_eq!(db.diagnostics().unwrap()[0]["hours"], serde_json::json!({(hour.to_string()): 1}));
        // Expected: a count, never an open problem, whatever the build.
        let expected = serde_json::json!({"domain":"e", "code":"1", "place":"p", "app":"ios/1", "os":"26", "model":"x",
            "severity":"expected"});
        db.diagnostic("error", &expected, None, hour * HOUR_MS + 1).unwrap();
        let mut newer = expected.clone();
        newer["app"] = serde_json::json!("ios/999");
        db.diagnostic("error", &newer, None, hour * HOUR_MS + 2).unwrap();
        assert_eq!(db.diagnostics().unwrap()[0]["done"], true);
    }

    /// Groups of one problem in several builds, from before its identity lost the build, become one (spec 4).
    #[test]
    fn the_groups_of_one_problem_in_several_builds_become_one() {
        let dir = std::env::temp_dir().join(format!("yo-problems-{}-{}", std::process::id(), crate::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hub.sqlite");
        let path = path.to_str().unwrap();
        {
            let db = Db::open(path).unwrap();
            let conn = db.conn();
            as_schema_10(&conn);
            conn.execute_batch(
                "ALTER TABLE diagnostics DROP COLUMN done_builds; ALTER TABLE diagnostics DROP COLUMN done_at;
                 INSERT INTO diagnostics(id, kind, signature, count, first_seen, last_seen, app, os, model, payload, done) VALUES
                 (1, 'error', 'old1', 3, 10, 20, 'ios/100', '26', 'x', '{\"domain\":\"d\",\"code\":\"1\",\"place\":\"p\",\"app\":\"ios/100\"}', 1),
                 (2, 'error', 'old2', 2, 15, 30, 'ios/200', '26', 'x', '{\"domain\":\"d\",\"code\":\"1\",\"place\":\"p\",\"app\":\"ios/200\"}', 1),
                 (3, 'error', 'old3', 1, 40, 40, 'ios/200', '26', 'x', '{\"domain\":\"other\",\"code\":\"1\",\"place\":\"p\",\"app\":\"ios/200\"}', 0);
                 INSERT INTO diagnostic_installs VALUES (1, X'01', 0, NULL), (2, X'01', 0, 5), (2, X'02', 0, NULL);
                 INSERT INTO diagnostic_seen VALUES ('u1', 1);",
            )
            .unwrap();
        }
        let db = Db::open(path).unwrap();
        let rows = db.diagnostics().unwrap();
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        let merged = rows.iter().find(|r| r["id"] == 2).unwrap();
        assert_eq!(
            (merged["count"].as_i64(), merged["first_seen"].as_i64(), merged["last_seen"].as_i64()),
            (Some(5), Some(10), Some(30))
        );
        assert_eq!((merged["done"].as_bool(), merged["done_builds"].as_str()), (Some(true), Some("ios/200")));
        assert_eq!(merged["installs"], 2);
        assert_eq!(merged["builds"], serde_json::json!([["ios/100", 3, 20], ["ios/200", 2, 30]]));
        let seen: i64 = db.conn().query_row("SELECT diagnostic_id FROM diag_seen", [], |r| r.get(0)).unwrap();
        assert_eq!(seen, 2);
        let open = rows.iter().find(|r| r["id"] == 3).unwrap();
        assert_eq!((open["done"].as_bool(), open["done_builds"].is_null()), (Some(false), true));
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn diagnostics_count_installs_and_go_after_90_days_unseen() {
        let db = Db::open(":memory:").unwrap();
        let crash = serde_json::json!({"exception":"1/0/11", "frames":"Yo +0x1", "termination":"", "app":"ios/1", "os":"26", "model":"iPhone"});
        let day = 20_000 * DAY_MS + HOUR_MS;
        // The hub's own: no install.
        db.diagnostic("crash", &crash, None, day).unwrap();
        for (install, at) in [([1; 16], day), ([1; 16], day + 1), ([2; 16], day + 2), ([1; 16], day + DAY_MS)] {
            db.diagnostic("crash", &crash, Some(&install), at).unwrap();
        }
        let rows = db.diagnostics().unwrap();
        assert_eq!((rows[0]["count"].as_i64(), rows[0]["installs"].as_i64()), (Some(5), Some(2)));
        let left = |table: &str| -> i64 {
            db.conn().query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0)).unwrap()
        };
        let counts = |metric: &str| -> Vec<(i64, i64)> {
            db.conn()
                .prepare("SELECT hour, value FROM stat WHERE metric=?1 ORDER BY hour")
                .unwrap()
                .query_map([metric], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let h = (day / HOUR_MS) as i64;
        assert_eq!(counts("diag"), [(h, 4), (h + 24, 1)]);
        assert_eq!(left("diag_install"), 3, "one row per install and day");
        db.prune(day + 90 * DAY_MS, DAY_MS).unwrap();
        assert_eq!((left("diag"), left("diag_install")), (1, 3), "seen 90 days ago: kept");
        db.prune(day + 91 * DAY_MS, DAY_MS).unwrap();
        assert_eq!((left("diag"), left("diag_install")), (1, 1), "an install's day 90 days ago goes");
        db.prune(day + 92 * DAY_MS, DAY_MS).unwrap();
        assert_eq!((left("diag"), left("diag_install")), (0, 0));
        assert!(counts("diag").is_empty());
        assert_eq!(counts("diagnostics:crash"), [(h, 4), (h + 24, 1)]);
    }

    #[test]
    fn schema_9_moves_counts_once_and_keeps_recent_minutes() {
        let path = std::env::temp_dir().join(format!("yo-stats-{}-{}.sqlite", std::process::id(), crate::now_ms()));
        let minute = (crate::now_ms() / 60_000) as i64;
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "PRAGMA user_version=9;
                 CREATE TABLE levels(kind TEXT, hour INTEGER, value INTEGER);
                 INSERT INTO levels VALUES ('active-24h', 2, 7);
                 CREATE TABLE deliveries_hourly(hour INTEGER, n INTEGER);
                 INSERT INTO deliveries_hourly VALUES (3, 12);
                 CREATE TABLE diagnostic_hours(diagnostic_id INTEGER, hour INTEGER, count INTEGER);
                 INSERT INTO diagnostic_hours VALUES (42, 4, 9);
                 CREATE TABLE diagnostic_days(day INTEGER, kind TEXT, count INTEGER);
                 INSERT INTO diagnostic_days VALUES (5, 'error', 11);
                 CREATE TABLE settings(key TEXT PRIMARY KEY, value INTEGER);
                 INSERT INTO settings VALUES ('reserved_enabled', 0);
                 CREATE TABLE events(kind TEXT, minute INTEGER, n INTEGER, PRIMARY KEY(kind, minute)) WITHOUT ROWID;
                 INSERT INTO events VALUES ('deliveries', 180, 2), ('deliveries', 199, 3);",
            )
            .unwrap();
            conn.execute_batch(LEGACY_FEEDBACK).unwrap();
            conn.execute("INSERT INTO events VALUES ('yo-in', ?1, 4)", [minute]).unwrap();
        }
        for _ in 0..2 {
            let db = Db::open(path.to_str().unwrap()).unwrap();
            let conn = db.conn();
            assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 11);
            let rows: Vec<(String, String, i64, i64)> = conn
                .prepare("SELECT metric, key, hour, value FROM stat ORDER BY metric, key, hour")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            let row = |metric: &str, key: &str, hour, value| (metric.to_owned(), key.to_owned(), hour, value);
            assert_eq!(
                rows,
                [
                    row("active-24h", "", 2, 7),
                    row("deliveries", "", 3, 17),
                    row("diag", "42", 4, 9),
                    row("diagnostics:error", "", 120, 11),
                    row("yo-in", "", minute / 60, 4),
                ]
            );
            assert_eq!(
                conn.query_row("SELECT count(*), min(minute) FROM stat_minute", [], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                })
                .unwrap(),
                (1, minute)
            );
            assert!(!conn.prepare("SELECT 1 FROM sqlite_master WHERE name IN
                ('levels','deliveries_hourly','diagnostic_hours','diagnostic_days','settings','feedback','feedback_by_user')")
                .unwrap().exists([]).unwrap());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn schema_8_keeps_threads_and_reports_under_placeholder_installs() {
        let path = std::env::temp_dir().join(format!("yo-hub-installs-{}.db", std::process::id()));
        let path = path.to_str().unwrap();
        {
            let db = Db::open(path).unwrap();
            let conn = db.conn();
            as_schema_10(&conn);
            conn.execute_batch(
                "DROP TABLE support_messages; DROP TABLE reports;
                CREATE TABLE support_messages (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id BLOB NOT NULL,
                    from_team INTEGER NOT NULL, text TEXT NOT NULL, created_at INTEGER NOT NULL, read_at INTEGER,
                    app_version TEXT NOT NULL DEFAULT '', done_at INTEGER);
                CREATE INDEX support_by_user ON support_messages(user_id, created_at, id);
                INSERT INTO support_messages(user_id,from_team,text,created_at) VALUES
                    (X'01010101010101010101010101010101',0,'hi',1),(X'01010101010101010101010101010101',1,'yo',2),
                    (X'02020202020202020202020202020202',0,'other',3);
                CREATE TABLE reports (reporter_id BLOB NOT NULL, reported_id BLOB NOT NULL, reason INTEGER NOT NULL,
                    created_at INTEGER NOT NULL, PRIMARY KEY (reporter_id, reported_id)) WITHOUT ROWID;
                INSERT INTO reports VALUES (X'01010101010101010101010101010101', X'03', 1, 5);
                PRAGMA user_version=8;",
            )
            .unwrap();
        }
        for _ in 0..2 {
            let db = Db::open(path).unwrap();
            let conn = db.conn();
            assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 11);
            let threads: Vec<(Vec<u8>, i64)> = conn
                .prepare("SELECT install, count(*) FROM support GROUP BY install ORDER BY min(id)")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(threads.iter().map(|t| t.1).collect::<Vec<_>>(), [2, 1], "each thread stays one thread");
            assert!(threads.iter().all(|t| t.0.len() == 16 && t.0 != [1; 16] && t.0 != [2; 16]), "no account left");
            let reporter: Vec<u8> = conn.query_row("SELECT reporter FROM support_report", [], |r| r.get(0)).unwrap();
            assert!(reporter.len() == 16 && reporter != [1; 16]);
            assert!(conn
                .prepare("SELECT 1 FROM sqlite_master WHERE name='support_by_install'")
                .unwrap()
                .exists([])
                .unwrap());
        }
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{path}{suffix}"));
        }
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
        db.conn().execute("DELETE FROM account_prekey", []).unwrap();
        let found = db.lookup(KIND_USER, &leo, true, &[]).unwrap().unwrap();
        assert_eq!((found.devices.len(), found.bundles.len()), (1, 0), "a device without prekeys: no bundle");
    }

    fn reserve(db: &Db, names: &[&str]) -> usize {
        let names: Vec<Reserved> = names.iter().map(|n| Reserved { spelling: n.to_string(), note: None }).collect();
        db.add_reserved(&names, 0).unwrap()
    }

    /// The spelling a lookup by any spelling of the name answers.
    fn spelling(db: &Db, name: &str) -> Option<String> {
        let skeleton = yoip::net::handle::skeleton(name);
        db.lookup(KIND_USERNAME, skeleton.as_bytes(), false, &[]).unwrap().and_then(|found| found.username)
    }

    #[test]
    fn a_username_keeps_its_spelling_and_is_one_name_in_any() {
        let db = Db::open(":memory:").unwrap();
        let gpt = user(register(&db, b"YoGPT", 1, 1, 0));
        for other in ["yogpt", "YOGPT", "Y0GPT", "yoGpt"] {
            assert!(matches!(register(&db, other.as_bytes(), 2, 2, 0), Write::Refused(Conflict::UsernameTaken)));
            assert_eq!(spelling(&db, other).as_deref(), Some("YoGPT"), "{other}");
            assert!(db.username_taken(yoip::net::handle::skeleton(other).as_bytes()).unwrap());
        }
        assert_eq!(db.lookup(KIND_USER, &gpt, false, &[]).unwrap().unwrap().username.as_deref(), Some("YoGPT"));
        // A new phone registering the name in another spelling keeps the spelling the user has.
        assert_eq!(user(register(&db, b"yogpt", 1, 3, 0)), gpt);
        assert_eq!(spelling(&db, "YoGPT").as_deref(), Some("YoGPT"));
    }

    #[test]
    fn a_reserved_name_cannot_be_claimed() {
        let db = Db::open(":memory:").unwrap();
        reserve(&db, &["Admin", "Support"]);
        assert!(db.username_taken(b"ADMIN").unwrap());
        assert!(matches!(register(&db, b"Admin", 2, 2, 0), Write::Refused(Conflict::UsernameTaken)));
        db.remove_reserved("Adm1n").unwrap();
        assert!(!db.username_taken(b"ADMIN").unwrap());
        assert_eq!(db.reserved_state().unwrap()["names"], serde_json::json!(["Support"]));
    }

    #[test]
    fn events_keep_two_hours_of_minutes_and_every_hour() {
        let db = Db::open(":memory:").unwrap();
        let minute = 60_000u64;
        db.count_events(10 * minute + 5, &[("yo-in", 2), ("media-in", 1)]).unwrap();
        db.count_events(10 * minute + 59_999, &[("yo-in", 3)]).unwrap();
        db.count_events(11 * minute, &[("yo-out", 4)]).unwrap();
        let rows = |db: &Db| -> Vec<(String, i64, i64)> {
            db.conn()
                .prepare("SELECT kind, minute, n FROM stat_minute ORDER BY minute, kind")
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
            .prepare("SELECT name FROM pragma_table_info('stat_minute')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(columns, ["kind", "minute", "n"], "no identifier of anyone or anything");
        db.count_events(131 * minute, &[]).unwrap();
        assert_eq!(rows(&db), [row("yo-out", 11, 4)]);
        assert_eq!(
            db.conn()
                .query_row("SELECT value FROM stat WHERE metric='yo-in' AND key='' AND hour=0", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            5
        );
    }

    /// When a device or an install was last heard from is kept to the hour (spec 1), never to the millisecond; the
    /// hourly levels' 24-hour windows still count a phone seen 23 hours ago, and one seen 25 hours ago no more.
    #[test]
    fn last_seen_is_kept_to_the_hour_and_the_daily_windows_still_count() {
        let db = Db::open(":memory:").unwrap();
        let hour = HOUR_MS;
        let now = 1000 * DAY + 5 * hour + 1_234_567;
        let seen = |n: u8| db.device(&[n; 16]).unwrap().unwrap().last_seen_at;
        assert!(matches!(register(&db, b"alice", 1, 1, now - 23 * hour), Write::Done(_)));
        assert!(matches!(register(&db, b"bob", 2, 2, now - 25 * hour), Write::Done(_)));
        assert_eq!(seen(1), to_the_hour(now - 23 * hour));
        assert_eq!(seen(1) % hour, 0, "registered: to the hour");
        db.touch(&[2; 16], now - 25 * hour + 59_999, Some(("ios/1", None)), false).unwrap();
        assert_eq!(seen(2), to_the_hour(now - 25 * hour), "touched: to the hour");
        db.touch(&[1; 16], now - 23 * hour + 1, Some(("ios/1", None)), false).unwrap();
        let install = [7u8; 16];
        let payload =
            serde_json::json!({"app": "ios/1", "os": "26", "model": "x", "domain": "d", "code": "1", "place": "p"});
        db.diagnostic("error", &payload, Some(&install), now - 23 * hour).unwrap();
        let at: i64 = db.conn().query_row("SELECT at FROM diag_install", [], |r| r.get(0)).unwrap();
        assert_eq!(at as u64, to_the_hour(now - 23 * hour), "an install's report: to the hour");
        db.sample_levels(now, false).unwrap();
        let level = |kind: &str| -> i64 {
            db.conn()
                .query_row(
                    "SELECT value FROM stat WHERE key = '' AND metric = ?1 AND hour = ?2",
                    params![kind, (now / hour) as i64],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!((level("phones-24h"), level("phones-24h:ios"), level("build:ios/1")), (1, 1, 2), "build: 48 hours");
        assert_eq!((level("active-24h"), level("active-48h")), (1, 2));
        assert_eq!(level("problem-installs-24h"), 1);
    }

    #[test]
    fn levels_keep_every_hour_for_good() {
        let db = Db::open(":memory:").unwrap();
        let hour = 3_600_000u64;
        let now = 1000 * DAY + 5 * hour + 30_000;
        assert!(matches!(register(&db, b"alice", 1, 1, now - 2 * DAY), Write::Done(_)));
        assert!(matches!(register(&db, b"bob", 2, 2, now - hour), Write::Done(_)));
        let conn = db.conn();
        for (kind, signature, count, done) in [("error", "a", 3, 0), ("error", "b", 4, 1), ("crash", "c", 2, 0)] {
            conn.execute(
                "INSERT INTO diag (kind, signature, count, first_seen, last_seen, app, os, model, payload, done)
                 VALUES (?1, ?2, ?3, 0, 0, '', '', '', '{}', ?4)",
                params![kind, signature, count, done],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO message (device_id, envelope, received_at) VALUES (X'01', X'00', 0)", []).unwrap();
        conn.execute(
            "INSERT INTO message_blob (id, user_id, size, fetches_left, created_at) VALUES (X'01', X'01', 700, 1, 0)",
            [],
        )
        .unwrap();
        drop(conn);
        let rows = |db: &Db| -> Vec<(String, i64, i64)> {
            db.conn()
                .prepare("SELECT metric, hour, value FROM stat WHERE key = '' ORDER BY metric, hour")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let h = (now / hour) as i64;
        let row = |kind: &str, hour, value| (kind.to_owned(), hour, value);
        db.sample_levels(now, false).unwrap();
        assert_eq!(
            rows(&db),
            [
                row("active-24h", h, 1),
                row("active-30d", h, 2),
                row("active-48h", h, 1),
                row("build:unknown", h, 1),
                row("open-crashes", h, 2),
                row("open-errors", h, 3),
                row("phones-24h", h, 0),
                row("phones-24h:android", h, 0),
                row("phones-24h:ios", h, 0),
                row("problem-installs-24h", h, 0),
                row("problem-installs-24h:android", h, 0),
                row("problem-installs-24h:ios", h, 0),
                row("queue-envelopes", h, 1),
                row("queue-media-bytes", h, 700),
            ]
        );
        // The hourly sample keeps the hour's first; a problem marked done replaces the open problems of the hour.
        db.diagnostic_done(1, true, 0).unwrap();
        db.sample_levels(now + 60_000, false).unwrap();
        assert_eq!(rows(&db)[5], row("open-errors", h, 3));
        db.sample_levels(now + 60_000, true).unwrap();
        assert_eq!(rows(&db)[5], row("open-errors", h, 0));
        assert_eq!(rows(&db).len(), 14);
        let columns: Vec<String> = db
            .conn()
            .prepare("SELECT name FROM pragma_table_info('stat')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(columns, ["metric", "key", "hour", "value"]);
        // A bot's device (the CLI, YoGPT) is no active account: bob on the CLI leaves no one seen within 48 hours.
        db.conn()
            .execute(
                "UPDATE account_device SET app = 'cli/1' WHERE last_seen_at = ?1",
                [to_the_hour(now - hour) as i64],
            )
            .unwrap();
        db.sample_levels(now + hour, false).unwrap();
        let bots: i64 = db
            .conn()
            .query_row("SELECT value FROM stat WHERE key = '' AND metric = 'active-48h' AND hour = ?1", [h + 1], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(bots, 0);
        // Devices per build seen within 48 hours, the apps' alone: bob on the CLI is no build, an iOS device is.
        let build = |db: &Db, hour: i64| -> Vec<(String, i64)> {
            db.conn()
                .prepare("SELECT metric, value FROM stat WHERE key = '' AND metric LIKE 'build:%' AND hour = ?1 ORDER BY metric")
                .unwrap()
                .query_map([hour], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(build(&db, h + 1), []);
        db.conn()
            .execute("UPDATE account_device SET app = 'ios/202611191626', last_seen_at = ?1", [now as i64])
            .unwrap();
        db.sample_levels(now + 2 * hour, false).unwrap();
        assert_eq!(build(&db, h + 2), [("build:ios/202611191626".to_owned(), 2)]);
        // The error-free share: an install with an error counts, one with only the network's (a timeout) does not.
        let report = |id: i64, install: u8, payload: &str| {
            db.conn()
                .execute(
                    "INSERT INTO diag(id, kind, signature, count, first_seen, last_seen, app, os, model, payload, done) \
                     VALUES (?1, 'error', ?1, 1, ?2, ?2, 'ios/202611191626', '26', 'iPhone', ?3, 0)",
                    params![id, now as i64, payload],
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO diag_install(diagnostic_id, install, day, at) VALUES (?1, ?2, ?3, ?4)",
                    params![id, [install], (now / DAY_MS) as i64, now as i64],
                )
                .unwrap();
        };
        report(91, 1, r#"{"domain":"NSURLErrorDomain","code":"-1001"}"#);
        report(92, 2, r#"{"domain":"YoCore.YoError","code":"bad"}"#);
        report(93, 3, r#"{"domain":"SharedAccess","code":"a slow request"}"#);
        db.sample_levels(now + 3 * hour, false).unwrap();
        let level = |kind: &str| -> i64 {
            db.conn()
                .query_row(
                    "SELECT value FROM stat WHERE key = '' AND metric = ?1 AND hour = ?2",
                    params![kind, h + 3],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(
            (level("phones-24h:ios"), level("problem-installs-24h"), level("problem-installs-24h:ios")),
            (2, 1, 1)
        );
        assert_eq!((level("phones-24h:android"), level("problem-installs-24h:android")), (0, 0));
        let hourly = rows(&db);
        db.prune(now + 1000 * DAY, DAY).unwrap();
        assert_eq!(rows(&db), hourly);
    }

    #[test]
    fn groups_made_before_created_at_keep_null() {
        let path = std::env::temp_dir().join(format!("yo-groups-created-{}.sqlite", std::process::id()));
        let path = path.to_str().unwrap();
        let _ = std::fs::remove_file(path);
        {
            let db = Db::open(path).unwrap();
            as_schema_10(&db.conn());
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
            .prepare("SELECT created_at FROM grp ORDER BY id")
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
        assert!(db.username_taken(yoip::net::handle::skeleton("open").as_bytes()).unwrap());
    }

    #[test]
    fn reserved_names_are_taken_in_any_spelling_but_stay_their_holders() {
        let db = Db::open(":memory:").unwrap();
        let mod_user = user(register(&db, b"Moderator", 1, 1, 0));
        assert_eq!(reserve(&db, &["Admin", "Moderator", "Support"]), 3);
        assert_eq!(reserve(&db, &["ADMIN", "Admin", "Help"]), 1, "seeding again adds only what is new");
        let count: i64 = db.conn().query_row("SELECT count(*) FROM name_reserved", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 4);
        for name in ["admin", "ADMIN", "Adm1n", "ÄDMIN", "help"] {
            assert!(
                matches!(register(&db, name.as_bytes(), 2, 2, 0), Write::Refused(Conflict::UsernameTaken)),
                "{name}"
            );
            assert!(db.username_taken(yoip::net::handle::skeleton(name).as_bytes()).unwrap(), "{name}");
        }
        // Who had the name before it was reserved keeps it: found, registered again on a new phone.
        assert_eq!(spelling(&db, "moderator").as_deref(), Some("Moderator"));
        assert_eq!(user(register(&db, b"Moderator", 1, 4, 0)), mod_user);
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
            let schema = SCHEMA_10
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
        let skeleton = yoip::net::handle::skeleton("Christopher");
        let again =
            db.register(skeleton.as_bytes(), "Christopher", &[9; 16], &curve, &device(9), &prekeys(0), 100, None, 0);
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
    fn add_devices_adds_up_to_sixteen_and_never_retires_one_by_itself() {
        let mut db = Db::open(":memory:").unwrap();
        db.add_devices = true;
        let add =
            |n, now| db.register(b"test", "Test", &[1; 16], &[2; 32], &device(n), &prekeys(0), 100, None, now).unwrap();
        for n in 1..=16 {
            assert!(matches!(add(n, 0), Write::Done((_, _, slot)) if slot == n));
        }
        let keys = (1..=16).map(|n| device(n).device_key).collect::<Vec<_>>();
        let devices_hash = yoip::net::devices::hash(&[2; 32], &keys);
        db.deliver(&[Incoming { user: [1; 16], devices_hash, from: None, copies: Copies::Legacy(b"queued") }], 0, 100)
            .unwrap();
        // The 17th: refused, nothing retired, every device keeps its mail.
        assert!(matches!(add(17, 7_200_000), Write::Refused(Conflict::DeviceLimit)));
        assert!(matches!(add(2, 7_200_001), Write::Refused(Conflict::DeviceTaken)));
        let found = db.lookup(KIND_USER, &[1; 16], true, &[]).unwrap().unwrap();
        assert_eq!(found.devices.len(), 16);
        assert_eq!(found.bundles.len(), 16);
        for n in 1..=16 {
            assert!(!db.is_gone(&device(n).device_id).unwrap());
            assert_eq!(db.fetch(&device(n).device_id, 10).unwrap().len(), 1);
        }
        assert!(db.device(&device(17).device_id).unwrap().is_none());
        assert_eq!(db.devices_of_user(&[1; 16]).unwrap().len(), 16);

        // The owner's choice: the 17th in place of number 5, which is gone (410) with its mail.
        let replacing = db
            .register(b"test", "Test", &[1; 16], &[2; 32], &device(17), &prekeys(0), 100, Some(5), 7_200_002)
            .unwrap();
        assert!(matches!(replacing, Write::Done((_, _, 5))));
        assert!(db.is_gone(&device(5).device_id).unwrap());
        assert!(db.fetch(&device(5).device_id, 10).unwrap().is_empty());
        assert_eq!(db.devices_of_user(&[1; 16]).unwrap().len(), 16);

        db.add_devices = false;
        let replaced =
            db.register(b"test", "Test", &[1; 16], &[2; 32], &device(18), &prekeys(0), 100, None, 8_000_000).unwrap();
        assert!(matches!(replaced, Write::Done((_, _, 1))));
        assert_eq!(db.lookup(KIND_USER, &[1; 16], false, &[]).unwrap().unwrap().devices.len(), 1);
    }

    /// Each device stores only its own envelope (spec 3.2, Signal's per-device messages); to its own user the
    /// sending device is left out of the check and gets nothing; numbers that are not the roster's store nothing.
    #[test]
    fn each_device_gets_its_own_copy_and_the_sender_none_of_its_own() {
        let mut db = Db::open(":memory:").unwrap();
        db.add_devices = true;
        for n in 1..=3 {
            db.register(b"test", "Test", &[1; 16], &[2; 32], &device(n), &prekeys(0), 100, None, 0).unwrap();
        }
        let keys = |ns: &[u8]| ns.iter().map(|n| device(*n).device_key).collect::<Vec<_>>();
        let hash = |ns: &[u8]| yoip::net::devices::hash(&[2; 32], &keys(ns));
        let each = |ns: &[u8]| Copies::Each(ns.iter().map(|n| (*n, &b"for each"[..*n as usize])).collect());
        let deliver = |hash, from, copies| {
            db.deliver(&[Incoming { user: [1; 16], devices_hash: hash, from, copies }], 0, 100).unwrap().remove(0)
        };
        assert!(matches!(deliver(hash(&[1, 2, 3]), None, each(&[1, 2, 3])), Delivered::Stored(d) if d.len() == 3));
        for n in 1..=3u8 {
            assert_eq!(db.fetch(&device(n).device_id, 10).unwrap()[0].1, &b"for each"[..n as usize]);
        }
        // Device 2 to its others: checked and stored without it.
        let from = Some(device(2).device_id);
        assert!(matches!(deliver(hash(&[1, 2, 3]), from, each(&[1, 3])), Delivered::Mismatch(_)));
        assert!(matches!(deliver(hash(&[1, 3]), from, each(&[1, 2, 3])), Delivered::Mismatch(_)));
        assert!(matches!(deliver(hash(&[1, 3]), from, each(&[1, 3])), Delivered::Stored(d) if d.len() == 2));
        assert_eq!(db.fetch(&device(2).device_id, 10).unwrap().len(), 1);
        assert_eq!(db.fetch(&device(3).device_id, 10).unwrap().len(), 2);
        // Some devices only (an answer to one device's decryption error): the roster known, stored for those.
        assert!(matches!(deliver(hash(&[1, 2, 3]), None, each(&[2])), Delivered::Stored(d) if d.len() == 1));
        assert_eq!(db.fetch(&device(2).device_id, 10).unwrap().len(), 2);
        assert!(matches!(deliver(hash(&[1, 2, 3]), None, each(&[4])), Delivered::Mismatch(_)));
        assert!(matches!(deliver(hash(&[1, 2]), None, each(&[1, 2])), Delivered::Mismatch(_)));
    }

    /// A registration has no time or nonce (spec 4.2): an old one played back (a TLS proxy's log, years later) must
    /// never register its long-replaced device again and so retire the current one. `gone` is kept for good.
    #[test]
    fn an_old_registration_played_back_years_later_is_still_refused() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        assert_eq!(user(register(&db, b"LEO", 1, 3, 10)), leo, "a new phone: device 1 replaced");
        assert!(matches!(register(&db, b"LEO", 1, 1, 20), Write::Refused(Conflict::DeviceTaken)));
        let years = 5 * 365 * DAY_MS;
        for now in [365 * DAY_MS + DAY_MS + 10, years] {
            db.prune(now, 30 * DAY_MS).unwrap();
            assert!(matches!(register(&db, b"LEO", 1, 1, now + 1), Write::Refused(Conflict::DeviceTaken)));
            assert_eq!(db.device(&[3; 16]).unwrap().unwrap().user_id, leo, "the current device stays");
            assert!(db.is_gone(&[1; 16]).unwrap(), "the old one still answers 410");
        }
        // A deleted account's devices too: its old registration cannot come back as a new account.
        assert!(db.delete_account(&[3; 16], years).is_ok());
        db.prune(years + 2 * 365 * DAY_MS, 30 * DAY_MS).unwrap();
        for n in [1, 3] {
            assert!(matches!(
                register(&db, b"LEO", 1, n, years + 2 * 365 * DAY_MS),
                Write::Refused(Conflict::DeviceTaken)
            ));
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

    /// U1: a name kept under the skeleton of before (with U+200D or tags in it) gets today's at the next start and is
    /// found again; one whose new skeleton another name has keeps its old one.
    #[test]
    fn names_get_todays_skeleton_at_start() {
        let db = Db::open(":memory:").unwrap();
        let (joined, apart) = ("Leo\u{1F468}\u{200D}\u{1F469}", "LEO\u{1F468}\u{1F469}");
        let old_skeleton = |name: &str| name.to_uppercase().into_bytes();
        let leo = user(register(&db, b"MIA", 1, 1, 0));
        let taken = user(register(&db, b"TOM", 2, 2, 0));
        db.conn()
            .execute(
                "UPDATE account_name SET value = ?1, spelling = ?2 WHERE user_id = ?3",
                params![old_skeleton(joined), joined, leo],
            )
            .unwrap();
        db.conn()
            .execute(
                "UPDATE account_name SET value = ?1, spelling = ?2 WHERE user_id = ?3",
                params![yoip::net::handle::skeleton(apart).into_bytes(), apart, taken],
            )
            .unwrap();
        let other = user(register(&db, b"ZOE", 3, 3, 0));
        let second = "Zoe\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}";
        db.conn()
            .execute(
                "UPDATE account_name SET value = ?1, spelling = ?2 WHERE user_id = ?3",
                params![old_skeleton(second), second, other],
            )
            .unwrap();
        schema::upgrade(&mut db.conn()).unwrap();
        let owner = |name: &str| db.username_owner(yoip::net::handle::skeleton(name).as_bytes()).unwrap();
        assert_eq!(owner(second), Some(other), "today's skeleton: found");
        assert_eq!(owner(apart), Some(taken), "the name that had it keeps it");
        assert_eq!(owner(joined), Some(taken), "one skeleton, one name");
        let kept: Vec<u8> =
            db.conn().query_row("SELECT value FROM account_name WHERE user_id = ?1", [leo], |r| r.get(0)).unwrap();
        assert_eq!(kept, old_skeleton(joined), "the later one keeps its old skeleton");
        schema::upgrade(&mut db.conn()).unwrap();
    }

    /// A backup knows the name the account had when it was made. Renamed since, its name taken by someone else or
    /// free again: the account key brings back the same user, with the name it has now (spec 4.2, 2.4).
    #[test]
    fn a_restore_finds_its_user_by_the_account_key_whatever_name_it_had() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"Leon", 1, 1, 0));
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
        db.conn().execute("DELETE FROM account_name WHERE user_id = ?1", [leo]).unwrap();
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

    /// A phone signed out of one account and into another (or the same app's token set again) keeps one device woken
    /// by its token: the device left behind is not pushed to with it any more. Another platform's same bytes, and an
    /// empty token, are left alone.
    #[test]
    fn a_push_token_wakes_one_device() {
        let db = Db::open(":memory:").unwrap();
        let token = |n: u8| -> (u8, Vec<u8>) {
            db.conn()
                .query_row("SELECT push_platform, push_token FROM account_device WHERE id = ?1", [[n; 16]], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .unwrap()
        };
        user(register(&db, b"ANA", 1, 1, 0));
        user(register(&db, b"BEN", 2, 2, 0));
        user(register(&db, b"CAT", 3, 3, 0));
        assert!(matches!(db.set_push(&[1; 16], 1, b"phone").unwrap(), Write::Done(())));
        assert!(matches!(db.set_push(&[3; 16], 2, b"phone").unwrap(), Write::Done(())));
        // Ben on the same phone: Ana's device no longer holds its token; Cat's (another platform) keeps hers.
        assert!(matches!(db.set_push(&[2; 16], 1, b"phone").unwrap(), Write::Done(())));
        assert_eq!(token(1), (0, Vec::new()));
        assert_eq!(token(2), (1, b"phone".to_vec()));
        assert_eq!(token(3), (2, b"phone".to_vec()));
        // A registration that brings the token takes it as well.
        let mut dan = device(4);
        (dan.push_platform, dan.push_token) = (1, b"phone".to_vec());
        let skeleton = yoip::net::handle::skeleton("DAN");
        db.register(skeleton.as_bytes(), "DAN", &[4; 16], &[4; 32], &dan, &prekeys(0), 100, None, 0).unwrap();
        assert_eq!(token(2), (0, Vec::new()));
        assert_eq!(token(4), (1, b"phone".to_vec()));
        // Signing out sets no token: nobody else's is cleared.
        assert!(matches!(db.set_push(&[2; 16], 0, b"").unwrap(), Write::Done(())));
        assert_eq!(token(4), (1, b"phone".to_vec()));
    }

    /// What a device writes for its user is checked against the device in the write's own transaction: one replaced
    /// after it authenticated (a new phone restored the account, or the account was deleted) changes nothing.
    #[test]
    fn a_replaced_device_writes_nothing_for_its_user() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        user(register(&db, b"LEO", 1, 2, 0));
        let gone = |w: Write<()>| matches!(w, Write::Refused(Conflict::DeviceGone));
        assert_eq!(spelling(&db, "LEO").as_deref(), Some("LEO"), "the name stays the new phone's");
        assert!(gone(db.add_prekeys(&[1; 16], &prekeys(1), 100).unwrap()));
        assert!(gone(db.set_push(&[1; 16], 1, b"ab").unwrap()));
        // Deleted: nothing is written for a user that is no more.
        assert!(matches!(db.delete_account(&[2; 16], 1).unwrap(), Write::Done(())));
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
            as_schema_10(&db.conn());
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
        // Its two fetches and three more (spec 4.3).
        for _ in 0..5 {
            assert_eq!(db.take_blob_get(&id, 5, 100).unwrap(), Some((100, 5)));
        }
        assert_eq!(db.take_blob_get(&id, 5, 100).unwrap(), None);
        db.recover_blobs().unwrap();
        assert_eq!(db.take_blob_get(&id, 5, 100).unwrap(), None, "recovery never resets budget");
        db.return_blob_get(&id, 5).unwrap();
        assert_eq!(db.take_blob_get(&id, 5, 100).unwrap(), Some((100, 5)), "cut-off slot returned");
        assert_eq!(db.ack_blob(&id, 5, 100).unwrap(), None, "one fetch remains despite five GETs");
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
        for _ in 0..5 {
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
        let devices_hash = yoip::net::devices::hash(&[1; 32], &[[1; 32]]);
        db.deliver(&[Incoming { user: leo, devices_hash, from: None, copies: Copies::Legacy(b"envelope") }], 0, 10)
            .unwrap();
        let synchronous = || -> i64 { db.conn().query_row("PRAGMA synchronous", [], |row| row.get(0)).unwrap() };
        assert_eq!(synchronous(), 1);
        assert_eq!(db.conn().query_row("PRAGMA wal_autocheckpoint", [], |row| row.get::<_, i64>(0)).unwrap(), 16384);
        let (id, _, _) = db.fetch(&[1; 16], 10).unwrap().remove(0);
        assert_eq!(db.ack(&[1; 16], &[id]).unwrap(), (vec![0], false), "when it was accepted");
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
        let devices_hash = yoip::net::devices::hash(&[1; 32], &[[1; 32]]);
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
        let delivered = db
            .deliver(&[Incoming { user: leo, devices_hash, from: None, copies: Copies::Legacy(b"envelope") }], 0, 10)
            .unwrap();
        assert!(matches!(&delivered[..], [Delivered::Stored(_)]));
        let ids: Vec<u64> = db.fetch(&[1; 16], 10).unwrap().into_iter().map(|(id, _, _)| id).collect();
        let committer = commit_later(committer.join().unwrap());
        assert_eq!(db.ack(&[1; 16], &ids).unwrap().0.len(), 1);
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
        db.touch(&[1; 16], 0, Some(("cli/7", None)), false).unwrap();
        let devices_hash = yoip::net::devices::hash(&[1; 32], &[[1; 32]]);
        let delivered = db
            .deliver(&[Incoming { user: bot, devices_hash, from: None, copies: Copies::Legacy(b"envelope") }], 0, 10)
            .unwrap();
        assert!(matches!(&delivered[..], [Delivered::Stored(devices)] if devices.iter().all(|w| w.bot)));
    }

    #[test]
    fn the_bot_mark_is_one_way() {
        let db = Db::open(":memory:").unwrap();
        register(&db, b"BOT", 1, 1, 0);
        assert!(db.set_bot(&[1; 16], false).unwrap().done().unwrap(), "0 on a non-bot is fine");
        assert!(db.set_bot(&[1; 16], true).unwrap().done().unwrap());
        assert!(!db.set_bot(&[1; 16], false).unwrap().done().unwrap(), "0 on a bot is refused");
        assert!(db.set_bot(&[1; 16], true).unwrap().done().unwrap(), "1 again stays fine");
    }

    #[test]
    fn a_devices_version_follows_its_app() {
        let db = Db::open(":memory:").unwrap();
        register(&db, b"ANN", 1, 1, 0);
        let version = || db.device(&[1; 16]).unwrap().map(|d| (d.app, d.version)).unwrap();
        db.touch(&[1; 16], 0, Some(("ios/202611241344", Some("1.0.0-RC36"))), false).unwrap();
        assert_eq!(version(), (Some("ios/202611241344".into()), Some("1.0.0-RC36".into())));
        db.touch(&[1; 16], 0, None, false).unwrap();
        assert_eq!(version().1.as_deref(), Some("1.0.0-RC36"), "no app named: both stay");
        db.touch(&[1; 16], 0, Some(("ios/202611250000", None)), false).unwrap();
        assert_eq!(version(), (Some("ios/202611250000".into()), None), "a build without one has none");
    }

    /// Exit without dropping SQLite (no close-time checkpoint): the committed WAL survives a process crash.
    #[test]
    fn committed_delivery_survives_process_exit() {
        const CHILD_DB: &str = "YO_DB_CRASH_TEST";
        if let Ok(path) = std::env::var(CHILD_DB) {
            let db = Db::open(&path).unwrap();
            let leo = user(register(&db, b"LEO", 1, 1, 0));
            let devices_hash = yoip::net::devices::hash(&[1; 32], &[[1; 32]]);
            db.deliver(
                &[Incoming { user: leo, devices_hash, from: None, copies: Copies::Legacy(b"committed") }],
                0,
                10,
            )
            .unwrap();
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

    /// Schema 10 to 11: every table under its area's name, its rows, indexes and trigger with it (`schema::RENAMED`).
    #[test]
    fn schema_10_reaches_11_with_every_row_under_the_new_names() {
        let path = std::env::temp_dir().join(format!("yo-hub-schema-11-{}.sqlite", std::process::id()));
        let path = path.to_str().unwrap().to_owned();
        let _ = std::fs::remove_file(&path);
        let count = |conn: &Connection, table: &str| -> i64 {
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap()
        };
        let before: Vec<i64> = {
            let db = Db::open(&path).unwrap();
            let leo = user(register(&db, b"LEO", 1, 1, 0));
            user(register(&db, b"MIA", 2, 2, 0));
            db.put_backup(&leo, Some(&[9; 16]), b"record", &BackupAuth::default(), 0).unwrap();
            let incoming = Incoming {
                user: leo,
                devices_hash: yoip::net::devices::hash(&[1; 32], &[[1; 32]]),
                from: None,
                copies: Copies::Legacy(b"envelope"),
            };
            db.deliver(&[incoming], 0, 10).unwrap();
            db.report(&[5; 16], &leo, 1, 0).unwrap();
            db.support_message(&[5; 16], "hi", "1", 0).unwrap();
            db.diagnostic("crash", &legacy_crash("trace", "1"), Some(&[5; 16]), 7).unwrap();
            reserve(&db, &["Admin"]);
            db.count_events(crate::now_ms(), &[("yo-in", 1)]).unwrap();
            let quota = BlobQuota { per_day: 10, bytes_per_day: 1 << 20, stored: 1 << 20 };
            db.put_blob(&leo, &[9; 16], 100, 2, 5, &quota).unwrap();
            assert!(db.create_group(&[2], &[], &[], 7).unwrap());
            let conn = db.conn();
            conn.execute_batch(
                "INSERT INTO account_device_gone VALUES (X'02', 1);
                 INSERT INTO grp_gone VALUES (X'03', 1);
                 INSERT INTO diag_seen VALUES ('u1', 1);
                 INSERT INTO stat_request VALUES (X'01', 1, 1);",
            )
            .unwrap();
            conn.execute("INSERT INTO account_attestation VALUES (?1, 'attested', 'ios', NULL, NULL, 1)", [leo])
                .unwrap();
            conn.execute("INSERT INTO account_storage VALUES (?1, 1, X'00', 1)", [leo]).unwrap();
            conn.execute("INSERT OR IGNORE INTO account_contact_key VALUES (?1, X'00', 1)", [leo]).unwrap();
            conn.execute("INSERT OR IGNORE INTO account_profile VALUES (?1, X'00')", [leo]).unwrap();
            let before = schema::RENAMED.iter().map(|(_, new)| count(&conn, new)).collect();
            as_schema_10(&conn);
            before
        };
        for (n, (old, _)) in before.iter().zip(schema::RENAMED) {
            assert!(*n > 0, "{old}: a row to move");
        }
        for _ in 0..2 {
            let db = Db::open(&path).unwrap();
            let conn = db.conn();
            assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 11);
            for (n, (old, new)) in before.iter().zip(schema::RENAMED) {
                assert_eq!(count(&conn, new), *n, "{new}");
                assert!(!conn.prepare("SELECT 1 FROM sqlite_master WHERE name = ?1").unwrap().exists([old]).unwrap());
            }
            let names: Vec<String> = conn
                .prepare("SELECT name FROM sqlite_master WHERE type IN ('index', 'trigger') AND sql IS NOT NULL ORDER BY name")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(
                names,
                [
                    "account_device_by_user",
                    "account_key_by_user",
                    "account_name_by_user",
                    "diag_install_by_day",
                    "diag_recent",
                    "diag_stat_deleted",
                    "message_blob_by_age",
                    "message_blob_by_user",
                    "message_by_age",
                    "message_by_device",
                    "support_by_install",
                    "support_report_by_reported",
                ]
            );
            let seq: Vec<String> = conn
                .prepare("SELECT name FROM sqlite_sequence ORDER BY name")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(seq, ["message", "support"]);
            let fresh = Db::open(":memory:").unwrap();
            let columns = |conn: &Connection, table: &str| -> Vec<String> {
                let mut names: Vec<String> = conn
                    .prepare("SELECT name FROM pragma_table_info(?1)")
                    .unwrap()
                    .query_map([table], |r| r.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                names.sort();
                names
            };
            for (_, new) in schema::RENAMED {
                assert_eq!(columns(&conn, new), columns(&fresh.conn(), new), "{new}: as a new hub makes it");
            }
            drop(conn);
            assert_eq!(db.audit().unwrap(), []);
            assert_eq!(db.support_thread(&[5; 16]).unwrap().messages.len(), 1);
        }
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{path}{suffix}"));
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
                devices_hash: yoip::net::devices::hash(&[1; 32], &[[1; 32]]),
                from: None,
                copies: Copies::Legacy(b"envelope"),
            };
            db.deliver(&[incoming], 0, 10).unwrap();
            let reported = |install: &Install, whom: &UserId| db.report(install, whom, 1, 0).unwrap();
            assert_eq!((reported(&[5; 16], &mia), reported(&[6; 16], &leo)), (true, true));
            assert!(matches!(db.support_message(&[5; 16], "more colors", "1.0 (1)", 0).unwrap(), Write::Done(())));
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
            assert_eq!(
                (left("support_report"), left("account_prekey"), left("account_key")),
                (1, 2, 1),
                "only MIA's are left"
            );
            assert_eq!(left("support"), 1, "help by install id names no account and stays");
            assert_eq!(db.audit().unwrap(), [], "nothing of LEO's is left behind");
            assert!(matches!(db.delete_account(&[1; 16], 1).unwrap(), Write::Refused(Conflict::DeviceGone)));
            // The name and the key are free again.
            user(register(&db, b"LEO", 1, 3, 2));
            assert!(!reported(&[6; 16], &leo), "no such user any more");
        }
    }

    #[test]
    fn support_is_kept_as_written_with_install_and_when() {
        let db = Db::open(":memory:").unwrap();
        let phone = [3; 16];
        assert!(matches!(db.support_message(&phone, "Mehr Farben, bitte 🎨", "1.2 (34)", 5).unwrap(), Write::Done(())));
        assert!(matches!(db.support_message(&phone, "again", "1.2 (35)", 6).unwrap(), Write::Done(())));
        let rows: Vec<(i64, Vec<u8>, String, String, i64)> = db
            .conn()
            .prepare("SELECT id, install, text, app_version, created_at FROM support ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                (1, phone.to_vec(), "Mehr Farben, bitte 🎨".to_owned(), "1.2 (34)".to_owned(), 5),
                (2, phone.to_vec(), "again".to_owned(), "1.2 (35)".to_owned(), 6),
            ]
        );
    }

    #[test]
    fn schema_7_with_feedback_reaches_10_with_its_rows_moved_and_the_table_gone() {
        let path = std::env::temp_dir().join(format!("yo-hub-feedback-{}.db", std::process::id()));
        let path = path.to_str().unwrap();
        let leo = {
            let db = Db::open(path).unwrap();
            let leo = user(register(&db, b"LEO", 1, 1, 0));
            let conn = db.conn();
            as_schema_10(&conn);
            conn.execute_batch(LEGACY_FEEDBACK).unwrap();
            conn.execute(
                "INSERT INTO feedback(user_id,text,app_version,created_at,auto,type) VALUES
                (?1,'Mehr Farben 🎨','ios/7',100,0,'feedback'),(X'','trace','ios/7',101,1,'crash'),
                (X'','a task','admin',102,0,'todo')",
                [leo],
            )
            .unwrap();
            // Schema 7 had help by account.
            conn.execute_batch(
                "ALTER TABLE support_messages RENAME COLUMN install TO user_id;
                ALTER TABLE reports RENAME COLUMN reporter TO reporter_id; PRAGMA user_version=7",
            )
            .unwrap();
            leo
        };
        for _ in 0..2 {
            let db = Db::open(path).unwrap();
            let conn = db.conn();
            assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 11);
            assert!(!conn
                .prepare("SELECT 1 FROM sqlite_master WHERE name LIKE 'feedback%'")
                .unwrap()
                .exists([])
                .unwrap());
            let message: (Vec<u8>, String, String, i64) = conn
                .query_row("SELECT install,text,app_version,created_at FROM support", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .unwrap();
            // Schema 9: kept under a random placeholder install id, no longer the account's.
            assert_eq!(
                (message.0.len(), &message.1[..], &message.2[..], message.3),
                (16, "Mehr Farben 🎨", "ios/7", 100)
            );
            assert_ne!(message.0, leo.to_vec());
            assert_eq!(conn.query_row("SELECT text FROM todos", [], |r| r.get::<_, String>(0)).unwrap(), "a task");
            assert_eq!(conn.query_row("SELECT first_seen FROM diag", [], |r| r.get::<_, i64>(0)).unwrap(), 101);
        }
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{path}{suffix}"));
        }
    }

    /// Profile keys of before signed requests (spec 3.3): the check before a change and the count per recipient go;
    /// the current check stays and counts alone.
    #[test]
    fn a_database_with_the_old_contact_keys_loses_the_grace() {
        let path = std::env::temp_dir().join(format!("yo-hub-grace-{}.db", std::process::id()));
        let path = path.to_str().unwrap();
        let leo = {
            let db = Db::open(path).unwrap();
            let leo = user(register(&db, b"LEO", 1, 1, 0));
            as_schema_10(&db.conn());
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
        db.set_access_key(&[2; 16], &[9; 32], true, &[1]).unwrap().done().unwrap();
        assert_eq!(db.admit(leo, None, Some(spam)).unwrap(), Admission::Request);
        assert_eq!(db.admit(leo, None, None).unwrap(), Admission::Closed, "unsigned");
        db.set_access_key(&[2; 16], &[9; 32], false, &[1]).unwrap().done().unwrap();
        assert_eq!(db.admit(leo, None, Some(spam)).unwrap(), Admission::Request, "the switch is the device's");
    }

    /// A delivery to a user that does not exist is answered before anything is counted for it.
    #[test]
    fn a_delivery_to_no_user_is_unknown() {
        let db = Db::open(":memory:").unwrap();
        let leo = user(register(&db, b"LEO", 1, 1, 0));
        assert_eq!(db.admit(&leo, None, None).unwrap(), Admission::Contact, "no key set: a contact's");
        assert_eq!(db.admit(&[0xEE; 16], None, None).unwrap(), Admission::Unknown);
        assert!(matches!(
            db.set_access_key(&[0xEE; 16], &[9; 32], true, &[1]).unwrap(),
            Write::Refused(Conflict::DeviceGone)
        ));
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
