//! The store's schema (`SCHEMA`, schema 9) and the way every older database a hub has written comes up to it.
//! A database of another schema is refused, never half migrated.

use super::{diagnostic_write, immediate, legacy_crash};
use rusqlite::{params, Connection};
use yo_wire::KEY_LEN;

/// Brings the database to this schema: each step does nothing where it was done before.
pub(super) fn upgrade(conn: &mut Connection) -> rusqlite::Result<()> {
    let existing = conn.prepare("SELECT 1 FROM sqlite_master WHERE type='table'")?.exists([])?;
    check_version(conn)?;
    if user_version(conn)? == 1 {
        from_1(conn)?;
    }
    if user_version(conn)? == 2 {
        from_2(conn)?;
    }
    without_grace(conn)?;
    conn.execute_batch(SCHEMA)?;
    if !has_column(conn, "blobs", "gets_left")? {
        immediate(conn, |tx| {
            tx.execute_batch(
                "ALTER TABLE blobs ADD COLUMN gets_left INTEGER NOT NULL DEFAULT 192;
                UPDATE blobs SET gets_left = max(3, 3 * (fetches_left + fetching));",
            )
        })?;
    }
    if !has_column(conn, "zk_groups", "created_at")? {
        conn.execute_batch("ALTER TABLE zk_groups ADD COLUMN created_at INTEGER")?;
    }
    // Initialize once: fresh hubs enforce reservations; existing hubs retain their old effective state.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value INTEGER NOT NULL) WITHOUT ROWID;",
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO settings(key,value) VALUES ('reserved_enabled', ?1)",
        [!existing || conn.prepare("SELECT 1 FROM reserved")?.exists([])?],
    )?;
    // The feedback table of schemas 3 to 7 is there until schema 8 (`without_feedback`) moves its rows and drops it.
    let legacy_max: i64 = if user_version(conn)? < SCHEMA_VERSION {
        conn.execute_batch(LEGACY_FEEDBACK)?;
        with_auto_feedback(conn)?;
        with_feedback_type(conn)?;
        conn.query_row("SELECT coalesce(max(id),0) FROM feedback", [], |r| r.get(0))?
    } else {
        0
    };
    conn.execute_batch(DIAGNOSTIC_DAYS)?;
    migrate_diagnostics(conn)?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS diagnostic_seen (uuid TEXT PRIMARY KEY, diagnostic_id INTEGER NOT NULL REFERENCES diagnostics(id) ON DELETE CASCADE) WITHOUT ROWID;")?;
    conn.execute_batch(DIAGNOSTIC_INSTALLS)?;
    with_support(conn, legacy_max)?;
    with_support_done(conn)?;
    with_todos(conn)?;
    without_feedback(conn)?;
    with_installs(conn)
}

/// The schema this hub writes; another is refused, never half migrated (1 and 2 are brought up: `from_1`, `from_2`).
const SCHEMA_VERSION: i64 = 9;

pub(super) const SCHEMA: &str = "
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
    app TEXT,
    registration_id INTEGER NOT NULL DEFAULT 0,
    number INTEGER NOT NULL DEFAULT 1,
    -- The highest one-time prekey id it uploaded: one not above it is one it uploaded before (a repeat), never taken
    -- again, so a key handed out once is not handed out again.
    prekey_max INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS devices_by_user ON devices(user_id);
