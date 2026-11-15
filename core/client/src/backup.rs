//! The backup record (spec 7) as the apps make and open it: K wrapped by the passkey's PRF output and by the recovery
//! key, what the app keeps sealed under K, the recovery key's locator, and each door's auth key, which signs the
//! record's fetch. The recovery key is Signal's: libsignal's account entropy pool (64 characters, a–z and 0–9), and
//! its backup key what Yo's keys come from. The formats are CryptoKit's too: HKDF-SHA256 without salt,
//! ChaCha20-Poly1305 as nonce || ciphertext || tag.

use crate::error::{Error, Result};
use crate::storage::{hkdf, open_padded, seal_padded};
use crate::{Client, Transport};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::ChaCha20Poly1305;
pub use libsignal_account_keys::AccountEntropyPool;
use libsignal_account_keys::BackupKey;
use libsignal_protocol::PrivateKey;
use std::sync::atomic::{AtomicU64, Ordering};
use yo_wire::api::{self, status, Backup, LOCATOR_LEN};
use yo_wire::KEY_LEN;

/// A new recovery key: 64 characters, a–z and 0–9 (libsignal's `AccountEntropyPool::generate`).
pub fn new_recovery_key() -> AccountEntropyPool {
    AccountEntropyPool::generate(&mut rand::rng())
}

/// The recovery key in what was pasted, typed or scanned: case, spaces, dashes and line breaks between the
/// characters do not matter. `None` when it is no recovery key.
pub fn parse_recovery_key(typed: &str) -> Option<AccountEntropyPool> {
    let plain: String =
        typed.chars().filter(|c| !c.is_whitespace() && *c != '-').map(|c| c.to_ascii_lowercase()).collect();
    plain.parse().ok()
}

/// What opens a backup: the passkey's PRF output, or the recovery key.
#[derive(Clone, Copy)]
pub enum Door<'a> {
    Passkey(&'a [u8]),
    Code(&'a AccountEntropyPool),
}

impl Door<'_> {
    /// HKDF of the door's secret: the PRF output, or the recovery key's backup key (libsignal's
    /// `BackupKey::derive_from_account_entropy_pool`).
    fn derive<const N: usize>(&self, info: &str) -> [u8; N] {
        let mut out = [0; N];
        match self {
            Door::Passkey(prf) => hkdf(prf, info.as_bytes(), &mut out),
            Door::Code(pool) => hkdf(&BackupKey::derive_from_account_entropy_pool(pool).0, info.as_bytes(), &mut out),
        }
        out
    }

    fn name(&self) -> &'static str {
        match self {
            Door::Passkey(_) => "passkey",
            Door::Code(_) => "code",
        }
    }

    /// The key K is wrapped under: HKDF(secret, "yo backup v3 passkey" or "yo backup v3 code").
    fn wrap_key(&self) -> [u8; 32] {
        self.derive(&format!("yo backup v3 {}", self.name()))
    }

    /// The key that signs the record's fetch (XEdDSA): HKDF(secret, "yo backup v3 auth passkey" or "... auth code") as
    /// a Curve25519 private key.
    pub fn auth_key(&self) -> PrivateKey {
        let bytes: [u8; 32] = self.derive(&format!("yo backup v3 auth {}", self.name()));
        PrivateKey::deserialize(&bytes).expect("32 bytes are a Curve25519 private key")
    }

    /// The auth key's public key, as the record has it.
    pub fn auth_public(&self) -> [u8; KEY_LEN] {
        let public = self.auth_key().public_key().expect("a Curve25519 public key");
        public.public_key_bytes().try_into().expect("32 bytes")
    }

    /// K wrapped for this door in a record.
    fn wrapped<'b>(&self, record: &'b Backup) -> &'b [u8] {
        match self {
            Door::Passkey(_) => record.by_passkey.as_slice(),
            Door::Code(_) => record.by_code.as_slice(),
        }
    }
}

