//! The hub's KeyPackage check: never panic, and what it accepts is the leaf key vouched for by the account key it was
//! checked against.
#![no_main]
use libfuzzer_sys::fuzz_target;
use openmls::prelude::{BasicCredential, KeyPackageIn};
use tls_codec::Deserialize as _;

fuzz_target!(|data: &[u8]| {
    let crypto = openmls_rust_crypto::RustCrypto::default();
    // Check it against whoever it claims to be, so the check runs all the way.
    let claimed = KeyPackageIn::tls_deserialize_exact(data).ok().and_then(|kp| {
        let credential = kp.unverified_credential();
        let device_key: [u8; 32] = credential.signature_key.as_slice().try_into().ok()?;
        let basic = BasicCredential::try_from(credential.credential).ok()?;
        let certificate = yo_wire::cert::DeviceCertificate::parse(basic.identity()).ok()?;
        Some((certificate.account_key, device_key))
    });
    let (account_key, device_key) = claimed.unwrap_or(([0; 32], [0; 32]));
    if yo_hub::kp::check(&crypto, data, &account_key, &device_key).is_some() {
        let key_package = KeyPackageIn::tls_deserialize_exact(data).unwrap();
        assert_eq!(key_package.unverified_credential().signature_key.as_slice(), device_key);
    }
});
