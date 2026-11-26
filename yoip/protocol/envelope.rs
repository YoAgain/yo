//! The envelope (spec 4.1): what the hub stores is a fixed bucket, so it sees a size class, not a kind or a length.
//! libsignal's sealed sender (v1) message, the padded payload inside, so nothing but the bucket shows.

use crate::{Error, UserId, KEY_LEN, VERSION};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};

/// The sizes an envelope can have.
pub const BUCKETS: [usize; 4] = [1024, 4096, 16 * 1024, 64 * 1024];
/// What `pad` puts before the message: its version and length.
pub const HEADER: usize = 1 + 4;
/// The first byte of a sealed envelope: libsignal's sealed sender v1 (`SEALED_SENDER_V1_FULL_VERSION`).
pub const SEALED: u8 = 0x11;
/// The private key of the test hubs' sender-certificate trust root (Curve25519): public, never a real hub's.
pub const TEST_TRUST_ROOT: [u8; 32] = [0x40; 32];

/// What a sealed envelope holds (spec 4.1): who sends (its sender certificate says it too), and the message for each
/// of the recipient's devices.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Payload {
    pub account_key: [u8; KEY_LEN],
    pub device_key: [u8; KEY_LEN],
    /// The sender device's number (`api::Device`): its session's address.
    pub device_id: u8,
    /// When the content was sent (its `sent_at`, ms), as Signal's envelope timestamp: what a
    /// `DecryptionErrorMessage` names (spec 3.2).
    pub sent_at: u64,
    pub copies: Vec<ForDevice>,
}

/// A copy's `kind`: a `SignalMessage`.
pub const KIND_MESSAGE: u8 = 0;
/// A copy's `kind`: a `PreKeySignalMessage`, which starts a session.
pub const KIND_PREKEY: u8 = 1;
/// A copy's `kind`: libsignal's `PlaintextContent`, outside the session (a `DecryptionErrorMessage`, spec 3.2).
pub const KIND_PLAINTEXT: u8 = 2;

/// A message for one device: a session message, or a plaintext one (`kind`).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct ForDevice {
    pub device_key: [u8; KEY_LEN],
    pub kind: u8,
    pub message: VLBytes,
}

/// What a session message carries (spec 3.2): the content, and for one's own other devices the user it was sent to.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Plain {
    pub to: Option<UserId>,
    pub content: VLBytes,
}

/// What the hub checks of a delivery: a bucket, sealed; or, `signed` by its sending device (the hub knows who sends),
/// identified: the padded payload as it is (`pad`, version 1), for a sender without a certificate (spec 4.1).
pub fn check(envelope: &[u8], signed: bool) -> Result<(), Error> {
    if !BUCKETS.contains(&envelope.len()) {
        return Err(Error::Size);
    }
    match envelope[0] {
        SEALED => Ok(()),
        VERSION if signed => Ok(()),
        _ => Err(Error::Unsupported),
    }
}

/// The envelope's padding to other sizes (ascending): version, length, the message, zeros up to the smallest that
/// fits: inside the sealed sender message, and what the storage key and a backup's K seal (spec 4.3, 2.4).
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
            let padded = pad(&message, &BUCKETS).unwrap();
            assert!(BUCKETS.contains(&padded.len()));
            assert_eq!(unpad(&padded, &BUCKETS).unwrap(), &message[..]);
        }
        assert_eq!(pad(&[0; 1019], &BUCKETS).unwrap().len(), 1024);
        assert_eq!(pad(&[0; 1020], &BUCKETS).unwrap().len(), 4096);
        assert_eq!(pad(&vec![0; 65532], &BUCKETS), Err(Error::Size));
    }

    #[test]
    fn refuses_bad_padding_size_and_version() {
        let mut padded = pad(b"hello", &BUCKETS).unwrap();
        padded[100] = 1;
        assert_eq!(unpad(&padded, &BUCKETS), Err(Error::Format));
        let mut padded = pad(b"hello", &BUCKETS).unwrap();
        padded[0] = 2;
        assert_eq!(unpad(&padded, &BUCKETS), Err(Error::Unsupported));
        assert_eq!(unpad(&[1, 0, 0, 0, 1, 7], &BUCKETS), Err(Error::Size));
        // A small message in a larger bucket than needed.
        let mut big = vec![0u8; 4096];
        big[0] = VERSION;
        big[4] = 3;
        assert_eq!(unpad(&big, &BUCKETS), Err(Error::Format));
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
        let mut sealed = vec![0; 1024];
        sealed[0] = SEALED;
        assert_eq!(check(&sealed, false), Ok(()));
        // Identified: only signed.
        assert_eq!(check(&pad(b"bare", &BUCKETS).unwrap(), false), Err(Error::Unsupported));
        assert_eq!(check(&pad(b"bare", &BUCKETS).unwrap(), true), Ok(()));
        assert_eq!(check(&[SEALED; 1000], false), Err(Error::Size));
        assert_eq!(check(&[], true), Err(Error::Size));
    }

    proptest! {
        #[test]
        fn never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..5000)) {
            let _ = unpad(&bytes, &BUCKETS);
            let _ = check(&bytes, true);
        }

        #[test]
        fn unpads_what_it_pads(message in proptest::collection::vec(any::<u8>(), 1..5000)) {
            let padded = pad(&message, &BUCKETS).unwrap();
            prop_assert_eq!(unpad(&padded, &BUCKETS).unwrap(), &message[..]);
        }
    }
}
