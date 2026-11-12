//! The device certificate (spec 3.1): the account key vouches for a device, and that is all it says.
//!
//! It is the `identity` of the device's MLS `BasicCredential`. The device key is not repeated in it: it is the leaf's
//! signature key, and the signature covers it. It names no person; who an account key is, the hub attests.

use crate::{Error, KEY_LEN, SIGNATURE_LEN};
use ed25519_dalek::{Signature, VerifyingKey};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize};

/// The label in front of the device key the account key signs. A later version has another label.
pub const LABEL: &[u8] = b"yo device v1";
/// The encoded size.
pub const LEN: usize = KEY_LEN + SIGNATURE_LEN;

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct DeviceCertificate {
    pub account_key: [u8; KEY_LEN],
    /// Ed25519 by `account_key` over `LABEL || device_key`.
    pub signature: [u8; SIGNATURE_LEN],
}

/// What the account key signs for a device.
pub fn to_be_signed(device_key: &[u8]) -> Vec<u8> {
    let mut out = LABEL.to_vec();
    out.extend_from_slice(device_key);
    out
}

impl DeviceCertificate {
    /// Parses a certificate and checks that its account key vouches for `device_key`: the signature key of the leaf
    /// that carries it.
    pub fn verify(bytes: &[u8], device_key: &[u8]) -> Result<Self, Error> {
        let certificate = Self::parse(bytes)?;
        if device_key.len() != KEY_LEN {
            return Err(Error::Signature);
        }
        let key = VerifyingKey::from_bytes(&certificate.account_key).map_err(|_| Error::Signature)?;
        key.verify_strict(&to_be_signed(device_key), &Signature::from_bytes(&certificate.signature))
            .map_err(|_| Error::Signature)?;
        Ok(certificate)
    }

    /// Parses without checking the signature: for a member whose certificate was checked when it joined.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        crate::decode(bytes)
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

    fn certificate(account: &SigningKey, device_key: &[u8]) -> DeviceCertificate {
        DeviceCertificate {
            account_key: account.verifying_key().to_bytes(),
            signature: account.sign(&to_be_signed(device_key)).to_bytes(),
        }
    }

    #[test]
    fn verifies_for_its_device_only() {
        let (account, device) = (key(), key().verifying_key().to_bytes());
        let cert = certificate(&account, &device);
        assert_eq!(cert.to_bytes().len(), LEN);
        assert_eq!(DeviceCertificate::verify(&cert.to_bytes(), &device).unwrap(), cert);
        let other = key().verifying_key().to_bytes();
        assert_eq!(DeviceCertificate::verify(&cert.to_bytes(), &other), Err(Error::Signature));
    }

    #[test]
    fn refuses_any_changed_byte() {
        let device = key().verifying_key().to_bytes();
        let bytes = certificate(&key(), &device).to_bytes();
        for i in 0..bytes.len() {
            let mut changed = bytes.clone();
            changed[i] ^= 1;
            assert!(DeviceCertificate::verify(&changed, &device).is_err(), "byte {i}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(DeviceCertificate::verify(&longer, &device).is_err());
    }
}
