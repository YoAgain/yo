//! The device's state where several processes share it (iOS: the app and its extensions in the App Group): one
//! SQLite database in WAL mode, its one row the sealed state (`state::seal`, encrypted as before) and the word of the
//! write that put it there.
//!
//! Readers see the last committed state in one read transaction, without any lock between the processes: a write
//! under way never hides it. A write replaces the row only over the state it started from (a compare and swap on the
//! word, inside `BEGIN IMMEDIATE`), so no process ever writes over a newer state from an older one, whatever its
//! turn did meanwhile. Only the write itself is inside the transaction, never a request to the hub.
//!
//! A connection lives for one access: between accesses no SQLite lock is held (not even WAL's shared one), so iOS
//! never suspends a process that holds a lock on a file in the group (0xdead10cc).
//!
//! As Signal-iOS keeps its database (SignalServiceKit/Storage/Database/GRDBDatabaseStorageAdapter.swift: GRDB's
//! `DatabasePool`, WAL): writes in `BEGIN IMMEDIATE`, `synchronous = NORMAL`, `fullfsync` and `checkpoint_fullfsync`
//! on (Apple's SQLite: barrier fsyncs), a busy wait for another process's write, and after each write a TRUNCATE
//! checkpoint that gives up after 50 ms of contention (SQLite's passive autocheckpoint does the rest; the last
//! connection's close checkpoints too). The database file's header is plain SQLite (the state in it is sealed), which
//! iOS needs to see to treat its locks as a database's (Signal keeps SQLCipher's first 32 bytes plain for that).

use crate::error::{Error, Result};
use crate::Store;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How long an access waits for another process's (a write, the last connection's checkpoint).
const BUSY: Duration = Duration::from_secs(3);
/// How long the checkpoint after a write waits for other connections before it leaves the WAL as it is (Signal's
/// `maxBusyTimeoutMs`).
const CHECKPOINT_BUSY: Duration = Duration::from_millis(50);

/// What a reader finds against the write it knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Snapshot {
    /// The state as this process read or wrote it last.
    Same,
    /// Written since (by another process): its word and the sealed state.
    Newer { word: String, blob: Vec<u8> },
    /// No state (signed out, or never made).
    Gone,
}

/// The database file (`-wal` and `-shm` beside it).
#[derive(Debug)]
pub struct StateDb {
    path: PathBuf,
}

/// Not readable now (the phone not unlocked since its start, another process's checkpoint): retryable, as no
/// network is. The apps take it as their "not now" (iOS: `SharedAccess.unavailable`).
fn unavailable(error: impl std::fmt::Display) -> Error {
    Error::Network(format!("shared storage is not available now: {error}"))
}

fn not_saved(error: impl std::fmt::Display) -> Error {
    Error::Save(error.to_string())
}

/// The WAL into the database and truncated, as Signal does after its writes; with other connections busy for more than
/// `CHECKPOINT_BUSY` it stays for the next one (written already, nothing lost).
fn checkpoint(conn: &Connection) {
    if conn.busy_timeout(CHECKPOINT_BUSY).is_ok() {
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }
}

/// A new word for a write: 128 random bits, never one a deleted state had.
fn new_word() -> String {
    crate::state::hex(&crate::identity::random::<16>())
}

