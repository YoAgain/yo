//! The device check of a delivery (spec 4.6): which devices of a user a sender encrypted for, as one hash.

use crate::KEY_LEN;
use sha2::{Digest, Sha256};

pub const LABEL: &[u8] = b"yo devices v1";

/// `SHA-256("yo devices v1" || account_key || sorted device keys)`: the same set gives the same hash, whatever the
/// order it was gathered in.
pub fn hash(account_key: &[u8; KEY_LEN], device_keys: &[[u8; KEY_LEN]]) -> [u8; 32] {
    let mut keys = device_keys.to_vec();
    keys.sort_unstable();
    keys.dedup();
    let mut hash = Sha256::new();
    hash.update(LABEL);
    hash.update(account_key);
    for key in keys {
        hash.update(key);
    }
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_and_repeats_do_not_matter_keys_do() {
        let account = [1; 32];
        assert_eq!(hash(&account, &[[2; 32], [3; 32]]), hash(&account, &[[3; 32], [2; 32], [2; 32]]));
        assert_ne!(hash(&account, &[[2; 32]]), hash(&account, &[[2; 32], [3; 32]]));
        assert_ne!(hash(&account, &[[2; 32]]), hash(&[9; 32], &[[2; 32]]));
    }
}
