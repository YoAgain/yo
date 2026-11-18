use super::{checkpoint, tables, BUSY};
use crate::{state, Error, Result};
use chacha20poly1305::{
    aead::{Aead, Payload},
    ChaCha20Poly1305, KeyInit,
};
use rusqlite::{params, Connection, OpenFlags, Transaction, TransactionBehavior};
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use zeroize::Zeroizing;

type Import = Option<(String, Vec<u8>)>;
type Step = fn(&Transaction<'_>, &mut dyn FnMut() -> Result<Import>, &[u8; 32]) -> Result<()>;
pub type Report = Arc<dyn Fn(u32, u32, u32, String) + Send + Sync>;

struct Migration {
    version: u32,
    sql: &'static str,
    run: Step,
}

// Append steps. Released SQL and its Rust import codec are immutable.
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: include_str!("../../migrations/V0001__move_sealed_state_into_tables.sql"),
    run: move_sealed_state_into_tables,
}];

pub(super) fn latest() -> u32 {
    MIGRATIONS.last().expect("initial migration").version
}

fn move_sealed_state_into_tables(
    tx: &Transaction<'_>,
    source: &mut dyn FnMut() -> Result<Import>,
    key: &[u8; 32],
) -> Result<()> {
    if let Some((word, blob)) = source()? {
        let (state, records) = state::open(&blob, key)?;
        tables::write(tx, key, &state, &records)?;
        tx.execute("UPDATE state_meta SET word = ?1 WHERE id = 1", [word]).map_err(sqlite_error)?;
    }
    tx.execute(
        "UPDATE state_meta SET key_check = ?1 WHERE id = 1",
        [seal(key, ["state_meta", "1", "", "key_check"], b"yo")?],
    )
    .map_err(sqlite_error)?;
    Ok(())
}

fn associated_data(parts: [&str; 4]) -> Vec<u8> {
    let mut aad = b"yo client sqlite column v1".to_vec();
    for part in parts {
        aad.extend_from_slice(&(part.len() as u64).to_le_bytes());
        aad.extend_from_slice(part.as_bytes());
    }
    aad
}

pub(super) fn seal(key: &[u8; 32], parts: [&str; 4], plain: &[u8]) -> Result<Vec<u8>> {
    let nonce = crate::identity::random::<12>();
    let ciphertext = ChaCha20Poly1305::new(key.into())
        .encrypt(&nonce.into(), Payload { msg: plain, aad: &associated_data(parts) })
        .map_err(|_| Error::State)?;
    Ok([nonce.as_slice(), &ciphertext].concat())
}

pub(super) fn unseal(key: &[u8; 32], parts: [&str; 4], sealed: &[u8]) -> Result<Vec<u8>> {
    let (nonce, ciphertext) = sealed.split_first_chunk::<12>().ok_or(Error::State)?;
    ChaCha20Poly1305::new(key.into())
        .decrypt(nonce.into(), Payload { msg: ciphertext, aad: &associated_data(parts) })
        .map_err(|_| Error::State)
}

pub(super) struct Cipher {
    pub(super) key: Zeroizing<[u8; 32]>,
    pub(super) report: Option<Report>,
}

pub(super) fn sqlite_error(error: rusqlite::Error) -> Error {
    use rusqlite::ErrorCode::*;
    // SQLite's message can contain paths or data. Retain only the primary error class.
    let kind = match error.sqlite_error_code() {
        Some(DatabaseBusy | DatabaseLocked) => "Busy",
        Some(DiskFull) => "Full",
        Some(SystemIoFailure | CannotOpen | PermissionDenied | ReadOnly) => "Io",
        Some(DatabaseCorrupt | NotADatabase) => "InvalidDatabase",
        Some(ConstraintViolation) => "Constraint",
        _ => "Sqlite",
    };
    Error::Save(kind.into())
}

fn io_error(_: std::io::Error) -> Error {
    Error::Save("Io".into())
}

pub(super) fn report_failure(report: Option<&Report>, step: u32, before: u32, after: u32, error: &Error) {
    let kind = match error {
        Error::Save(kind) => match kind.as_str() {
            "Busy" => "Busy",
            "Full" => "Full",
            "Io" => "Io",
            "InvalidDatabase" => "InvalidDatabase",
            "Constraint" => "Constraint",
            "UnsupportedVersion" => "UnsupportedVersion",
            "LegacyConflict" => "LegacyConflict",
            "Sqlite" => "Sqlite",
            "LegacySuperseded" => "LegacySuperseded",
            "TurnEnded" => "TurnEnded",
            _ => "Write",
        },
        Error::State => "InvalidState",
        _ => "Read",
    };
    if let Some(report) = report {
        report(step, before, after, kind.into());
    }
}

fn parent(path: &Path) -> &Path {
    path.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(parent(path)).and_then(|dir| dir.sync_all()).map_err(io_error)
}

pub(super) fn verify_unmigrated(path: &Path, key: &[u8; 32]) -> Result<()> {
    if !path.try_exists().map_err(io_error)? {
        return Ok(());
    }
    let cipher = Cipher { key: Zeroizing::new(*key), report: None };
    if version(&cipher.connect(path, false)?)? != 0 {
        return Err(Error::Save("LegacySuperseded".into()));
    }
    Ok(())
}

