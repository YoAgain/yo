//! WAL snapshots and compare-and-swap persistence for the client. The legacy backend remains available for import
//! and for a launch whose first migration rolled back.

mod cipher;
mod tables;

use crate::error::{Error, Result};
use crate::Store;
pub use cipher::Report;
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

/// A database snapshot for a live client. Encrypted tables can be loaded without sealing them again.
pub enum Loaded {
    Same,
    Newer { word: String, state: Box<crate::state::State>, records: crate::signal::Entries },
    Gone,
}

/// The database file (`-wal` and `-shm` beside it).
pub struct StateDb {
    path: PathBuf,
    cipher: Option<cipher::Cipher>,
    legacy_fence: Option<(PathBuf, zeroize::Zeroizing<[u8; 32]>, Report)>,
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

/// Apple's SQLite uses this flag for the database and its WAL sidecars.
fn protected(flags: OpenFlags) -> OpenFlags {
    #[cfg(target_os = "ios")]
    let flags = flags | OpenFlags::from_bits_retain(0x0030_0000);
    flags
}

/// A new word for a write: 128 random bits, never one a deleted state had.
fn new_word() -> String {
    crate::state::hex(&crate::identity::random::<16>())
}

impl StateDb {
    /// Opens the database at `path` (made if there is none; its folder must be there), in WAL mode.
    pub fn open(path: impl Into<PathBuf>) -> Result<StateDb> {
        let db = StateDb { path: path.into(), cipher: None, legacy_fence: None };
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

    /// Opens an already-current database without taking the platform's write turn.
    pub fn open_current(path: impl Into<PathBuf>, key: [u8; 32], report: Report) -> Result<Option<Self>> {
        let path = path.into();
        Ok(cipher::Cipher::open_current(&path, key, report)?.map(|cipher| Self {
            path,
            cipher: Some(cipher),
            legacy_fence: None,
        }))
    }

    /// Opens the encrypted database and runs pending migrations. The legacy source is read only by V0001.
    /// `rolled_back` allows a caller to keep using a legacy store when V0001 did not publish anything.
    pub fn open_encrypted(
        path: impl Into<PathBuf>,
        key: [u8; 32],
        report: Option<Report>,
        source: impl FnMut() -> Result<Option<(String, Vec<u8>)>>,
        held: Option<&dyn Fn() -> bool>,
        rolled_back: &mut bool,
    ) -> Result<Self> {
        let path = path.into();
        let cipher = cipher::Cipher::open(&path, key, report, source, held, rolled_back)?;
        Ok(Self { path, cipher: Some(cipher), legacy_fence: None })
    }

    /// The old iOS database for a launch whose import rolled back. Other processes may retry the migration;
    /// recheck the destination under the caller's write turn before changing the legacy source.
    pub fn open_fallback(
        path: impl Into<PathBuf>,
        file: &Path,
        word_len: usize,
        destination: impl Into<PathBuf>,
        key: [u8; 32],
        report: Report,
    ) -> Result<Self> {
        let result = (|| {
            let mut db = Self::open(path)?;
            if db.word()?.is_none() {
                db.move_in(file, word_len)?;
            }
            db.legacy_fence = Some((destination.into(), zeroize::Zeroizing::new(key), report.clone()));
            Ok(db)
        })();
        if let Err(error) = &result {
            cipher::report_failure(Some(&report), 0, 0, 0, error);
        }
        result
    }

    fn on_legacy<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let Some((destination, key, report)) = &self.legacy_fence else { return operation() };
        let result = cipher::verify_unmigrated(destination, key).and_then(|()| operation());
        if let Err(error) = &result {
            cipher::report_failure(Some(report), 0, 0, 0, error);
        }
        result
    }

    pub fn encrypted(&self) -> bool {
        self.cipher.is_some()
    }

