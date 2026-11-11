//! Device certificates: never panic; a certificate that verifies encodes back to the same bytes.
#![no_main]
use libfuzzer_sys::fuzz_target;
use yo_wire::cert::DeviceCertificate;

fuzz_target!(|data: &[u8]| {
    if let Ok(certificate) = DeviceCertificate::verify(data) {
        assert_eq!(certificate.to_bytes(), data);
    }
});
