//! Envelopes: never panic; what unpads pads back to the same bytes (one encoding per message), and a payload read
//! is encoded back the same.
#![no_main]
use libfuzzer_sys::fuzz_target;
use yoip::protocol::envelope::{self, Payload, Plain};

fuzz_target!(|data: &[u8]| {
    let _ = envelope::check(data, true);
    // Inside a sealed message: padded to the size that makes the bucket; as a storage blob: to the buckets.
    for sizes in [&[data.len()][..], &envelope::BUCKETS[..]] {
        if let Ok(message) = envelope::unpad(data, sizes) {
            assert_eq!(envelope::pad(message, sizes).unwrap(), data);
        }
    }
    if let Ok(payload) = yoip::decode::<Payload>(data) {
        assert_eq!(yoip::encode(&payload), data);
    }
    if let Ok(plain) = yoip::decode::<Plain>(data) {
        assert_eq!(yoip::encode(&plain), data);
    }
});
