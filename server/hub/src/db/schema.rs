//! The store's schema (`SCHEMA`, schema 11) and the way every older database a hub has written comes up to it.
//! A database of another schema is refused, never half migrated.

use super::{diagnostic_signature, diagnostic_write, immediate, legacy_crash, newest_builds};
use rusqlite::{params, Connection};
use yoip::KEY_LEN;

/// Brings the database to this schema: each step does nothing where it was done before.
pub(super) fn upgrade(conn: &mut Connection) -> rusqlite::Result<()> {
    check_version(conn)?;
    if user_version(conn)? < SCHEMA_VERSION {
        to_10(conn)?;
        to_11(conn)?;
    }
    conn.execute_batch(SCHEMA)?;
    // A column of its own, no schema step, so the hub before it still opens the store (#8).
    if !has_column(conn, "account_device", "ohttp_at")? {
        conn.execute_batch("ALTER TABLE account_device ADD COLUMN ohttp_at INTEGER")?;
    }
    skeletons_of_today(conn)
}

/// Schemas 1 to 10, under the table names of before (`SCHEMA_10`): each step does nothing where it was done before.
fn to_10(conn: &mut Connection) -> rusqlite::Result<()> {
    if user_version(conn)? == 1 {
        from_1(conn)?;
    }
    if user_version(conn)? == 2 {
        from_2(conn)?;
    }
    without_grace(conn)?;
    conn.execute_batch(SCHEMA_10)?;
    if !has_column(conn, "blobs", "gets_left")? {
        immediate(conn, |tx| {
            tx.execute_batch(
                "ALTER TABLE blobs ADD COLUMN gets_left INTEGER NOT NULL DEFAULT 192;
                UPDATE blobs SET gets_left = max(3, 3 * (fetches_left + fetching));",
            )
        })?;
    }
    if !has_column(conn, "reserved", "note")? {
        conn.execute_batch("ALTER TABLE reserved ADD COLUMN note TEXT")?;
    }
    if !has_column(conn, "devices", "version")? {
        conn.execute_batch("ALTER TABLE devices ADD COLUMN version TEXT")?;
    }
    if !has_column(conn, "users", "bot")? {
        conn.execute_batch("ALTER TABLE users ADD COLUMN bot INTEGER NOT NULL DEFAULT 0")?;
    }
    if !has_column(conn, "zk_groups", "created_at")? {
        conn.execute_batch("ALTER TABLE zk_groups ADD COLUMN created_at INTEGER")?;
    }
    // Device attestation (spec 4.2): a table of its own, no schema step, so the hub before it still opens the store.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS attestation (
            user_id BLOB PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
            state TEXT NOT NULL CHECK(state IN ('attested','failed')),
            platform TEXT NOT NULL CHECK(platform IN ('ios','android')),
            reason TEXT, level TEXT, at_ms INTEGER NOT NULL
        ) WITHOUT ROWID;",
    )?;
    // The feedback table of schemas 3 to 7 is there until schema 8 (`without_feedback`) moves its rows and drops it.
    let legacy_max: i64 = if user_version(conn)? < 8 {
        conn.execute_batch(LEGACY_FEEDBACK)?;
        with_auto_feedback(conn)?;
        with_feedback_type(conn)?;
        conn.query_row("SELECT coalesce(max(id),0) FROM feedback", [], |r| r.get(0))?
    } else {
        conn.execute_batch(DIAGNOSTICS)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS support_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT, install BLOB NOT NULL,
                from_team INTEGER NOT NULL CHECK(from_team IN (0,1)), text TEXT NOT NULL,
                created_at INTEGER NOT NULL, read_at INTEGER, app_version TEXT NOT NULL DEFAULT '', done_at INTEGER);",
        )?;
        0
    };
    conn.execute_batch(DIAGNOSTIC_BUILDS)?;
    migrate_diagnostics(conn)?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS diagnostic_seen (uuid TEXT PRIMARY KEY, diagnostic_id INTEGER NOT NULL REFERENCES diagnostics(id) ON DELETE CASCADE) WITHOUT ROWID;")?;
    conn.execute_batch(DIAGNOSTIC_INSTALLS)?;
    if !has_column(conn, "diagnostic_installs", "at")? {
        conn.execute_batch("ALTER TABLE diagnostic_installs ADD COLUMN at INTEGER")?;
    }
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS diagnostic_stats_deleted AFTER DELETE ON diagnostics BEGIN
             DELETE FROM stats WHERE metric = 'diag' AND key = CAST(OLD.id AS TEXT);
         END;",
    )?;
    problems_without_builds(conn)?;
    with_support(conn, legacy_max)?;
    with_support_done(conn)?;
    with_todos(conn)?;
    without_feedback(conn)?;
    with_installs(conn)?;
    conn.execute_batch("CREATE INDEX IF NOT EXISTS support_by_install ON support_messages(install, created_at, id)")?;
    with_stats(conn)
}

/// Schema 11: every table named by its area (`account`, `name`, `message`, `grp`, `support`, `diag`, `stat`), rows
/// and all; the indexes and the trigger come back under their new names with `SCHEMA`.
fn to_11(conn: &mut Connection) -> rusqlite::Result<()> {
    immediate(conn, |tx| {
        tx.execute_batch(
            "DROP TRIGGER IF EXISTS diagnostic_stats_deleted;
             DROP INDEX IF EXISTS account_keys_by_user; DROP INDEX IF EXISTS handles_by_user;
             DROP INDEX IF EXISTS devices_by_user; DROP INDEX IF EXISTS messages_by_device;
             DROP INDEX IF EXISTS messages_by_age; DROP INDEX IF EXISTS reports_by_reported;
             DROP INDEX IF EXISTS blobs_by_user; DROP INDEX IF EXISTS blobs_by_age;
             DROP INDEX IF EXISTS diagnostics_recent; DROP INDEX IF EXISTS diagnostic_installs_by_day;",
        )?;
        for (old, new) in RENAMED {
            if has_table(tx, old)? {
                tx.execute_batch(&format!("ALTER TABLE {old} RENAME TO {new}"))?;
            }
        }
        tx.execute_batch("PRAGMA user_version = 11")
    })
}