impl StateDb {
    /// Opens the database at `path` (made if there is none; its folder must be there), in WAL mode.
    pub fn open(path: impl Into<PathBuf>) -> Result<StateDb> {
        let db = StateDb { path: path.into() };
        let conn = db.connect()?;
        // WAL is kept in the file: set once, read from there by every later connection.
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0)).map_err(unavailable)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(unavailable(format!("journal mode {mode}")));
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                word TEXT NOT NULL,
                blob BLOB NOT NULL
            )",
        )
        .map_err(unavailable)?;
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn connect(&self) -> Result<Connection> {
        #[allow(unused_mut)]
        let mut flags =
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        // Apple's SQLite: the database, its -wal and its -shm in the class the state file had (readable after the
        // first unlock since the start, as the notification service needs it).
        #[cfg(target_os = "ios")]
        {
            const SQLITE_OPEN_FILEPROTECTION_COMPLETEUNTILFIRSTUSERAUTHENTICATION: i32 = 0x0030_0000;
            flags |= OpenFlags::from_bits_retain(SQLITE_OPEN_FILEPROTECTION_COMPLETEUNTILFIRSTUSERAUTHENTICATION);
        }
        let conn = Connection::open_with_flags(&self.path, flags).map_err(unavailable)?;
        conn.busy_timeout(BUSY).map_err(unavailable)?;
        // WAL with NORMAL: a commit is durable at the next checkpoint; a crash of the app never loses one.
        conn.pragma_update(None, "synchronous", "NORMAL").map_err(unavailable)?;
        conn.pragma_update(None, "fullfsync", "ON").map_err(unavailable)?;
        conn.pragma_update(None, "checkpoint_fullfsync", "ON").map_err(unavailable)?;
        Ok(conn)
    }

    /// The state now against `known` (the word this process read or wrote last), in one read transaction.
    pub fn snapshot(&self, known: Option<&str>) -> Result<Snapshot> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred).map_err(unavailable)?;
        let word: Option<String> = tx
            .query_row("SELECT word FROM state WHERE id = 1", [], |row| row.get(0))
            .optional()
            .map_err(unavailable)?;
        Ok(match word {
            None => Snapshot::Gone,
            Some(word) if Some(word.as_str()) == known => Snapshot::Same,
            Some(word) => {
                let blob =
                    tx.query_row("SELECT blob FROM state WHERE id = 1", [], |row| row.get(0)).map_err(unavailable)?;
                Snapshot::Newer { word, blob }
            }
        })
    }

    /// The state and its word; none when there is none.
    pub fn read(&self) -> Result<Option<(String, Vec<u8>)>> {
        Ok(match self.snapshot(None)? {
            Snapshot::Newer { word, blob } => Some((word, blob)),
            _ => None,
        })
    }

    /// The word of the last write; none when there is no state.
    pub fn word(&self) -> Result<Option<String>> {
        let conn = self.connect()?;
        conn.query_row("SELECT word FROM state WHERE id = 1", [], |row| row.get(0)).optional().map_err(unavailable)
    }

    /// Puts `blob` in place of the state `known` names (none: there is none yet, a new account), atomically; its new
    /// word. Refused (`Save`) when another write came since, or the state went (signed out) though `known` names one:
    /// the step that wanted it fails, and nothing that needed it leaves.
    pub fn write(&self, known: Option<&str>, blob: &[u8]) -> Result<String> {
        let mut conn = self.connect().map_err(not_saved)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(not_saved)?;
        let word: Option<String> =
            tx.query_row("SELECT word FROM state WHERE id = 1", [], |row| row.get(0)).optional().map_err(not_saved)?;
        if word.as_deref() != known {
            return Err(Error::Save("an older state".into()));
        }
        let new = new_word();
        tx.execute("INSERT OR REPLACE INTO state (id, word, blob) VALUES (1, ?1, ?2)", params![new, blob])
            .map_err(not_saved)?;
        tx.commit().map_err(not_saved)?;
        checkpoint(&conn);
        Ok(new)
    }

    /// Signed out: the state goes, whatever was written last.
    pub fn clear(&self) -> Result<()> {
        let conn = self.connect().map_err(not_saved)?;
        conn.execute("DELETE FROM state", []).map_err(not_saved)?;
        Ok(())
    }

    /// The state file of before moved in (iOS: `yo-state`; `word_len`: the 36-character word a shared one starts
    /// with, 0 for the app's own): taken as the state when the database has none, then the file goes. A file left
    /// beside a state (stopped after the move, before the delete) goes too when it is that state; one that differs
    /// stays aside as `<file>.unmoved` (an older build wrote it after the move), never over the database's. True when
    /// it came in.
    pub fn move_in(&self, file: &Path, word_len: usize) -> Result<bool> {
        let data = match std::fs::read(file) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(unavailable(error)),
        };
        if data.len() <= word_len {
            return Ok(false); // no state in it: left as it is
        }
        let blob = &data[word_len..];
        let mut conn = self.connect().map_err(not_saved)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(not_saved)?;
        let there: Option<Vec<u8>> =
            tx.query_row("SELECT blob FROM state WHERE id = 1", [], |row| row.get(0)).optional().map_err(not_saved)?;
        let moved = match &there {
            None => {
                tx.execute("INSERT INTO state (id, word, blob) VALUES (1, ?1, ?2)", params![new_word(), blob])
                    .map_err(not_saved)?;
                true
            }
            Some(_) => false,
        };
        tx.commit().map_err(not_saved)?;
        // Only now, the state in the database: never no state. Another process may have moved it at once.
        let gone = if moved || there.as_deref() == Some(blob) {
            std::fs::remove_file(file)
        } else {
            let mut aside = file.as_os_str().to_owned();
            aside.push(".unmoved");
            std::fs::rename(file, aside)
        };
        match gone {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(unavailable(error)),
            _ => Ok(moved),
        }
    }

    /// The state of `other` (a database of the app's own, before the state moved into the shared one) moved in when
    /// this one has none; `other` is emptied once it is here (also when this one has that very state). True when it
    /// came in.
    pub fn adopt(&self, other: &StateDb) -> Result<bool> {
        let Some((_, blob)) = other.read()? else { return Ok(false) };
        let mut conn = self.connect().map_err(not_saved)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(not_saved)?;
        let there: Option<Vec<u8>> =
            tx.query_row("SELECT blob FROM state WHERE id = 1", [], |row| row.get(0)).optional().map_err(not_saved)?;
        let moved = there.is_none();
        if moved {
            tx.execute("INSERT INTO state (id, word, blob) VALUES (1, ?1, ?2)", params![new_word(), blob])
                .map_err(not_saved)?;
        }
        tx.commit().map_err(not_saved)?;
        if moved || there == Some(blob) {
            other.clear()?;
        }
        Ok(moved)
    }
}

