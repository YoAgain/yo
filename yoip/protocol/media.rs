use crate::net::limits::MAX_BLOB_LEN;
use crate::{sha256, Error};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::ChaCha20Poly1305;

pub use crate::protocol::content::Media;

/// ChaCha20-Poly1305's tag.
const TAG: usize = 16;
/// The largest photo or video that fits in a blob.
pub const MAX_MEDIA_LEN: usize = MAX_BLOB_LEN - TAG;

/// Every key is used once: the nonce is zero.
const NONCE: [u8; 12] = [0; 12];

/// Seals a photo or video under a new key, with a new id: what the Yo carries, and the ciphertext to upload.
pub fn seal(
    plain: &[u8],
    width: u16,
    height: u16,
    key: impl FnOnce() -> [u8; 32],
    id: impl FnOnce() -> crate::protocol::content::BlobId,
) -> Result<(Media, Vec<u8>), Error> {
    if plain.is_empty() || plain.len() > MAX_MEDIA_LEN {
        return Err(Error::Size);
    }
    let key = key();
    let ciphertext = ChaCha20Poly1305::new(&key.into()).encrypt(&NONCE.into(), plain).map_err(|_| Error::Format)?;
    let media = Media { id: id(), key, sha256: sha256(&ciphertext), size: ciphertext.len() as u32, width, height };
    Ok((media, ciphertext))
}

/// Opens a fetched blob: its size and hash must be the ones the Yo named, then it must decrypt.
pub fn open(media: &Media, ciphertext: &[u8]) -> Result<Vec<u8>, &'static str> {
    if ciphertext.len() != media.size as usize || sha256(ciphertext) != media.sha256 {
        return Err("media hash");
    }
    ChaCha20Poly1305::new(&media.key.into()).decrypt(&NONCE.into(), ciphertext).map_err(|_| "media")
}

/// The same blob without its key: what a client that forgets media keys keeps.
pub fn without_key(media: Media) -> Media {
    Media { key: [0; 32], ..media }
}
