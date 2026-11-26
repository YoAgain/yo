//! WAL snapshots and compare-and-swap persistence for the client.

pub(crate) mod cipher;
mod tables;

use crate::error::{Error, Result};
use crate::Store;
pub use cipher::Report;
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How long an access waits for another process's (a write, the last connection's checkpoint).
const BUSY: Duration = Duration::from_secs(3);
/// How long the checkpoint after a write waits for other connections (Signal's `maxBusyTimeoutMs`).
const CHECKPOINT_BUSY: Duration = Duration::from_millis(50);

/// A save whose turn was taken back meanwhile (iOS ended the background time, an extension's request ended): the
/// step runs again in a later turn. Not a failure of the database, so never reported as one (`cipher::report_failure`).
pub const TURN_ENDED: &str = "the turn on the state ended";
/// A save over a state that is no longer the one in the database although the turn was held: another writer went
/// without the turn. Reported ("Stale"): it would be a bug.
pub const STALE: &str = "an older state";

/// A database snapshot for a live client. Encrypted tables can be loaded without sealing them again.
pub enum Loaded {
    Same,
    Newer { word: String, state: Box<crate::state::State>, records: crate::signal::Entries },
    Gone,
}

/// Around each access to the database (a connection's whole life, its transaction and checkpoint in it): the app keeps
/// itself from being suspended until `end`, as iOS ends a process suspended with a lock on a file in a shared container
/// (0xdead10cc). `begin` false refuses the access (no time left to finish it): nothing is read or written.
pub trait Access: Send + Sync {
    fn begin(&self) -> bool;
    fn end(&self);
}

/// The database file (`-wal` and `-shm` beside it).
pub struct StateDb {
    path: PathBuf,
    cipher: cipher::Cipher,
    access: Mutex<Option<Arc<dyn Access>>>,
    /// Without an access guard (a process that is never suspended: Android, the CLI, the bots) one connection stays
    /// open, as Signal keeps its database open: a connection per step made and deleted the `-wal` and `-shm` files every
    /// time, and on a phone that was most of a save.
    kept: Mutex<Option<Connection>>,
    /// The rows of the state last read or written here, under its word: a write that replaces that state compares
    /// with them instead of reading every row back (each word is one state's). With the number of clears: a read or
    /// write begun before a clear never puts its rows back.
    snapshot: Mutex<(Option<Snapshot>, u64)>,
}

struct Snapshot {
    word: String,
    rows: tables::Rows,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.rows.values_mut().for_each(Zeroize::zeroize);
    }
}