    /// Reads the old iOS WAL database as a snapshot and the older flat file, without changing either source.
    pub fn legacy_source(db: &Path, file: &Path, word_len: usize) -> Result<Option<(String, Vec<u8>)>> {
        let saved: Option<(String, Vec<u8>)> = if db.try_exists().map_err(unavailable)? {
            let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(unavailable)?;
            conn.busy_timeout(BUSY).map_err(unavailable)?;
            conn.query_row("SELECT word, blob FROM state WHERE id = 1", [], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()
                .map_err(unavailable)?
        } else {
            None
        };
        let from_file = match std::fs::read(file) {
            Ok(bytes) if bytes.len() > word_len => {
                let word = if word_len == 0 {
                    new_word()
                } else {
                    std::str::from_utf8(&bytes[..word_len]).map_err(|_| Error::State)?.to_owned()
                };
                Some((word, bytes[word_len..].to_vec()))
            }
            Ok(_) => return Err(Error::State),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(unavailable(error)),
        };
        if let (Some((_, saved)), Some((_, flat))) = (&saved, &from_file) {
            if saved != flat {
                return Err(Error::Save("LegacyConflict".into()));
            }
        }
        Ok(saved.or(from_file))
    }

    fn on_cipher<T>(&self, operation: impl FnOnce(&cipher::Cipher) -> Result<T>) -> Result<T> {
        let cipher = self.cipher.as_ref().expect("encrypted backend");
        let result = operation(cipher);
        if let Err(error) = &result {
            let version = cipher::latest();
            cipher::report_failure(cipher.report.as_ref(), version, version, version, error);
        }
        result
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn connect(&self) -> Result<Connection> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&self.path, protected(flags)).map_err(unavailable)?;
        conn.busy_timeout(BUSY).map_err(unavailable)?;
        // WAL with NORMAL: a commit is durable at the next checkpoint; a crash of the app never loses one.
        conn.pragma_update(None, "synchronous", "NORMAL").map_err(unavailable)?;
        conn.pragma_update(None, "fullfsync", "ON").map_err(unavailable)?;
        conn.pragma_update(None, "checkpoint_fullfsync", "ON").map_err(unavailable)?;
        Ok(conn)
    }

    /// The state now against `known` (the word this process read or wrote last), in one read transaction.
    pub fn snapshot(&self, known: Option<&str>) -> Result<Snapshot> {
        if let Some(cipher) = &self.cipher {
            return Ok(match self.load(known, &cipher.key)? {
                Loaded::Same => Snapshot::Same,
                Loaded::Gone => Snapshot::Gone,
                Loaded::Newer { word, state, records } => {
                    Snapshot::Newer { word, blob: crate::state::seal(&state, records, &cipher.key) }
                }
            });
        }
        self.on_legacy(|| self.snapshot_legacy(known))
    }

