//! Every request and answer body: never panic, and the encoding is canonical (decode then encode is the identity).
#![no_main]
use libfuzzer_sys::fuzz_target;
use yo_wire::api::*;

fn canonical<T: tls_codec::DeserializeBytes + tls_codec::Serialize>(data: &[u8]) {
    if let Ok(value) = yo_wire::decode::<T>(data) {
        assert_eq!(yo_wire::encode(&value), data);
    }
}

fuzz_target!(|data: &[u8]| {
    canonical::<VerifyStart>(data);
    canonical::<VerifyCheck>(data);
    canonical::<KeyPackages>(data);
    canonical::<Lookup>(data);
    canonical::<LookupResult>(data);
    canonical::<Push>(data);
    canonical::<Inbox>(data);
    canonical::<Ack>(data);
    canonical::<GroupCreate>(data);
    canonical::<Commit>(data);
    canonical::<Epoch>(data);
});