/// The `Store` of a client on a `StateDb`: a save replaces the state this client read or wrote last
/// (`StateDb::write`), never one another process wrote since. `held` says whether the process still has its turn
/// between processes (the platform's lock; iOS takes it back when background time ends): asked before each write, so a
/// turn taken back writes nothing more.
pub struct DbStore {
    db: Arc<StateDb>,
    known: Mutex<Option<String>>,
    held: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    saved: AtomicBool,
}

impl DbStore {
    /// `known`: the word of the state the client opened (none: a new one, made by the first save).
    pub fn new(db: Arc<StateDb>, known: Option<String>, held: Option<Box<dyn Fn() -> bool + Send + Sync>>) -> Self {
        DbStore { db, known: Mutex::new(known), held, saved: AtomicBool::new(false) }
    }

    pub fn db(&self) -> &Arc<StateDb> {
        &self.db
    }

    pub fn known(&self) -> Option<String> {
        self.known.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The state now, if another process wrote it since this client read or wrote it last.
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.db.snapshot(self.known().as_deref())
    }

    /// The client took in the state of `word` (`Snapshot::Newer`), or none is its own any more (`Gone`).
    pub fn took(&self, word: Option<String>) {
        *self.known.lock().unwrap_or_else(PoisonError::into_inner) = word;
    }

    /// Whether a save went through since the last ask.
    pub fn take_saved(&self) -> bool {
        self.saved.swap(false, Ordering::Relaxed)
    }
}

