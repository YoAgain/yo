//! The book: one table of all bot traffic, written by the executors alone. A row per Yo received or sent, with its
//! time; who it was only on one with a reminder, for a service that remembers users, or of a connected user. And
//! who connected what: a second table, of hashes.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// How long who was reminded stays readable after it is over (for the admin page); the row stays for the statistics.
const KEPT_S: i64 = 7 * 86_400;
/// A reminder this late is not sent any more.
const TOO_LATE_S: i64 = 86_400;
/// What a connected user sent, for the other side to ask for.
const ASKED: i64 = 50;
/// How long a request to connect may be allowed, and then its code turned into a key.
pub const REQUEST_S: i64 = 600;
pub const CODE_S: i64 = 300;
/// Requests of one service that wait.
const MAX_REQUESTS: i64 = 50_000;
/// Connections of one service.
const MAX_CONNECTED: i64 = 100_000;
/// Waiting reminders of one service.
const MAX_WAITING: i64 = 100_000;

/// Waiting: due and not done. Done, or far too late: over. `kind`: a connected user's, kept with its user.
const TABLE: &str = "CREATE TABLE IF NOT EXISTS NAME (
    id INTEGER PRIMARY KEY,
    bot TEXT NOT NULL,
    way TEXT NOT NULL CHECK (way IN ('received', 'sent')),
    at INTEGER NOT NULL,
    user TEXT,
    cron_due INTEGER CHECK (cron_due IS NULL OR way = 'received'),
    cron_done INTEGER,
    kind TEXT CHECK (kind IS NULL OR way = 'received'),
    CHECK (cron_due IS NOT NULL OR cron_done IS NULL),
    CHECK (user IS NULL OR cron_due IS NOT NULL OR kind IS NOT NULL)
 ) STRICT;";
/// A connection: `key` is the hash of what a call for the user carries, `out` what the other side knows them by. A
/// request: `link` is the code the app sends, `poll` the hash of what the connect page asks with, `code` that of the
/// OAuth 2 code (made of `salt` and what it asks with). `salt` of a service makes `out`.
const INDEXES: &str = "
 CREATE UNIQUE INDEX IF NOT EXISTS services_yos_waiting ON services_yos (bot, user)
    WHERE user IS NOT NULL AND cron_due IS NOT NULL AND cron_done IS NULL;
 CREATE INDEX IF NOT EXISTS services_yos_due ON services_yos (bot, cron_due)
    WHERE cron_due IS NOT NULL AND cron_done IS NULL;
 CREATE INDEX IF NOT EXISTS services_yos_user ON services_yos (bot, user) WHERE user IS NOT NULL;
 CREATE INDEX IF NOT EXISTS services_yos_at ON services_yos (bot, at);
 CREATE TABLE IF NOT EXISTS services_secrets (
    bot TEXT NOT NULL,
    user TEXT NOT NULL,
    key BLOB NOT NULL UNIQUE,
    out TEXT NOT NULL,
    PRIMARY KEY (bot, user)
 ) STRICT;
 CREATE TABLE IF NOT EXISTS services_requests (
    bot TEXT NOT NULL,
    link TEXT PRIMARY KEY,
    poll BLOB NOT NULL UNIQUE,
    salt BLOB NOT NULL,
    state TEXT NOT NULL,
    at INTEGER NOT NULL,
    user TEXT,
    declined INTEGER NOT NULL DEFAULT 0,
    code BLOB UNIQUE,
    code_at INTEGER,
    CHECK ((code IS NULL) = (code_at IS NULL) AND (code IS NULL OR user IS NOT NULL) AND (declined = 0 OR user IS NULL))
 ) STRICT;
 CREATE INDEX IF NOT EXISTS services_requests_at ON services_requests (bot, at);
 CREATE TABLE IF NOT EXISTS services_salts (bot TEXT PRIMARY KEY, salt BLOB NOT NULL) STRICT;";

pub struct Book {
    db: Mutex<Connection>,
    bot: String,
}

