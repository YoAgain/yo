//! The backup record (spec 7) as the apps make and open it: K wrapped by the passkey's PRF output and by the 12 words,
//! what the app keeps sealed under K, the words' locator, and each door's auth key, which signs the record's fetch.
//! The formats are CryptoKit's too: HKDF-SHA256 without salt, ChaCha20-Poly1305 as nonce || ciphertext || tag.

use crate::error::{Error, Result};
use crate::storage::{hkdf, open_padded, seal_padded};
use crate::{Client, Transport};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::ChaCha20Poly1305;
use ed25519_dalek::SigningKey;
use std::sync::atomic::{AtomicU64, Ordering};
use yo_wire::api::{self, status, Backup, LOCATOR_LEN};
use yo_wire::KEY_LEN;

/// What opens a backup: the passkey's PRF output, or the 12 recovery words.
#[derive(Clone, Copy)]
pub enum Door<'a> {
    Passkey(&'a [u8]),
    Code(&'a [String]),
}

impl Door<'_> {
    /// HKDF of the door's secret: the PRF output, or the words joined by single spaces.
    fn derive<const N: usize>(&self, info: &str) -> [u8; N] {
        let mut out = [0; N];
        match self {
            Door::Passkey(prf) => hkdf(prf, info.as_bytes(), &mut out),
            Door::Code(words) => hkdf(words.join(" ").as_bytes(), info.as_bytes(), &mut out),
        }
        out
    }

    fn name(&self) -> &'static str {
        match self {
            Door::Passkey(_) => "passkey",
            Door::Code(_) => "code",
        }
    }

    /// The key K is wrapped under: HKDF(secret, "yo backup v1 passkey" or "yo backup v1 code").
    fn wrap_key(&self) -> [u8; 32] {
        self.derive(&format!("yo backup v1 {}", self.name()))
    }

    /// The key that signs the record's fetch: HKDF(secret, "yo backup v1 auth passkey" or "... auth code") as an
    /// Ed25519 seed.
    pub fn auth_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.derive(&format!("yo backup v1 auth {}", self.name())))
    }

    fn auth_public(&self) -> [u8; KEY_LEN] {
        self.auth_key().verifying_key().to_bytes()
    }

    /// K wrapped for this door in a record.
    fn wrapped<'b>(&self, record: &'b Backup) -> &'b [u8] {
        match self {
            Door::Passkey(_) => record.by_passkey.as_slice(),
            Door::Code(_) => record.by_code.as_slice(),
        }
    }
}

/// Where the words alone find the record: HKDF(the words, "yo backup v1 locator"), 16 bytes.
pub fn locator(words: &[String]) -> [u8; LOCATOR_LEN] {
    Door::Code(words).derive("yo backup v1 locator")
}

/// The passkey's door in a record being made.
pub enum PasskeyDoor<'a> {
    /// None: the words alone open it.
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

/// A record for the hub: K under the words always, under the passkey as `passkey` says; `plain` (the app's account
/// and words) padded and sealed under K; the words' locator; the doors' auth keys.
pub fn make(k: &[u8; 32], plain: &[u8], words: &[String], passkey: PasskeyDoor) -> Result<Backup> {
    let (by_passkey, auth_passkey) = match passkey {
        PasskeyDoor::None => (Vec::new(), None),
        PasskeyDoor::New(prf) => (seal(&Door::Passkey(prf).wrap_key(), k), Some(Door::Passkey(prf).auth_public())),
        PasskeyDoor::Keep(record) => (record.by_passkey.as_slice().to_vec(), record.auth_keys().0),
    };
    let code = Door::Code(words);
    let backup = Backup::new(
        by_passkey,
        seal(&code.wrap_key(), k),
        seal_padded(k, &[], plain)?,
        locator(words).to_vec(),
        auth_passkey,
        code.auth_public(),
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
    /// Fetches a backup record by the user id (a passkey's user handle) or by the words' locator (spec 7), before
    /// there is an account on this phone, signed with the door's auth key; `None` when there is none, or none this
    /// door may fetch (the hub says the same for both).
    pub fn fetch_backup(transport: &dyn Transport, id: &[u8], door: &Door) -> Result<Option<Backup>> {
        if id.len() != LOCATOR_LEN {
            return Err(Error::Protocol("backup id"));
        }
        let path = api::backup_path(id);
        let wall = crate::wall_ms();
        let next = |last: u64| wall.max(last + 1);
        let time =
            next(SIGNED_AT.try_update(Ordering::Relaxed, Ordering::Relaxed, |last| Some(next(last))).unwrap_or(0));
        let headers = crate::signature_headers(&door.auth_key(), "GET", &path, time, &yo_wire::sha256(&[]));
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
    /// words' locator. `None` when there is none it opens.
    pub fn open_backup(transport: &dyn Transport, user_id: Option<&[u8]>, door: &Door) -> Result<Option<Opened>> {
        let id = match (user_id, door) {
            (Some(id), _) => id.to_vec(),
            (None, Door::Code(words)) => locator(words).to_vec(),
            (None, Door::Passkey(_)) => return Err(Error::Protocol("backup id")),
        };
        Ok(Self::fetch_backup(transport, &id, door)?.and_then(|record| open(record, door)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words() -> Vec<String> {
        "abandon ability able about above absent absorb abstract absurd abuse access accident"
            .split(' ')
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn either_door_opens_and_signs() {
        let (k, prf, words) = ([1; 32], [2; 32], words());
        let record = make(&k, b"{\"account\":1}", &words, PasskeyDoor::New(&prf)).unwrap();
        assert_eq!(record.sealed.as_slice().len(), 1024, "padded");
        assert_eq!(
            record.auth_keys(),
            (Some(Door::Passkey(&prf).auth_public()), Some(Door::Code(&words).auth_public()))
        );
        assert_ne!(Door::Passkey(&prf).auth_public(), Door::Code(&words).auth_public());
        for door in [Door::Passkey(&prf), Door::Code(&words)] {
            let opened = open(record.clone(), &door).unwrap();
            assert_eq!((opened.k, opened.plain.as_slice()), (k, &b"{\"account\":1}"[..]));
        }
        assert!(open(record.clone(), &Door::Passkey(&[3; 32])).is_none());
        let without = make(&k, b"{}", &words, PasskeyDoor::None).unwrap();
        assert!(without.by_passkey.as_slice().is_empty() && without.auth_keys().0.is_none());
        assert!(open(without, &Door::Passkey(&prf)).is_none());
    }

    #[test]
    fn a_kept_door_stays_under_new_words() {
        let (k, prf, words) = ([1; 32], [2; 32], words());
        let first = make(&k, b"{}", &words, PasskeyDoor::New(&prf)).unwrap();
        let renewed: Vec<String> = words.iter().rev().cloned().collect();
        let kept = make(&k, b"{}", &renewed, PasskeyDoor::Keep(&first)).unwrap();
        assert_eq!((&kept.by_passkey, kept.auth_keys().0), (&first.by_passkey, first.auth_keys().0));
        assert!(open(kept.clone(), &Door::Passkey(&prf)).is_some());
        assert!(open(kept, &Door::Code(&words)).is_none());
    }
}
