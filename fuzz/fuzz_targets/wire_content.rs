//! Content: never panic; what decodes to a known type encodes back to the same bytes.
#![no_main]
use libfuzzer_sys::fuzz_target;
use yo_wire::content::{Content, Message};

fuzz_target!(|data: &[u8]| {
    if let Ok(message) = Message::decode(data) {
        if !matches!(message.content, Content::Unknown(_)) {
            assert_eq!(message.encode().unwrap(), data);
        }
    }
});