/// What became of a request to connect.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Waits,
    /// The code for the other side, and its state.
    Allowed(String, String),
    /// Its state.
    Declined(String),
    /// Or never there.
    Over,
}

/// A Yo of a connected user.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Kept {
    pub id: String,
    pub kind: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reminder {
    pub id: i64,
    pub user: String,
}

impl Book {
    pub fn open(path: &Path, bot: &str) -> rusqlite::Result<Book> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(10))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.pragma_update(None, "secure_delete", true)?;
        db.execute_batch(&format!("BEGIN IMMEDIATE; {}", TABLE.replace("NAME", "services_yos")))?;
        // A table of before 11/2026: no kind, and nobody without a reminder.
        let old: bool = db.query_row(
            "SELECT NOT EXISTS (SELECT 1 FROM pragma_table_info('services_yos') WHERE name = 'kind')",
            [],
            |r| r.get(0),
        )?;
        if old {
            db.execute_batch(&format!(
                "{}
                 INSERT INTO services_yos_new (id, bot, way, at, user, cron_due, cron_done)
                    SELECT id, bot, way, at, user, cron_due, cron_done FROM services_yos;
                 DROP TABLE services_yos;
                 ALTER TABLE services_yos_new RENAME TO services_yos;",
                TABLE.replace("NAME", "services_yos_new")
            ))?;
        }
        // Connections of before 11/2026 had their code in the same row.
        let old: bool = db.query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('services_secrets') WHERE name = 'code')",
            [],
            |r| r.get(0),
        )?;
        if old {
            db.execute_batch("ALTER TABLE services_secrets RENAME TO services_secrets_old")?;
        }
        // Requests of before they could be declined.
        let old_requests: bool = db.query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('services_requests'))
                AND NOT EXISTS (SELECT 1 FROM pragma_table_info('services_requests') WHERE name = 'declined')",
            [],
            |r| r.get(0),
        )?;
        if old_requests {
            db.execute_batch("ALTER TABLE services_requests ADD COLUMN declined INTEGER NOT NULL DEFAULT 0")?;
        }
        db.execute_batch(INDEXES)?;
        if old {
            db.execute_batch(
                "INSERT INTO services_secrets (bot, user, key, out)
                    SELECT bot, user, key, out FROM services_secrets_old WHERE key IS NOT NULL;
                 DROP TABLE services_secrets_old;",
            )?;
        }
        db.execute("INSERT INTO services_salts (bot, salt) VALUES (?1, randomblob(32)) ON CONFLICT DO NOTHING", [bot])?;
        db.execute_batch("COMMIT")?;
        Ok(Book { db: Mutex::new(db), bot: bot.to_owned() })
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A Yo that came: its row.
    pub fn received(&self, now: i64) -> rusqlite::Result<i64> {
        self.row("received", now)
    }

    /// A Yo that went.
    pub fn sent(&self, now: i64) -> rusqlite::Result<i64> {
        self.row("sent", now)
    }

    fn row(&self, way: &str, now: i64) -> rusqlite::Result<i64> {
        let db = self.db();
        db.execute("INSERT INTO services_yos (bot, way, at) VALUES (?1, ?2, ?3)", params![self.bot, way, now])?;
        Ok(db.last_insert_rowid())
    }

    /// A Yo that came is a connected user's: it is kept with them.
    pub fn keep(&self, row: i64, user: &str, kind: &str) -> rusqlite::Result<()> {
        let query = "UPDATE services_yos SET user = ?3, kind = ?4 WHERE id = ?2 AND bot = ?1 AND EXISTS
            (SELECT 1 FROM services_secrets WHERE bot = ?1 AND user = ?3)";
        self.db().execute(query, params![self.bot, row, user, kind]).map(drop)
    }

    /// What a connected user sent, the newest first.
    pub fn kept(&self, user: &str, now: i64) -> rusqlite::Result<Vec<Kept>> {
        let db = self.db();
        let mut rows = db.prepare_cached(
            "SELECT id, kind, at FROM services_yos WHERE bot = ?1 AND user = ?2 AND kind IS NOT NULL AND at > ?3
             ORDER BY at DESC, id DESC LIMIT ?4",
        )?;
        let kept = rows.query_map(params![self.bot, user, now - KEPT_S, ASKED], |r| {
            Ok(Kept { id: r.get::<_, i64>(0)?.to_string(), kind: r.get(1)?, at: r.get(2)? })
        })?;
        kept.collect()
    }

    /// A request to connect: the code the app will send, the hash of what the connect page asks with, the other side's
    /// state. False: too many wait.
    pub fn request(&self, link: &str, poll: &[u8; 32], state: &str, now: i64) -> rusqlite::Result<bool> {
        let changed = self.db().execute(
            "INSERT INTO services_requests (bot, link, poll, salt, state, at) SELECT ?1, ?2, ?3, randomblob(32), ?4, ?5
             WHERE (SELECT count(*) FROM services_requests WHERE bot = ?1) < ?6",
            params![self.bot, link, poll, state, now, MAX_REQUESTS],
        )?;
        Ok(changed == 1)
    }

    /// `user` allowed a request: theirs from now on, whoever allows it too. Not one there is not, or that is over.
    pub fn allow(&self, user: &str, link: &str, now: i64) -> rusqlite::Result<bool> {
        let changed = self.db().execute(
            "UPDATE services_requests SET user = ?2
             WHERE bot = ?1 AND link = ?3 AND at > ?4 AND declined = 0 AND (user IS NULL OR user = ?2)
               AND ((SELECT count(*) FROM services_secrets WHERE bot = ?1) < ?5
                 OR EXISTS (SELECT 1 FROM services_secrets WHERE bot = ?1 AND user = ?2))",
            params![self.bot, user, link, now - REQUEST_S, MAX_CONNECTED],
        )?;
        Ok(changed == 1)
    }

    /// A request was declined, by whoever opened its link first: nobody allows it any more.
    pub fn decline(&self, link: &str, now: i64) -> rusqlite::Result<()> {
        let query =
            "UPDATE services_requests SET declined = 1 WHERE bot = ?1 AND link = ?2 AND at > ?3 AND user IS NULL";
        self.db().execute(query, params![self.bot, link, now - REQUEST_S]).map(drop)
    }

    /// What became of a request, by what the connect page asks with. Once allowed its code, the same each time: an answer
    /// lost, or asked for twice, is no harm, and it becomes a key once.
    pub fn asked(&self, poll: &str, now: i64) -> rusqlite::Result<Request> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let hash: [u8; 32] = Sha256::digest(poll).into();
        let found = tx
            .query_row(
                "SELECT declined, user IS NOT NULL, salt, state, coalesce(code_at + ?4 > ?3, at + ?5 > ?3)
                 FROM services_requests WHERE bot = ?1 AND poll = ?2",
                params![self.bot, hash, now, CODE_S, REQUEST_S],
                |r| {
                    let (declined, allowed): (bool, bool) = (r.get(0)?, r.get(1)?);
                    Ok((declined, allowed, r.get::<_, Vec<u8>>(2)?, r.get::<_, String>(3)?, r.get::<_, bool>(4)?))
                },
            )
            .optional()?;
        let Some((declined, allowed, salt, state, true)) = found else { return Ok(Request::Over) };
        if declined {
            return Ok(Request::Declined(state));
        }
        if !allowed {
            return Ok(Request::Waits);
        }
        let code = URL_SAFE_NO_PAD.encode(Sha256::new().chain_update(salt).chain_update(poll).finalize());
        tx.execute(
            "UPDATE services_requests SET code = ?3, code_at = ?4 WHERE bot = ?1 AND poll = ?2 AND code IS NULL",
            params![self.bot, hash, Sha256::digest(&code).as_slice(), now],
        )?;
        tx.commit()?;
        Ok(Request::Allowed(code, state))
    }

    /// `user` is connected no more, and nothing is kept of theirs but a reminder.
    /// Whether they were, or had allowed it.
    pub fn disconnect(&self, user: &str) -> rusqlite::Result<bool> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let who = params![self.bot, user];
        let was = tx.execute("DELETE FROM services_secrets WHERE bot = ?1 AND user = ?2", who)?;
        let allowed = tx.execute("DELETE FROM services_requests WHERE bot = ?1 AND user = ?2", who)?;
        tx.execute(
            "UPDATE services_yos SET kind = NULL, user = iif(cron_due IS NULL, NULL, user)
             WHERE bot = ?1 AND user = ?2 AND kind IS NOT NULL",
            who,
        )?;
        tx.commit()?;
        Ok(was + allowed > 0)
    }

    /// The code of a request becomes its user's key, once: the user, and the code the app sent. What the other side
    /// knows them by stays the same, also after an end.
    pub fn exchange(&self, code: &str, key: &[u8; 32], now: i64) -> rusqlite::Result<Option<(String, String)>> {
        let mut db = self.db();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let found: Option<(String, String)> = tx
            .query_row(
                "DELETE FROM services_requests WHERE bot = ?1 AND code = ?2 AND code_at > ?3 RETURNING user, link",
                params![self.bot, Sha256::digest(code).as_slice(), now - CODE_S],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((user, _)) = &found {
            let query = "SELECT salt FROM services_salts WHERE bot = ?1";
            let salt: Vec<u8> = tx.query_row(query, [&self.bot], |r| r.get(0))?;
            let out = URL_SAFE_NO_PAD.encode(Sha256::new().chain_update(salt).chain_update(user).finalize());
            let made = tx.execute(
                "INSERT INTO services_secrets (bot, user, key, out) SELECT ?1, ?2, ?3, ?4
                 WHERE (SELECT count(*) FROM services_secrets WHERE bot = ?1) < ?5
                    OR EXISTS (SELECT 1 FROM services_secrets WHERE bot = ?1 AND user = ?2)
                 ON CONFLICT (bot, user) DO UPDATE SET key = excluded.key, out = excluded.out",
                params![self.bot, user, key, out, MAX_CONNECTED],
            )?;
            // Too many meanwhile: nothing of it is done.
            if made == 0 {
                return Ok(None);
            }
        }
        tx.commit()?;
        Ok(found)
    }

    /// Whom a call with this key is for, and what the other side knows them by.
    pub fn connected_by(&self, key: &[u8; 32]) -> rusqlite::Result<Option<(String, String)>> {
        let query = "SELECT user, out FROM services_secrets WHERE bot = ?1 AND key = ?2";
        self.db().query_row(query, params![self.bot, key], |r| Ok((r.get(0)?, r.get(1)?))).optional()
    }

    /// What the other side knows `user` by, if connected.
    pub fn out(&self, user: &str) -> rusqlite::Result<Option<String>> {
        let query = "SELECT out FROM services_secrets WHERE bot = ?1 AND user = ?2";
        self.db().query_row(query, params![self.bot, user], |r| r.get(0)).optional()
    }

    /// How many went since `since`.
    pub fn sent_since(&self, since: i64) -> rusqlite::Result<usize> {
        self.db().query_row(
            "SELECT count(*) FROM services_yos WHERE bot = ?1 AND way = 'sent' AND at > ?2",
            params![self.bot, since],
            |r| r.get::<_, i64>(0).map(|n| n as usize),
        )
    }

    /// A reminder of `user`, on the row of the Yo that asked for it, instead of one that waits.
    pub fn remind(&self, row: i64, user: &str, due: i64) -> rusqlite::Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        unremind(&tx, &self.bot, user)?;
        tx.execute(
            "UPDATE services_yos SET user = ?3, cron_due = ?4 WHERE id = ?2 AND bot = ?1 AND (SELECT count(*) FROM services_yos
             WHERE bot = ?1 AND cron_due IS NOT NULL AND cron_done IS NULL) < ?5",
            params![self.bot, row, user, due, MAX_WAITING],
        )?;
        tx.commit()
    }

    /// Whether a reminder of `user` waits.
    pub fn remembers(&self, user: &str, now: i64) -> rusqlite::Result<bool> {
        self.db().query_row(
            "SELECT EXISTS (SELECT 1 FROM services_yos WHERE bot = ?1 AND user = ?2 AND cron_done IS NULL AND cron_due > ?3)",
            params![self.bot, user, now - TOO_LATE_S],
            |r| r.get(0),
        )
    }

    /// Waiting reminders whose time has come, the oldest first.
    pub fn due(&self, now: i64) -> rusqlite::Result<Vec<Reminder>> {
        let db = self.db();
        let mut rows = db.prepare_cached(
            "SELECT id, user FROM services_yos WHERE bot = ?1 AND cron_due > ?2 AND cron_due <= ?3 AND cron_done IS NULL
             AND user IS NOT NULL ORDER BY cron_due LIMIT 1000",
        )?;
        let reminders = rows
            .query_map(params![self.bot, now - TOO_LATE_S, now], |r| Ok(Reminder { id: r.get(0)?, user: r.get(1)? }))?;
        reminders.collect()
    }

    /// Whether a reminder still waits (not forgotten or replaced meanwhile).
    pub fn waits(&self, id: i64) -> rusqlite::Result<bool> {
        let query =
            "SELECT EXISTS (SELECT 1 FROM services_yos WHERE id = ?1 AND user IS NOT NULL AND cron_done IS NULL)";
        self.db().query_row(query, [id], |r| r.get(0))
    }

    /// A reminder was sent.
    pub fn done(&self, id: i64, now: i64) -> rusqlite::Result<()> {
        let query =
            "UPDATE services_yos SET cron_done = ?2 WHERE id = ?1 AND cron_due IS NOT NULL AND cron_done IS NULL";
        self.db().execute(query, [id, now]).map(drop)
    }

    /// Nothing of `user` is left.
    pub fn forget(&self, user: &str) -> rusqlite::Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        unremind(&tx, &self.bot, user)?;
        tx.execute("UPDATE services_yos SET user = NULL WHERE bot = ?1 AND user = ?2", params![self.bot, user])?;
        tx.commit()
    }

    /// What is over for `KEPT_S` loses its user.
    pub fn tidy(&self, now: i64) -> rusqlite::Result<()> {
        self.db().execute(
            "UPDATE services_yos SET user = NULL
             WHERE bot = ?1 AND user IS NOT NULL AND coalesce(cron_done, cron_due + ?3, at) < ?2",
            params![self.bot, now - KEPT_S, TOO_LATE_S],
        )?;
        self.db().execute(
            "DELETE FROM services_requests WHERE bot = ?1 AND coalesce(code_at + ?3, at + ?4) <= ?2",
            params![self.bot, now, CODE_S, REQUEST_S],
        )?;
        Ok(())
    }
}