/// Where the recovery key alone finds the record: HKDF(its backup key, "yo backup v3 locator"), 16 bytes.
pub fn locator(code: &AccountEntropyPool) -> [u8; LOCATOR_LEN] {
    Door::Code(code).derive("yo backup v3 locator")
}

/// The passkey's door in a record being made.
pub enum PasskeyDoor<'a> {
    /// None: the recovery key alone opens it.
    None,
    /// A passkey's PRF output: K wrapped under it, and its auth key.
    New(&'a [u8]),
    /// As in this record (K the same): its wrap and its auth key, or none.
    Keep(&'a Backup),
}

fn seal(key: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    let nonce: [u8; 12] = crate::identity::random();
    let sealed = ChaCha20Poly1305::new(key.into()).encrypt(&nonce.into(), plain).expect("encryption");
    [nonce.to_vec(), sealed].concat()
}

/// K from its wrap under a door.
fn unwrap_k(wrapped: &[u8], door: &Door) -> Option<[u8; 32]> {
    let (nonce, sealed) = wrapped.split_at_checked(12)?;
    ChaCha20Poly1305::new(&door.wrap_key().into()).decrypt(nonce.into(), sealed).ok()?.try_into().ok()
}

/// A record for the hub: K under the recovery key always, under the passkey as `passkey` says; `plain` (the app's
/// account and recovery key) padded and sealed under K; the recovery key's locator; the doors' auth keys.
pub fn make(k: &[u8; 32], plain: &[u8], code: &AccountEntropyPool, passkey: PasskeyDoor) -> Result<Backup> {
    let (by_passkey, auth_passkey) = match passkey {
        PasskeyDoor::None => (Vec::new(), None),
        PasskeyDoor::New(prf) => (seal(&Door::Passkey(prf).wrap_key(), k), Some(Door::Passkey(prf).auth_public())),
        PasskeyDoor::Keep(record) => (record.by_passkey.as_slice().to_vec(), record.auth_keys().0),
    };
    let door = Door::Code(code);
    let backup = Backup::new(
        by_passkey,
        seal(&door.wrap_key(), k),
        seal_padded(k, &[], plain)?,
        locator(code).to_vec(),
        auth_passkey,
        door.auth_public(),
    );
    backup.check()?;
    Ok(backup)
}

/// A record opened by a door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub k: [u8; 32],
    /// What K sealed, without its padding.
    pub plain: Vec<u8>,
    /// The record as the hub has it: its passkey door to keep (`PasskeyDoor::Keep`).
    pub record: Backup,
}

/// Opens a record with a door; `None` when the door does not open it.
pub fn open(record: Backup, door: &Door) -> Option<Opened> {
    let k = unwrap_k(door.wrapped(&record), door)?;
    let plain = open_padded(&k, &[], record.sealed.as_slice())?;
    Some(Opened { k, plain, record })
}

/// The last time a fetch was signed at, here: each signature once (the hub's replay cache), also two in one
/// millisecond.
static SIGNED_AT: AtomicU64 = AtomicU64::new(0);

impl Client {
    /// Fetches a backup record by the user id (a passkey's user handle) or by the recovery key's locator (spec 7),
    /// before there is an account on this phone, signed with the door's auth key; `None` when there is none, or none
    /// this door may fetch (the hub says the same for both).
    pub fn fetch_backup(transport: &dyn Transport, id: &[u8], door: &Door) -> Result<Option<Backup>> {
        if id.len() != LOCATOR_LEN {
            return Err(Error::Protocol("backup id"));
        }
        let path = api::backup_path(id);
        let wall = crate::wall_ms();
        let next = |last: u64| wall.max(last + 1);
        let time =
            next(SIGNED_AT.try_update(Ordering::Relaxed, Ordering::Relaxed, |last| Some(next(last))).unwrap_or(0));
        let key = door.auth_key();
        let sign = |message: &[u8]| {
            let signature = key.calculate_signature(message, &mut rand::rng()).expect("a Curve25519 key signs");
            (*signature).try_into().expect("an XEdDSA signature is 64 bytes")
        };
        let headers = crate::signature_headers(sign, "GET", &path, time, &yo_wire::sha256(&[]));
        let response = transport.request("GET", &path, &headers, &[]).map_err(Error::Network)?;
        match response.status {
            status::OK => {
                let backup: Backup = yo_wire::decode(&response.body)?;
                backup.check()?;
                Ok(Some(backup))
            }
            status::NOT_FOUND => Ok(None),
            _ => Err(crate::refused(&response)),
        }
    }

