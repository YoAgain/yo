use crate::protocol::envelope::{self, BUCKETS, SMALL_BUCKETS};
use libsignal_protocol::PublicKey;

/// The pinned trust roots as libsignal's keys (32 bytes each, as the apps write them).
pub fn roots(keys: &[[u8; crate::KEY_LEN]]) -> Vec<PublicKey> {
    keys.iter().filter_map(|k| PublicKey::from_djb_public_key_bytes(k).ok()).collect()
}

/// Fits a sealed sender message to an envelope bucket. `make` seals the payload padded to the supplied size.
pub fn seal<E>(
    buckets: &[usize],
    payload_len: usize,
    mut make: impl FnMut(usize) -> Result<Vec<u8>, E>,
    too_large: E,
) -> Result<Vec<u8>, E> {
    let needed = envelope::HEADER + payload_len;
    // What sealing adds (the certificate, keys, MACs, protobuf lengths), measured: the padding makes up the rest.
    let overhead = make(needed)?.len() - needed;
    for &bucket in buckets {
        let Some(mut size) = bucket.checked_sub(overhead).filter(|s| *s >= needed) else { continue };
        // A length's varint may grow by a byte on the way up: a few rounds settle it.
        for _ in 0..3 {
            let sealed = make(size)?;
            if sealed.len() == bucket {
                return Ok(sealed);
            }
            match (size + bucket).checked_sub(sealed.len()).filter(|s| *s >= needed) {
                Some(next) => size = next,
                None => break,
            }
        }
    }
    Err(too_large)
}

/// Checks the envelope bucket and opens an identified payload. A sealed sender message needs its recipient.
pub fn identified(sealed: &[u8]) -> Result<Option<Vec<u8>>, &'static str> {
    if !SMALL_BUCKETS.contains(&sealed.len()) {
        return Err("not a bucket");
    }
    if sealed[0] == crate::VERSION {
        let payload = envelope::unpad(sealed, &BUCKETS).map_err(|_| "format")?.to_vec();
        return Ok(Some(payload));
    }
    Ok(None)
}
