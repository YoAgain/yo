//! The envelope (spec 6.1): an MLSMessage padded to a fixed bucket, so the hub sees a size class, not a kind.

use crate::{Error, VERSION};

/// The sizes an envelope can have.
pub const BUCKETS: [usize; 4] = [1024, 4096, 16 * 1024, 64 * 1024];
const HEADER: usize = 1 + 4;

/// Wraps an MLSMessage: version, length, the message, zeros up to the smallest bucket that fits.
pub fn seal(message: &[u8]) -> Result<Vec<u8>, Error> {
    let needed = HEADER + message.len();
    let size = *BUCKETS.iter().find(|b| **b >= needed).ok_or(Error::Size)?;
    let mut out = Vec::with_capacity(size);
    out.push(VERSION);
    out.extend_from_slice(&(message.len() as u32).to_be_bytes());
    out.extend_from_slice(message);
    out.resize(size, 0);
    Ok(out)
}

/// Unwraps an envelope: its size must be a bucket, the padding zeros, the version known.
pub fn open(envelope: &[u8]) -> Result<&[u8], Error> {
    check(envelope)?;
    let len = u32::from_be_bytes(envelope[1..5].try_into().unwrap()) as usize;
    Ok(&envelope[HEADER..HEADER + len])
}

/// What the hub checks before it stores an envelope: everything but the MLS message itself.
pub fn check(envelope: &[u8]) -> Result<(), Error> {
    if !BUCKETS.contains(&envelope.len()) {
        return Err(Error::Size);
    }
    if envelope[0] != VERSION {
        return Err(Error::Unsupported);
    }
    let len = u32::from_be_bytes(envelope[1..5].try_into().unwrap()) as usize;
    let end = HEADER.checked_add(len).filter(|end| *end <= envelope.len()).ok_or(Error::Format)?;
    if len == 0 || envelope[end..].iter().any(|b| *b != 0) {
        return Err(Error::Format);
    }
    // The smallest bucket that fits: a larger one would be a second way to say the same.
    if BUCKETS.iter().find(|b| **b >= end) != Some(&envelope.len()) {
        return Err(Error::Format);
    }
    Ok(())
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
