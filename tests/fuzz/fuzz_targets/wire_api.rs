//! Every request and answer body: never panic, and the encoding is canonical (decode then encode is the identity).
#![no_main]
use libfuzzer_sys::fuzz_target;
use yoip::net::api::*;

fn canonical<T: tls_codec::DeserializeBytes + tls_codec::Serialize>(data: &[u8]) {
    if let Ok(value) = yoip::decode::<T>(data) {
        assert_eq!(yoip::encode(&value), data);
    }
}

fuzz_target!(|data: &[u8]| {
    canonical::<Prekeys>(data);
    canonical::<Bundle>(data);
    canonical::<Lookup>(data);
    canonical::<LookupResult>(data);
    canonical::<LookupNamed>(data);
    canonical::<LookupBot>(data);
    canonical::<Push>(data);
    canonical::<Inbox>(data);
    canonical::<Ack>(data);
    canonical::<Devices>(data);
    canonical::<Register>(data);
    canonical::<Backup>(data);
    if let Ok(backup) = yoip::decode::<Backup>(data) {
        let _ = backup.check();
    }
});
