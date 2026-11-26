//! The backup record (spec 2.4): K wrapped by the passkey's PRF output and by the recovery key (libsignal's account
//! entropy pool), the app's data sealed under K, the recovery key's locator, and each door's auth key, which signs
//! the fetch. CryptoKit-compatible: HKDF-SHA256 without salt, ChaCha20-Poly1305 as nonce || ciphertext || tag.

use crate::error::{Error, Result};
use crate::identity::Key;
use crate::{Client, Transport};
use std::sync::atomic::{AtomicU64, Ordering};
use yoip::net::api::{self, status, Backup, LOCATOR_LEN};
use yoip::net::hubsig;
use yoip::protocol::backup;
pub use yoip::protocol::backup::{locator, open, parse_recovery_key, AccountEntropyPool, Door, Opened, PasskeyDoor};
use zeroize::Zeroizing;

/// A new recovery key: 64 characters, a–z and 0–9 (libsignal's `AccountEntropyPool::generate`).
pub fn new_recovery_key() -> AccountEntropyPool {
    AccountEntropyPool::generate(&mut rand::rng())
}

/// A new K (spec 2.4): for a first record, and whenever a door changes (a new recovery key, a passkey added, replaced
/// or removed), so that no old door opens what is sealed from then on.
pub fn new_k() -> Zeroizing<[u8; 32]> {
    Zeroizing::new(crate::identity::random())
}

/// A record for the hub: K under the recovery key and, as `passkey` says, the passkey; `plain` padded and sealed
/// under K; the locator; the doors' auth keys.
pub fn make(k: &[u8; 32], plain: &[u8], code: &AccountEntropyPool, passkey: PasskeyDoor) -> Result<Backup> {
    backup::make(k, plain, code, passkey, crate::identity::random).map_err(Error::Protocol)
}

/// The last signing time of a fetch: each signature once (the hub's replay cache), also within one millisecond.
static SIGNED_AT: AtomicU64 = AtomicU64::new(0);

impl Client {
    /// Fetches a backup record by user id or locator (spec 2.4) before there is an account, signed with the door's auth
    /// key; `None` when there is none this door may fetch. With the hub's keys pinned (`hub_keys`, spec 4.1) the
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
        let mut headers = crate::signature_headers(sign, "GET", &path, time, &yoip::sha256(&[]));
        let nonce = (!hub_keys.is_empty()).then(crate::identity::random::<{ hubsig::NONCE_LEN }>);
        if let Some(nonce) = &nonce {
            headers.push((hubsig::NONCE_HEADER.to_owned(), yoip::b64(nonce)));
        }
        let response = transport.request("GET", &path, &headers, &[]).map_err(Error::Network)?;
        match response.status {
            status::OK => {
                let body = crate::hub_signed(hub_keys, nonce.as_ref(), &path, &[], &response.body)?;
                let backup: Backup = yoip::decode(body)?;
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
