//! Connecting a service (spec 3.5, `skills/ifttt`): the app shows that this account says yes to one link request.
//!
//! A service's page names a ticket, random and once; the app signs it with the account key, bound to the page's
//! origin, and gives the page back the signature; the service checks it against the account key the hub attests
//! (lookup). A signature for one origin or ticket is worth nothing for another.

use crate::{KEY_LEN, SIGNATURE_LEN};

/// The label in front of what is signed. A later version has another label.
pub const LABEL: &[u8] = b"yo connect v1";
/// The longest origin and ticket signed.
pub const MAX_FIELD: usize = 255;

/// What the account key signs: `LABEL || len(origin) || origin || len(ticket) || ticket`, each length one byte.
/// `None`: a field empty or longer than `MAX_FIELD`.
pub fn to_be_signed(origin: &str, ticket: &str) -> Option<Vec<u8>> {
    let mut out = LABEL.to_vec();
    for field in [origin, ticket] {
        if field.is_empty() || field.len() > MAX_FIELD {
            return None;
        }
        out.push(field.len() as u8);
        out.extend_from_slice(field.as_bytes());
    }
    Some(out)
}

/// Whether `signature` is `account_key`'s over `origin` and `ticket`.
pub fn verify(account_key: &[u8; KEY_LEN], origin: &str, ticket: &str, signature: &[u8]) -> bool {
    signature.len() == SIGNATURE_LEN
        && crate::curve_key_usable(account_key)
        && to_be_signed(origin, ticket).is_some_and(|message| crate::xeddsa_verify(account_key, &message, signature))
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsignal_core::curve::KeyPair;

    fn signed(pair: &KeyPair, origin: &str, ticket: &str) -> Vec<u8> {
        pair.private_key.calculate_signature(&to_be_signed(origin, ticket).unwrap(), &mut rand::rng()).unwrap().into()
    }

    fn public(pair: &KeyPair) -> [u8; KEY_LEN] {
        pair.public_key.public_key_bytes().try_into().unwrap()
    }

    #[test]
    fn binds_origin_and_ticket() {
        let pair = KeyPair::generate(&mut rand::rng());
        let signature = signed(&pair, "https://justyo.co", "t1");
        assert!(verify(&public(&pair), "https://justyo.co", "t1", &signature));
        assert!(!verify(&public(&pair), "https://evil.example", "t1", &signature));
        assert!(!verify(&public(&pair), "https://justyo.co", "t2", &signature));
        let other = KeyPair::generate(&mut rand::rng());
        assert!(!verify(&public(&other), "https://justyo.co", "t1", &signature));
        assert!(!verify(&public(&pair), "https://justyo.co", "t1", &signature[1..]));
    }

    #[test]
    fn fields_cannot_shift() {
        // "ab" + "c" and "a" + "bc" are different messages.
        assert_ne!(to_be_signed("ab", "c"), to_be_signed("a", "bc"));
        assert_eq!(to_be_signed("", "t"), None);
        assert_eq!(to_be_signed("o", &"t".repeat(MAX_FIELD + 1)), None);
    }
}
