//! The book: one table of all bot traffic, written by the executors alone. A row per Yo received or sent, with its
//! time; who it was only on one with a reminder, for a service that remembers users, or of a connected user. And
//! who connected what: a second table, of hashes.

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// How long who was reminded stays readable after it is over (for the admin page); the row stays for the statistics.
const KEPT_S: i64 = 7 * 86_400;
/// A reminder this late is not sent any more.
const TOO_LATE_S: i64 = 86_400;
/// What a connected user sent, for the other side to ask for.
const ASKED: i64 = 50;
/// How long a connect link's code may be turned into a key.
const CODE_S: i64 = 600;
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
/// A connection: `key` is the hash of what a call for the user carries, `out` what the other side knows them by; both
/// once `code` (a hash too) was turned into them.
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
    code BLOB,
    code_at INTEGER,
    key BLOB UNIQUE,
    out TEXT,
    CHECK ((code IS NULL) = (code_at IS NULL) AND (key IS NULL) = (out IS NULL)),
    PRIMARY KEY (bot, user)
 ) STRICT;";

pub struct Book {
    db: Mutex<Connection>,
    bot: String,
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
        db.execute_batch(INDEXES)?;
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
            (SELECT 1 FROM services_secrets WHERE bot = ?1 AND user = ?3 AND out IS NOT NULL)";
        self.db().execute(query, params![self.bot, row, user, kind]).map(drop)
    }

    /// What a connected user sent, the newest first.
    pub fn kept(&self, user: &str, now: i64) -> rusqlite::Result<Vec<Kept>> {
        let db = self.db();
        let mut rows = db.prepare_cached(
            "SELECT id, kind, at FROM services_yos WHERE bot = ?1 AND user = ?2 AND kind IS NOT NULL AND at > ?3
             ORDER BY id DESC LIMIT ?4",
        )?;
        let kept = rows.query_map(params![self.bot, user, now - KEPT_S, ASKED], |r| {
            Ok(Kept { id: r.get::<_, i64>(0)?.to_string(), kind: r.get(1)?, at: r.get(2)? })
        })?;
        kept.collect()
    }

    /// `user` allowed a connection: its code, instead of one that waits. A connection there is goes on until the code
    /// is turned into a new one.
    pub fn connect(&self, user: &str, code: &[u8; 32], now: i64) -> rusqlite::Result<bool> {
        let changed = self.db().execute(
            "INSERT INTO services_secrets (bot, user, code, code_at) SELECT ?1, ?2, ?3, ?4
             WHERE (SELECT count(*) FROM services_secrets WHERE bot = ?1) < ?5
                OR EXISTS (SELECT 1 FROM services_secrets WHERE bot = ?1 AND user = ?2)
             ON CONFLICT (bot, user) DO UPDATE SET code = excluded.code, code_at = excluded.code_at",
            params![self.bot, user, code, now, MAX_CONNECTED],
        )?;
        Ok(changed == 1)
    }

    /// `user` is connected no more, and nothing is kept of theirs but a reminder.
    pub fn disconnect(&self, user: &str) -> rusqlite::Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let who = params![self.bot, user];
        tx.execute("DELETE FROM services_secrets WHERE bot = ?1 AND user = ?2", who)?;
        tx.execute(
            "UPDATE services_yos SET kind = NULL, user = iif(cron_due IS NULL, NULL, user)
             WHERE bot = ?1 AND user = ?2 AND kind IS NOT NULL",
            who,
        )?;
        tx.commit()
    }

    /// The code of a connection becomes its key, once.
    pub fn exchange(&self, code: &[u8; 32], key: &[u8; 32], out: &str, now: i64) -> rusqlite::Result<bool> {
        let changed = self.db().execute(
            "UPDATE services_secrets SET code = NULL, code_at = NULL, key = ?3, out = ?4
             WHERE bot = ?1 AND code = ?2 AND code_at > ?5",
            params![self.bot, code, key, out, now - CODE_S],
        )?;
        Ok(changed == 1)
    }

    /// Whom a call with this key is for, and what the other side knows them by.
    pub fn connected_by(&self, key: &[u8; 32]) -> rusqlite::Result<Option<(String, String)>> {
        let query = "SELECT user, out FROM services_secrets WHERE bot = ?1 AND key = ?2";
        self.db().query_row(query, params![self.bot, key], |r| Ok((r.get(0)?, r.get(1)?))).optional()
    }

    /// What the other side knows `user` by, if connected.
    pub fn out(&self, user: &str) -> rusqlite::Result<Option<String>> {
        let query = "SELECT out FROM services_secrets WHERE bot = ?1 AND user = ?2 AND out IS NOT NULL";
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
        // A connect link's code not used in time is none any more.
        let late = params![self.bot, now - CODE_S];
        self.db().execute("DELETE FROM services_secrets WHERE bot = ?1 AND code_at <= ?2 AND key IS NULL", late)?;
        self.db().execute(
            "UPDATE services_secrets SET code = NULL, code_at = NULL WHERE bot = ?1 AND code_at <= ?2",
            late,
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
        let (book, code) = (book(), [1; 32]);
        book.connect("a", &code, 100).unwrap();
        assert_eq!(book.out("a").unwrap(), None);
        assert!(!book.exchange(&[9; 32], &[2; 32], "o", 101).unwrap());
        assert!(!book.exchange(&code, &[2; 32], "o", 100 + CODE_S).unwrap(), "too late");
        assert!(book.exchange(&code, &[2; 32], "o", 101).unwrap());
        assert!(!book.exchange(&code, &[3; 32], "p", 102).unwrap(), "once");
        assert_eq!(book.connected_by(&[2; 32]).unwrap(), Some(("a".into(), "o".into())));
        assert_eq!((book.connected_by(&[3; 32]).unwrap(), book.out("a").unwrap()), (None, Some("o".into())));
        book.connect("a", &[4; 32], 200).unwrap();
        assert!(book.connected_by(&[2; 32]).unwrap().is_some(), "until the new one is made");
        book.tidy(200 + CODE_S).unwrap();
        assert!(!book.exchange(&[4; 32], &[5; 32], "q", 201).unwrap() && book.out("a").unwrap().is_some());
        book.connect("b", &[6; 32], 1).unwrap();
        book.tidy(1 + CODE_S).unwrap();
        assert!(!book.exchange(&[6; 32], &[7; 32], "q", 2).unwrap(), "a row of a code alone is gone");

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
            CREATE UNIQUE INDEX services_yos_waiting ON services_yos (bot, user)
                WHERE user IS NOT NULL AND cron_done IS NULL;
            INSERT INTO services_yos VALUES (7, 'DST', 'received', 1, 'a', 50, NULL);";
        Connection::open(&path).unwrap().execute_batch(old).unwrap();
        for _ in 0..2 {
            let book = Book::open(&path, "DST").unwrap();
            assert_eq!(book.due(60).unwrap(), [Reminder { id: 7, user: "a".into() }]);
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
