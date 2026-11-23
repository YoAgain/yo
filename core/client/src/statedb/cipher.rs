use super::{checkpoint, BUSY};
use crate::{Error, Result};
use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use zeroize::Zeroizing;

/// Told the kind of every failure (a fixed word, never SQLite's message), for counted diagnostics.
pub type Report = Arc<dyn Fn(String) + Send + Sync>;

fn associated_data(parts: [&str; 4]) -> Vec<u8> {
    let mut aad = b"yo client sqlite column v1".to_vec();
    for part in parts {
        aad.extend_from_slice(&(part.len() as u64).to_le_bytes());
        aad.extend_from_slice(part.as_bytes());
    }
    aad
}

pub(super) fn seal(key: &[u8; 32], parts: [&str; 4], plain: &[u8]) -> Result<Vec<u8>> {
    Ok(crate::aead::seal(key, &associated_data(parts), plain))
}

pub(crate) fn unseal(key: &[u8; 32], parts: [&str; 4], sealed: &[u8]) -> Result<Vec<u8>> {
    crate::aead::open(key, &associated_data(parts), sealed).ok_or(Error::State)
}

pub(super) struct Cipher {
    pub(super) key: Zeroizing<[u8; 32]>,
    pub(super) report: Report,
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

/// What the database failed with, as a fixed kind for counted diagnostics. A turn taken back is not the database's
/// failure (`TURN_ENDED`): not reported.
pub(super) fn report_failure(report: &Report, error: &Error) {
    const KINDS: [&str; 7] = ["Busy", "Full", "Io", "InvalidDatabase", "Constraint", "UnsupportedVersion", "Sqlite"];
    let kind = match error {
        Error::Save(kind) if kind == super::TURN_ENDED => return,
        Error::Save(kind) if kind == super::STALE => "Stale",
        Error::Save(kind) if KINDS.contains(&kind.as_str()) => kind,
        Error::Save(_) => "Write",
        Error::State => "InvalidState",
        _ => "Read",
    };
    report(kind.into());
}

fn parent(path: &Path) -> &Path {
    path.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(parent(path)).and_then(|dir| dir.sync_all()).map_err(io_error)
}

impl Cipher {
    pub(super) fn open_fresh(path: &Path, key: [u8; 32], report: Report) -> Result<Self> {
        let cipher = Self { key: Zeroizing::new(key), report };
        let result = (|| {
            let mut conn = cipher.connect(path, true)?;
            if version(&conn)? != 0 {
                cipher.check_schema(&conn)?;
                return Ok(());
            }
            let mode: String =
                conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0)).map_err(sqlite_error)?;
            if mode != "wal" {
                return Err(Error::Save("Sqlite".into()));
            }
            conn.pragma_update(None, "synchronous", "FULL").map_err(sqlite_error)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sqlite_error)?;
            if version(&tx)? == 0 {
                tx.execute_batch(include_str!("../../migrations/V0001__initial_state.sql")).map_err(sqlite_error)?;
                tx.execute(
                    "UPDATE state_meta SET key_check = ?1 WHERE id = 1",
                    [seal(&cipher.key, ["state_meta", "1", "", "key_check"], b"yo state v3")?],
                )
                .map_err(sqlite_error)?;
                tx.pragma_update(None, "user_version", 1).map_err(sqlite_error)?;
                sync_directory(path)?;
                tx.commit().map_err(sqlite_error)?;
                checkpoint(&conn);
            } else {
                cipher.check_schema(&tx)?;
            }
            Ok(())
        })();
        if let Err(error) = &result {
            report_failure(&cipher.report, error);
        }
        result.map(|()| cipher)
    }

    fn check_schema(&self, conn: &Connection) -> Result<()> {
        if version(conn)? != 1 {
            return Err(Error::Save("UnsupportedVersion".into()));
        }
        let check: Vec<u8> = conn
            .query_row("SELECT key_check FROM state_meta WHERE id = 1", [], |row| row.get(0))
            .map_err(sqlite_error)?;
        if unseal(&self.key, ["state_meta", "1", "", "key_check"], &check)? != b"yo state v3" {
            return Err(Error::State);
        }
        Ok(())
    }

    pub(super) fn connect(&self, path: &Path, create: bool) -> Result<Connection> {
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        if create {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        let conn = Connection::open_with_flags(path, super::protected(flags)).map_err(sqlite_error)?;
        conn.busy_timeout(BUSY).map_err(sqlite_error)?;
        conn.execute_batch(
            "PRAGMA synchronous = NORMAL; PRAGMA temp_store = MEMORY; PRAGMA secure_delete = ON;
             PRAGMA fullfsync = ON; PRAGMA checkpoint_fullfsync = ON",
        )
        .map_err(sqlite_error)?;
        Ok(conn)
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
