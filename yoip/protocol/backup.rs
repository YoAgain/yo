use crate::net::api::{Backup, LOCATOR_LEN};
use crate::protocol::aead;
use crate::protocol::storage::{hkdf, open_padded, seal_padded};
use crate::KEY_LEN;
pub use libsignal_account_keys::AccountEntropyPool;
use libsignal_account_keys::BackupKey;
use libsignal_protocol::PrivateKey;
use zeroize::Zeroizing;

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

/// K from its wrap under a door.
fn unwrap_k(wrapped: &[u8], door: &Door) -> Option<Zeroizing<[u8; 32]>> {
    let plain = Zeroizing::new(aead::open(&door.wrap_key(), &[], wrapped)?);
    plain.as_slice().try_into().ok().map(Zeroizing::new)
}

/// A record for the hub: K under the recovery key and, as `passkey` says, the passkey; `plain` padded and sealed
/// under K; the locator; the doors' auth keys.
pub fn make(
    k: &[u8; 32],
    plain: &[u8],
    code: &AccountEntropyPool,
    passkey: PasskeyDoor,
    mut nonce: impl FnMut() -> [u8; 12],
) -> Result<Backup, &'static str> {
    // A PRF output is 32 bytes (WebAuthn's PRF extension): anything shorter, an empty one above all, is a door anyone opens.
    if matches!(passkey, PasskeyDoor::New(prf) if prf.len() != 32) {
        return Err("passkey prf");
    }
    let (by_passkey, auth_passkey) = match passkey {
        PasskeyDoor::None => (Vec::new(), None),
        PasskeyDoor::New(prf) => {
            (aead::seal(&Door::Passkey(prf).wrap_key(), &[], k, nonce()), Some(Door::Passkey(prf).auth_public()))
        }
        PasskeyDoor::Keep(record) => {
            // Only under the recovery key that opens it to this K: a rotation keeps no K (`new_k`).
            if unwrap_k(record.by_code.as_slice(), &Door::Code(code)).as_deref() != Some(k) {
                return Err("passkey door kept under another recovery key or K");
            }
            (record.by_passkey.as_slice().to_vec(), record.auth_keys().0)
        }
    };
    let door = Door::Code(code);
    let backup = Backup::new(
        by_passkey,
        aead::seal(&door.wrap_key(), &[], k, nonce()),
        seal_padded(k, &[], plain, nonce).map_err(|_| "too large")?,
        locator(code).to_vec(),
        auth_passkey,
        door.auth_public(),
    );
    backup.check().map_err(|_| "format")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recovery_key_reads_back_however_it_is_written() {
        let code = AccountEntropyPool::generate(&mut rand::rng());
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
        let (k, prf, code) = ([1; 32], [2; 32], AccountEntropyPool::generate(&mut rand::rng()));
        let record = make(&k, b"{\"account\":1}", &code, PasskeyDoor::New(&prf), rand::random).unwrap();
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
            assert!(
                make(&k, b"{}", &code, PasskeyDoor::New(short), rand::random).is_err(),
                "no door from a PRF that is not one"
            );
        }
        assert!(open(record.clone(), &Door::Code(&AccountEntropyPool::generate(&mut rand::rng()))).is_none());
        let without = make(&k, b"{}", &code, PasskeyDoor::None, rand::random).unwrap();
        assert!(without.by_passkey.as_slice().is_empty() && without.auth_keys().0.is_none());
        assert!(open(without, &Door::Passkey(&prf)).is_none());
    }

    /// B1: a new recovery key (or a passkey door changed) comes with a new K. A passkey door can be kept only under
    /// the recovery key and K it was made with; the rotation's record, spliced with the old recovery key's wrap (a
    /// hub database dump and the old key), opens nothing.
    #[test]
    fn a_rotation_keeps_no_k_and_the_old_door_opens_nothing_since() {
        let (k, prf, old_code) =
            (Zeroizing::new(rand::random()), [2; 32], AccountEntropyPool::generate(&mut rand::rng()));
        let first = make(&k, b"{\"code\":\"old\"}", &old_code, PasskeyDoor::New(&prf), rand::random).unwrap();
        // Content changed, the doors the same: the passkey door is kept (same recovery key, same K).
        let again =
            make(&k, b"{\"code\":\"old\",\"name\":2}", &old_code, PasskeyDoor::Keep(&first), rand::random).unwrap();
        assert_eq!((&again.by_passkey, again.auth_keys().0), (&first.by_passkey, first.auth_keys().0));
        assert!(open(again, &Door::Passkey(&prf)).is_some());
        // The door kept under a new recovery key, or with another K: refused.
        let new_code = AccountEntropyPool::generate(&mut rand::rng());
        assert!(make(&k, b"{}", &new_code, PasskeyDoor::Keep(&first), rand::random).is_err(), "another recovery key");
        assert!(
            make(&Zeroizing::new(rand::random()), b"{}", &old_code, PasskeyDoor::Keep(&first), rand::random).is_err(),
            "another K"
        );
        // The rotation: a new K, the passkey asked again (or its door gone).
        let (k2, new) = (Zeroizing::new(rand::random()), b"{\"code\":\"NEW\"}");
        assert_ne!(k2, k);
        for passkey in [PasskeyDoor::New(&prf), PasskeyDoor::None] {
            let rotated = make(&k2, new, &new_code, passkey, rand::random).unwrap();
            assert!(open(rotated.clone(), &Door::Code(&old_code)).is_none(), "the old door is not in it");
            let mut spliced = rotated.clone();
            spliced.by_code = first.by_code.clone();
            assert!(open(spliced, &Door::Code(&old_code)).is_none(), "the old wrap's K opens nothing new");
            assert_eq!(open(rotated, &Door::Code(&new_code)).unwrap().plain, new);
        }
        // A passkey removed: the new record's sealed part does not open with the old passkey's K.
        let removed = make(&Zeroizing::new(rand::random()), b"{}", &old_code, PasskeyDoor::None, rand::random).unwrap();
        let mut spliced = removed.clone();
        spliced.by_passkey = first.by_passkey.clone();
        assert!(open(spliced, &Door::Passkey(&prf)).is_none());
    }
}