impl Store for DbStore {
    fn save(&self, blob: Vec<u8>) -> std::result::Result<(), String> {
        if self.held.as_ref().is_some_and(|held| !held()) {
            return Err("the turn on the state ended".into());
        }
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        let word = self.db.write(known.as_deref(), &blob).map_err(|error| match error {
            Error::Save(message) => message,
            other => other.to_string(),
        })?;
        *known = Some(word);
        self.saved.store(true, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);
    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("yo-statedb-{name}-{}", new_word()));
            std::fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn wal_and_compare_and_swap() {
        let dir = Dir::new("cas");
        let db = StateDb::open(dir.0.join("state.sqlite")).unwrap();
        let mode: String = db.connect().unwrap().query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "wal");
        assert_eq!(db.snapshot(None).unwrap(), Snapshot::Gone);
        let one = db.write(None, b"one").unwrap();
        assert!(matches!(db.write(None, b"again"), Err(Error::Save(_))), "a new state never over one there");
        assert_eq!(db.snapshot(Some(&one)).unwrap(), Snapshot::Same);
        let two = db.write(Some(&one), b"two").unwrap();
        assert!(matches!(db.write(Some(&one), b"old"), Err(Error::Save(_))), "never over a newer state");
        assert_eq!(db.snapshot(Some(&one)).unwrap(), Snapshot::Newer { word: two.clone(), blob: b"two".to_vec() });
        db.clear().unwrap();
        assert!(matches!(db.write(Some(&two), b"late"), Err(Error::Save(_))), "a state signed out of stays gone");
        assert_eq!(db.read().unwrap(), None);
    }

    /// A reader in its transaction keeps its snapshot while another connection (another process) writes, and is
    /// never blocked by the write.
    #[test]
    fn readers_see_a_snapshot_while_a_write_is_under_way() {
        let dir = Dir::new("snapshot");
        let db = StateDb::open(dir.0.join("state.sqlite")).unwrap();
        let first = db.write(None, b"first").unwrap();
        // A writer holding its write transaction open (in the app: only for the write itself).
        let mut writer = db.connect().unwrap();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        tx.execute("UPDATE state SET word = 'w2', blob = x'7365636f6e64' WHERE id = 1", []).unwrap();
        let started = std::time::Instant::now();
        assert_eq!(db.read().unwrap(), Some((first.clone(), b"first".to_vec())), "the committed state, at once");
        assert!(started.elapsed() < Duration::from_millis(500), "a reader does not wait for the writer");
        tx.commit().unwrap();
        assert_eq!(db.read().unwrap(), Some(("w2".into(), b"second".to_vec())));
    }

    #[test]
    fn processes_writing_in_turns_lose_nothing() {
        let dir = Dir::new("turns");
        let db = Arc::new(StateDb::open(dir.0.join("state.sqlite")).unwrap());
        db.write(None, &0u64.to_le_bytes()).unwrap();
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let db = Arc::new(StateDb::open(db.path()).unwrap());
                std::thread::spawn(move || {
                    let mut done = 0;
                    while done < 50 {
                        let (word, blob) = db.read().unwrap().unwrap();
                        let n = u64::from_le_bytes(blob.try_into().unwrap()) + 1;
                        if db.write(Some(&word), &n.to_le_bytes()).is_ok() {
                            done += 1;
                        }
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let (_, blob) = db.read().unwrap().unwrap();
        assert_eq!(u64::from_le_bytes(blob.try_into().unwrap()), 200, "every increment once");
    }

    #[test]
    fn a_state_file_of_before_moves_in_once() {
        let dir = Dir::new("move");
        let db = StateDb::open(dir.0.join("state.sqlite")).unwrap();
        let file = dir.0.join("yo-state");
        assert!(!db.move_in(&file, 36).unwrap(), "no file: nothing");
        // The shared file: its word, then the sealed state.
        let word = "0123456789abcdef0123456789abcdef0123";
        std::fs::write(&file, [word.as_bytes(), b"sealed"].concat()).unwrap();
        assert!(db.move_in(&file, 36).unwrap());
        assert!(!file.exists(), "the file goes once the state is in");
        assert_eq!(db.read().unwrap().unwrap().1, b"sealed");
        // Left beside it (stopped before the delete): goes, the database's state stays.
        std::fs::write(&file, b"sealed").unwrap();
        assert!(!db.move_in(&file, 0).unwrap());
        assert!(!file.exists());
        // One that differs (an older build's): never over the database's, kept aside.
        std::fs::write(&file, b"other").unwrap();
        assert!(!db.move_in(&file, 0).unwrap());
        assert_eq!(db.read().unwrap().unwrap().1, b"sealed");
        assert_eq!(std::fs::read(dir.0.join("yo-state.unmoved")).unwrap(), b"other");
    }

    #[test]
    fn the_apps_own_database_moves_into_the_shared_one() {
        let dir = Dir::new("adopt");
        let own = StateDb::open(dir.0.join("own.sqlite")).unwrap();
        let shared = StateDb::open(dir.0.join("shared.sqlite")).unwrap();
        assert!(!shared.adopt(&own).unwrap(), "nothing to move");
        own.write(None, b"mine").unwrap();
        assert!(shared.adopt(&own).unwrap());
        assert_eq!(shared.read().unwrap().unwrap().1, b"mine");
        assert_eq!(own.read().unwrap(), None, "emptied once moved");
        own.write(None, b"other").unwrap();
        assert!(!shared.adopt(&own).unwrap(), "never over the shared state");
        assert_eq!(shared.read().unwrap().unwrap().1, b"mine");
    }

    #[test]
    fn a_save_after_the_turn_ended_writes_nothing() {
        let dir = Dir::new("held");
        let db = Arc::new(StateDb::open(dir.0.join("state.sqlite")).unwrap());
        let held = Arc::new(AtomicBool::new(true));
        let flag = held.clone();
        let store = DbStore::new(db.clone(), None, Some(Box::new(move || flag.load(Ordering::Relaxed))));
        store.save(b"one".to_vec()).unwrap();
        assert!(store.take_saved());
        assert!(!store.take_saved());
        held.store(false, Ordering::Relaxed);
        assert!(store.save(b"two".to_vec()).is_err());
        assert_eq!(db.read().unwrap().unwrap().1, b"one");
        assert_eq!(store.snapshot().unwrap(), Snapshot::Same);
    }
}