/// Checkpoints and truncates the WAL after a guarded write (iOS); busy connections leave it for the next one. A kept
/// connection leaves it to SQLite's own checkpoint every 1000 pages, as Signal's Android database does: no fsync on a
/// step's way (WAL with `synchronous = NORMAL`: a crash of the app loses nothing, a power cut at most the last step).
fn checkpoint(conn: &Connection) {
    if conn.busy_timeout(CHECKPOINT_BUSY).is_ok() {
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }
    // A kept connection goes on with the access's patience.
    let _ = conn.busy_timeout(BUSY);
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
    pub fn open_fresh(path: impl Into<PathBuf>, key: [u8; 32], report: Report) -> Result<Self> {
        let path = path.into();
        let cipher = cipher::Cipher::open_fresh(&path, key, report)?;
        Ok(Self { path, cipher, access: Mutex::new(None), kept: Mutex::new(None), snapshot: Mutex::new((None, 0)) })
    }

    /// Every later access runs within `access` (none: unguarded, a process that is never suspended).
    pub fn set_access(&self, access: Option<Arc<dyn Access>>) {
        // The kept connection first, as an access takes them: one under way finishes before it goes.
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        // A guarded process opens and closes its connection within each access.
        if access.is_some() {
            *kept = None;
        }
        *self.access.lock().unwrap_or_else(PoisonError::into_inner) = access;
    }

    /// Every access goes through here. Guarded, its connection is opened and closed within: no SQLite lock outlives a
    /// call. Unguarded, the kept connection (no transaction outlives a call either).
    /// `operation` is told whether the access is guarded.
    fn on_cipher<T>(&self, operation: impl FnOnce(&cipher::Cipher, &mut Connection, bool) -> Result<T>) -> Result<T> {
        let cipher = &self.cipher;
        let access = self.access.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if access.as_ref().is_some_and(|access| !access.begin()) {
            // Expected in the background: not counted as a failure.
            return Err(Error::Save("Busy".into()));
        }
        // Ended when dropped, also when the access panics.
        struct Ends(Option<Arc<dyn Access>>);
        impl Drop for Ends {
            fn drop(&mut self) {
                if let Some(access) = &self.0 {
                    access.end();
                }
            }
        }
        let ends = Ends(access);
        let result = if ends.0.is_some() {
            cipher.connect(&self.path, false).and_then(|mut conn| operation(cipher, &mut conn, true))
        } else {
            let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
            let conn = match kept.take() {
                Some(conn) => Ok(conn),
                None => cipher.connect(&self.path, false),
            };
            conn.and_then(|mut conn| {
                let result = operation(cipher, &mut conn, false);
                // A failed access starts again on a new connection; a guard set meanwhile closes it.
                if result.is_ok() && self.access.lock().unwrap_or_else(PoisonError::into_inner).is_none() {
                    *kept = Some(conn);
                }
                result
            })
        };
        drop(ends);
        if let Err(error) = &result {
            cipher::report_failure(&cipher.report, error);
        }
        result
    }

    pub fn load(&self, known: Option<&str>, key: &[u8; 32]) -> Result<Loaded> {
        self.on_cipher(|cipher, conn, _| {
            if key != &*cipher.key {
                return Err(Error::State);
            }
            let clears = self.snapshot().1;
            let tx = conn.transaction().map_err(cipher::sqlite_error)?;
            Ok(match cipher::Cipher::word(&tx)? {
                None => Loaded::Gone,
                Some(word) if Some(word.as_str()) == known => Loaded::Same,
                Some(word) => {
                    let (state, records, rows) = tables::read(&tx, &cipher.key)?;
                    self.keep(clears, Snapshot { word: word.clone(), rows });
                    Loaded::Newer { word, state: Box::new(state), records }
                }
            })
        })
    }

    /// Whether the state here finished its registration: false for none, or one still waiting for the hub's answer.
    pub fn registered(&self) -> Result<bool> {
        Ok(match self.load(None, &self.cipher.key)? {
            Loaded::Newer { state, .. } => state.registering.is_none(),
            Loaded::Same | Loaded::Gone => false,
        })
    }

    pub fn word(&self) -> Result<Option<String>> {
        self.on_cipher(|_, conn, _| cipher::Cipher::word(conn))
    }

    fn snapshot(&self) -> std::sync::MutexGuard<'_, (Option<Snapshot>, u64)> {
        self.snapshot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The rows read or written by a step begun after `clears` clears: kept unless the state was cleared since.
    fn keep(&self, clears: u64, snapshot: Snapshot) {
        let mut kept = self.snapshot();
        if kept.1 == clears {
            kept.0 = Some(snapshot);
        }
    }

    /// No rows of the state held here in the clear any more, nor put back by a step under way.
    fn forget(&self) {
        let mut kept = self.snapshot();
        kept.0 = None;
        kept.1 += 1;
    }

    fn write_tables(
        &self,
        known: Option<&str>,
        state: &crate::state::State,
        records: &crate::signal::Entries,
        held: Option<&dyn Fn() -> bool>,
    ) -> Result<String> {
        self.on_cipher(|cipher, conn, guarded| {
            let clears = self.snapshot().1;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(cipher::sqlite_error)?;
            // The turn first: a turn taken back lets another process write, and its newer state is no stale save.
            if held.is_some_and(|held| !held()) {
                return Err(Error::Save(TURN_ENDED.into()));
            }
            if cipher::Cipher::word(&tx)?.as_deref() != known {
                return Err(Error::Save(STALE.into()));
            }
            // The rows of the state replaced, if they are the ones known here (checked against the word above).
            let baseline = self.snapshot().0.take().filter(|s| Some(s.word.as_str()) == known);
            let rows = tables::write(&tx, &cipher.key, state, records, baseline.as_ref().map(|s| &s.rows))?;
            let word = new_word();
            cipher::Cipher::set_word(&tx, Some(&word))?;
            if held.is_some_and(|held| !held()) {
                return Err(Error::Save(TURN_ENDED.into()));
            }
            tx.commit().map_err(cipher::sqlite_error)?;
            self.keep(clears, Snapshot { word: word.clone(), rows });
            if guarded {
                checkpoint(conn);
            }
            Ok(word)
        })
    }

    /// Signed out: the state goes, whatever was written last, and its rows held here in the clear with it.
    pub fn clear(&self) -> Result<()> {
        // Forgotten first, also when the clear fails, and again after it.
        self.forget();
        self.on_cipher(|_, conn, _| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(cipher::sqlite_error)?;
            tables::clear(&tx).map_err(cipher::sqlite_error)?;
            cipher::Cipher::set_word(&tx, None)?;
            tx.commit().map_err(cipher::sqlite_error)?;
            self.forget();
            // Signed out: what was in the WAL goes now, whoever keeps the connection.
            checkpoint(conn);
            Ok(())
        })
    }
}

