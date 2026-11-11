//! Identities (spec 3): the account key certifies devices; every member of a group is checked against its
//! certificate; the safety number is computed from both account keys.

use crate::error::{Error, Result};
use ed25519_dalek::{Signer as _, SigningKey};
use openmls::prelude::{BasicCredential, Credential, CredentialType};
use sha2::{Digest, Sha512};
use yo_wire::cert::{CertificateBody, DeviceCertificate};
use yo_wire::{Mailbox, VERSION};

/// Fresh random bytes from the operating system.
pub fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::getrandom(&mut out).expect("the operating system's random source");
    out
}

/// Makes the certificate of a device, signed by the account key.
pub fn certify(account: &SigningKey, device_key: &[u8], mailbox: &Mailbox, flags: u8) -> Result<DeviceCertificate> {
    let body = CertificateBody {
        version: VERSION,
        account_key: account.verifying_key().to_bytes(),
        device_key: device_key.try_into().map_err(|_| Error::Internal("device key length"))?,
        mailbox: *mailbox,
        flags,
    };
    let signature = account.sign(&body.to_be_signed()).to_bytes();
    Ok(DeviceCertificate { body, signature })
}

/// The certificate of a leaf, checked: basic credential, valid certificate, and its device key is the leaf's
/// signature key.
pub fn member(credential: &Credential, signature_key: &[u8]) -> Result<CertificateBody> {
    if credential.credential_type() != CredentialType::Basic {
        return Err(Error::Protocol("credential type"));
    }
    let basic = BasicCredential::try_from(credential.clone()).map_err(|_| Error::Protocol("credential"))?;
    let certificate = DeviceCertificate::verify(basic.identity()).map_err(|_| Error::Protocol("certificate"))?;
    if certificate.body.device_key.as_slice() != signature_key {
        return Err(Error::Protocol("certificate names another key"));
    }
    Ok(certificate.body)
}

/// The account key and mailbox a certificate names, unchecked: for a sender whose leaf OpenMLS has verified.
pub fn claimed(credential: &Credential) -> Option<CertificateBody> {
    let basic = BasicCredential::try_from(credential.clone()).ok()?;
    yo_wire::decode::<DeviceCertificate>(basic.identity()).ok().map(|c| c.body)
}

/// The safety number of two users (spec 3): 60 digits, the same on both sides. Each side is its user id and
/// account key.
pub fn safety_number(a: (&[u8], &[u8; 32]), b: (&[u8], &[u8; 32])) -> String {
    let part = |(user, key): (&[u8], &[u8; 32])| {
        let mut out = vec![user.len() as u8];
        out.extend_from_slice(user);
        out.extend_from_slice(key);
        out
    };
    let (a, b) = (part(a), part(b));
    let (low, high) = if a <= b { (a, b) } else { (b, a) };
    let mut hash = Sha512::new();
    hash.update(b"yo safety number v1");
    hash.update(&low);
    hash.update(&high);
    let digest = hash.finalize();
    (0..12)
        .map(|i| {
            let chunk = &digest[5 * i..5 * i + 5];
            let value = chunk.iter().fold(0u64, |acc, b| (acc << 8) | *b as u64);
            format!("{:05}", value % 100_000)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_number_is_symmetric_and_binds_both() {
        let (ka, kb, kc) = ([1u8; 32], [2u8; 32], [3u8; 32]);
        let (ua, ub) = ([1u8; 16], [2u8; 16]);
        let ab = safety_number((&ua, &ka), (&ub, &kb));
        assert_eq!(ab, safety_number((&ub, &kb), (&ua, &ka)));
        assert_eq!(ab.replace(' ', "").len(), 60);
        assert_ne!(ab, safety_number((&ua, &ka), (&ub, &kc)));
        assert_ne!(ab, safety_number((&[3u8; 16], &ka), (&ub, &kb)));
    }
}
