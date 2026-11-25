//! The book: one table of all bot traffic, written by the executors alone. A row per Yo received or sent, with its
//! time; who it was only on one with a reminder, and only for a service that remembers users.

use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// How long who was reminded stays readable after it is over (for the admin page); the row stays for the statistics.
const KEPT_S: i64 = 7 * 86_400;
/// A reminder this late is not sent any more.
const TOO_LATE_S: i64 = 86_400;
/// Waiting reminders of one service.
const MAX_WAITING: i64 = 100_000;

pub struct Book {
    db: Mutex<Connection>,
    bot: String,
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
        // Waiting: due and not done. Done, or far too late: over.
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS services_yos (
                id INTEGER PRIMARY KEY,
                bot TEXT NOT NULL,
                way TEXT NOT NULL CHECK (way IN ('received', 'sent')),
                at INTEGER NOT NULL,
                user TEXT,
                cron_due INTEGER CHECK (cron_due IS NULL OR way = 'received'),
                cron_done INTEGER,
                CHECK (cron_due IS NOT NULL OR (user IS NULL AND cron_done IS NULL))
             ) STRICT;
             CREATE UNIQUE INDEX IF NOT EXISTS services_yos_waiting ON services_yos (bot, user)
                WHERE user IS NOT NULL AND cron_done IS NULL;
             CREATE INDEX IF NOT EXISTS services_yos_due ON services_yos (bot, cron_due)
                WHERE cron_due IS NOT NULL AND cron_done IS NULL;
             CREATE INDEX IF NOT EXISTS services_yos_user ON services_yos (bot, user) WHERE user IS NOT NULL;
             CREATE INDEX IF NOT EXISTS services_yos_at ON services_yos (bot, at);",
        )?;
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
             WHERE bot = ?1 AND user IS NOT NULL AND coalesce(cron_done, cron_due + ?3) < ?2",
            params![self.bot, now - KEPT_S, TOO_LATE_S],
        )?;
        Ok(())
    }
}

/// The waiting reminder of a user is none any more: its row a plain Yo again.
fn unremind(db: &Connection, bot: &str, user: &str) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE services_yos SET user = NULL, cron_due = NULL WHERE bot = ?1 AND user = ?2 AND cron_done IS NULL",
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
