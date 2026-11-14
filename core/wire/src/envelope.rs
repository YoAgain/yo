//! The envelope (spec 6.1): what the hub stores is a fixed bucket, so it sees a size class, not a kind or a length.
//!
//! Two forms. **Sealed**, what goes to an inbox: the version `SEALED` and an HPKE seal of the padded MLS message, the
//! padding inside, so nothing but the bucket shows. **Padded**, what a commit route takes (its path names the group
//! anyway): version, length, the message, zeros.

use crate::{Error, VERSION};

/// The sizes an envelope can have.
pub const BUCKETS: [usize; 4] = [1024, 4096, 16 * 1024, 64 * 1024];
const HEADER: usize = 1 + 4;
/// The version of a sealed envelope.
pub const SEALED: u8 = 2;
/// What a seal adds to its plaintext: the version, the KEM output (an X25519 key) and the AEAD tag.
pub const SEAL_OVERHEAD: usize = 1 + 32 + 16;
/// The sizes of a seal's plaintext: each a bucket once sealed.
pub const SEALED_PLAIN: [usize; 4] =
    [BUCKETS[0] - SEAL_OVERHEAD, BUCKETS[1] - SEAL_OVERHEAD, BUCKETS[2] - SEAL_OVERHEAD, BUCKETS[3] - SEAL_OVERHEAD];

/// What the hub checks of a delivery: a bucket, sealed. The rest only the recipient can read.
pub fn check_sealed(envelope: &[u8]) -> Result<(), Error> {
    if !BUCKETS.contains(&envelope.len()) {
        return Err(Error::Size);
    }
    if envelope[0] != SEALED {
        return Err(Error::Unsupported);
    }
    Ok(())
}

/// Wraps an MLSMessage: version, length, the message, zeros up to the smallest bucket that fits.
pub fn seal(message: &[u8]) -> Result<Vec<u8>, Error> {
    pad(message, &BUCKETS)
}

/// Unwraps an envelope: its size must be a bucket, the padding zeros, the version known.
pub fn open(envelope: &[u8]) -> Result<&[u8], Error> {
    unpad(envelope, &BUCKETS)
}

/// What the hub checks before it stores a padded envelope: everything but the MLS message itself.
pub fn check(envelope: &[u8]) -> Result<(), Error> {
    unpad(envelope, &BUCKETS).map(|_| ())
}

/// The envelope's padding to other sizes (ascending): version, length, the message, zeros up to the smallest that
/// fits. Also what the storage key and a backup's K seal (spec 6.9, 7).
pub fn pad(message: &[u8], sizes: &[usize]) -> Result<Vec<u8>, Error> {
    let needed = HEADER + message.len();
    let size = *sizes.iter().find(|s| **s >= needed).ok_or(Error::Size)?;
    let mut out = Vec::with_capacity(size);
    out.push(VERSION);
    out.extend_from_slice(&(message.len() as u32).to_be_bytes());
    out.extend_from_slice(message);
    out.resize(size, 0);
    Ok(out)
}

/// The message in what `pad` made: its size one of `sizes`, the smallest that fits, the padding zeros.
pub fn unpad<'a>(padded: &'a [u8], sizes: &[usize]) -> Result<&'a [u8], Error> {
    if !sizes.contains(&padded.len()) || padded.len() < HEADER {
        return Err(Error::Size);
    }
    if padded[0] != VERSION {
        return Err(Error::Unsupported);
    }
    let len = u32::from_be_bytes(padded[1..5].try_into().unwrap()) as usize;
    let end = HEADER.checked_add(len).filter(|end| *end <= padded.len()).ok_or(Error::Format)?;
    if len == 0 || padded[end..].iter().any(|b| *b != 0) {
        return Err(Error::Format);
    }
    // The smallest size that fits: a larger one would be a second way to say the same.
    if sizes.iter().find(|s| **s >= end) != Some(&padded.len()) {
        return Err(Error::Format);
    }
    Ok(&padded[HEADER..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn round_trips_into_the_smallest_bucket() {
        for len in [1, 100, 1019, 1020, 4091, 4092, 65531] {
            let message = vec![0xAB; len];
            let sealed = seal(&message).unwrap();
            assert!(BUCKETS.contains(&sealed.len()));
            assert_eq!(open(&sealed).unwrap(), &message[..]);
        }
        assert_eq!(seal(&vec![0; 1019]).unwrap().len(), 1024);
        assert_eq!(seal(&vec![0; 1020]).unwrap().len(), 4096);
        assert_eq!(seal(&vec![0; 65532]), Err(Error::Size));
    }

    #[test]
    fn refuses_bad_padding_size_and_version() {
        let mut sealed = seal(b"hello").unwrap();
        sealed[100] = 1;
        assert_eq!(open(&sealed), Err(Error::Format));
        let mut sealed = seal(b"hello").unwrap();
        sealed[0] = 2;
        assert_eq!(open(&sealed), Err(Error::Unsupported));
        assert_eq!(open(&[1, 0, 0, 0, 1, 7]), Err(Error::Size));
        // A small message in a larger bucket than needed.
        let mut big = vec![0u8; 4096];
        big[0] = VERSION;
        big[4] = 3;
        assert_eq!(open(&big), Err(Error::Format));
    }

    #[test]
    fn pads_to_other_sizes() {
        let sizes = [100, 1000];
        assert_eq!(pad(b"hi", &sizes).unwrap().len(), 100);
        assert_eq!(unpad(&pad(b"hi", &sizes).unwrap(), &sizes).unwrap(), b"hi");
        assert_eq!(pad(&[1; 96], &sizes).unwrap().len(), 1000);
        assert_eq!(pad(&[1; 996], &sizes), Err(Error::Size));
        assert_eq!(unpad(&pad(b"hi", &sizes).unwrap(), &BUCKETS), Err(Error::Size));
    }

    #[test]
    fn a_sealed_envelope_is_a_bucket_with_its_version() {
        assert_eq!(
            check_sealed(&{
                let mut e = vec![0; 1024];
                e[0] = SEALED;
                e
            }),
            Ok(())
        );
        assert_eq!(check_sealed(&seal(b"bare").unwrap()), Err(Error::Unsupported));
        assert_eq!(check_sealed(&[SEALED; 1000]), Err(Error::Size));
        assert_eq!(check_sealed(&[]), Err(Error::Size));
        // A message as large as a seal takes still fits the largest bucket, sealed.
        let largest = SEALED_PLAIN[3] - HEADER;
        assert_eq!(pad(&vec![1; largest], &SEALED_PLAIN).unwrap().len() + SEAL_OVERHEAD, BUCKETS[3]);
    }

    proptest! {
        #[test]
        fn never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..5000)) {
            let _ = open(&bytes);
        }

        #[test]
        fn opens_what_it_seals(message in proptest::collection::vec(any::<u8>(), 1..5000)) {
            let sealed = seal(&message).unwrap();
            prop_assert_eq!(open(&sealed).unwrap(), &message[..]);
        }
    }
}