impl Cipher {
    pub(super) fn open_current(path: &Path, key: [u8; 32], report: Report) -> Result<Option<Self>> {
        let mut before = 0;
        let result = (|| {
            if !path.try_exists().map_err(io_error)? {
                return Ok(None);
            }
            let cipher = Self { key: Zeroizing::new(key), report: Some(report.clone()) };
            let conn = cipher.connect(path, false)?;
            before = version(&conn)?;
            if before == latest() {
                cipher.check_key(&conn)?;
            }
            if before > latest() {
                return Err(Error::Save("UnsupportedVersion".into()));
            }
            Ok((before == latest()).then_some(cipher))
        })();
        if let Err(error) = &result {
            report_failure(Some(&report), 0, before, before, error);
        }
        result
    }

    pub(super) fn open(
        path: &Path,
        key: [u8; 32],
        report: Option<Report>,
        mut source: impl FnMut() -> Result<Import>,
        held: Option<&dyn Fn() -> bool>,
        rolled_back: &mut bool,
    ) -> Result<Self> {
        let mut before = 0;
        let mut step = 0;
        let result = (|| {
            *rolled_back = !path.try_exists().map_err(io_error)?;
            let key = Zeroizing::new(key);
            let cipher = Self { key, report: report.clone() };
            let mut conn = cipher.connect(path, true)?;
            before = version(&conn)?;
            *rolled_back = before == 0;
            if before > latest() {
                return Err(Error::Save("UnsupportedVersion".into()));
            }
            if before == latest() {
                cipher.check_key(&conn)?;
                return Ok(cipher);
            }
            // A V0000 database can only be a new destination or a rolled-back import.
            step = before + 1;
            let mode: String =
                conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0)).map_err(sqlite_error)?;
            if mode != "wal" {
                return Err(Error::Save("Sqlite".into()));
            }
            conn.pragma_update(None, "synchronous", "FULL").map_err(sqlite_error)?;
            loop {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sqlite_error)?;
                // Recheck within the writer transaction: another opener may have completed the step.
                before = version(&tx)?;
                *rolled_back = before == 0;
                if before == latest() {
                    cipher.check_key(&tx)?;
                    break;
                }
                let migration = MIGRATIONS
                    .iter()
                    .find(|m| m.version == before + 1)
                    .ok_or_else(|| Error::Save("UnsupportedVersion".into()))?;
                step = migration.version;
                run_step(&tx, migration, &mut source, &cipher.key)?;
                sync_directory(path)?;
                if held.is_some_and(|held| !held()) {
                    return Err(Error::Save("TurnEnded".into()));
                }
                // An I/O error during COMMIT can make the outcome ambiguous. Never fall back then.
                *rolled_back = false;
                tx.commit().map_err(sqlite_error)?;
            }
            checkpoint(&conn);
            Ok(cipher)
        })();
        if let Err(error) = &result {
            report_failure(report.as_ref(), step, before, before, error);
        }
        result
    }

    pub(super) fn connect(&self, path: &Path, create: bool) -> Result<Connection> {
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        if create {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        let conn = Connection::open_with_flags(path, super::protected(flags)).map_err(sqlite_error)?;
        conn.busy_timeout(BUSY).map_err(sqlite_error)?;
        conn.execute_batch(
            "PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON; PRAGMA temp_store = MEMORY; PRAGMA secure_delete = ON;
             PRAGMA fullfsync = ON; PRAGMA checkpoint_fullfsync = ON",
        )
        .map_err(sqlite_error)?;
        Ok(conn)
    }

    fn check_key(&self, conn: &Connection) -> Result<()> {
        let check: Vec<u8> = conn
            .query_row("SELECT key_check FROM state_meta WHERE id = 1", [], |row| row.get(0))
            .map_err(sqlite_error)?;
        if unseal(&self.key, ["state_meta", "1", "", "key_check"], &check)? != b"yo" {
            return Err(Error::State);
        }
        Ok(())
    }

    pub(super) fn word(conn: &Connection) -> Result<Option<String>> {
        conn.query_row("SELECT word FROM state_meta WHERE id = 1", [], |row| row.get(0)).map_err(sqlite_error)
    }

    pub(super) fn set_word(conn: &Connection, word: Option<&str>) -> Result<()> {
        conn.execute("UPDATE state_meta SET word = ?1 WHERE id = 1", params![word]).map_err(sqlite_error)?;
        Ok(())
    }
}

fn version(conn: &Connection) -> Result<u32> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0)).map_err(sqlite_error)
}

fn run_step(
    tx: &Transaction<'_>,
    migration: &Migration,
    source: &mut dyn FnMut() -> Result<Import>,
    key: &[u8; 32],
) -> Result<()> {
    tx.execute_batch(migration.sql).map_err(sqlite_error)?;
    (migration.run)(tx, source, key)?;
    tx.pragma_update(None, "user_version", migration.version).map_err(sqlite_error)?;
    Ok(())
}
