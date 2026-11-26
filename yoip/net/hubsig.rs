//! The hub's signature on the answers that hand out keys (spec 4.1): a lookup's `200` (account key, devices, bundles)
//! and a delivery's `409` (the roster). A client that pins the hub's key sends a fresh `Yo-Nonce` with such a request;
//! the answer then ends in the hub's XEdDSA signature (Curve25519, libsignal) over what was asked and what is answered,
//! so a party between them (or an old answer played back) cannot hand out other keys. Without the header the answer is
//! as before.

use crate::{sha256, Error, KEY_LEN, SIGNATURE_LEN};

/// The request header with the client's nonce, base64url of `NONCE_LEN` random bytes.
pub const NONCE_HEADER: &str = "Yo-Nonce";
pub const NONCE_LEN: usize = 16;
/// Where the hub publishes its public key (32 bytes), for tools: apps use the copy they ship with.
pub const KEY_PATH: &str = "/.well-known/yo-hub-key";

const CONTEXT: &[u8] = b"yo hub answer v1";

/// What the hub signs: the context, the nonce, the request's path (as sent, without a query), the hash of the
/// request's body, and the answer's body.
pub fn message(nonce: &[u8; NONCE_LEN], path: &str, request: &[u8], answer: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(CONTEXT.len() + NONCE_LEN + 2 + path.len() + 32 + answer.len());
    m.extend_from_slice(CONTEXT);
    m.extend_from_slice(nonce);
    m.extend_from_slice(&(path.len() as u16).to_be_bytes());
    m.extend_from_slice(path.as_bytes());
    m.extend_from_slice(&sha256(request));
    m.extend_from_slice(answer);
    m
}

/// The nonce of a request's `Yo-Nonce` header value, if it is one.
pub fn nonce(header: &str) -> Option<[u8; NONCE_LEN]> {
    crate::unb64(header).ok()?.try_into().ok()
}

/// The answer in a signed body (the answer, then the signature), if the hub's `key` signed it for this request.
pub fn open<'a>(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    path: &str,
    request: &[u8],
    signed: &'a [u8],
) -> Result<&'a [u8], Error> {
    let at = signed.len().checked_sub(SIGNATURE_LEN).ok_or(Error::Signature)?;
    let (answer, signature) = signed.split_at(at);
    if !crate::xeddsa_verify(key, &message(nonce, path, request, answer), signature) {
        return Err(Error::Signature);
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsignal_core::curve::KeyPair;

    fn public(pair: &KeyPair) -> [u8; KEY_LEN] {
        pair.public_key.public_key_bytes().try_into().unwrap()
    }

    #[test]
    fn opens_only_what_the_hub_signed_for_this_question() {
        let hub = KeyPair::generate(&mut rand::rng());
        let key = public(&hub);
        let (nonce, path, request, answer) = ([1; NONCE_LEN], "/v1/lookup", b"who".as_slice(), b"keys".as_slice());
        let mut signed = answer.to_vec();
        let message = message(&nonce, path, request, answer);
        signed.extend_from_slice(&hub.private_key.calculate_signature(&message, &mut rand::rng()).unwrap());
        assert_eq!(open(&key, &nonce, path, request, &signed), Ok(answer));
        // Another question, another nonce (an old answer played back), other keys, another hub, nothing signed.
        assert_eq!(open(&key, &nonce, path, b"whom", &signed), Err(Error::Signature));
        assert_eq!(open(&key, &[2; NONCE_LEN], path, request, &signed), Err(Error::Signature));
        assert_eq!(open(&key, &nonce, "/v1/inbox/x", request, &signed), Err(Error::Signature));
        let mut other = signed.clone();
        other[0] ^= 1;
        assert_eq!(open(&key, &nonce, path, request, &other), Err(Error::Signature));
        let stranger = public(&KeyPair::generate(&mut rand::rng()));
        assert_eq!(open(&stranger, &nonce, path, request, &signed), Err(Error::Signature));
        assert_eq!(open(&key, &nonce, path, request, answer), Err(Error::Signature));
        assert_eq!(nonce_of(&crate::b64(&[3; NONCE_LEN])), Some([3; NONCE_LEN]));
        assert_eq!(nonce_of("short"), None);
    }

    fn nonce_of(header: &str) -> Option<[u8; NONCE_LEN]> {
        super::nonce(header)
    }
}