    /// Fetches and opens the record a door opens: by the user id when given (a passkey's user handle), else by the
    /// recovery key's locator. `None` when there is none it opens.
    pub fn open_backup(transport: &dyn Transport, user_id: Option<&[u8]>, door: &Door) -> Result<Option<Opened>> {
        let id = match (user_id, door) {
            (Some(id), _) => id.to_vec(),
            (None, Door::Code(code)) => locator(code).to_vec(),
            (None, Door::Passkey(_)) => return Err(Error::Protocol("backup id")),
        };
        Ok(Self::fetch_backup(transport, &id, door)?.and_then(|record| open(record, door)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recovery_key_reads_back_however_it_is_written() {
        let code = new_recovery_key();
        let text = code.to_string();
        assert_eq!(text.len(), 64);
        let grouped: Vec<String> =
            text.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).to_uppercase()).collect();
        assert_eq!(parse_recovery_key(&grouped.join(" ")).map(|c| c.to_string()), Some(text.clone()));
        assert_eq!(parse_recovery_key(&grouped.join("-\n")).map(|c| c.to_string()), Some(text.clone()));
        assert!(parse_recovery_key(&text[1..]).is_none());
        assert!(parse_recovery_key(&format!("{}!", &text[1..])).is_none());
    }

    #[test]
    fn either_door_opens_and_signs() {
        let (k, prf, code) = ([1; 32], [2; 32], new_recovery_key());
        let record = make(&k, b"{\"account\":1}", &code, PasskeyDoor::New(&prf)).unwrap();
        assert_eq!(record.sealed.as_slice().len(), 1024, "padded");
        assert_eq!(
            record.auth_keys(),
            (Some(Door::Passkey(&prf).auth_public()), Some(Door::Code(&code).auth_public()))
        );
        assert_ne!(Door::Passkey(&prf).auth_public(), Door::Code(&code).auth_public());
        for door in [Door::Passkey(&prf), Door::Code(&code)] {
            let opened = open(record.clone(), &door).unwrap();
            assert_eq!((opened.k, opened.plain.as_slice()), (k, &b"{\"account\":1}"[..]));
        }
        assert!(open(record.clone(), &Door::Passkey(&[3; 32])).is_none());
        assert!(open(record.clone(), &Door::Code(&new_recovery_key())).is_none());
        let without = make(&k, b"{}", &code, PasskeyDoor::None).unwrap();
        assert!(without.by_passkey.as_slice().is_empty() && without.auth_keys().0.is_none());
        assert!(open(without, &Door::Passkey(&prf)).is_none());
    }

    #[test]
    fn a_kept_door_stays_under_a_new_recovery_key() {
        let (k, prf, code) = ([1; 32], [2; 32], new_recovery_key());
        let first = make(&k, b"{}", &code, PasskeyDoor::New(&prf)).unwrap();
        let renewed = new_recovery_key();
        let kept = make(&k, b"{}", &renewed, PasskeyDoor::Keep(&first)).unwrap();
        assert_eq!((&kept.by_passkey, kept.auth_keys().0), (&first.by_passkey, first.auth_keys().0));
        assert_ne!(kept.locator(), first.locator());
        assert!(open(kept.clone(), &Door::Passkey(&prf)).is_some());
        assert!(open(kept, &Door::Code(&code)).is_none());
    }
}
