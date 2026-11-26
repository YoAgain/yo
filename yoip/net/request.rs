//! Signed requests (spec 4.1): a device proves itself with its auth key, per request, without a session; a backup's
//! fetch (spec 2.4) is signed the same way by a door's auth key. Both Curve25519, XEdDSA (libsignal).

use crate::{sha256, Error, KEY_LEN};
use tls_codec::{TlsSerialize, TlsSize, VLBytes};

pub const HEADER_DEVICE: &str = "yo-device";
pub const HEADER_TIME: &str = "yo-time";
pub const HEADER_SIGNATURE: &str = "yo-signature";
pub const LABEL: &[u8] = b"yo request v1";
/// A `401`'s body when the request's time is outside the hub's window: `clock` and the hub's time, ms (spec 4.1).
const CLOCK: &str = "clock ";

/// The body of a `401` for a time outside the window, telling the hub's time.
pub fn clock_body(hub_ms: u64) -> Vec<u8> {
    format!("{CLOCK}{hub_ms}").into_bytes()
}

/// The hub's time a `401`'s body tells (`clock_body`): the request's time was outside the window.
pub fn clock_of(body: &[u8]) -> Option<u64> {
    std::str::from_utf8(body).ok()?.strip_prefix(CLOCK)?.parse().ok()
}

#[derive(TlsSerialize, TlsSize)]
struct SignedRequest {
    label: VLBytes,
    method: VLBytes,
    path: VLBytes,
    time: u64,
    body_hash: [u8; 32],
}

/// The bytes a device signs for a request. `path` includes the query.
pub fn to_be_signed(method: &str, path: &str, time_ms: u64, body: &[u8]) -> Vec<u8> {
    to_be_signed_hash(method, path, time_ms, &sha256(body))
}

/// `to_be_signed` by the body's hash: for a body too large to hold (a blob, spec 4.3).
pub fn to_be_signed_hash(method: &str, path: &str, time_ms: u64, body_hash: &[u8; 32]) -> Vec<u8> {
    crate::encode(&SignedRequest {
        label: LABEL.to_vec().into(),
        method: method.as_bytes().to_vec().into(),
        path: path.as_bytes().to_vec().into(),
        time: time_ms,
        body_hash: *body_hash,
    })
}

/// Checks a request's signature. The time window and the replay cache are the hub's.
pub fn verify(
    auth_key: &[u8; KEY_LEN],
    method: &str,
    path: &str,
    time_ms: u64,
    body: &[u8],
    signature: &[u8],
) -> Result<(), Error> {
    verify_hash(auth_key, method, path, time_ms, &sha256(body), signature)
}

/// `verify` by the body's hash: the body itself is checked against it as it comes.
pub fn verify_hash(
    auth_key: &[u8; KEY_LEN],
    method: &str,
    path: &str,
    time_ms: u64,
    body_hash: &[u8; 32],
    signature: &[u8],
) -> Result<(), Error> {
    crate::xeddsa_verify(auth_key, &to_be_signed_hash(method, path, time_ms, body_hash), signature)
        .then_some(())
        .ok_or(Error::Signature)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_clock_refusal_tells_the_hub_time() {
        assert_eq!(super::clock_of(&super::clock_body(1_700_000_000_123)), Some(1_700_000_000_123));
        assert_eq!(super::clock_of(b""), None);
        assert_eq!(super::clock_of(b"clock soon"), None);
    }

    use super::*;
    use libsignal_core::curve::KeyPair;

    #[test]
    fn binds_every_part() {
        let key = KeyPair::generate(&mut rand::rng());
        let public: [u8; KEY_LEN] = key.public_key.public_key_bytes().try_into().unwrap();
        let message = to_be_signed("GET", "/v1/inbox?wait=5", 1000, b"");
        let sig = key.private_key.calculate_signature(&message, &mut rand::rng()).unwrap();
        assert!(verify(&public, "GET", "/v1/inbox?wait=5", 1000, b"", &sig).is_ok());
        assert!(verify(&public, "POST", "/v1/inbox?wait=5", 1000, b"", &sig).is_err());
        assert!(verify(&public, "GET", "/v1/inbox?wait=6", 1000, b"", &sig).is_err());
        assert!(verify(&public, "GET", "/v1/inbox?wait=5", 1001, b"", &sig).is_err());
        assert!(verify(&public, "GET", "/v1/inbox?wait=5", 1000, b"x", &sig).is_err());
    }

    /// One signature, one form: its scalar plus the group order (the same signature to libsignal) is refused, or a
    /// replay cache keyed by the signature would take it as new.
    #[test]
    fn a_scalar_not_reduced_is_refused() {
        let key = KeyPair::generate(&mut rand::rng());
        let public: [u8; KEY_LEN] = key.public_key.public_key_bytes().try_into().unwrap();
        let (mut tried, mut refused) = (0, 0);
        for time in 0..64 {
            let message = to_be_signed("GET", "/v1/inbox", time, b"");
            let sig = key.private_key.calculate_signature(&message, &mut rand::rng()).unwrap();
            assert!(verify(&public, "GET", "/v1/inbox", time, b"", &sig).is_ok());
            let mut s: [u8; 32] = sig[32..].try_into().unwrap();
            let sign = s[31] & 0x80;
            s[31] &= 0x7f;
            let mut carry = 0u16;
            for (byte, add) in s.iter_mut().zip(crate::GROUP_ORDER) {
                let sum = *byte as u16 + add as u16 + carry;
                *byte = sum as u8;
                carry = sum >> 8;
            }
            if s[31] & 0xe0 != 0 {
                continue;
            }
            s[31] |= sign;
            tried += 1;
            let other = [&sig[..32], &s[..]].concat();
            refused += verify(&public, "GET", "/v1/inbox", time, b"", &other).is_err() as u32;
        }
        assert!(tried > 0);
        assert_eq!(refused, tried);
    }
}
