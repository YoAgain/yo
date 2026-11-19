//! Remove once every keychain item is v3.

use libsignal_core::curve::PrivateKey;

/// A secret of a state from before the keys were libsignal's (an Ed25519 seed, of the account key or the auth key):
/// the clamped scalar its X25519 form was (SHA-512 of the seed, its lower half clamped), so the account keeps the
/// identity contacts' sessions know, and the hub's converted keys match.
pub fn legacy_account_secret(seed: &[u8; 32]) -> [u8; 32] {
    let hash = <sha2::Sha512 as sha2::Digest>::digest(seed);
    let lower: [u8; 32] = hash[..32].try_into().expect("32 bytes");
    PrivateKey::deserialize(&lower).expect("32 bytes").serialize().try_into().expect("32 bytes")
}
