//! The hub's KeyPackage check: never panic, and what it accepts names exactly the uploader it was checked for.
#![no_main]
use libfuzzer_sys::fuzz_target;
use openmls::prelude::{BasicCredential, KeyPackageIn};
use tls_codec::Deserialize as _;

fuzz_target!(|data: &[u8]| {
    let crypto = openmls_rust_crypto::RustCrypto::default();
    // Check it against whoever it claims to be, so the check runs all the way.
    let claimed = KeyPackageIn::tls_deserialize_exact(data).ok().and_then(|kp| {
        let credential = BasicCredential::try_from(kp.unverified_credential().credential).ok()?;
        yo_wire::decode::<yo_wire::cert::DeviceCertificate>(credential.identity()).ok()
    });
    let (key, mailbox) = match &claimed {
        Some(c) => (c.body.account_key, c.body.mailbox),
        None => ([0; 32], [0; 16]),
    };
    if let Some(checked) = yo_hub::kp::check(&crypto, data, &key, &mailbox) {
        let body = &checked.certificate.body;
        assert!(body.account_key == key && body.mailbox == mailbox);
    }
});