    fn snapshot_legacy(&self, known: Option<&str>) -> Result<Snapshot> {
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

    pub fn load(&self, known: Option<&str>, key: &[u8; 32]) -> Result<Loaded> {
        if self.cipher.is_some() {
            return self.on_cipher(|cipher| {
                if key != &*cipher.key {
                    return Err(Error::State);
                }
                let mut conn = cipher.connect(&self.path, false)?;
                let tx = conn.transaction().map_err(cipher::sqlite_error)?;
                Ok(match cipher::Cipher::word(&tx)? {
                    None => Loaded::Gone,
                    Some(word) if Some(word.as_str()) == known => Loaded::Same,
                    Some(word) => {
                        let (state, records) = tables::read(&tx, &cipher.key)?;
                        Loaded::Newer { word, state: Box::new(state), records }
                    }
                })
            });
        }
        self.on_legacy(|| {
            Ok(match self.snapshot_legacy(known)? {
                Snapshot::Same => Loaded::Same,
                Snapshot::Gone => Loaded::Gone,
                Snapshot::Newer { word, blob } => {
                    let (state, records) = crate::state::open(&blob, key)?;
                    Loaded::Newer { word, state: Box::new(state), records }
                }
            })
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
        if self.cipher.is_some() {
            return self.on_cipher(|cipher| cipher::Cipher::word(&cipher.connect(&self.path, false)?));
        }
        self.on_legacy(|| {
            let conn = self.connect()?;
            conn.query_row("SELECT word FROM state WHERE id = 1", [], |row| row.get(0)).optional().map_err(unavailable)
        })
    }

    /// Puts `blob` in place of the state `known` names (none: there is none yet, a new account), atomically; its new
    /// word. Refused (`Save`) when another write came since, or the state went (signed out) though `known` names one:
    /// the step that wanted it fails, and nothing that needed it leaves.
    pub fn write(&self, known: Option<&str>, blob: &[u8]) -> Result<String> {
        if let Some(cipher) = &self.cipher {
            let (state, records) = self.on_cipher(|_| crate::state::open(blob, &cipher.key))?;
            return self.write_tables(known, &state, &records, None, None, false);
        }
        self.on_legacy(|| self.write_legacy(known, blob))
    }

    fn write_legacy(&self, known: Option<&str>, blob: &[u8]) -> Result<String> {
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

    fn write_tables(
        &self,
        known: Option<&str>,
        state: &crate::state::State,
        records: &crate::signal::Entries,
        baseline: Option<&crate::signal::Entries>,
        held: Option<&dyn Fn() -> bool>,
        durable: bool,
    ) -> Result<String> {
        self.on_cipher(|cipher| {
            let mut conn = cipher.connect(&self.path, false)?;
            if durable {
                conn.pragma_update(None, "synchronous", "FULL").map_err(cipher::sqlite_error)?;
            }
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(cipher::sqlite_error)?;
            if cipher::Cipher::word(&tx)?.as_deref() != known {
                return Err(Error::Save("an older state".into()));
            }
            tables::write_with_baseline(&tx, &cipher.key, state, records, baseline)?;
            let word = new_word();
            cipher::Cipher::set_word(&tx, Some(&word))?;
            if held.is_some_and(|held| !held()) {
                return Err(Error::Save("the turn on the state ended".into()));
            }
            tx.commit().map_err(cipher::sqlite_error)?;
            checkpoint(&conn);
            Ok(word)
        })
    }

    /// Signed out: the state goes, whatever was written last.
    pub fn clear(&self) -> Result<()> {
        if self.cipher.is_some() {
            return self.on_cipher(|cipher| {
                let mut conn = cipher.connect(&self.path, false)?;
                let tx =
                    conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(cipher::sqlite_error)?;
                tables::clear(&tx).map_err(cipher::sqlite_error)?;
                cipher::Cipher::set_word(&tx, None)?;
                tx.commit().map_err(cipher::sqlite_error)?;
                checkpoint(&conn);
                Ok(())
            });
        }
        self.on_legacy(|| {
            let conn = self.connect().map_err(not_saved)?;
            conn.execute("DELETE FROM state", []).map_err(not_saved)?;
            Ok(())
        })
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
        self.adopt_with_turn(other, None)
    }

    pub fn adopt_with_turn(&self, other: &StateDb, held: Option<&dyn Fn() -> bool>) -> Result<bool> {
        if let Some(cipher) = &self.cipher {
            let Some((_, blob)) = other.read()? else { return Ok(false) };
            if self.word()?.is_some() {
                return Ok(false);
            }
            let (state, records) = self.on_cipher(|_| {
                crate::state::open(&blob, other.cipher.as_ref().map_or(&cipher.key, |other| &other.key))
            })?;
            self.write_tables(None, &state, &records, None, held, true)?;
            other.clear()?;
            return Ok(true);
        }
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
    baseline: Mutex<Option<(String, ProtocolBaseline)>>,
    held: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    saved: AtomicBool,
}

/// Keeps the last committed libsignal records for row-level change detection.
struct ProtocolBaseline(crate::signal::Entries);

impl Drop for ProtocolBaseline {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        for value in self.0.values_mut() {
            value.zeroize();
        }
    }
}

impl DbStore {
    /// `known`: the word of the state the client opened (none: a new one, made by the first save).
    pub fn new(db: Arc<StateDb>, known: Option<String>, held: Option<Box<dyn Fn() -> bool + Send + Sync>>) -> Self {
        DbStore { db, known: Mutex::new(known), baseline: Mutex::new(None), held, saved: AtomicBool::new(false) }
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

    pub fn load(&self, key: &[u8; 32]) -> Result<Loaded> {
        let known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        let loaded = self.db.load(known.as_deref(), key)?;
        if let Loaded::Newer { word, records, .. } = &loaded {
            if self.db.encrypted() {
                *self.baseline.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some((word.clone(), ProtocolBaseline(records.clone())));
            }
        }
        Ok(loaded)
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

impl DbStore {
    fn save_with(&self, write: impl FnOnce(Option<&str>) -> Result<String>) -> std::result::Result<(), String> {
        if self.held.as_ref().is_some_and(|held| !held()) {
            return Err("the turn on the state ended".into());
        }
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        let word = write(known.as_deref()).map_err(|error| match error {
            Error::Save(message) => message,
            other => other.to_string(),
        })?;
        *known = Some(word);
        self.saved.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl Store for DbStore {
    fn save(&self, blob: Vec<u8>) -> std::result::Result<(), String> {
        self.save_with(|known| self.db.write(known, &blob))
    }

    fn save_state(
        &self,
        state: &crate::state::State,
        records: crate::signal::Entries,
        key: &[u8; 32],
    ) -> std::result::Result<(), String> {
        if self.db.cipher.is_some() {
            self.save_with(|known| {
                let mut baseline = self.baseline.lock().unwrap_or_else(PoisonError::into_inner);
                let previous = baseline.as_ref().filter(|(word, _)| Some(word.as_str()) == known);
                let word = self.db.write_tables(
                    known,
                    state,
                    &records,
                    previous.map(|(_, records)| &records.0),
                    self.held.as_deref().map(|held| held as &dyn Fn() -> bool),
                    false,
                )?;
                *baseline = Some((word.clone(), ProtocolBaseline(records)));
                Ok(word)
            })
        } else {
            self.save(crate::state::seal(state, records, key))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) struct Dir(pub(super) PathBuf);
    impl Dir {
        pub(super) fn new(name: &str) -> Dir {
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

#[cfg(test)]
mod migration_tests {
    use super::*;
    use crate::{Client, Push, Response, Transport};

    const KEY: [u8; 32] = [7; 32];

    struct Offline;
    impl Transport for Offline {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> std::result::Result<Response, String> {
            panic!("no network needed to migrate");
        }
    }

    #[test]
    fn v0_legacy_wal_import_is_atomic_and_runs_once() {
        let dir = super::tests::Dir::new("migration");
        let legacy_path = dir.0.join("state.sqlite");
        let flat_path = dir.0.join("yo-state");
        let destination = dir.0.join("tables.sqlite");
        let legacy = StateDb::open(&legacy_path).unwrap();
        let mut client = Client::registering(Arc::new(Offline), "Example", None, None, None, Push::none()).unwrap();
        for seq in [3, 1, 2] {
            client.state.outbox.push_back(crate::state::Outgoing {
                queued_at: 12,
                seq,
                user: "recipient".into(),
                plain: vec![1, 2, 3],
                device: None,
                non_urgent: false,
                payload: Some(vec![4, 5, 6]),
                made_for: vec![[7; 32]],
                refused: true,
            });
        }
        client.state.contacts.insert(
            "one".into(),
            crate::state::Contact {
                name: Some("First".into()),
                profile_key: Some([19; 32]),
                ..crate::state::Contact::unknown()
            },
        );
        client.state.contacts.insert(
            "two".into(),
            crate::state::Contact {
                name: Some("Second".into()),
                profile_key: Some([21; 32]),
                ..crate::state::Contact::unknown()
            },
        );
        client.state.seen.extend([[3; 32], [2; 32]]);
        client.state.received.push_back([9; 32]);
        client.state.storage.extra.insert("future".into(), serde_json::json!({"when": 4, "v": [true, null]}));
        let blob = client.export(&KEY);
        let expected = crate::state::open(&blob, &KEY).unwrap();
        let word = legacy.write(None, &blob).unwrap();

        let reports = Arc::new(Mutex::new(Vec::new()));
        let captured = reports.clone();
        let report: Report = Arc::new(move |step, before, after, kind| {
            captured.lock().unwrap().push((step, before, after, kind));
        });
        let mut rolled_back = false;
        let source = || StateDb::legacy_source(&legacy_path, &flat_path, 0);
        let interrupted =
            StateDb::open_encrypted(&destination, KEY, Some(report), source, Some(&|| false), &mut rolled_back);
        assert!(interrupted.is_err());
        assert!(rolled_back);
        assert_eq!(*reports.lock().unwrap(), [(1, 0, 0, "TurnEnded".to_owned())]);
        assert_eq!(legacy.read().unwrap(), Some((word.clone(), blob)));
        let incomplete = Connection::open(&destination).unwrap();
        assert_eq!(incomplete.pragma_query_value::<u32, _>(None, "user_version", |row| row.get(0)).unwrap(), 0);
        assert_eq!(
            incomplete
                .query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table'", [], |row| row.get::<_, u32>(0))
                .unwrap(),
            0
        );
        drop(incomplete);

        let db = StateDb::open_encrypted(&destination, KEY, None, source, None, &mut rolled_back).unwrap();
        let conn = db.cipher.as_ref().unwrap().connect(&destination, false).unwrap();
        assert_eq!(conn.pragma_query_value::<u32, _>(None, "user_version", |row| row.get(0)).unwrap(), 1);
        let (state, records) = tables::read(&conn, &KEY).unwrap();
        assert_eq!(serde_json::to_value(&expected.0).unwrap(), serde_json::to_value(state).unwrap());
        assert_eq!(expected.1, records);
        assert_eq!(db.word().unwrap().as_deref(), Some(word.as_str()));

        let unkeyed = Connection::open(&destination).unwrap();
        assert_eq!(
            unkeyed
                .query_row("SELECT user FROM outbox ORDER BY owner LIMIT 1", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "recipient"
        );
        assert!(StateDb::open_encrypted(
            &destination,
            [8; 32],
            None,
            || panic!("wrong key must fail"),
            None,
            &mut false
        )
        .is_err());

        let stored_key: Vec<u8> =
            unkeyed.query_row("SELECT profile_key FROM contacts WHERE entry = 'one'", [], |row| row.get(0)).unwrap();
        let expected_key = serde_json::to_vec(&[19; 32]).unwrap();
        assert_ne!(stored_key, expected_key);
        assert_eq!(cipher::unseal(&KEY, ["contacts", "", "one", "profile_key"], &stored_key).unwrap(), expected_key);
        assert!(cipher::unseal(&KEY, ["profile_keys", "", "one", "profile_key"], &stored_key).is_err());
        unkeyed.execute("UPDATE contacts SET profile_key = ?1 WHERE entry = 'two'", [&stored_key]).unwrap();
        assert!(matches!(db.load(None, &KEY), Err(Error::State)));

        let reopen = || {
            StateDb::open_encrypted(&destination, KEY, None, || panic!("V0001 ran again"), None, &mut false).unwrap()
        };
        assert_eq!(reopen().word().unwrap(), Some(word));
        db.clear().unwrap();
        assert_eq!(reopen().word().unwrap(), None, "a source left behind cannot undo sign-out");
    }

    #[test]
    fn v0_flat_sources_migrate_with_or_without_a_cas_prefix() {
        let dir = super::tests::Dir::new("flat-migration");
        let client = Client::registering(Arc::new(Offline), "Example", None, None, None, Push::none()).unwrap();
        let legacy_db = dir.0.join("state.sqlite");
        let blob = client.export(&KEY);
        for prefix in ["", "0123456789abcdef0123456789abcdef0123"] {
            let file = dir.0.join(format!("old-{}", prefix.len()));
            let destination = dir.0.join(format!("new-{}.sqlite", prefix.len()));
            std::fs::write(&file, prefix.as_bytes()).unwrap();
            let source = || StateDb::legacy_source(&legacy_db, &file, prefix.len());
            let mut rolled_back = false;
            assert!(StateDb::open_encrypted(&destination, KEY, None, source, None, &mut rolled_back).is_err());
            assert!(rolled_back);
            assert_eq!(std::fs::read(&file).unwrap(), prefix.as_bytes());

            let old = [prefix.as_bytes(), blob.as_slice()].concat();
            std::fs::write(&file, &old).unwrap();
            let db = StateDb::open_encrypted(&destination, KEY, None, source, None, &mut rolled_back).unwrap();
            let Loaded::Newer { word, state, .. } = db.load(None, &KEY).unwrap() else {
                panic!("the flat file must be imported");
            };
            assert!(!word.is_empty());
            if !prefix.is_empty() {
                assert_eq!(word, prefix);
            }
            assert_eq!(state.account_secret, client.state.account_secret);
            assert_eq!(std::fs::read(&file).unwrap(), old);
        }
    }

    #[test]
    fn fresh_database_rejects_a_future_schema() {
        let dir = super::tests::Dir::new("fresh");
        let path = dir.0.join("tables.sqlite");
        let db = StateDb::open_encrypted(&path, KEY, None, || Ok(None), None, &mut false).unwrap();
        assert_eq!(db.word().unwrap(), None);
        let conn = db.cipher.as_ref().unwrap().connect(&path, false).unwrap();
        conn.pragma_update(None, "user_version", cipher::latest() + 1).unwrap();
        assert!(matches!(
            StateDb::open_encrypted(&path, KEY, None, || panic!("future schema"), None, &mut false),
            Err(Error::Save(kind)) if kind == "UnsupportedVersion"
        ));
    }
}
