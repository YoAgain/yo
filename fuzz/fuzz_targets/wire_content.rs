//! Content: never panic; what decodes to a known type encodes back to bytes that decode to it again, and to the same
//! bytes where nothing was skipped (a kind of `With` this version does not know).
#![no_main]
use libfuzzer_sys::fuzz_target;
use yo_wire::content::{Content, Message};

fuzz_target!(|data: &[u8]| {
    if let Ok(message) = Message::decode(data) {
        if !matches!(message.content, Content::Unknown(_)) {
            let encoded = message.encode().unwrap();
            assert_eq!(Message::decode(&encoded).unwrap(), message);
            if encoded.len() == data.len() {
                assert_eq!(encoded, data);
            }
        }
    }
});
