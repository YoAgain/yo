//! The backup record (spec 7): K wrapped by the passkey's PRF output and by the recovery key (libsignal's account
//! entropy pool), the app's data sealed under K, the recovery key's locator, and each door's auth key, which signs
//! the fetch. CryptoKit-compatible: HKDF-SHA256 without salt, ChaCha20-Poly1305 as nonce || ciphertext || tag.

use crate::aead;
use crate::error::{Error, Result};
use crate::identity::Key;
use crate::storage::{hkdf, open_padded, seal_padded};
use crate::{Client, Transport};
pub use libsignal_account_keys::AccountEntropyPool;
use libsignal_account_keys::BackupKey;
use libsignal_protocol::PrivateKey;
use std::sync::atomic::{AtomicU64, Ordering};
use yo_wire::api::{self, status, Backup, LOCATOR_LEN};
use yo_wire::{hubsig, KEY_LEN};
use zeroize::Zeroizing;

/// A new recovery key: 64 characters, a–z and 0–9 (libsignal's `AccountEntropyPool::generate`).
pub fn new_recovery_key() -> AccountEntropyPool {
    AccountEntropyPool::generate(&mut rand::rng())
}

/// The recovery key in what was pasted, typed or scanned (case, spaces, dashes and line breaks ignored).
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
    /// HKDF of the door's secret: the PRF output, or the recovery key's backup key (both wiped after use).
    fn derive<const N: usize>(&self, info: &str) -> Zeroizing<[u8; N]> {
        let mut out = Zeroizing::new([0; N]);
        match self {
            Door::Passkey(prf) => hkdf(prf, info.as_bytes(), &mut *out),
            Door::Code(pool) => {
                let backup_key = Zeroizing::new(BackupKey::derive_from_account_entropy_pool(pool).0);
                hkdf(&*backup_key, info.as_bytes(), &mut *out)
            }
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
    fn wrap_key(&self) -> Zeroizing<[u8; 32]> {
        self.derive(&format!("yo backup v3 {}", self.name()))
    }

    /// The key that signs the record's fetch (XEdDSA), from HKDF(secret, "yo backup v3 auth ...").
    pub fn auth_key(&self) -> PrivateKey {
        let bytes: Zeroizing<[u8; 32]> = self.derive(&format!("yo backup v3 auth {}", self.name()));
        PrivateKey::deserialize(&*bytes).expect("32 bytes are a Curve25519 private key")
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
    *Door::Code(code).derive("yo backup v3 locator")
}

/// The passkey's door in a record being made.
pub enum PasskeyDoor<'a> {
    /// None: the recovery key alone opens it.
    None,
    /// A passkey's PRF output: K wrapped under it, and its auth key.
    New(&'a [u8]),
    /// As in this record, under the same recovery key and K: its wrap and its auth key, or none. A new recovery key
    /// or another passkey door means a new K (`new_k`): whoever kept an old door's secret and wrap must not open the
    /// records made since.
    Keep(&'a Backup),
}

/// A new K (spec 7): for a first record, and whenever a door changes (a new recovery key, a passkey added, replaced
/// or removed), so that no old door opens what is sealed from then on.
pub fn new_k() -> Zeroizing<[u8; 32]> {
    Zeroizing::new(crate::identity::random())
}

/// K from its wrap under a door.
fn unwrap_k(wrapped: &[u8], door: &Door) -> Option<Zeroizing<[u8; 32]>> {
    let plain = Zeroizing::new(aead::open(&door.wrap_key(), &[], wrapped)?);
    plain.as_slice().try_into().ok().map(Zeroizing::new)
}

/// A record for the hub: K under the recovery key and, as `passkey` says, the passkey; `plain` padded and sealed
/// under K; the locator; the doors' auth keys.
pub fn make(k: &[u8; 32], plain: &[u8], code: &AccountEntropyPool, passkey: PasskeyDoor) -> Result<Backup> {
    // A PRF output is 32 bytes (WebAuthn's PRF extension): anything shorter, an empty one above all, is a door anyone opens.
    if matches!(passkey, PasskeyDoor::New(prf) if prf.len() != 32) {
        return Err(Error::Protocol("passkey prf"));
    }
    let (by_passkey, auth_passkey) = match passkey {
        PasskeyDoor::None => (Vec::new(), None),
        PasskeyDoor::New(prf) => {
            (aead::seal(&Door::Passkey(prf).wrap_key(), &[], k), Some(Door::Passkey(prf).auth_public()))
        }
        PasskeyDoor::Keep(record) => {
            // Only under the recovery key that opens it to this K: a rotation keeps no K (`new_k`).
            if unwrap_k(record.by_code.as_slice(), &Door::Code(code)).as_deref() != Some(k) {
                return Err(Error::Protocol("passkey door kept under another recovery key or K"));
            }
            (record.by_passkey.as_slice().to_vec(), record.auth_keys().0)
        }
    };
    let door = Door::Code(code);
    let backup = Backup::new(
        by_passkey,
        aead::seal(&door.wrap_key(), &[], k),
        seal_padded(k, &[], plain)?,
        locator(code).to_vec(),
        auth_passkey,
        door.auth_public(),
    );
    backup.check()?;
    Ok(backup)
}

/// A record opened by a door. Its `Debug` names no secret: neither K nor what K sealed (the account key in it).
#[derive(Clone, PartialEq, Eq)]
pub struct Opened {
    pub k: Zeroizing<[u8; 32]>,
    /// What K sealed, without its padding.
    pub plain: Vec<u8>,
    /// The record as the hub has it: its passkey door to keep (`PasskeyDoor::Keep`).
    pub record: Backup,
}

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opened").field("plain", &format_args!("{} bytes", self.plain.len())).finish_non_exhaustive()
    }
}

/// Opens a record with a door; `None` when the door does not open it.
pub fn open(record: Backup, door: &Door) -> Option<Opened> {
    let k = unwrap_k(door.wrapped(&record), door)?;
    let plain = open_padded(&k, &[], record.sealed.as_slice())?;
    Some(Opened { k, plain, record })
}

/// The last signing time of a fetch: each signature once (the hub's replay cache), also within one millisecond.
static SIGNED_AT: AtomicU64 = AtomicU64::new(0);

impl Client {
    /// Fetches a backup record by user id or locator (spec 7) before there is an account, signed with the door's auth
    /// key; `None` when there is none this door may fetch. With the hub's keys pinned (`hub_keys`, spec 6.10) the
    /// answer must be the hub's, signed for this request's nonce: no party between (a TLS proxy) can hand back an
    /// older record it kept.
    pub fn fetch_backup(transport: &dyn Transport, id: &[u8], door: &Door, hub_keys: &[Key]) -> Result<Option<Backup>> {
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
        let mut headers = crate::signature_headers(sign, "GET", &path, time, &yo_wire::sha256(&[]));
        let nonce = (!hub_keys.is_empty()).then(crate::identity::random::<{ hubsig::NONCE_LEN }>);
        if let Some(nonce) = &nonce {
            headers.push((hubsig::NONCE_HEADER.to_owned(), yo_wire::b64(nonce)));
        }
        let response = transport.request("GET", &path, &headers, &[]).map_err(Error::Network)?;
        match response.status {
            status::OK => {
                let body = crate::hub_signed(hub_keys, nonce.as_ref(), &path, &[], &response.body)?;
                let backup: Backup = yo_wire::decode(body)?;
                backup.check()?;
                Ok(Some(backup))
            }
            status::NOT_FOUND => Ok(None),
            _ => Err(crate::refused(&response)),
        }
    }

    /// Fetches and opens the record a door opens, by user id when given, else by the recovery key's locator; the
    /// answer checked against `hub_keys` when pinned (`fetch_backup`).
    pub fn open_backup(
        transport: &dyn Transport,
        user_id: Option<&[u8]>,
        door: &Door,
        hub_keys: &[Key],
    ) -> Result<Option<Opened>> {
        let id = match (user_id, door) {
            (Some(id), _) => id.to_vec(),
            (None, Door::Code(code)) => locator(code).to_vec(),
            (None, Door::Passkey(_)) => return Err(Error::Protocol("backup id")),
        };
        Ok(Self::fetch_backup(transport, &id, door, hub_keys)?.and_then(|record| open(record, door)))
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
            assert_eq!((*opened.k, opened.plain.as_slice()), (k, &b"{\"account\":1}"[..]));
        }
        assert!(open(record.clone(), &Door::Passkey(&[3; 32])).is_none());
        for short in [&[][..], &[2; 16][..]] {
            assert!(make(&k, b"{}", &code, PasskeyDoor::New(short)).is_err(), "no door from a PRF that is not one");
        }
        assert!(open(record.clone(), &Door::Code(&new_recovery_key())).is_none());
        let without = make(&k, b"{}", &code, PasskeyDoor::None).unwrap();
        assert!(without.by_passkey.as_slice().is_empty() && without.auth_keys().0.is_none());
        assert!(open(without, &Door::Passkey(&prf)).is_none());
    }

    /// B1: a new recovery key (or a passkey door changed) comes with a new K. A passkey door can be kept only under
    /// the recovery key and K it was made with; the rotation's record, spliced with the old recovery key's wrap (a
    /// hub database dump and the old key), opens nothing.
    #[test]
    fn a_rotation_keeps_no_k_and_the_old_door_opens_nothing_since() {
        let (k, prf, old_code) = (new_k(), [2; 32], new_recovery_key());
        let first = make(&k, b"{\"code\":\"old\"}", &old_code, PasskeyDoor::New(&prf)).unwrap();
        // Content changed, the doors the same: the passkey door is kept (same recovery key, same K).
        let again = make(&k, b"{\"code\":\"old\",\"name\":2}", &old_code, PasskeyDoor::Keep(&first)).unwrap();
        assert_eq!((&again.by_passkey, again.auth_keys().0), (&first.by_passkey, first.auth_keys().0));
        assert!(open(again, &Door::Passkey(&prf)).is_some());
        // The door kept under a new recovery key, or with another K: refused.
        let new_code = new_recovery_key();
        assert!(make(&k, b"{}", &new_code, PasskeyDoor::Keep(&first)).is_err(), "another recovery key");
        assert!(make(&new_k(), b"{}", &old_code, PasskeyDoor::Keep(&first)).is_err(), "another K");
        // The rotation: a new K, the passkey asked again (or its door gone).
        let (k2, new) = (new_k(), b"{\"code\":\"NEW\"}");
        assert_ne!(k2, k);
        for passkey in [PasskeyDoor::New(&prf), PasskeyDoor::None] {
            let rotated = make(&k2, new, &new_code, passkey).unwrap();
            assert!(open(rotated.clone(), &Door::Code(&old_code)).is_none(), "the old door is not in it");
            let mut spliced = rotated.clone();
            spliced.by_code = first.by_code.clone();
            assert!(open(spliced, &Door::Code(&old_code)).is_none(), "the old wrap's K opens nothing new");
            assert_eq!(open(rotated, &Door::Code(&new_code)).unwrap().plain, new);
        }
        // A passkey removed: the new record's sealed part does not open with the old passkey's K.
        let removed = make(&new_k(), b"{}", &old_code, PasskeyDoor::None).unwrap();
        let mut spliced = removed.clone();
        spliced.by_passkey = first.by_passkey.clone();
        assert!(open(spliced, &Door::Passkey(&prf)).is_none());
    }

    /// I3: what is printed of a record opened or a keychain item names no secret.
    #[test]
    fn debug_names_no_secret() {
        let (k, code) = (Zeroizing::new([0xAB; 32]), new_recovery_key());
        let opened =
            open(make(&k, b"{\"accountKey\":\"SECRET\"}", &code, PasskeyDoor::None).unwrap(), &Door::Code(&code));
        let shown = format!("{:?}", opened.unwrap());
        assert!(
            !shown.contains("SECRET") && !shown.contains("171") && !shown.to_lowercase().contains("ab, ab"),
            "{shown}"
        );
        let item = crate::keychain::Item {
            username: "leo".into(),
            user_id: vec![1; 16],
            account_key: vec![0xCD; 32],
            by_passkey: false,
            by_code: Some(true),
            code_auth: None,
            passkey_auth: None,
            recovery_key: Some("secretrecoverykey".into()),
            signed_out: None,
            version: Some(3),
            legacy_seed: Some(vec![0xEF; 32]),
        };
        let shown = format!("{item:?}");
        assert!(!shown.contains("secretrecoverykey") && !shown.contains("205") && !shown.contains("239"), "{shown}");
    }
}