-- Devices that were replaced: their own requests answer 410.
CREATE TABLE IF NOT EXISTS gone (
    id BLOB PRIMARY KEY,
    gone_at INTEGER NOT NULL
) WITHOUT ROWID;
-- Each device's prekeys (spec 4.1), as it uploaded them: kind 0 its signed prekey, 1 its last resort Kyber prekey
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
-- Each user's backup record (spec 7), as the client sent it: found by the user id or by its locator, handed out to a
-- request signed by one of its doors' auth keys (the passkey's NULL without one).
CREATE TABLE IF NOT EXISTS backups (
    user_id BLOB PRIMARY KEY REFERENCES users(id),
    locator BLOB UNIQUE,
    record BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    auth_passkey BLOB,
    auth_code BLOB
) WITHOUT ROWID;
-- Users reported (spec 6.4), for the operator to review: the reporting install id (`with_installs`; never an
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
-- Deliveries the hub took, per hour: a count, nothing of who or to whom (6.6); kept 8 days, for the operator's page.
CREATE TABLE IF NOT EXISTS deliveries_hourly (
    hour INTEGER PRIMARY KEY,
    n INTEGER NOT NULL
);
-- Events per minute (minutes since the epoch), a count of each kind and nothing else: no user, device, envelope,
-- blob or group (6.6). `yo-in` a Yo accepted into an inbox, `yo-out` one a device fetched and acknowledged,
-- `media-in` a blob uploaded (`media-in-bytes` its bytes), `media-out` a blob fetched, `group-create` a group made;
-- `yo-in-bot`, `latency-*`, `push-ok`/`push-fail`, `push-folded`/`push-paced`, `http`/`http-4xx`/`http-429`/`http-5xx`, `account-gone` (spec 6.6).
-- Statistics of no one: kept for good, for the operator's page.
CREATE TABLE IF NOT EXISTS events (
    kind TEXT NOT NULL,
    minute INTEGER NOT NULL,
    n INTEGER NOT NULL,
    PRIMARY KEY (kind, minute)
) WITHOUT ROWID;
-- Levels sampled once an hour (hours since the epoch, at the hour's start; the open problems also right after one is
-- marked done), a count of each kind and nothing else: `open-errors`/`open-crashes` the reports of the error and
-- crash groups not done (their counts summed), `queue-envelopes` the envelopes waiting, `queue-media-bytes` the bytes
-- of the blobs waiting, `active-24h`/`active-48h`/`active-30d` the users seen in the last 24 hours / 48 hours / 30 days. Every hour for 30
-- days, then the day's last (taken at the next midnight, UTC) kept for good, for the operator's page.
CREATE TABLE IF NOT EXISTS levels (
    kind TEXT NOT NULL,
    hour INTEGER NOT NULL,
    value INTEGER NOT NULL,
    PRIMARY KEY (kind, hour)
) WITHOUT ROWID;
-- Each user's access key as a check, never the key (spec 4.10): a delivery that shows it is a contact's; else it
-- is a request, signed by its sender (`requests_open` is kept, unused: the switch is the device's). Gone with the
-- account. (The table keeps its
-- name from the contact keys before Signal's access keys: a check of the old kind matches no delivery, and every
-- client tells its new one.)
CREATE TABLE IF NOT EXISTS contact_keys (
    user_id BLOB PRIMARY KEY REFERENCES users(id),
    current BLOB NOT NULL,
    requests_open INTEGER NOT NULL DEFAULT 1
) WITHOUT ROWID;
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
-- Each user's profile key commitment (zkgroup's `ProfileKeyCommitment`, spec 4.9), told with its access key: what
-- profile key credentials are issued against. Gone with the account.
CREATE TABLE IF NOT EXISTS profile_commitments (
    user_id BLOB PRIMARY KEY,
    commitment BLOB NOT NULL
) WITHOUT ROWID;
-- Groups (spec 4.9, Signal's Groups v2): by their identifier, the `GroupPublicParams`, the revision and the state
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
 payload TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS diagnostics_recent ON diagnostics(last_seen DESC);";

// Which install sent a crash or error group on which day (days since the epoch, UTC), so the operator counts
// installs, not reports; none for the hub's own. Kept 90 days, and gone with its group (spec 6.4).
pub(super) const DIAGNOSTIC_INSTALLS: &str = "CREATE TABLE IF NOT EXISTS diagnostic_installs (
    diagnostic_id INTEGER NOT NULL REFERENCES diagnostics(id) ON DELETE CASCADE, install BLOB NOT NULL,
    day INTEGER NOT NULL, PRIMARY KEY(diagnostic_id, install, day)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS diagnostic_installs_by_day ON diagnostic_installs(day);";

// Daily counts start with schema 5: none are made up from schema 4's first/last aggregates.
pub(super) const DIAGNOSTIC_DAYS: &str = "CREATE TABLE IF NOT EXISTS diagnostic_days (
    day INTEGER NOT NULL, kind TEXT NOT NULL, count INTEGER NOT NULL,
    PRIMARY KEY(day,kind)) WITHOUT ROWID;";

fn check_version(conn: &Connection) -> rusqlite::Result<()> {
    let version = user_version(conn)?;
    let tables: i64 =
        conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table'", [], |row| row.get(0))?;
    match version {
        SCHEMA_VERSION | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 => Ok(()),
        // A new database is made by the same steps as one of schema 3.
        0 if tables == 0 => conn.execute_batch("PRAGMA user_version = 3"),
        _ => Err(rusqlite::Error::InvalidParameterName(format!(
            "database schema {version}, this hub writes {SCHEMA_VERSION}: start from an empty database"
        ))),
    }
}

/// Schema 1 (the MLS hubs) to 2 in one transaction: accounts stay, the MLS tables and waiting envelopes go (spec 4.2).
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
            // A table schema 1 never had is made whole by `SCHEMA`.
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

/// Schema 2 to 3 (the later steps follow): Ed25519 account and auth keys to their Curve25519 form (spec 4.1); word backups go (spec 7).
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

/// The grace profile keys and the request tables of before (spec 4.10, 6.6) go.
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
            diagnostic_write(tx, "crash", &legacy_crash(&text, &app), at)?;
        }
        tx.execute_batch("DELETE FROM feedback WHERE type='crash' OR auto=1; PRAGMA user_version = 4")
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
                diagnostic_write(tx, "crash", &legacy_crash(&text, &app), at)?;
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

/// Schema 9: help goes by install id, not by account (spec 6.4). Support threads and the reporters of reports
/// before keep their rows under one random placeholder install id per old account: the account link is gone, and
/// no phone holds such an id, so those threads are the operator's to read only.
pub(super) fn with_installs(conn: &mut Connection) -> rusqlite::Result<()> {
    immediate(conn, |tx| {
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
        if has_table(tx, "support_messages")? {
            tx.execute_batch(
                "CREATE INDEX IF NOT EXISTS support_by_install ON support_messages(install, created_at, id);",
            )?;
        }
        tx.execute_batch("PRAGMA user_version=9;")
    })
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
