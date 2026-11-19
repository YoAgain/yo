use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::ChaCha20Poly1305;

pub(crate) fn seal(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let nonce = crate::identity::random::<12>();
    let ciphertext =
        ChaCha20Poly1305::new(key.into()).encrypt(&nonce.into(), Payload { msg: plain, aad }).expect("encryption");
    [nonce.as_slice(), &ciphertext].concat()
}

pub(crate) fn open(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let (nonce, ciphertext) = sealed.split_first_chunk::<12>()?;
    ChaCha20Poly1305::new(key.into()).decrypt(nonce.into(), Payload { msg: ciphertext, aad }).ok()
}
