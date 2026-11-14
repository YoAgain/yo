//! Every request and answer body: never panic, and the encoding is canonical (decode then encode is the identity).
#![no_main]
use libfuzzer_sys::fuzz_target;
use yo_wire::api::*;
use yo_wire::token::{BatchTokenRequest, BatchTokenResponse, Token};

fn canonical<T: tls_codec::DeserializeBytes + tls_codec::Serialize>(data: &[u8]) {
    if let Ok(value) = yo_wire::decode::<T>(data) {
        assert_eq!(yo_wire::encode(&value), data);
    }
}

fuzz_target!(|data: &[u8]| {
    canonical::<KeyPackages>(data);
    canonical::<Lookup>(data);
    canonical::<LookupResult>(data);
    canonical::<LookupNamed>(data);
    canonical::<Push>(data);
    canonical::<Inbox>(data);
    canonical::<Ack>(data);
    canonical::<GroupCreate>(data);
    canonical::<Commit>(data);
    canonical::<Epoch>(data);
    canonical::<Register>(data);
    canonical::<Backup>(data);
    if let Ok(backup) = yo_wire::decode::<Backup>(data) {
        let _ = backup.check();
    }
    canonical::<BatchTokenRequest>(data);
    canonical::<BatchTokenResponse>(data);
    canonical::<Token>(data);
    if let Ok(header) = std::str::from_utf8(data) {
        let _ = Token::from_authorization(header);
    }
});
