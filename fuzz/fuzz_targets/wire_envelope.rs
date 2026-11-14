//! Envelopes: never panic; what unpads pads back to the same bytes (one encoding per message), and a payload read
//! is encoded back the same.
#![no_main]
use libfuzzer_sys::fuzz_target;
use yo_wire::envelope::{self, Payload, Plain};

fuzz_target!(|data: &[u8]| {
    let _ = envelope::check_sealed(data);
    for sizes in [&envelope::SEALED_PLAIN[..], &envelope::BUCKETS[..]] {
        if let Ok(message) = envelope::unpad(data, sizes) {
            assert_eq!(envelope::pad(message, sizes).unwrap(), data);
        }
    }
    if let Ok(payload) = yo_wire::decode::<Payload>(data) {
        assert_eq!(yo_wire::encode(&payload), data);
    }
    if let Ok(plain) = yo_wire::decode::<Plain>(data) {
        assert_eq!(yo_wire::encode(&plain), data);
    }
});
