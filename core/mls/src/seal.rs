//! Sealed sender (spec 6.1): every envelope a user receives is the MLS message sealed by HPKE to the user's account
//! key, then padded. The hub sees a size class and the inbox it goes to; group, epoch and kind are inside, readable
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

/// Seals an MLS message to the account with `account_key` and pads it into an envelope.
pub fn to(account_key: &Key, message: &[u8]) -> Result<Vec<u8>> {
    let public = VerifyingKey::from_bytes(account_key).map_err(|_| Error::Protocol("account key"))?.to_montgomery();
    let sealed = RustCrypto::default()
        .hpke_seal(CONFIG, public.as_bytes(), INFO, &[], message)
        .map_err(|_| Error::Internal("seal"))?;
    let mut out = sealed.kem_output.as_slice().to_vec();
    out.extend_from_slice(sealed.ciphertext.as_slice());
    Ok(envelope::seal(&out)?)
}

/// The MLS message in an envelope sealed to the account whose secret is `account_secret`.
pub fn open(account_secret: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>> {
    let inner = envelope::open(sealed)?;
    if inner.len() < KEM_OUTPUT {
        return Err(Error::Protocol("sealed envelope"));
    }
    let (kem_output, ciphertext) = inner.split_at(KEM_OUTPUT);
    let input = HpkeCiphertext { kem_output: kem_output.to_vec().into(), ciphertext: ciphertext.to_vec().into() };
    let mut secret = SigningKey::from_bytes(account_secret).to_scalar_bytes();
    let opened = RustCrypto::default().hpke_open(CONFIG, &input, &secret, INFO, &[]);
    secret.zeroize();
    opened.map_err(|_| Error::Protocol("not sealed to this account"))
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
    }
}