/// Schema 10's table names and schema 11's.
pub(super) const RENAMED: [(&str, &str); 26] = [
    ("users", "account"),
    ("devices", "account_device"),
    ("handles", "account_name"),
    ("account_keys", "account_key"),
    ("contact_keys", "account_contact_key"),
    ("prekeys", "account_prekey"),
    ("profile_commitments", "account_profile"),
    ("storage", "account_storage"),
    ("backups", "account_backup"),
    ("attestation", "account_attestation"),
    ("reserved", "name_reserved"),
    ("gone", "account_device_gone"),
    ("messages", "message"),
    ("blobs", "message_blob"),
    ("blob_quota", "message_blob_quota"),
    ("zk_groups", "grp"),
    ("zk_groups_gone", "grp_gone"),
    ("support_messages", "support"),
    ("reports", "support_report"),
    ("diagnostics", "diag"),
    ("diagnostic_seen", "diag_seen"),
    ("diagnostic_installs", "diag_install"),
    ("diagnostic_builds", "diag_build"),
    ("stats", "stat"),
    ("events", "stat_minute"),
    ("request_hours", "stat_request"),
];

/// Usernames are found by their skeleton (spec 2.2), which drops more since 2026-11-24 (U+200D, tag characters): a
/// name kept under the skeleton of before gets today's, unless another name has that one already (that one keeps it; this
/// one then is found by its user id, as before). Nothing to do where every skeleton is today's.
fn skeletons_of_today(conn: &mut Connection) -> rusqlite::Result<()> {
    immediate(conn, |tx| {
        let names: Vec<(Vec<u8>, String)> = tx
            .prepare("SELECT value, spelling FROM account_name WHERE kind = ?1 AND spelling IS NOT NULL")?
            .query_map([yoip::net::handle::KIND_USERNAME], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (value, spelling) in names {
            let today = yoip::net::handle::skeleton(&spelling).into_bytes();
            if today != value {
                tx.execute(
                    "UPDATE OR IGNORE account_name SET value = ?3 WHERE kind = ?1 AND value = ?2",
                    params![yoip::net::handle::KIND_USERNAME, value, today],
                )?;
            }
        }
        Ok(())
    })
}

/// The schema this hub writes; another is refused, never half migrated (1 and 2 are brought up: `from_1`, `from_2`).
const SCHEMA_VERSION: i64 = 11;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS account (
    id BLOB PRIMARY KEY,
    account_key BLOB NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    bot INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
-- Every account key a user ever had: a key belongs to one user forever.
CREATE TABLE IF NOT EXISTS account_key (
    key BLOB PRIMARY KEY,
    user_id BLOB NOT NULL REFERENCES account(id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS account_key_by_user ON account_key(user_id);
-- How people find a user: a username by its skeleton (spec 2.2); kind 1, a phone number as its HMAC, is no longer
-- made. `spelling`: a username as its owner writes it (\"YoGPT\"); NULL for names from before spellings were kept,
-- shown as their skeleton (`migrate` adds the column to such a database).
CREATE TABLE IF NOT EXISTS account_name (
    kind INTEGER NOT NULL,
    value BLOB NOT NULL,
    user_id BLOB NOT NULL REFERENCES account(id),
    spelling TEXT,
    PRIMARY KEY (kind, value)
) WITHOUT ROWID;
-- Usernames nobody may claim, by skeleton (spec 2.2): seeded from the operator's list at every start (INSERT OR
-- IGNORE), never deleted by the hub. A user who has such a name keeps it.
CREATE TABLE IF NOT EXISTS name_reserved (
    skeleton TEXT PRIMARY KEY,
    spelling TEXT NOT NULL,
    note TEXT,
    created_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS account_name_by_user ON account_name(user_id, kind);
CREATE TABLE IF NOT EXISTS account_device (
    id BLOB PRIMARY KEY,
    user_id BLOB NOT NULL REFERENCES account(id),
    device_key BLOB NOT NULL UNIQUE,
    auth_key BLOB NOT NULL,
    push_platform INTEGER NOT NULL,
    push_token BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    app TEXT,
    registration_id INTEGER NOT NULL DEFAULT 0,
    number INTEGER NOT NULL DEFAULT 1,
    -- The highest one-time prekey id it uploaded: one not above it is one it uploaded before (a repeat), never taken
    -- again, so a key handed out once is not handed out again.
    prekey_max INTEGER NOT NULL DEFAULT 0,
    -- The version its app named with it (`Yo-App`, spec 4.1: 1.0.0-RC36), if any.
    version TEXT,
    -- When it last made a request inside OHTTP (spec 4.1), to the hour and written once a day; none before it did.
    ohttp_at INTEGER
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS account_device_by_user ON account_device(user_id);
-- Devices that were replaced: their own requests answer 410.
CREATE TABLE IF NOT EXISTS account_device_gone (
    id BLOB PRIMARY KEY,
    gone_at INTEGER NOT NULL
) WITHOUT ROWID;
-- Each device's prekeys (spec 3.1), as it uploaded them: kind 0 its signed prekey, 1 its last resort Kyber prekey
-- (one of each), 2 one-time EC prekeys, 3 one-time Kyber prekeys (each handed out once).
CREATE TABLE IF NOT EXISTS account_prekey (
    device_id BLOB NOT NULL,
    kind INTEGER NOT NULL,
    id INTEGER NOT NULL,
    key BLOB NOT NULL,
    signature BLOB NOT NULL,
    PRIMARY KEY (device_id, kind, id)
) WITHOUT ROWID;
-- AUTOINCREMENT: an id is never handed out twice, so an ack can never delete an envelope it did not see.
CREATE TABLE IF NOT EXISTS message (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id BLOB NOT NULL,
    envelope BLOB NOT NULL,
    received_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS message_by_device ON message(device_id, id);
CREATE INDEX IF NOT EXISTS message_by_age ON message(received_at);
-- Each user's backup record (spec 2.4), as the client sent it: found by the user id or by its locator, handed out to a
-- request signed by one of its doors' auth keys (the passkey's NULL without one).
CREATE TABLE IF NOT EXISTS account_backup (
    user_id BLOB PRIMARY KEY REFERENCES account(id),
    locator BLOB UNIQUE,
    record BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    auth_passkey BLOB,
    auth_code BLOB
) WITHOUT ROWID;
-- Users reported (spec 4.2), for the operator to review: the reporting install id (`with_installs`; never an
-- account), the reported user, why (1 spam, 2 abuse, 3 other), when. One row per pair, the latest report. No
-- content: the hub has none. Gone with the reported account.
CREATE TABLE IF NOT EXISTS support_report (
    reporter BLOB NOT NULL,
    reported_id BLOB NOT NULL,
    reason INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (reporter, reported_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS support_report_by_reported ON support_report(reported_id);
-- Signed requests (4.10) each sender sent, per hour: a count, nothing of to whom; kept 25 hours, for the operator's
-- Users page (strangers' floods). Sealed deliveries are never counted by sender (6.6).
CREATE TABLE IF NOT EXISTS stat_request (
    user_id BLOB NOT NULL,
    hour INTEGER NOT NULL,
    n INTEGER NOT NULL,
    PRIMARY KEY (user_id, hour)
) WITHOUT ROWID;
-- The hub's hourly summaries, counts only, nothing of who (spec 1/6.6).
-- One row per metric, key and hour (hours since the epoch), kept for good, never pruned or folded.
CREATE TABLE IF NOT EXISTS stat (
    metric TEXT NOT NULL,
    key TEXT NOT NULL DEFAULT '',
    hour INTEGER NOT NULL,
    value INTEGER NOT NULL,
    PRIMARY KEY (metric, key, hour)
) WITHOUT ROWID;
-- Events per minute (minutes since the epoch), a count of each kind and nothing else: no user, device, envelope,
-- blob or group (6.6). `yo-in` a Yo accepted into an inbox, `yo-out` one a device fetched and acknowledged,
-- `media-in` a blob uploaded (`media-in-bytes` its bytes), `media-out` a blob fetched, `group-create` a group made;
-- `yo-in-bot`, `latency-*`, `push-ok`/`push-fail`, `push-folded`/`push-paced`, `http`/`http-4xx`/`http-429`/`http-5xx`, `account-gone` (spec 4.1).
-- The last 2 hours per minute for the live strip; the hours go to stat.
CREATE TABLE IF NOT EXISTS stat_minute (
    kind TEXT NOT NULL,
    minute INTEGER NOT NULL,
    n INTEGER NOT NULL,
    PRIMARY KEY (kind, minute)
) WITHOUT ROWID;
-- Each user's access key as a check, never the key (spec 3.3): a delivery that shows it is a contact's; else it
-- is a request, signed by its sender (`requests_open` is kept, unused: the switch is the device's). Gone with the
-- account. (The table keeps its
-- name from the contact keys before Signal's access keys: a check of the old kind matches no delivery, and every
-- client tells its new one.)
CREATE TABLE IF NOT EXISTS account_contact_key (
    user_id BLOB PRIMARY KEY REFERENCES account(id),
    current BLOB NOT NULL,
    requests_open INTEGER NOT NULL DEFAULT 1
) WITHOUT ROWID;
-- Each user's storage (spec 4.3), sealed by its devices under a key the hub never has; `version` counts its writes,
-- so one writer never overwrites another's unseen. Gone with the account.
CREATE TABLE IF NOT EXISTS account_storage (
    user_id BLOB PRIMARY KEY,
    version INTEGER NOT NULL,
    data BLOB NOT NULL,
    updated_at INTEGER NOT NULL
) WITHOUT ROWID;
-- Photos and videos (spec 4.3): the file is on disk, by its id; here what the hub needs to hand it out and delete it.
-- `user_id` is the uploader (for its quota), never a recipient. Gone after the last fetch, or after its time.
-- `gets_left`: persisted abuse budget for GETs (three per original fetch).
-- `fetching`: reserved by older hub versions; recovered once at startup.
-- `ready`: 0 until the file is in place; such a blob is not handed out, and one left
-- so (a crash in between) is removed at the next start, or within the hour.
CREATE TABLE IF NOT EXISTS message_blob (
    id BLOB PRIMARY KEY,
    user_id BLOB NOT NULL,
    size INTEGER NOT NULL,
    fetches_left INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    fetching INTEGER NOT NULL DEFAULT 0,
    gets_left INTEGER NOT NULL DEFAULT 192,
    ready INTEGER NOT NULL DEFAULT 1
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS message_blob_by_user ON message_blob(user_id);
CREATE INDEX IF NOT EXISTS message_blob_by_age ON message_blob(created_at);
-- Each user's profile key commitment (zkgroup's `ProfileKeyCommitment`, spec 3.4), told with its access key: what
-- profile key credentials are issued against. Gone with the account.
CREATE TABLE IF NOT EXISTS account_profile (
    user_id BLOB PRIMARY KEY,
    commitment BLOB NOT NULL
) WITHOUT ROWID;
-- Groups (spec 3.4, Signal's Groups v2): by their identifier, the `GroupPublicParams`, the revision and the state
-- (`GroupState`), whose members and title are encrypted under the group's key. The last member out ends the row.
CREATE TABLE IF NOT EXISTS grp (
    id BLOB PRIMARY KEY,
    params BLOB NOT NULL,
    revision INTEGER NOT NULL,
    state BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    -- When it was made (ms); NULL for groups made before hubs kept it.
    created_at INTEGER
) WITHOUT ROWID;
-- Groups ended (their last member left), by identifier: never made again (whoever has an old key cannot bring one
-- back under its members' eyes). Kept for good, 32 bytes and a time each.
CREATE TABLE IF NOT EXISTS grp_gone (
    id BLOB PRIMARY KEY,
    at INTEGER NOT NULL
) WITHOUT ROWID;
-- How many blobs, and bytes, each user uploaded on a day (days since the epoch, UTC); kept two days.
CREATE TABLE IF NOT EXISTS message_blob_quota (
    user_id BLOB NOT NULL,
    day INTEGER NOT NULL,
    count INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    PRIMARY KEY (user_id, day)
) WITHOUT ROWID;
-- Device attestation (spec 4.2), the last verdict per user.
CREATE TABLE IF NOT EXISTS account_attestation (
    user_id BLOB PRIMARY KEY REFERENCES account(id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK(state IN ('attested','failed')),
    platform TEXT NOT NULL CHECK(platform IN ('ios','android')),
    reason TEXT, level TEXT, at_ms INTEGER NOT NULL
) WITHOUT ROWID;
-- Help threads by install id (spec 4): `from_team` 1 the team's answers.
CREATE TABLE IF NOT EXISTS support (
    id INTEGER PRIMARY KEY AUTOINCREMENT, install BLOB NOT NULL,
    from_team INTEGER NOT NULL CHECK(from_team IN (0,1)), text TEXT NOT NULL,
    created_at INTEGER NOT NULL, read_at INTEGER, app_version TEXT NOT NULL DEFAULT '', done_at INTEGER);
CREATE INDEX IF NOT EXISTS support_by_install ON support(install, created_at, id);
-- Crash and error groups (spec 4), one per problem; its builds, the uuids it was sent under and the installs per day.
CREATE TABLE IF NOT EXISTS diag (
    id INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK(kind IN ('crash','error')),
    signature TEXT NOT NULL UNIQUE, count INTEGER NOT NULL DEFAULT 1,
    first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
    app TEXT NOT NULL, os TEXT NOT NULL, model TEXT NOT NULL,
    payload TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0,
    done_builds TEXT, done_at INTEGER
);
CREATE INDEX IF NOT EXISTS diag_recent ON diag(last_seen DESC);
CREATE TABLE IF NOT EXISTS diag_build (
    diagnostic_id INTEGER NOT NULL REFERENCES diag(id) ON DELETE CASCADE, app TEXT NOT NULL,
    count INTEGER NOT NULL, last_seen INTEGER NOT NULL, PRIMARY KEY(diagnostic_id, app)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS diag_seen (uuid TEXT PRIMARY KEY,
    diagnostic_id INTEGER NOT NULL REFERENCES diag(id) ON DELETE CASCADE) WITHOUT ROWID;
-- Which install sent a group on which day (`at` its last report that day, to the hour); kept 90 days.
CREATE TABLE IF NOT EXISTS diag_install (
    diagnostic_id INTEGER NOT NULL REFERENCES diag(id) ON DELETE CASCADE, install BLOB NOT NULL,
    day INTEGER NOT NULL, at INTEGER, PRIMARY KEY(diagnostic_id, install, day)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS diag_install_by_day ON diag_install(day);
CREATE TRIGGER IF NOT EXISTS diag_stat_deleted AFTER DELETE ON diag BEGIN
    DELETE FROM stat WHERE metric = 'diag' AND key = CAST(OLD.id AS TEXT);
END;
";

/// Schema 10 under its table names (the steps up to it, `to_10`, write these).
pub(super) const SCHEMA_10: &str = "
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
-- How people find a user: a username by its skeleton (spec 2.2); kind 1, a phone number as its HMAC, is no longer
-- made. `spelling`: a username as its owner writes it (\"YoGPT\"); NULL for names from before spellings were kept,
-- shown as their skeleton (`migrate` adds the column to such a database).
CREATE TABLE IF NOT EXISTS handles (
    kind INTEGER NOT NULL,
    value BLOB NOT NULL,
    user_id BLOB NOT NULL REFERENCES users(id),
    spelling TEXT,
    PRIMARY KEY (kind, value)
) WITHOUT ROWID;
-- Usernames nobody may claim, by skeleton (spec 2.2): seeded from the operator's list at every start (INSERT OR
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
    app TEXT,
    registration_id INTEGER NOT NULL DEFAULT 0,
    number INTEGER NOT NULL DEFAULT 1,
    -- The highest one-time prekey id it uploaded: one not above it is one it uploaded before (a repeat), never taken
    -- again, so a key handed out once is not handed out again.
    prekey_max INTEGER NOT NULL DEFAULT 0,
    -- The version its app named with it (`Yo-App`, spec 4.1: 1.0.0-RC36), if any.
    version TEXT
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS devices_by_user ON devices(user_id);
-- Devices that were replaced: their own requests answer 410.
CREATE TABLE IF NOT EXISTS gone (
    id BLOB PRIMARY KEY,
    gone_at INTEGER NOT NULL
) WITHOUT ROWID;
-- Each device's prekeys (spec 3.1), as it uploaded them: kind 0 its signed prekey, 1 its last resort Kyber prekey
-- (one of each), 2 one-time EC prekeys, 3 one-time Kyber prekeys (each handed out once).
CREATE TABLE IF NOT EXISTS prekeys (
    device_id BLOB NOT NULL,
    kind INTEGER NOT NULL,
    id INTEGER NOT NULL,
    key BLOB NOT NULL,
    signature BLOB NOT NULL,
    PRIMARY KEY (device_id, kind, id)
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
-- Each user's backup record (spec 2.4), as the client sent it: found by the user id or by its locator, handed out to a
-- request signed by one of its doors' auth keys (the passkey's NULL without one).
CREATE TABLE IF NOT EXISTS backups (
    user_id BLOB PRIMARY KEY REFERENCES users(id),
    locator BLOB UNIQUE,
    record BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    auth_passkey BLOB,
    auth_code BLOB
) WITHOUT ROWID;
-- Users reported (spec 4.2), for the operator to review: the reporting install id (`with_installs`; never an
-- account), the reported user, why (1 spam, 2 abuse, 3 other), when. One row per pair, the latest report. No
-- content: the hub has none. Gone with the reported account.
CREATE TABLE IF NOT EXISTS reports (
    reporter BLOB NOT NULL,
    reported_id BLOB NOT NULL,
    reason INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (reporter, reported_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS reports_by_reported ON reports(reported_id);
-- Signed requests (4.10) each sender sent, per hour: a count, nothing of to whom; kept 25 hours, for the operator's
-- Users page (strangers' floods). Sealed deliveries are never counted by sender (6.6).
CREATE TABLE IF NOT EXISTS request_hours (
    user_id BLOB NOT NULL,
    hour INTEGER NOT NULL,
    n INTEGER NOT NULL,
    PRIMARY KEY (user_id, hour)
) WITHOUT ROWID;
-- The hub's hourly summaries, counts only, nothing of who (spec 1/6.6).
-- One row per metric, key and hour (hours since the epoch), kept for good, never pruned or folded.
CREATE TABLE IF NOT EXISTS stats (
    metric TEXT NOT NULL,
    key TEXT NOT NULL DEFAULT '',
    hour INTEGER NOT NULL,
    value INTEGER NOT NULL,
    PRIMARY KEY (metric, key, hour)
) WITHOUT ROWID;
-- Events per minute (minutes since the epoch), a count of each kind and nothing else: no user, device, envelope,
-- blob or group (6.6). `yo-in` a Yo accepted into an inbox, `yo-out` one a device fetched and acknowledged,
-- `media-in` a blob uploaded (`media-in-bytes` its bytes), `media-out` a blob fetched, `group-create` a group made;
-- `yo-in-bot`, `latency-*`, `push-ok`/`push-fail`, `push-folded`/`push-paced`, `http`/`http-4xx`/`http-429`/`http-5xx`, `account-gone` (spec 4.1).
-- The last 2 hours per minute for the live strip; the hours go to stats.
CREATE TABLE IF NOT EXISTS events (
    kind TEXT NOT NULL,
    minute INTEGER NOT NULL,
    n INTEGER NOT NULL,
    PRIMARY KEY (kind, minute)
) WITHOUT ROWID;
-- Each user's access key as a check, never the key (spec 3.3): a delivery that shows it is a contact's; else it
-- is a request, signed by its sender (`requests_open` is kept, unused: the switch is the device's). Gone with the
-- account. (The table keeps its
-- name from the contact keys before Signal's access keys: a check of the old kind matches no delivery, and every
-- client tells its new one.)
CREATE TABLE IF NOT EXISTS contact_keys (
    user_id BLOB PRIMARY KEY REFERENCES users(id),
    current BLOB NOT NULL,
    requests_open INTEGER NOT NULL DEFAULT 1
) WITHOUT ROWID;
-- Each user's storage (spec 4.3), sealed by its devices under a key the hub never has; `version` counts its writes,
-- so one writer never overwrites another's unseen. Gone with the account.
CREATE TABLE IF NOT EXISTS storage (
    user_id BLOB PRIMARY KEY,
    version INTEGER NOT NULL,
    data BLOB NOT NULL,
    updated_at INTEGER NOT NULL
) WITHOUT ROWID;
-- Photos and videos (spec 4.3): the file is on disk, by its id; here what the hub needs to hand it out and delete it.
-- `user_id` is the uploader (for its quota), never a recipient. Gone after the last fetch, or after its time.
-- `gets_left`: persisted abuse budget for GETs (three per original fetch).
-- `fetching`: reserved by older hub versions; recovered once at startup.
-- `ready`: 0 until the file is in place; such a blob is not handed out, and one left
-- so (a crash in between) is removed at the next start, or within the hour.
CREATE TABLE IF NOT EXISTS blobs (
    id BLOB PRIMARY KEY,
    user_id BLOB NOT NULL,
    size INTEGER NOT NULL,
    fetches_left INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    fetching INTEGER NOT NULL DEFAULT 0,
    gets_left INTEGER NOT NULL DEFAULT 192,
    ready INTEGER NOT NULL DEFAULT 1
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS blobs_by_user ON blobs(user_id);
CREATE INDEX IF NOT EXISTS blobs_by_age ON blobs(created_at);
-- Each user's profile key commitment (zkgroup's `ProfileKeyCommitment`, spec 3.4), told with its access key: what
-- profile key credentials are issued against. Gone with the account.
CREATE TABLE IF NOT EXISTS profile_commitments (
    user_id BLOB PRIMARY KEY,
    commitment BLOB NOT NULL
) WITHOUT ROWID;
-- Groups (spec 3.4, Signal's Groups v2): by their identifier, the `GroupPublicParams`, the revision and the state
-- (`GroupState`), whose members and title are encrypted under the group's key. The last member out ends the row.
CREATE TABLE IF NOT EXISTS zk_groups (
    id BLOB PRIMARY KEY,
    params BLOB NOT NULL,
    revision INTEGER NOT NULL,
    state BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    -- When it was made (ms); NULL for groups made before hubs kept it.
    created_at INTEGER
) WITHOUT ROWID;
-- Groups ended (their last member left), by identifier: never made again (whoever has an old key cannot bring one
-- back under its members' eyes). Kept for good, 32 bytes and a time each.
CREATE TABLE IF NOT EXISTS zk_groups_gone (
    id BLOB PRIMARY KEY,
    at INTEGER NOT NULL
) WITHOUT ROWID;
-- How many blobs, and bytes, each user uploaded on a day (days since the epoch, UTC); kept two days.
CREATE TABLE IF NOT EXISTS blob_quota (
    user_id BLOB NOT NULL,
    day INTEGER NOT NULL,
    count INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    PRIMARY KEY (user_id, day)
) WITHOUT ROWID;
";

/// The feedback table of schemas 3 to 7: what users wrote (`type` 'feedback'), the team's to-dos ('todo') and crash
/// reports ('crash', `auto` 1); gone with schema 8.
pub(super) const LEGACY_FEEDBACK: &str = "
CREATE TABLE IF NOT EXISTS feedback (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id BLOB NOT NULL,
    text TEXT NOT NULL,
    app_version TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    auto INTEGER NOT NULL DEFAULT 0,
    type TEXT NOT NULL DEFAULT 'feedback'
);
CREATE INDEX IF NOT EXISTS feedback_by_user ON feedback(user_id);";

const DIAGNOSTICS: &str = "
CREATE TABLE IF NOT EXISTS diagnostics (
 id INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK(kind IN ('crash','error')),
 signature TEXT NOT NULL UNIQUE, count INTEGER NOT NULL DEFAULT 1,
 first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
 app TEXT NOT NULL, os TEXT NOT NULL, model TEXT NOT NULL,
 payload TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0,
 done_builds TEXT, done_at INTEGER
);
CREATE INDEX IF NOT EXISTS diagnostics_recent ON diagnostics(last_seen DESC);";

// A problem's builds (its reports per build, the last when), spec 4.
pub(super) const DIAGNOSTIC_BUILDS: &str = "CREATE TABLE IF NOT EXISTS diagnostic_builds (
    diagnostic_id INTEGER NOT NULL REFERENCES diagnostics(id) ON DELETE CASCADE, app TEXT NOT NULL,
    count INTEGER NOT NULL, last_seen INTEGER NOT NULL, PRIMARY KEY(diagnostic_id, app)) WITHOUT ROWID;";

// Which install sent a crash or error group on which day (days since the epoch, UTC), so the operator counts
// installs, not reports (`at` the last report of that day, to the hour; none on rows from before it); none for the hub's own. Kept 90 days, and gone with its group (spec 4).
pub(super) const DIAGNOSTIC_INSTALLS: &str = "CREATE TABLE IF NOT EXISTS diagnostic_installs (
    diagnostic_id INTEGER NOT NULL REFERENCES diagnostics(id) ON DELETE CASCADE, install BLOB NOT NULL,
    day INTEGER NOT NULL, at INTEGER, PRIMARY KEY(diagnostic_id, install, day)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS diagnostic_installs_by_day ON diagnostic_installs(day);";

fn check_version(conn: &Connection) -> rusqlite::Result<()> {
    let version = user_version(conn)?;
    let tables: i64 =
        conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table'", [], |row| row.get(0))?;
    match version {
        SCHEMA_VERSION | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 => Ok(()),
        // An empty database needs none of the legacy tables.
        0 if tables == 0 => conn.pragma_update(None, "user_version", SCHEMA_VERSION),
        _ => Err(rusqlite::Error::InvalidParameterName(format!(
            "database schema {version}, this hub writes {SCHEMA_VERSION}: start from an empty database"
        ))),
    }
}

/// Schema 1 (the MLS hubs) to 2 in one transaction: accounts stay, the MLS tables and waiting envelopes go (spec 3.1).
fn from_1(conn: &mut Connection) -> rusqlite::Result<()> {
    immediate(conn, |tx| {
        for (table, column, kind) in [
            ("handles", "spelling", "TEXT"),
            ("backups", "auth_passkey", "BLOB"),
            ("backups", "auth_code", "BLOB"),
            ("blobs", "fetching", "INTEGER NOT NULL DEFAULT 0"),
            ("blobs", "ready", "INTEGER NOT NULL DEFAULT 1"),
            ("devices", "app", "TEXT"),
            ("devices", "registration_id", "INTEGER NOT NULL DEFAULT 0"),
            ("devices", "number", "INTEGER NOT NULL DEFAULT 1"),
            ("devices", "prekey_max", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            // A table schema 1 never had is made whole by `SCHEMA_10`.
            if has_table(tx, table)? && !has_column(tx, table, column)? {
                tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"))?;
            }
        }
        tx.execute_batch(
            "DROP TABLE IF EXISTS key_packages;
         DROP TABLE IF EXISTS seen_key_packages;
         DROP TABLE IF EXISTS groups;
         DROP TABLE IF EXISTS accepted;
         DROP TABLE IF EXISTS commits;
         DELETE FROM messages;
         PRAGMA user_version = 2;",
        )
    })
}

/// Schema 2 to 3 (the later steps follow): Ed25519 account and auth keys to their Curve25519 form (spec 1.3); word backups go (spec 2.4).
fn from_2(conn: &mut Connection) -> rusqlite::Result<()> {
    immediate(conn, |tx| {
        let curve = |key: &[u8]| -> Option<Vec<u8>> {
            let key: [u8; KEY_LEN] = key.try_into().ok()?;
            let point = curve25519_dalek::edwards::CompressedEdwardsY(key).decompress()?;
            Some(point.to_montgomery().to_bytes().to_vec())
        };
        for (table, column) in [("users", "account_key"), ("account_keys", "key"), ("devices", "auth_key")] {
            let keys: Vec<Vec<u8>> = tx
                .prepare(&format!("SELECT {column} FROM {table}"))?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for key in keys {
                if let Some(new) = curve(&key) {
                    tx.execute(&format!("UPDATE {table} SET {column} = ?1 WHERE {column} = ?2"), params![new, key])?;
                }
            }
        }
        if has_table(tx, "backups")? {
            tx.execute_batch("DELETE FROM backups")?;
        }
        tx.execute_batch("PRAGMA user_version = 3")
    })
}

/// The grace profile keys and the request tables of before (spec 3.3, 4.1) go.
fn without_grace(conn: &Connection) -> rusqlite::Result<()> {
    if has_column(conn, "contact_keys", "previous")? {
        conn.execute_batch(
            "ALTER TABLE contact_keys DROP COLUMN previous;
             ALTER TABLE contact_keys DROP COLUMN previous_until;",
        )?;
    }
    conn.execute_batch(
        "DROP TABLE IF EXISTS requests_day; DROP TABLE IF EXISTS requests;
         DROP TABLE IF EXISTS spent_tokens; DROP TABLE IF EXISTS token_keys;",
    )
}

/// Feedback of before crash reports: the `auto` column comes (0 for all of it).
fn with_auto_feedback(conn: &Connection) -> rusqlite::Result<()> {
    if !has_column(conn, "feedback", "auto")? {
        conn.execute_batch("ALTER TABLE feedback ADD COLUMN auto INTEGER NOT NULL DEFAULT 0")?;
    }
    Ok(())
}

pub(super) fn with_feedback_type(conn: &Connection) -> rusqlite::Result<()> {
    if !has_column(conn, "feedback", "type")? {
        conn.execute_batch(
            "BEGIN IMMEDIATE;
            ALTER TABLE feedback ADD COLUMN type TEXT NOT NULL DEFAULT 'feedback';
            UPDATE feedback SET type = CASE WHEN auto = 1 THEN 'crash'
                WHEN app_version = 'admin' THEN 'todo' ELSE 'feedback' END;
            COMMIT;",
        )?;
    }
    Ok(())
}

pub(super) fn migrate_diagnostics(conn: &mut Connection) -> rusqlite::Result<()> {
    immediate(conn, |tx| {
        if has_table(tx, "diagnostics")? {
            return Ok(());
        }
        tx.execute_batch(DIAGNOSTICS)?;
        let rows = tx
        .prepare(
            "SELECT text, app_version, created_at FROM feedback WHERE type='crash' OR auto=1 ORDER BY created_at, id",
        )?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, u64>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
        for (text, app, at) in rows {
            legacy_diagnostic(tx, &text, &app, at)?;
        }
        tx.execute_batch("DELETE FROM feedback WHERE type='crash' OR auto=1; PRAGMA user_version = 4")
    })
}

/// A problem's identity lost its build (2026-11-24, spec 4): the groups of one problem in several builds become
/// one, their counts per build in `diagnostic_builds`, its installs and uuids with it; open if any was open, else done
/// in the newest build per platform it had. Nothing to do where `done_builds` is there.
fn problems_without_builds(conn: &mut Connection) -> rusqlite::Result<()> {
    if has_column(conn, "diagnostics", "done_builds")? {
        return Ok(());
    }
    immediate(conn, |tx| {
        tx.execute_batch(
            "ALTER TABLE diagnostics ADD COLUMN done_builds TEXT; ALTER TABLE diagnostics ADD COLUMN done_at INTEGER;
             INSERT OR IGNORE INTO diagnostic_builds(diagnostic_id, app, count, last_seen)
                 SELECT id, app, count, last_seen FROM diagnostics;",
        )?;
        let rows: Vec<(i64, String, String)> = tx
            .prepare("SELECT id, kind, payload FROM diagnostics ORDER BY last_seen DESC, id DESC")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut keeper: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for (id, kind, payload) in &rows {
            let payload: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
            let signature = diagnostic_signature(kind, &payload);
            let Some(&into) = keeper.get(&signature) else {
                keeper.insert(signature, *id);
                continue;
            };
            // The newest group stays (its latest payload); this older one goes into it.
            tx.execute(
                "UPDATE diagnostics SET count = count + (SELECT count FROM diagnostics WHERE id = ?2),
                     first_seen = min(first_seen, (SELECT first_seen FROM diagnostics WHERE id = ?2)),
                     done = min(done, (SELECT done FROM diagnostics WHERE id = ?2)) WHERE id = ?1",
                params![into, id],
            )?;
            tx.execute(
                "INSERT INTO diagnostic_builds(diagnostic_id, app, count, last_seen)
                     SELECT ?1, app, count, last_seen FROM diagnostic_builds WHERE diagnostic_id = ?2 AND true
                 ON CONFLICT(diagnostic_id, app) DO UPDATE SET count = count + excluded.count,
                     last_seen = max(last_seen, excluded.last_seen)",
                params![into, id],
            )?;
            tx.execute(
                "INSERT INTO diagnostic_installs(diagnostic_id, install, day, at)
                     SELECT ?1, install, day, at FROM diagnostic_installs WHERE diagnostic_id = ?2 AND true
                 ON CONFLICT(diagnostic_id, install, day) DO UPDATE SET at = coalesce(max(at, excluded.at), at, excluded.at)",
                params![into, id],
            )?;
            if has_table(tx, "diagnostic_hours")? {
                tx.execute(
                    "INSERT INTO diagnostic_hours(diagnostic_id, hour, count)
                         SELECT ?1, hour, count FROM diagnostic_hours WHERE diagnostic_id = ?2
                     ON CONFLICT(diagnostic_id, hour) DO UPDATE SET count = count + excluded.count",
                    params![into, id],
                )?;
            }
            tx.execute("UPDATE diagnostic_seen SET diagnostic_id = ?1 WHERE diagnostic_id = ?2", params![into, id])?;
            tx.execute("DELETE FROM diagnostics WHERE id = ?1", [id])?;
        }
        for (signature, id) in keeper {
            let apps: Vec<String> = tx
                .prepare("SELECT app FROM diagnostic_builds WHERE diagnostic_id = ?1")?
                .query_map([id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            tx.execute(
                "UPDATE diagnostics SET signature = ?2, done_builds = CASE WHEN done = 1 THEN ?3 END,
                     done_at = CASE WHEN done = 1 THEN last_seen END WHERE id = ?1",
                params![id, signature, newest_builds(apps.iter().map(String::as_str))],
            )?;
        }
        Ok(())
    })
}

/// Add and backfill the kind only once; later starts preserve explicitly written kinds.
pub(super) fn with_support(conn: &Connection, legacy_max: i64) -> rusqlite::Result<()> {
    if has_column(conn, "support_messages", "done_at")? {
        return Ok(());
    }
    conn.execute_batch(&format!(
        "BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS support_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT, user_id BLOB NOT NULL,
            from_team INTEGER NOT NULL CHECK(from_team IN (0,1)), text TEXT NOT NULL,
            created_at INTEGER NOT NULL, read_at INTEGER, app_version TEXT NOT NULL DEFAULT '');
        CREATE INDEX IF NOT EXISTS support_by_user ON support_messages(user_id, created_at, id);
        CREATE TABLE IF NOT EXISTS todos (id INTEGER PRIMARY KEY AUTOINCREMENT,
            text TEXT NOT NULL, created_at INTEGER NOT NULL, done_at INTEGER);
        INSERT INTO support_messages(id,user_id,from_team,text,created_at,app_version)
            SELECT id,user_id,0,text,created_at,app_version FROM feedback WHERE type = 'feedback';
        INSERT INTO todos(id,text,created_at) SELECT id,text,created_at FROM feedback WHERE type = 'todo';
        -- Reserve every legacy feedback id in both sequences: old dashboard done keys
        -- must never accidentally identify a newly created row in the other table.
        INSERT INTO sqlite_sequence(name,seq) SELECT 'support_messages',coalesce((SELECT max(id) FROM feedback),0)
            WHERE NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='support_messages');
        INSERT INTO sqlite_sequence(name,seq) SELECT 'todos',coalesce((SELECT max(id) FROM feedback),0)
            WHERE NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='todos');
        UPDATE sqlite_sequence SET seq=max(seq,{legacy_max},(SELECT coalesce(max(id),0) FROM feedback))
            WHERE name IN ('support_messages','todos');
        DELETE FROM feedback WHERE type IN ('feedback','todo');
        PRAGMA user_version = 5;
        COMMIT;",
    ))
}

/// Schema 6: X'' is the accountless team thread; to-dos move there with new ids, their times kept.
pub(super) fn with_support_done(conn: &mut Connection) -> rusqlite::Result<()> {
    immediate(conn, |tx| {
        if !has_column(tx, "support_messages", "done_at")? {
            tx.execute_batch(
                "ALTER TABLE support_messages ADD COLUMN done_at INTEGER;
            UPDATE support_messages SET done_at=read_at WHERE from_team=0 AND read_at IS NOT NULL;",
            )?;
        }
        if user_version(tx)? >= 6 {
            return Ok(());
        }
        if has_table(tx, "todos")? {
            tx.execute_batch(
                "INSERT INTO support_messages(user_id,from_team,text,created_at,done_at)
            SELECT X'',1,text,created_at,done_at FROM todos ORDER BY created_at,id;
            DROP TABLE todos;",
            )?;
        }
        tx.execute_batch("PRAGMA user_version=6;")
    })
}

/// Schema 7: the team's to-dos in a table of their own again; the X'' thread's rows move there, text and times kept.
pub(super) fn with_todos(conn: &mut Connection) -> rusqlite::Result<()> {
    immediate(conn, |tx| {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS todos (id INTEGER PRIMARY KEY AUTOINCREMENT,
            text TEXT NOT NULL, created_at INTEGER NOT NULL, done_at INTEGER);",
        )?;
        if user_version(tx)? >= 7 {
            return Ok(());
        }
        tx.execute_batch(
            "INSERT INTO todos(text,created_at,done_at)
            SELECT text,created_at,done_at FROM support_messages WHERE user_id=X'' ORDER BY created_at,id;
            DELETE FROM support_messages WHERE user_id=X'';
            PRAGMA user_version=7;",
        )
    })
}

/// Schema 8: the feedback table goes; a row still in it moves where its kind lives now (a user's message to
/// support_messages, a crash to diagnostics, a to-do to todos), text and times kept.
pub(super) fn without_feedback(conn: &mut Connection) -> rusqlite::Result<()> {
    immediate(conn, |tx| {
        if user_version(tx)? >= 8 {
            return Ok(());
        }
        if has_table(tx, "feedback")? {
            let crashes = tx
                .prepare("SELECT text, app_version, created_at FROM feedback WHERE type='crash' OR auto=1 ORDER BY created_at, id")?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, u64>(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (text, app, at) in crashes {
                legacy_diagnostic(tx, &text, &app, at)?;
            }
            tx.execute_batch(
                "INSERT INTO todos(text,created_at) SELECT text,created_at FROM feedback
                    WHERE type='todo' AND auto=0 ORDER BY created_at,id;
                INSERT INTO support_messages(user_id,from_team,text,created_at,app_version)
                    SELECT user_id,0,text,created_at,app_version FROM feedback
                    WHERE type NOT IN ('crash','todo') AND auto=0 ORDER BY created_at,id;
                DROP INDEX IF EXISTS feedback_by_user;
                DROP TABLE feedback;",
            )?;
        }
        tx.execute_batch("PRAGMA user_version=8;")
    })
}

/// Schema 9: help goes by install id, not by account (spec 4). Support threads and the reporters of reports
/// before keep their rows under one random placeholder install id per old account: the account link is gone, and
/// no phone holds such an id, so those threads are the operator's to read only.
pub(super) fn with_installs(conn: &mut Connection) -> rusqlite::Result<()> {
    immediate(conn, |tx| {
        if user_version(tx)? >= 9 {
            return Ok(());
        }
        for (table, column, index) in
            [("support_messages", "user_id", "support_by_user"), ("reports", "reporter_id", "")]
        {
            if !has_column(tx, table, column)? {
                continue;
            }
            let to = if table == "reports" { "reporter" } else { "install" };
            tx.execute_batch(&format!(
                "CREATE TEMP TABLE install_of AS SELECT old, randomblob(16) AS new FROM (SELECT DISTINCT {column} AS old FROM {table});
                ALTER TABLE {table} RENAME COLUMN {column} TO {to};
                UPDATE {table} SET {to} = (SELECT m.new FROM temp.install_of m WHERE m.old = {table}.{to});
                DROP TABLE temp.install_of;"
            ))?;
            if !index.is_empty() {
                tx.execute_batch(&format!("DROP INDEX IF EXISTS {index};"))?;
            }
        }
        tx.execute_batch("PRAGMA user_version=9;")
    })
}

/// Schema 10: keep every hour and leave only the live strip's minutes.
fn with_stats(conn: &mut Connection) -> rusqlite::Result<()> {
    immediate(conn, |tx| {
        if user_version(tx)? >= 10 {
            return Ok(());
        }
        for (table, rows) in [
            ("levels", "SELECT kind, '', hour, value FROM levels"),
            ("deliveries_hourly", "SELECT 'deliveries', '', hour, n FROM deliveries_hourly"),
            ("diagnostic_hours", "SELECT 'diag', CAST(diagnostic_id AS TEXT), hour, count FROM diagnostic_hours"),
            ("diagnostic_days", "SELECT 'diagnostics:' || kind, '', day * 24, count FROM diagnostic_days"),
            ("events", "SELECT kind, '', minute / 60, sum(n) FROM events GROUP BY kind, minute / 60"),
        ] {
            if has_table(tx, table)? {
                tx.execute_batch(&format!(
                    "INSERT INTO stats(metric, key, hour, value) SELECT * FROM ({rows}) WHERE true
                     ON CONFLICT(metric, key, hour) DO UPDATE SET value = value + excluded.value;"
                ))?;
            }
        }
        tx.execute_batch(
            "DROP TABLE IF EXISTS levels;
             DROP TABLE IF EXISTS deliveries_hourly;
             DROP TABLE IF EXISTS diagnostic_hours;
             DROP TABLE IF EXISTS diagnostic_days;
             DROP INDEX IF EXISTS feedback_by_user;
             DROP TABLE IF EXISTS feedback;
             DROP TABLE IF EXISTS settings;
             DELETE FROM events WHERE minute < unixepoch() / 60 - 120;
             PRAGMA user_version=10;",
        )
    })
}

/// A crash of the feedback table of before written as a problem (`diagnostic_write`, which writes schema 11's names)
/// into the tables of schema 10: they carry the new names for that one write.
fn legacy_diagnostic(tx: &Connection, text: &str, app: &str, at: u64) -> rusqlite::Result<()> {
    const WRITTEN: [(&str, &str); 3] =
        [("diagnostics", "diag"), ("diagnostic_builds", "diag_build"), ("stats", "stat")];
    for (old, new) in WRITTEN {
        tx.execute_batch(&format!("ALTER TABLE {old} RENAME TO {new}"))?;
    }
    diagnostic_write(tx, "crash", &legacy_crash(text, app), at)?;
    for (old, new) in WRITTEN {
        tx.execute_batch(&format!("ALTER TABLE {new} RENAME TO {old}"))?;
    }
    Ok(())
}

fn has_table(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?.exists([table])
}

fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    conn.prepare("SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2")?.exists([table, column])
}

fn user_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
}

/// A store of this schema back under schema 10's names (tests: a hub of before wrote it).
#[cfg(test)]
pub(super) fn as_schema_10(conn: &Connection) {
    for (old, new) in RENAMED {
        conn.execute_batch(&format!("ALTER TABLE {new} RENAME TO {old}")).unwrap();
    }
    conn.execute_batch("PRAGMA user_version = 10").unwrap();
}
