//! Signed requests (spec 6.2): a device proves itself with its auth key, per request, without a session.

use crate::{sha256, Error, KEY_LEN};
use ed25519_dalek::{Signature, VerifyingKey};
use tls_codec::{TlsSerialize, TlsSize, VLBytes};

pub const HEADER_DEVICE: &str = "yo-device";
pub const HEADER_TIME: &str = "yo-time";
pub const HEADER_SIGNATURE: &str = "yo-signature";
pub const LABEL: &[u8] = b"yo request v1";

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
    crate::encode(&SignedRequest {
        label: LABEL.to_vec().into(),
        method: method.as_bytes().to_vec().into(),
        path: path.as_bytes().to_vec().into(),
        time: time_ms,
        body_hash: sha256(body),
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
    let key = VerifyingKey::from_bytes(auth_key).map_err(|_| Error::Signature)?;
    let signature = Signature::from_slice(signature).map_err(|_| Error::Signature)?;
    key.verify_strict(&to_be_signed(method, path, time_ms, body), &signature)
        .map_err(|_| Error::Signature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn binds_every_part() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let public = key.verifying_key().to_bytes();
        let sig = key.sign(&to_be_signed("GET", "/v1/inbox?wait=5", 1000, b"")).to_bytes();
        assert!(verify(&public, "GET", "/v1/inbox?wait=5", 1000, b"", &sig).is_ok());
        assert!(verify(&public, "POST", "/v1/inbox?wait=5", 1000, b"", &sig).is_err());
        assert!(verify(&public, "GET", "/v1/inbox?wait=6", 1000, b"", &sig).is_err());
        assert!(verify(&public, "GET", "/v1/inbox?wait=5", 1001, b"", &sig).is_err());
        assert!(verify(&public, "GET", "/v1/inbox?wait=5", 1000, b"x", &sig).is_err());
    }
}
