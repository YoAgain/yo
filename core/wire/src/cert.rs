//! The device certificate (spec 3): the account key vouches for a device. It is the `identity` of the device's
//! MLS `BasicCredential`, so every member of every group carries it. It names no person: no phone number, no
//! username, no user id. Which account key belongs to which user is what the hub attests (spec 3).

use crate::{Error, Mailbox, KEY_LEN, SIGNATURE_LEN, VERSION};
use ed25519_dalek::{Signature, VerifyingKey};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize};

/// The label in front of every signed certificate body.
pub const LABEL: &[u8] = b"yo device v1";
/// Flag bit 0: the account key is backed up by a passkey, so the identity survives a device change.
pub const FLAG_PASSKEY: u8 = 1;
const KNOWN_FLAGS: u8 = FLAG_PASSKEY;

/// What the account key signs.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct CertificateBody {
    pub version: u8,
    pub account_key: [u8; KEY_LEN],
    /// The device's MLS signature key: the leaf's `signature_key`.
    pub device_key: [u8; KEY_LEN],
    pub mailbox: Mailbox,
    pub flags: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct DeviceCertificate {
    pub body: CertificateBody,
    pub signature: [u8; SIGNATURE_LEN],
}

impl CertificateBody {
    /// The bytes the account key signs: the label, then the body.
    pub fn to_be_signed(&self) -> Vec<u8> {
        let mut out = LABEL.to_vec();
        out.extend_from_slice(&crate::encode(self));
        out
    }

    pub fn has_passkey(&self) -> bool {
        self.flags & FLAG_PASSKEY != 0
    }
}

impl DeviceCertificate {
    /// Parses and checks a certificate: strict encoding, version, phone, flags, and the account key's signature.
    /// Whether `device_key` is the leaf's key is for the caller to check: it knows the leaf.
    pub fn verify(bytes: &[u8]) -> Result<Self, Error> {
        let cert: Self = crate::decode(bytes)?;
        let body = &cert.body;
        if body.version != VERSION {
            return Err(Error::Unsupported);
        }
        if body.flags & !KNOWN_FLAGS != 0 {
            return Err(Error::Format);
        }
        let key = VerifyingKey::from_bytes(&body.account_key).map_err(|_| Error::Signature)?;
        key.verify_strict(&body.to_be_signed(), &Signature::from_bytes(&cert.signature))
            .map_err(|_| Error::Signature)?;
        Ok(cert)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        crate::encode(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn key() -> SigningKey {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).unwrap();
        SigningKey::from_bytes(&seed)
    }

    fn certificate(account: &SigningKey) -> DeviceCertificate {
        let body = CertificateBody {
            version: VERSION,
            account_key: account.verifying_key().to_bytes(),
            device_key: key().verifying_key().to_bytes(),
            mailbox: [7; 16],
            flags: 0,
        };
        let signature = account.sign(&body.to_be_signed()).to_bytes();
        DeviceCertificate { body, signature }
    }

    #[test]
    fn verifies_and_round_trips() {
        let cert = certificate(&key());
        assert_eq!(DeviceCertificate::verify(&cert.to_bytes()).unwrap(), cert);
    }

    #[test]
    fn refuses_any_changed_field() {
        let cert = certificate(&key());
        let bytes = cert.to_bytes();
        for i in 0..bytes.len() {
            let mut changed = bytes.clone();
            changed[i] ^= 1;
            assert!(DeviceCertificate::verify(&changed).is_err(), "byte {i}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(DeviceCertificate::verify(&longer).is_err());
    }

    #[test]
    fn refuses_another_account_key() {
        let mut cert = certificate(&key());
        cert.body.account_key = key().verifying_key().to_bytes();
        assert_eq!(DeviceCertificate::verify(&cert.to_bytes()), Err(Error::Signature));
    }
}
