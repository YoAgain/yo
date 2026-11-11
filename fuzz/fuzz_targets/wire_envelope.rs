//! Envelopes: never panic; what opens, seals back to the same bytes (one encoding per message).
#![no_main]
use libfuzzer_sys::fuzz_target;
use yo_wire::envelope;

fuzz_target!(|data: &[u8]| {
    if let Ok(message) = envelope::open(data) {
        assert_eq!(envelope::seal(message).unwrap(), data);
    }
});
