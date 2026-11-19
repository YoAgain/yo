//! Identities (spec 3): the account key, a libsignal identity key pair (Curve25519), vouches for devices by XEdDSA.

use crate::error::{Error, Result};
use crate::signal::identity_of;
use libsignal_protocol::{Fingerprint, IdentityKey, IdentityKeyPair, PrivateKey};
use yo_wire::cert::{self, DeviceCertificate};
use yo_wire::KEY_LEN;

pub type Key = [u8; KEY_LEN];

/// Fresh random bytes from the operating system.
pub fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    rand::fill(&mut out);
    out
}

/// A new account's secret: a libsignal identity key pair's private key (32 bytes).
pub fn new_account_secret() -> Key {
    let pair = IdentityKeyPair::generate(&mut rand::rng());
    pair.private_key().serialize().try_into().expect("a Curve25519 private key is 32 bytes")
}

/// The account's identity key pair from its secret (spec 3, 4.1): the session identity and the sealed sender's.
pub fn account_pair(secret: &Key) -> Result<IdentityKeyPair> {
    let private = PrivateKey::deserialize(secret).map_err(|_| Error::Internal("account key"))?;
    let public = private.public_key().map_err(|_| Error::Internal("account key"))?;
    Ok(IdentityKeyPair::new(IdentityKey::new(public), private))
}

/// The account key: its identity key's 32 bytes (libsignal's `public_key_bytes`), as the hub and contacts know it.
pub fn account_key(pair: &IdentityKeyPair) -> Key {
    pair.public_key().public_key_bytes().try_into().expect("a Curve25519 public key is 32 bytes")
}

/// The account key's XEdDSA signature over `message` (libsignal's `calculate_signature`).
pub fn sign(pair: &IdentityKeyPair, message: &[u8]) -> [u8; yo_wire::SIGNATURE_LEN] {
    let signature = pair.private_key().calculate_signature(message, &mut rand::rng()).expect("a Curve25519 key signs");
    (*signature).try_into().expect("an XEdDSA signature is 64 bytes")
}

/// The certificate of a device: the account key vouching for its device key.
pub fn certify(account: &IdentityKeyPair, device_key: &[u8]) -> DeviceCertificate {
    DeviceCertificate { account_key: account_key(account), signature: sign(account, &cert::to_be_signed(device_key)) }
}

/// An XEdDSA signature by the Curve25519 private key `secret` (a device's auth key, spec 6.2).
pub fn sign_with(secret: &Key, message: &[u8]) -> [u8; yo_wire::SIGNATURE_LEN] {
    let private = PrivateKey::deserialize(secret).expect("32 bytes are a Curve25519 private key");
    let signature = private.calculate_signature(message, &mut rand::rng()).expect("a Curve25519 key signs");
    (*signature).try_into().expect("an XEdDSA signature is 64 bytes")
}

/// The public key of the Curve25519 private key `secret`.
pub fn public_of(secret: &Key) -> Key {
    let private = PrivateKey::deserialize(secret).expect("32 bytes are a Curve25519 private key");
    private.public_key().expect("a public key").public_key_bytes().try_into().expect("32 bytes")
}

pub use yo_wire::legacy::legacy_account_secret;

/// Signal's safety number: its fingerprint version and hash iterations (spec 3.4).
const FINGERPRINT_VERSION: u32 = 2;
const FINGERPRINT_ITERATIONS: u32 = 5200;

/// libsignal's `Fingerprint` of two users' ids and account keys (4.1), as Signal makes it from ACIs.
fn fingerprint(me: (&[u8], &Key), them: (&[u8], &Key)) -> Result<Fingerprint> {
    let (mine, theirs) = (identity_of(me.1)?, identity_of(them.1)?);
    Fingerprint::new(FINGERPRINT_VERSION, FINGERPRINT_ITERATIONS, me.0, &mine, them.0, &theirs)
        .map_err(|_| Error::Internal("fingerprint"))
}

/// The safety number of two users (spec 3.4): 60 digits in twelve fives, the same on both sides.
pub fn safety_number(me: (&[u8], &Key), them: (&[u8], &Key)) -> Result<String> {
    let digits = fingerprint(me, them)?.display_string().map_err(|_| Error::Internal("fingerprint"))?;
    let fives: Vec<&str> = digits.as_bytes().chunks(5).map(|five| std::str::from_utf8(five).unwrap_or("")).collect();
    Ok(fives.join(" "))
}

/// The safety number's scannable form (Signal's QR code payload) as this user shows it.
pub fn safety_code(me: (&[u8], &Key), them: (&[u8], &Key)) -> Result<Vec<u8>> {
    fingerprint(me, them)?.scannable.serialize().map_err(|_| Error::Internal("fingerprint"))
}

/// Whether a code scanned off the other's screen matches (it names both sides the other way round); one that does
/// not parse, or of another version, does not.
pub fn safety_code_matches(me: (&[u8], &Key), them: (&[u8], &Key), scanned: &[u8]) -> Result<bool> {
    Ok(fingerprint(me, them)?.scannable.compare(scanned).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_certificate_verifies_and_a_legacy_secret_keeps_its_identity() {
        let pair = account_pair(&new_account_secret()).unwrap();
        let device = [5u8; 32];
        let certificate = certify(&pair, &device);
        assert!(DeviceCertificate::verify(&certificate.to_bytes(), &device).is_ok());
        // An Ed25519 seed (RFC 8032 test vector 1) maps to the account key the hub converted.
        let seed: [u8; 32] = hex32("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60");
        let ed = hex32("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let migrated = account_pair(&legacy_account_secret(&seed)).unwrap();
        let montgomery = curve25519_dalek::edwards::CompressedEdwardsY(ed).decompress().unwrap().to_montgomery();
        assert_eq!(account_key(&migrated), montgomery.to_bytes(), "the account key as the hub converted it");
    }

    fn hex32(text: &str) -> [u8; 32] {
        crate::state::unhex(text).unwrap().try_into().unwrap()
    }

    #[test]
    fn safety_number_is_symmetric_and_binds_both() {
        let key = |seed: u8| account_key(&account_pair(&[seed; 32]).unwrap());
        let (ka, kb, kc) = (key(1), key(2), key(3));
        let (ua, ub) = ([1u8; 16], [2u8; 16]);
        let ab = safety_number((&ua, &ka), (&ub, &kb)).unwrap();
        assert_eq!(ab, safety_number((&ub, &kb), (&ua, &ka)).unwrap());
        assert_eq!(ab.replace(' ', "").len(), 60);
        assert!(ab.replace(' ', "").bytes().all(|b| b.is_ascii_digit()));
        assert_ne!(ab, safety_number((&ua, &ka), (&ub, &kc)).unwrap());
        assert_ne!(ab, safety_number((&[3u8; 16], &ka), (&ub, &kb)).unwrap());
        // The scannable form: B's code matches on A's side, not against a third key, and garbage does not.
        let code = safety_code((&ub, &kb), (&ua, &ka)).unwrap();
        assert!(safety_code_matches((&ua, &ka), (&ub, &kb), &code).unwrap());
        assert!(!safety_code_matches((&ua, &ka), (&ub, &kc), &code).unwrap());
        assert!(!safety_code_matches((&ua, &ka), (&ub, &kb), b"no code").unwrap());
    }
}
