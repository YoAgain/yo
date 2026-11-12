//! Device certificates: never panic; whatever parses encodes back to the same bytes; one that verifies does so only
//! for the device key it was made for. Input: 32 bytes of device key, then the certificate.
#![no_main]
use libfuzzer_sys::fuzz_target;
use yo_wire::cert::DeviceCertificate;

fuzz_target!(|data: &[u8]| {
    let Some((device_key, bytes)) = data.split_first_chunk::<32>() else { return };
    if let Ok(certificate) = DeviceCertificate::parse(bytes) {
        assert_eq!(certificate.to_bytes(), bytes);
    }
    if DeviceCertificate::verify(bytes, device_key).is_ok() {
        let mut other = *device_key;
        other[0] ^= 1;
        assert!(DeviceCertificate::verify(bytes, &other).is_err());
    }
});