/// The waiting reminder of a user is none any more: its row a plain Yo again.
fn unremind(db: &Connection, bot: &str, user: &str) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE services_yos SET user = iif(kind IS NULL, NULL, user), cron_due = NULL
         WHERE bot = ?1 AND user = ?2 AND cron_due IS NOT NULL AND cron_done IS NULL",
        params![bot, user],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Book {
        Book::open(Path::new(":memory:"), "DST").unwrap()
    }

    fn count(book: &Book, clause: &str) -> i64 {
        book.db().query_row(&format!("SELECT count(*) FROM services_yos WHERE {clause}"), [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn plain_traffic_has_a_time_and_nobody() {
        let book = book();
        book.received(10).unwrap();
        book.sent(11).unwrap();
        assert_eq!((book.sent_since(10).unwrap(), book.sent_since(11).unwrap()), (1, 0));
        assert_eq!(count(&book, "user IS NULL AND cron_due IS NULL"), 2);
        for bad in ["'received', 1, 'a', NULL, NULL", "'sent', 1, 'a', 5, NULL", "'received', 1, NULL, NULL, 5"] {
            let row = format!("INSERT INTO services_yos (bot, way, at, user, cron_due, cron_done) VALUES ('X', {bad})");
            assert!(book.db().execute(&row, []).is_err(), "no user without a reminder, whatever asks for it: {bad}");
        }
    }

    #[test]
    fn a_reminder_comes_due_once_and_is_done() {
        let book = book();
        let row = book.received(10).unwrap();
        book.remind(row, "a", 100).unwrap();
        assert_eq!(count(&book, "1"), 1, "on the Yo's own row");
        assert!(book.remembers("a", 50).unwrap() && !book.remembers("b", 50).unwrap());
        assert_eq!(book.due(99).unwrap(), []);
        assert_eq!(book.due(100).unwrap(), [Reminder { id: row, user: "a".into() }]);
        book.done(row, 101).unwrap();
        book.done(row, 102).unwrap();
        assert_eq!(count(&book, "cron_done = 101"), 1);
        assert!(book.due(1000).unwrap().is_empty() && !book.waits(row).unwrap() && !book.remembers("a", 102).unwrap());
        book.tidy(101 + KEPT_S).unwrap();
        assert_eq!(count(&book, "user = 'a'"), 1);
        book.tidy(102 + KEPT_S).unwrap();
        assert_eq!(count(&book, "user IS NULL AND cron_done = 101"), 1);
    }

    #[test]
    fn one_waits_per_user_and_forgetting_leaves_nothing() {
        let book = book();
        let (first, second, other) = (book.received(1).unwrap(), book.received(2).unwrap(), book.received(3).unwrap());
        book.remind(first, "a", 50).unwrap();
        book.remind(second, "a", 60).unwrap();
        book.remind(other, "b", 70).unwrap();
        assert!(!book.waits(first).unwrap() && book.waits(second).unwrap());
        assert_eq!(count(&book, "cron_due IS NOT NULL"), 2, "the first is a plain Yo again");
        book.forget("a").unwrap();
        assert_eq!(count(&book, "user = 'a' OR cron_due = 60"), 0);
        assert_eq!(book.due(80).unwrap().len(), 1);
    }

    #[test]
    fn too_late_is_never_sent_and_loses_its_user() {
        let book = book();
        let row = book.received(1).unwrap();
        book.remind(row, "a", 100).unwrap();
        assert_eq!(book.due(100 + TOO_LATE_S - 1).unwrap().len(), 1);
        assert_eq!(book.due(100 + TOO_LATE_S).unwrap(), []);
        assert!(!book.remembers("a", 100 + TOO_LATE_S).unwrap());
        book.tidy(101 + TOO_LATE_S + KEPT_S).unwrap();
        assert_eq!(count(&book, "user IS NULL AND cron_due = 100"), 1);
    }

    #[test]
    fn a_connection_is_made_once_and_its_yos_are_kept() {
        let book = book();
        let code = |poll: &str, now| match book.asked(poll, now).unwrap() {
            Request::Allowed(code, state) => {
                assert_eq!(state, "s");
                code
            }
            other => panic!("{other:?}"),
        };
        let request = |link: &str, poll: &str, now| {
            assert!(book.request(link, &Sha256::digest(poll).into(), "s", now).unwrap());
        };
        assert!(!book.allow("a", "l", 100).unwrap(), "no request, nothing allowed");
        request("l", "p", 100);
        assert_eq!((book.asked("p", 101).unwrap(), book.asked("x", 101).unwrap()), (Request::Waits, Request::Over));
        assert_eq!(book.asked("p", 100 + REQUEST_S).unwrap(), Request::Over);
        assert!(!book.allow("a", "l", 100 + REQUEST_S).unwrap(), "too late");
        assert!(book.allow("a", "l", 101).unwrap() && book.allow("a", "l", 102).unwrap());
        assert!(!book.allow("b", "l", 102).unwrap(), "the first is the one");
        assert_eq!(book.out("a").unwrap(), None);
        let first = code("p", 110);
        assert_eq!(code("p", 111), first);
        assert_eq!(book.asked("p", 110 + CODE_S).unwrap(), Request::Over);
        assert_eq!(book.exchange("p", &[2; 32], 111).unwrap(), None);
        assert_eq!(book.exchange(&first, &[2; 32], 110 + CODE_S).unwrap(), None, "too late");
        assert_eq!(book.exchange(&first, &[2; 32], 111).unwrap(), Some(("a".into(), "l".into())));
        assert_eq!(book.exchange(&first, &[3; 32], 112).unwrap(), None, "once");
        assert_eq!(book.asked("p", 112).unwrap(), Request::Over);
        assert!(!book.allow("a", "l", 112).unwrap(), "no second time");
        let out = book.out("a").unwrap().unwrap();
        assert_eq!(book.connected_by(&[2; 32]).unwrap(), Some(("a".into(), out.clone())));
        assert_eq!(book.connected_by(&[3; 32]).unwrap(), None);
        request("m", "q", 200);
        book.allow("a", "m", 201).unwrap();
        let second = code("q", 202);
        assert!(book.connected_by(&[2; 32]).unwrap().is_some(), "until the new one is made");
        book.exchange(&second, &[4; 32], 203).unwrap().unwrap();
        assert_eq!(book.connected_by(&[2; 32]).unwrap(), None);
        assert_eq!(book.out("a").unwrap(), Some(out.clone()), "the same as before");
        request("n", "r", 300);
        book.allow("b", "n", 301).unwrap();
        let third = code("r", 302);
        assert!(book.disconnect("b").unwrap() && !book.disconnect("b").unwrap());
        assert_eq!(book.exchange(&third, &[5; 32], 303).unwrap(), None, "ended");
        // An approval that failed, and a disconnect: nothing of it comes later.
        request("late", "late", 350);
        {
            let db = book.db();
            db.execute_batch(
                "CREATE TEMP TRIGGER full BEFORE UPDATE ON services_requests BEGIN SELECT RAISE(ABORT, 'full'); END",
            )
            .unwrap();
        }
        assert!(book.allow("a", "late", 351).is_err());
        assert!(book.disconnect("a").unwrap());
        book.db().execute_batch("DROP TRIGGER full").unwrap();
        assert_eq!(book.asked("late", 352).unwrap(), Request::Waits);
        assert_eq!(book.out("a").unwrap(), None);
        request("more", "more", 353);
        book.allow("a", "more", 354).unwrap();
        book.disconnect("a").unwrap();
        assert!(book.asked("more", 355).unwrap() == Request::Over && !book.allow("a", "more", 355).unwrap());
        request("again", "again", 356);
        book.allow("a", "again", 357).unwrap();
        book.exchange(&code("again", 358), &[6; 32], 359).unwrap().unwrap();
        request("u", "v", 400);
        book.decline("u", 401).unwrap();
        assert_eq!(book.asked("v", 402).unwrap(), Request::Declined("s".into()));
        assert!(!book.allow("a", "u", 402).unwrap(), "declined stays");
        request("w", "y", 400);
        book.allow("a", "w", 401).unwrap();
        book.decline("w", 402).unwrap();
        code("y", 403);
        {
            let db = book.db();
            db.execute_batch(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100000)
                INSERT INTO services_secrets SELECT 'DST', 'n' || i, randomblob(32), 'o' FROM n",
            )
            .unwrap();
        }
        request("full", "full", 400);
        assert!(!book.allow("new", "full", 401).unwrap(), "as many as there may be");
        book.db().execute("UPDATE services_requests SET user = 'new' WHERE link = 'full'", []).unwrap();
        let late = code("full", 402);
        assert_eq!(book.exchange(&late, &[8; 32], 403).unwrap(), None, "nor at the exchange");
        book.db().execute("DELETE FROM services_secrets WHERE user LIKE 'n%'", []).unwrap();
        request("o", "t", 400);
        book.tidy(400 + REQUEST_S).unwrap();
        let left: i64 = book.db().query_row("SELECT count(*) FROM services_requests", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);

        let (first, second) = (book.received(10).unwrap(), book.received(20).unwrap());
        book.keep(first, "a", "yo").unwrap();
        book.keep(second, "a", "picture").unwrap();
        let ids: Vec<_> = book.kept("a", 30).unwrap().into_iter().map(|k| (k.id, k.kind, k.at)).collect();
        assert_eq!(ids, [(second.to_string(), "picture".into(), 20), (first.to_string(), "yo".into(), 10)]);
        book.remind(second, "a", 100).unwrap();
        book.remind(first, "a", 100).unwrap();
        assert_eq!(book.kept("a", 30).unwrap().len(), 2, "another reminder takes none of them away");
        assert!(book.kept("b", 30).unwrap().is_empty());
        book.keep(first, "b", "yo").unwrap();
        assert!(book.kept("b", 30).unwrap().is_empty(), "not connected");
        book.disconnect("a").unwrap();
        assert!(book.kept("a", 30).unwrap().is_empty() && book.out("a").unwrap().is_none());
        assert_eq!(count(&book, "user IS NOT NULL"), 1, "the reminder stays");
        book.tidy(30 + KEPT_S + TOO_LATE_S + 100).unwrap();
        assert_eq!(count(&book, "user IS NOT NULL"), 0);
    }

    #[test]
    fn a_book_of_before_is_taken_over() {
        let path = std::env::temp_dir().join(format!("yo-book-old-{}", std::process::id()));
        let old = "CREATE TABLE services_yos (id INTEGER PRIMARY KEY, bot TEXT NOT NULL, way TEXT NOT NULL,
            at INTEGER NOT NULL, user TEXT, cron_due INTEGER, cron_done INTEGER,
            CHECK (cron_due IS NOT NULL OR (user IS NULL AND cron_done IS NULL))) STRICT;
            CREATE TABLE services_secrets (bot TEXT NOT NULL, user TEXT NOT NULL, code BLOB, code_at INTEGER,
            key BLOB UNIQUE, out TEXT, PRIMARY KEY (bot, user)) STRICT;
            INSERT INTO services_secrets VALUES ('DST', 'a', NULL, NULL, zeroblob(32), 'o'), ('DST', 'b', x'01', 1, NULL, NULL);
            CREATE TABLE services_requests (bot TEXT NOT NULL, link TEXT PRIMARY KEY, poll BLOB NOT NULL UNIQUE,
            salt BLOB NOT NULL, state TEXT NOT NULL, at INTEGER NOT NULL, user TEXT, code BLOB UNIQUE, code_at INTEGER) STRICT;
            CREATE UNIQUE INDEX services_yos_waiting ON services_yos (bot, user)
                WHERE user IS NOT NULL AND cron_done IS NULL;
            INSERT INTO services_yos VALUES (7, 'DST', 'received', 1, 'a', 50, NULL);";
        Connection::open(&path).unwrap().execute_batch(old).unwrap();
        for _ in 0..2 {
            let book = Book::open(&path, "DST").unwrap();
            assert_eq!(book.due(60).unwrap(), [Reminder { id: 7, user: "a".into() }]);
            assert_eq!(book.connected_by(&[0; 32]).unwrap(), Some(("a".into(), "o".into())));
            assert_eq!(book.out("b").unwrap(), None);
            let _ = book.request("l", &Sha256::digest("p").into(), "s", 1);
            book.decline("l", 2).unwrap();
            assert_eq!(book.asked("p", 3).unwrap(), Request::Declined("s".into()));
            let row = book.received(2).unwrap();
            book.keep(row, "a", "yo").unwrap();
        }
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn one_book_keeps_the_services_apart() {
        let path = std::env::temp_dir().join(format!("yo-book-{}", std::process::id()));
        let (a, b) = (Book::open(&path, "A").unwrap(), Book::open(&path, "B").unwrap());
        let row = a.received(1).unwrap();
        b.remind(row, "u", 5).unwrap();
        assert!(!a.remembers("u", 1).unwrap(), "no row of another bot");
        a.remind(row, "u", 5).unwrap();
        assert!(!b.remembers("u", 1).unwrap() && b.due(9).unwrap().is_empty());
        b.forget("u").unwrap();
        assert!(a.waits(row).unwrap());
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
