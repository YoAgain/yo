//! Envelopes: never panic; what opens, seals back to the same bytes (one encoding per message).
#![no_main]
use libfuzzer_sys::fuzz_target;
use yo_wire::envelope;

fuzz_target!(|data: &[u8]| {
    let _ = envelope::check_sealed(data);
    if let Ok(message) = envelope::unpad(data, &envelope::SEALED_PLAIN) {
        assert_eq!(envelope::pad(message, &envelope::SEALED_PLAIN).unwrap(), data);
    }
    if let Ok(message) = envelope::open(data) {
        assert_eq!(envelope::seal(message).unwrap(), data);
    }
});
