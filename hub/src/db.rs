//! The hub's store: SQLite, one connection. What it holds is listed in spec 1: no content key, no sender, no
//! membership. Every method is one transaction.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::sync::{Mutex, MutexGuard, PoisonError};
use yo_wire::{GroupId, Mailbox};

/// A phone number's HMAC: the only form in which the hub keeps it.
pub type PhoneHmac = [u8; 32];

pub struct Db {
    conn: Mutex<Connection>,
}

pub struct Device {
    pub phone: PhoneHmac,
    pub auth_key: [u8; 32],
}

pub struct NewDevice {
    pub mailbox: Mailbox,
    pub auth_key: [u8; 32],
    pub push_platform: u8,
    pub push_token: Vec<u8>,
}

/// A KeyPackage the hub checked: its bytes and the hash it is remembered by.
pub struct StoredKeyPackage {
    pub hash: [u8; 32],
    pub data: Vec<u8>,
}

/// A number's account key and the KeyPackages handed out for it.
pub type Found = ([u8; 32], Vec<Vec<u8>>);

pub enum Mailboxes {
    Live { push_platform: u8, push_token: Vec<u8> },
    Gone,
    Unknown,
}

pub enum Commit {
    Accepted,
    Stale(u64),
    BadToken,
    Unknown,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Conflict {
    MailboxTaken,
    KeyPackageSeen,
}

pub enum Write<T> {
    Done(T),
    Refused(Conflict),
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts (
    phone BLOB PRIMARY KEY,
    account_key BLOB NOT NULL,
    created_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS devices (
    mailbox BLOB PRIMARY KEY,
    phone BLOB NOT NULL REFERENCES accounts(phone),
    auth_key BLOB NOT NULL,
    push_platform INTEGER NOT NULL,
    push_token BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS devices_by_phone ON devices(phone);
CREATE TABLE IF NOT EXISTS gone (
    mailbox BLOB PRIMARY KEY,
    gone_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS key_packages (
    id INTEGER PRIMARY KEY,
    mailbox BLOB NOT NULL,
    data BLOB NOT NULL,
    last_resort INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS key_packages_by_mailbox ON key_packages(mailbox, last_resort, id);
CREATE TABLE IF NOT EXISTS seen_key_packages (
    hash BLOB PRIMARY KEY
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY,
    mailbox BLOB NOT NULL,
    envelope BLOB NOT NULL,
    received_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_by_mailbox ON messages(mailbox, id);
CREATE INDEX IF NOT EXISTS messages_by_age ON messages(received_at);
CREATE TABLE IF NOT EXISTS groups (
    id BLOB PRIMARY KEY,
    epoch INTEGER NOT NULL,
    token_hash BLOB NOT NULL,
    updated_at INTEGER NOT NULL
) WITHOUT ROWID;
";

fn array<const N: usize>(bytes: Vec<u8>) -> rusqlite::Result<[u8; N]> {
    bytes.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn device(&self, mailbox: &Mailbox) -> rusqlite::Result<Option<Device>> {
        self.conn()
            .prepare_cached("SELECT phone, auth_key FROM devices WHERE mailbox = ?1")?
            .query_row([mailbox], |row| Ok(Device { phone: array(row.get(0)?)?, auth_key: array(row.get(1)?)? }))
            .optional()
    }

    pub fn is_gone(&self, mailbox: &Mailbox) -> rusqlite::Result<bool> {
        self.conn().prepare_cached("SELECT 1 FROM gone WHERE mailbox = ?1")?.exists([mailbox])
    }

    pub fn touch(&self, mailbox: &Mailbox, now: u64) -> rusqlite::Result<()> {
        self.conn()
            .prepare_cached("UPDATE devices SET last_seen_at = ?2 WHERE mailbox = ?1")?
            .execute(params![mailbox, now])?;
        Ok(())
    }

    pub fn account_key(&self, phone: &PhoneHmac) -> rusqlite::Result<Option<[u8; 32]>> {
        self.conn()
            .prepare_cached("SELECT account_key FROM accounts WHERE phone = ?1")?
            .query_row([phone], |row| array(row.get(0)?))
            .optional()
    }

    /// Registers a device for a number that passed SMS verification. The number's earlier devices are replaced:
    /// their mailboxes are gone, their KeyPackages and envelopes deleted.
    pub fn register(
        &self,
        phone: &PhoneHmac,
        account_key: &[u8; 32],
        device: &NewDevice,
        key_packages: &[StoredKeyPackage],
        last_resort: &StoredKeyPackage,
        now: u64,
    ) -> rusqlite::Result<Write<()>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let taken = tx
            .prepare_cached("SELECT 1 FROM devices WHERE mailbox = ?1 UNION ALL SELECT 1 FROM gone WHERE mailbox = ?1")?
            .exists([&device.mailbox])?;
        if taken {
            return Ok(Write::Refused(Conflict::MailboxTaken));
        }
        let old: Vec<Vec<u8>> = tx
            .prepare_cached("SELECT mailbox FROM devices WHERE phone = ?1")?
            .query_map([phone], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        for mailbox in &old {
            tx.execute("INSERT OR REPLACE INTO gone (mailbox, gone_at) VALUES (?1, ?2)", params![mailbox, now])?;
            tx.execute("DELETE FROM key_packages WHERE mailbox = ?1", [mailbox])?;
            tx.execute("DELETE FROM messages WHERE mailbox = ?1", [mailbox])?;
            tx.execute("DELETE FROM devices WHERE mailbox = ?1", [mailbox])?;
        }
        tx.execute(
            "INSERT INTO accounts (phone, account_key, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (phone) DO UPDATE SET account_key = excluded.account_key",
            params![phone, account_key, now],
        )?;
        tx.execute(
            "INSERT INTO devices (mailbox, phone, auth_key, push_platform, push_token, created_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![device.mailbox, phone, device.auth_key, device.push_platform, device.push_token, now],
        )?;
        if let Some(conflict) = insert_key_packages(&tx, &device.mailbox, key_packages, Some(last_resort))? {
            return Ok(Write::Refused(conflict));
        }
        tx.commit()?;
        Ok(Write::Done(()))
    }

    pub fn add_key_packages(
        &self,
        mailbox: &Mailbox,
        key_packages: &[StoredKeyPackage],
        last_resort: Option<&StoredKeyPackage>,
        most: u32,
    ) -> rusqlite::Result<Write<()>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let held: u32 = tx
            .prepare_cached("SELECT count(*) FROM key_packages WHERE mailbox = ?1 AND last_resort = 0")?
            .query_row([mailbox], |row| row.get(0))?;
        if held as usize + key_packages.len() > most as usize {
            // More than a device needs: refuse rather than store without end.
            return Ok(Write::Refused(Conflict::KeyPackageSeen));
        }
        if let Some(conflict) = insert_key_packages(&tx, mailbox, key_packages, last_resort)? {
            return Ok(Write::Refused(conflict));
        }
        tx.commit()?;
        Ok(Write::Done(()))
    }

    pub fn key_packages_left(&self, mailbox: &Mailbox) -> rusqlite::Result<u32> {
        self.conn()
            .prepare_cached("SELECT count(*) FROM key_packages WHERE mailbox = ?1 AND last_resort = 0")?
            .query_row([mailbox], |row| row.get(0))
    }

    /// The account key of a number and, if asked, one KeyPackage per device: a single-use one while there is
    /// one, else the last resort.
    pub fn lookup(&self, phone: &PhoneHmac, take: bool) -> rusqlite::Result<Option<Found>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(account_key) = tx
            .prepare_cached("SELECT account_key FROM accounts WHERE phone = ?1")?
            .query_row([phone], |row| array::<32>(row.get(0)?))
            .optional()?
        else {
            return Ok(None);
        };
        let mailboxes: Vec<Vec<u8>> = tx
            .prepare_cached("SELECT mailbox FROM devices WHERE phone = ?1 ORDER BY created_at, mailbox LIMIT 16")?
            .query_map([phone], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        if mailboxes.is_empty() {
            return Ok(None);
        }
        let mut key_packages = Vec::new();
        if take {
            for mailbox in &mailboxes {
                let found: Option<(i64, Vec<u8>, bool)> = tx
                    .prepare_cached(
                        "SELECT id, data, last_resort FROM key_packages WHERE mailbox = ?1
                         ORDER BY last_resort, id LIMIT 1",
                    )?
                    .query_row([mailbox], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .optional()?;
                if let Some((id, data, last_resort)) = found {
                    if !last_resort {
                        tx.execute("DELETE FROM key_packages WHERE id = ?1", [id])?;
                    }
                    key_packages.push(data);
                }
            }
        }
        tx.commit()?;
        Ok(Some((account_key, key_packages)))
    }

    pub fn set_push(&self, mailbox: &Mailbox, platform: u8, token: &[u8]) -> rusqlite::Result<()> {
        self.conn()
            .prepare_cached("UPDATE devices SET push_platform = ?2, push_token = ?3 WHERE mailbox = ?1")?
            .execute(params![mailbox, platform, token])?;
        Ok(())
    }

    /// Clears a push token the push service refused, unless the device set another meanwhile.
    pub fn clear_push(&self, mailbox: &Mailbox, token: &[u8]) -> rusqlite::Result<()> {
        self.conn()
            .prepare_cached(
                "UPDATE devices SET push_platform = 0, push_token = x'' WHERE mailbox = ?1 AND push_token = ?2",
            )?
            .execute(params![mailbox, token])?;
        Ok(())
    }

    /// Stores envelopes for live mailboxes, all in one transaction (group commit); says for each whom to wake.
    pub fn deliver(&self, items: &[(Mailbox, &[u8])], now: u64) -> rusqlite::Result<Vec<Mailboxes>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut outcomes = Vec::with_capacity(items.len());
        {
            let mut device = tx.prepare_cached("SELECT push_platform, push_token FROM devices WHERE mailbox = ?1")?;
            let mut insert =
                tx.prepare_cached("INSERT INTO messages (mailbox, envelope, received_at) VALUES (?1, ?2, ?3)")?;
            let mut gone = tx.prepare_cached("SELECT 1 FROM gone WHERE mailbox = ?1")?;
            for (mailbox, envelope) in items {
                let live = device
                    .query_row([mailbox], |row| Ok((row.get::<_, u8>(0)?, row.get::<_, Vec<u8>>(1)?)))
                    .optional()?;
                outcomes.push(match live {
                    Some((push_platform, push_token)) => {
                        insert.execute(params![mailbox, envelope, now])?;
                        Mailboxes::Live { push_platform, push_token }
                    }
                    None if gone.exists([mailbox])? => Mailboxes::Gone,
                    None => Mailboxes::Unknown,
                });
            }
        }
        tx.commit()?;
        Ok(outcomes)
    }

    pub fn fetch(&self, mailbox: &Mailbox, most: usize) -> rusqlite::Result<Vec<(u64, Vec<u8>)>> {
        self.conn()
            .prepare_cached("SELECT id, envelope FROM messages WHERE mailbox = ?1 ORDER BY id LIMIT ?2")?
            .query_map(params![mailbox, most as i64], |row| Ok((row.get::<_, i64>(0)? as u64, row.get(1)?)))?
            .collect()
    }

    pub fn ack(&self, mailbox: &Mailbox, ids: &[u64]) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut delete = tx.prepare_cached("DELETE FROM messages WHERE mailbox = ?1 AND id = ?2")?;
            for id in ids {
                delete.execute(params![mailbox, *id as i64])?;
            }
        }
        tx.commit()
    }

    /// Registers a group at an epoch; false if the id exists.
    pub fn create_group(&self, group: &GroupId, epoch: u64, token_hash: &[u8; 32], now: u64) -> rusqlite::Result<bool> {
        let changed = self
            .conn()
            .prepare_cached(
                "INSERT INTO groups (id, epoch, token_hash, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (id) DO NOTHING",
            )?
            .execute(params![group, epoch as i64, token_hash, now])?;
        Ok(changed == 1)
    }

    /// Orders a commit (spec 4.3): it is accepted only on the group's current epoch with that epoch's token.
    pub fn commit(
        &self,
        group: &GroupId,
        epoch: u64,
        token_hash: &[u8; 32],
        next_token_hash: &[u8; 32],
        now: u64,
    ) -> rusqlite::Result<Commit> {
        let conn = self.conn();
        let current: Option<(i64, Vec<u8>)> = conn
            .prepare_cached("SELECT epoch, token_hash FROM groups WHERE id = ?1")?
            .query_row([group], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        let Some((current, stored)) = current else {
            return Ok(Commit::Unknown);
        };
        if current as u64 != epoch {
            return Ok(Commit::Stale(current as u64));
        }
        if !constant_time_eq(&stored, token_hash) {
            return Ok(Commit::BadToken);
        }
        conn.prepare_cached("UPDATE groups SET epoch = ?2, token_hash = ?3, updated_at = ?4 WHERE id = ?1")?
            .execute(params![group, (epoch + 1) as i64, next_token_hash, now])?;
        Ok(Commit::Accepted)
    }

    /// Deletes envelopes older than the TTL and remembers gone mailboxes for a year.
    pub fn prune(&self, now: u64, envelope_ttl_ms: u64) -> rusqlite::Result<usize> {
        let conn = self.conn();
        let messages =
            conn.execute("DELETE FROM messages WHERE received_at < ?1", [now.saturating_sub(envelope_ttl_ms) as i64])?;
        conn.execute("DELETE FROM gone WHERE gone_at < ?1", [now.saturating_sub(365 * 24 * 3600 * 1000) as i64])?;
        Ok(messages)
    }
}

fn insert_key_packages(
    tx: &rusqlite::Transaction<'_>,
    mailbox: &Mailbox,
    key_packages: &[StoredKeyPackage],
    last_resort: Option<&StoredKeyPackage>,
) -> rusqlite::Result<Option<Conflict>> {
    let mut seen = tx.prepare_cached("INSERT INTO seen_key_packages (hash) VALUES (?1) ON CONFLICT DO NOTHING")?;
    let mut insert = tx.prepare_cached("INSERT INTO key_packages (mailbox, data, last_resort) VALUES (?1, ?2, ?3)")?;
    for (key_package, last) in key_packages.iter().map(|k| (k, false)).chain(last_resort.map(|k| (k, true))) {
        if seen.execute([&key_package.hash])? == 0 {
            return Ok(Some(Conflict::KeyPackageSeen));
        }
        if last {
            tx.execute("DELETE FROM key_packages WHERE mailbox = ?1 AND last_resort = 1", [mailbox])?;
        }
        insert.execute(params![mailbox, key_package.data, last])?;
    }
    Ok(None)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
