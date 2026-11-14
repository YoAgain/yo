//! Sealed sender (spec 6.1): every envelope a user receives is the MLS message, padded, sealed by HPKE to the user's
//! account key. The hub sees a bucket and the inbox it goes to; group, epoch, kind and length are inside, readable
//! only by the recipient's devices. The account key is the Ed25519 key every member's certificate is checked against
//! and every contact pinned, its X25519 form the seal key: nothing to publish, nothing the hub could swap.

use crate::error::{Error, Result};
use crate::identity::Key;
use ed25519_dalek::{SigningKey, VerifyingKey};
use openmls_rust_crypto::RustCrypto;
use openmls_traits::crypto::OpenMlsCrypto;
use openmls_traits::types::{HpkeAeadType, HpkeCiphertext, HpkeConfig, HpkeKdfType, HpkeKemType};
use yo_wire::envelope;
use zeroize::Zeroize;

const CONFIG: HpkeConfig = HpkeConfig(HpkeKemType::DhKem25519, HpkeKdfType::HkdfSha256, HpkeAeadType::ChaCha20Poly1305);
const INFO: &[u8] = b"yo sealed envelope v1";
/// The KEM output that leads the sealed message: an X25519 public key.
const KEM_OUTPUT: usize = 32;

/// Pads an MLS message and seals it to the account with `account_key`: an envelope of exactly one bucket.
pub fn to(account_key: &Key, message: &[u8]) -> Result<Vec<u8>> {
    let public = VerifyingKey::from_bytes(account_key).map_err(|_| Error::Protocol("account key"))?.to_montgomery();
    let plain = envelope::pad(message, &envelope::SEALED_PLAIN)?;
    let sealed = RustCrypto::default()
        .hpke_seal(CONFIG, public.as_bytes(), INFO, &[], &plain)
        .map_err(|_| Error::Internal("seal"))?;
    let mut out = Vec::with_capacity(plain.len() + envelope::SEAL_OVERHEAD);
    out.push(envelope::SEALED);
    out.extend_from_slice(sealed.kem_output.as_slice());
    out.extend_from_slice(sealed.ciphertext.as_slice());
    debug_assert!(envelope::BUCKETS.contains(&out.len()));
    Ok(out)
}

/// The MLS message in an envelope sealed to the account whose secret is `account_secret`.
pub fn open(account_secret: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>> {
    envelope::check_sealed(sealed)?;
    let (kem_output, ciphertext) = sealed[1..].split_at(KEM_OUTPUT);
    let input = HpkeCiphertext { kem_output: kem_output.to_vec().into(), ciphertext: ciphertext.to_vec().into() };
    let mut secret = SigningKey::from_bytes(account_secret).to_scalar_bytes();
    let opened = RustCrypto::default().hpke_open(CONFIG, &input, &secret, INFO, &[]);
    secret.zeroize();
    let plain = opened.map_err(|_| Error::Protocol("not sealed to this account"))?;
    Ok(envelope::unpad(&plain, &envelope::SEALED_PLAIN)?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_only_for_the_account_sealed_to() {
        let (secret, other) = ([7u8; 32], [8u8; 32]);
        let key = SigningKey::from_bytes(&secret).verifying_key().to_bytes();
        let message = b"an MLS message".to_vec();
        let sealed = to(&key, &message).unwrap();
        assert!(envelope::BUCKETS.contains(&sealed.len()));
        assert_eq!(open(&secret, &sealed).unwrap(), message);
        assert!(open(&other, &sealed).is_err());
        // Fresh each time: two copies of one message are not the same bytes.
        assert_ne!(to(&key, &message).unwrap(), sealed);
        // Not sealed (a bare padded message, as before): refused.
        assert!(open(&secret, &envelope::seal(&message).unwrap()).is_err());
        // The length is inside: messages of any size within a bucket are the same size sealed.
        assert!([1, 500, 900].iter().all(|n| to(&key, &vec![1; *n]).unwrap().len() == 1024));
        assert_eq!(open(&secret, &to(&key, &vec![1; 900]).unwrap()).unwrap(), vec![1; 900]);
        assert_eq!(to(&key, &vec![1; 1024 - envelope::SEAL_OVERHEAD - 4]).unwrap().len(), 4096);
    }
}