/// A state written to the tables of a database in memory and read back: what a restart would read (tests).
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn round_trip(
    state: &crate::state::State,
    records: &crate::signal::Entries,
) -> Result<(crate::state::State, crate::signal::Entries)> {
    let mut conn = Connection::open_in_memory().map_err(cipher::sqlite_error)?;
    conn.execute_batch(include_str!("statedb/schema.sql")).map_err(cipher::sqlite_error)?;
    let tx = conn.transaction().map_err(cipher::sqlite_error)?;
    tables::write(&tx, &[0; 32], state, records, None)?;
    tables::read(&tx, &[0; 32]).map(|(state, records, _)| (state, records))
}

/// A client's `Store` on a `StateDb`: a save replaces the state this client read or wrote last in one transaction,
/// never another process's newer one. `held`: whether the process still has its turn, asked before each write.
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

    pub fn load(&self, key: &[u8; 32]) -> Result<Loaded> {
        let known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        self.db.load(known.as_deref(), key)
    }

    /// The client took in the state of `word` (`Loaded::Newer`), or none is its own any more (`Gone`).
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
            return Err(TURN_ENDED.into());
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
    fn save_state(
        &self,
        state: &crate::state::State,
        records: crate::signal::Entries,
    ) -> std::result::Result<(), String> {
        self.save_with(|known| {
            let held = self.held.as_deref().map(|held| held as &dyn Fn() -> bool);
            self.db.write_tables(known, state, &records, held)
        })
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

    const KEY: [u8; 32] = [7; 32];

    fn open(path: &std::path::Path) -> StateDb {
        StateDb::open_fresh(path, KEY, Arc::new(|_| {})).unwrap()
    }

    struct Offline;
    impl crate::Transport for Offline {
        fn request(
            &self,
            _: &str,
            _: &str,
            _: &[(String, String)],
            _: &[u8],
        ) -> std::result::Result<crate::Response, String> {
            Err("offline".into())
        }
    }

    fn initial() -> crate::state::State {
        let push = yoip::net::api::Push::none();
        crate::Client::registering(Arc::new(Offline), "JO", None, None, None, push, None).unwrap().saved().state
    }

    fn write(db: &StateDb, known: Option<&str>, state: &crate::state::State) -> Result<String> {
        db.write_tables(known, state, &Default::default(), None)
    }

    fn read(db: &StateDb) -> (String, crate::state::State) {
        let Loaded::Newer { word, state, .. } = db.load(None, &KEY).unwrap() else {
            panic!("expected a stored state");
        };
        (word, *state)
    }

    #[test]
    fn wal_and_compare_and_swap() {
        let dir = Dir::new("cas");
        let db = open(&dir.0.join("state.sqlite"));
        let conn = db.cipher.connect(&db.path, false).unwrap();
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "wal");
        assert!(matches!(db.load(None, &KEY).unwrap(), Loaded::Gone));
        let mut state = initial();
        let first = write(&db, None, &state).unwrap();
        assert!(write(&db, None, &state).is_err());
        assert!(matches!(db.load(Some(&first), &KEY).unwrap(), Loaded::Same));
        state.next_seq += 1;
        let second = write(&db, Some(&first), &state).unwrap();
        assert!(write(&db, Some(&first), &state).is_err());
        assert_eq!(read(&db).1.next_seq, state.next_seq);
        db.clear().unwrap();
        assert!(write(&db, Some(&second), &state).is_err());
        assert!(matches!(db.load(None, &KEY).unwrap(), Loaded::Gone));
    }

    #[test]
    fn readers_see_a_snapshot_while_a_write_is_under_way() {
        let dir = Dir::new("snapshot");
        let db = open(&dir.0.join("state.sqlite"));
        let first = write(&db, None, &initial()).unwrap();
        let mut writer = db.cipher.connect(&db.path, false).unwrap();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        cipher::Cipher::set_word(&tx, Some("second")).unwrap();
        let started = std::time::Instant::now();
        assert_eq!(read(&db).0, first);
        assert!(started.elapsed() < Duration::from_millis(500));
        tx.commit().unwrap();
        assert_eq!(read(&db).0, "second");
    }

    #[test]
    fn concurrent_writers_preserve_every_increment() {
        let dir = Dir::new("turns");
        let db = open(&dir.0.join("state.sqlite"));
        let state = initial();
        write(&db, None, &state).unwrap();
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let db = open(&db.path);
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        loop {
                            let (word, mut state) = read(&db);
                            state.next_seq += 1;
                            if write(&db, Some(&word), &state).is_ok() {
                                break;
                            }
                        }
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(read(&db).1.next_seq, state.next_seq + 40);
    }

    #[test]
    fn a_save_after_the_turn_ended_writes_nothing() {
        let dir = Dir::new("held");
        let db = Arc::new(open(&dir.0.join("state.sqlite")));
        let held = Arc::new(AtomicBool::new(true));
        let flag = held.clone();
        let store = DbStore::new(db.clone(), None, Some(Box::new(move || flag.load(Ordering::Relaxed))));
        let mut state = initial();
        store.save_state(&state, Default::default()).unwrap();
        assert!(store.take_saved());
        assert!(!store.take_saved());
        let word = read(&db).0;
        held.store(false, Ordering::Relaxed);
        state.next_seq += 1;
        assert!(store.save_state(&state, Default::default()).is_err());
        assert!(matches!(db.load(Some(&word), &KEY).unwrap(), Loaded::Same));
    }

    /// A turn taken back is no failure of the database (iOS ended the time; not counted), a stale save under a held
    /// turn is ("Stale"); a turn taken back while another process wrote says the turn ended, not stale.
    #[test]
    fn a_turn_taken_back_is_not_reported_and_a_stale_save_is() {
        let dir = Dir::new("report");
        let kinds = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = kinds.clone();
        let report: Report = Arc::new(move |kind| seen.lock().unwrap().push(kind));
        let db = StateDb::open_fresh(dir.0.join("state.sqlite"), KEY, report).unwrap();
        let mut state = initial();
        let first = write(&db, None, &state).unwrap();
        state.next_seq += 1;
        write(&db, Some(&first), &state).unwrap();
        let ended = db.write_tables(Some(&first), &state, &Default::default(), Some(&|| false));
        assert!(matches!(ended, Err(Error::Save(kind)) if kind == TURN_ENDED));
        assert!(kinds.lock().unwrap().is_empty());
        let stale = db.write_tables(Some(&first), &state, &Default::default(), Some(&|| true));
        assert!(matches!(stale, Err(Error::Save(kind)) if kind == STALE));
        assert_eq!(*kinds.lock().unwrap(), ["Stale"]);
    }

    /// The snapshot read or written last is the baseline: a small save after opening (with full dedup lists) reads
    /// no row back, and one after another process wrote compares with the database again.
    #[test]
    fn a_small_save_reads_nothing_back() {
        let dir = Dir::new("baseline");
        let path = dir.0.join("state.sqlite");
        let mut state = initial();
        state.seen = (0..crate::SEEN as u32).map(|i| crate::sha256(&i.to_be_bytes())).collect();
        state.received = state.seen.clone();
        let other = open(&path);
        write(&other, None, &state).unwrap();
        let db = open(&path);
        let loads = || tables::LOADS.with(|loads| loads.get());
        let (mut word, _) = read(&db);
        let before = loads();
        let started = std::time::Instant::now();
        for _ in 0..20 {
            state.signed_until += 1;
            word = write(&db, Some(&word), &state).unwrap();
        }
        eprintln!("20 small saves, {} dedup hashes: {:?}", 2 * crate::SEEN, started.elapsed());
        assert_eq!(loads(), before, "compared with the snapshot, nothing read back");
        assert_eq!(read(&db).1.signed_until, state.signed_until);
        // Another process wrote: the next save here compares with the database, whose word it checked.
        state.next_seq += 1;
        word = write(&other, Some(&word), &state).unwrap();
        let before = loads();
        state.next_seq += 1;
        write(&db, Some(&word), &state).unwrap();
        assert_eq!(loads(), before + 1);
        assert_eq!(read(&other).1.next_seq, state.next_seq);
    }

    /// A read begun before a sign-out's clear, ending after it, does not put the old account's rows back in the
    /// handle held open; a clear that fails forgets them too.
    #[test]
    fn a_reader_over_a_clear_keeps_nothing_in_the_clear() {
        let dir = Dir::new("clear");
        let db = open(&dir.0.join("state.sqlite"));
        write(&db, None, &initial()).unwrap();
        let held = |db: &StateDb| db.snapshot().0.is_some();
        assert!(held(&db));
        // The reader's view: begun before the clear (its count), its rows read.
        let clears = db.snapshot().1;
        let mut reader = db.cipher.connect(&db.path, false).unwrap();
        let tx = reader.transaction().unwrap();
        let word = cipher::Cipher::word(&tx).unwrap().unwrap();
        let (_, _, rows) = tables::read(&tx, &db.cipher.key).unwrap();
        db.clear().unwrap();
        db.keep(clears, Snapshot { word, rows });
        drop(tx);
        assert!(!held(&db), "the reader's rows are not kept");
        // A failed clear (another process writing) forgets them as well.
        write(&db, None, &initial()).unwrap();
        assert!(held(&db));
        let mut writer = db.cipher.connect(&db.path, false).unwrap();
        let busy = writer.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        assert!(db.clear().is_err());
        assert!(!held(&db));
        drop(busy);
    }

    /// Each access begins and ends within its call, its connection closed: afterwards another process can take the
    /// database exclusively and truncate the WAL at once (no transaction, read mark or lock left behind). A refused
    /// access touches nothing.
    #[test]
    fn no_lock_outlives_an_access() {
        #[derive(Default)]
        struct Counted {
            open: std::sync::atomic::AtomicI32,
            begun: std::sync::atomic::AtomicI32,
            refuse: AtomicBool,
        }
        impl Access for Counted {
            fn begin(&self) -> bool {
                if self.refuse.load(Ordering::SeqCst) {
                    return false;
                }
                self.begun.fetch_add(1, Ordering::SeqCst);
                assert_eq!(self.open.fetch_add(1, Ordering::SeqCst), 0, "accesses do not nest");
                true
            }
            fn end(&self) {
                self.open.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let dir = Dir::new("access");
        let db = open(&dir.0.join("state.sqlite"));
        let counted = Arc::new(Counted::default());
        db.set_access(Some(counted.clone()));
        let other = Connection::open(&db.path).unwrap();
        let free = || {
            assert_eq!(counted.open.load(Ordering::SeqCst), 0);
            other.execute_batch("BEGIN EXCLUSIVE; COMMIT").expect("no lock left on the database");
            let busy: i64 = other.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0)).unwrap();
            assert_eq!(busy, 0, "no reader left on the WAL");
        };
        let mut state = initial();
        let word = write(&db, None, &state).unwrap();
        free();
        assert!(matches!(db.load(Some(&word), &KEY).unwrap(), Loaded::Same));
        free();
        let (word, _) = read(&db);
        free();
        assert!(db.registered().is_ok());
        free();
        state.next_seq += 1;
        assert!(write(&db, Some("an older one"), &state).is_err());
        free();
        let word = write(&db, Some(&word), &state).unwrap();
        free();
        assert_eq!(counted.begun.load(Ordering::SeqCst), 6);
        counted.refuse.store(true, Ordering::SeqCst);
        state.next_seq += 1;
        assert!(matches!(write(&db, Some(&word), &state), Err(Error::Save(kind)) if kind == "Busy"));
        assert!(db.clear().is_err());
        assert_eq!(counted.begun.load(Ordering::SeqCst), 6);
        counted.refuse.store(false, Ordering::SeqCst);
        assert_eq!(db.word().unwrap().as_deref(), Some(word.as_str()));
        db.clear().unwrap();
        free();
    }

    #[test]
    fn fresh_database_rejects_a_future_schema() {
        let dir = Dir::new("fresh");
        let path = dir.0.join("state.sqlite");
        let db = open(&path);
        let conn = db.cipher.connect(&path, false).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        assert!(matches!(
            StateDb::open_fresh(&path, KEY, Arc::new(|_| {})),
            Err(Error::Save(kind)) if kind == "UnsupportedVersion"
        ));
    }

    #[test]
    fn a_failure_keeps_sqlites_extended_code_and_is_reported_by_its_class() {
        let failure = |code| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), Some("/a/path".into()));
        let kind = |error| match cipher::sqlite_error(error) {
            Error::Save(kind) => kind,
            other => panic!("{other:?}"),
        };
        assert_eq!(kind(failure(rusqlite::ffi::SQLITE_IOERR_SHMMAP)), "Io/5386");
        assert_eq!(kind(failure(rusqlite::ffi::SQLITE_CANTOPEN)), "Io/14");
        assert_eq!(kind(failure(rusqlite::ffi::SQLITE_BUSY)), "Busy");
        assert_eq!(kind(failure(rusqlite::ffi::SQLITE_FULL)), "Full");
        assert_eq!(kind(rusqlite::Error::QueryReturnedNoRows), "Sqlite");
        let reported = Arc::new(Mutex::new(Vec::new()));
        let into = reported.clone();
        let report: Report = Arc::new(move |kind| into.lock().unwrap().push(kind));
        cipher::report_failure(&report, &cipher::sqlite_error(failure(rusqlite::ffi::SQLITE_IOERR_SHMMAP)));
        cipher::report_failure(&report, &Error::Save("something else".into()));
        assert_eq!(*reported.lock().unwrap(), ["Io/5386", "Write"]);
    }
}
