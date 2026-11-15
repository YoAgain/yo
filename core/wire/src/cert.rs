//! The device certificate (spec 3.1): the account key vouches for a device, and that is all it says.
//!
//! The device key is not repeated in it: it is registered beside it, and the signature covers it. It names no person;
//! who an account key is, the hub attests.

use crate::{Error, KEY_LEN, SIGNATURE_LEN};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize};

/// The label in front of the device key the account key signs. A later version has another label.
pub const LABEL: &[u8] = b"yo device v1";
/// The encoded size.
pub const LEN: usize = KEY_LEN + SIGNATURE_LEN;

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct DeviceCertificate {
    /// The account's Curve25519 identity key (its 32 bytes, libsignal's `public_key_bytes`).
    pub account_key: [u8; KEY_LEN],
    /// XEdDSA by `account_key` over `LABEL || device_key`.
    pub signature: [u8; SIGNATURE_LEN],
}

/// What the account key signs for a device.
pub fn to_be_signed(device_key: &[u8]) -> Vec<u8> {
    let mut out = LABEL.to_vec();
    out.extend_from_slice(device_key);
    out
}

impl DeviceCertificate {
    /// Parses a certificate and checks that its account key vouches for `device_key`.
    pub fn verify(bytes: &[u8], device_key: &[u8]) -> Result<Self, Error> {
        let certificate = Self::parse(bytes)?;
        if device_key.len() != KEY_LEN {
            return Err(Error::Signature);
        }
        // One encoding per key: a key with its top bit set would verify too (XEdDSA ignores it), as another account.
        if !crate::curve_key_usable(&certificate.account_key)
            || !crate::xeddsa_verify(&certificate.account_key, &to_be_signed(device_key), &certificate.signature)
        {
            return Err(Error::Signature);
        }
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
    use libsignal_core::curve::KeyPair;

    fn key() -> KeyPair {
        KeyPair::generate(&mut rand::rng())
    }

    fn public(pair: &KeyPair) -> [u8; KEY_LEN] {
        pair.public_key.public_key_bytes().try_into().unwrap()
    }

    fn certificate(account: &KeyPair, device_key: &[u8]) -> DeviceCertificate {
        let signature = account.private_key.calculate_signature(&to_be_signed(device_key), &mut rand::rng()).unwrap();
        DeviceCertificate { account_key: public(account), signature: (*signature).try_into().unwrap() }
    }

    #[test]
    fn verifies_for_its_device_only() {
        let (account, device) = (key(), public(&key()));
        let cert = certificate(&account, &device);
        assert_eq!(cert.to_bytes().len(), LEN);
        assert_eq!(DeviceCertificate::verify(&cert.to_bytes(), &device).unwrap(), cert);
        let other = public(&key());
        assert_eq!(DeviceCertificate::verify(&cert.to_bytes(), &other), Err(Error::Signature));
    }

    #[test]
    fn refuses_any_changed_byte() {
        let device = public(&key());
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
