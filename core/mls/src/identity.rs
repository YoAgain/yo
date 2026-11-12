//! Identities (spec 3): the account key vouches for devices; every member of a group is checked against its
//! certificate; the safety number is computed from user ids and account keys.

use crate::error::{Error, Result};
use ed25519_dalek::{Signer as _, SigningKey};
use openmls::prelude::{BasicCredential, Credential, CredentialType};
use sha2::{Digest, Sha512};
use yo_wire::cert::{self, DeviceCertificate};
use yo_wire::KEY_LEN;

pub type Key = [u8; KEY_LEN];

/// Fresh random bytes from the operating system.
pub fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::getrandom(&mut out).expect("the operating system's random source");
    out
}

/// The certificate of a device: the account key vouching for its MLS signature key.
pub fn certify(account: &SigningKey, device_key: &[u8]) -> DeviceCertificate {
    DeviceCertificate {
        account_key: account.verifying_key().to_bytes(),
        signature: account.sign(&cert::to_be_signed(device_key)).to_bytes(),
    }
}

fn identity(credential: &Credential) -> Result<Vec<u8>> {
    if credential.credential_type() != CredentialType::Basic {
        return Err(Error::Protocol("credential type"));
    }
    let basic = BasicCredential::try_from(credential.clone()).map_err(|_| Error::Protocol("credential"))?;
    Ok(basic.identity().to_vec())
}

/// The account key vouching for a leaf, checked: a basic credential whose certificate verifies against the leaf's
/// signature key.
pub fn member(credential: &Credential, signature_key: &[u8]) -> Result<Key> {
    let certificate =
        DeviceCertificate::verify(&identity(credential)?, signature_key).map_err(|_| Error::Protocol("certificate"))?;
    Ok(certificate.account_key)
}

/// The account key a leaf names, unchecked: for members whose certificates were checked when they joined.
pub fn claimed(credential: &Credential) -> Option<Key> {
    DeviceCertificate::parse(&identity(credential).ok()?).ok().map(|c| c.account_key)
}

/// The safety number of two users (spec 3.4): 60 digits, the same on both sides. Each side is its user id and
/// account key.
pub fn safety_number(a: (&[u8], &Key), b: (&[u8], &Key)) -> String {
    let part = |(user, key): (&[u8], &Key)| {
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
            let value = digest[5 * i..5 * i + 5].iter().fold(0u64, |acc, b| (acc << 8) | *b as u64);
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
