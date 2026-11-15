//! Identities (spec 3): the account key vouches for devices; the safety number is computed from user ids and account
//! keys.

use crate::error::{Error, Result};
use crate::signal::identity_of;
use ed25519_dalek::{Signer as _, SigningKey};
use libsignal_protocol::Fingerprint;
use yo_wire::cert::{self, DeviceCertificate};
use yo_wire::KEY_LEN;

pub type Key = [u8; KEY_LEN];

/// Fresh random bytes from the operating system.
pub fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::getrandom(&mut out).expect("the operating system's random source");
    out
}

/// The certificate of a device: the account key vouching for its device key.
pub fn certify(account: &SigningKey, device_key: &[u8]) -> DeviceCertificate {
    DeviceCertificate {
        account_key: account.verifying_key().to_bytes(),
        signature: account.sign(&cert::to_be_signed(device_key)).to_bytes(),
    }
}

/// Signal's safety number: its fingerprint version and hash iterations (spec 3.4).
const FINGERPRINT_VERSION: u32 = 2;
const FINGERPRINT_ITERATIONS: u32 = 5200;

/// libsignal's `Fingerprint` of this user (`me`) with another: each side its user id (16 bytes) and the identity key
/// of its account key (4.1), as Signal makes it from ACIs.
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
    fn safety_number_is_symmetric_and_binds_both() {
        let key = |seed: u8| SigningKey::from_bytes(&[seed; 32]).verifying_key().to_bytes();
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
