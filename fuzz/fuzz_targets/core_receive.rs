//! The client core's receive path, deep: a device's real state before a chat, then a sequence of envelopes (each a
//! u32 length and the bytes), seeded with a real chat (Welcome, profile, Yos, a commit). Never a panic; whatever
//! arrives, the device can still export its state.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::Arc;
use yo_core::{Client, Response, Transport};

const STATE: &[u8] = include_bytes!("../seeds/core_state.bin");
const LOOKUP: &[u8] = include_bytes!("../seeds/core_lookup.bin");
const KEY: [u8; 32] = [0x42; 32];

/// The hub, as far as receiving asks it: the lookup that attests the Welcome's sender.
struct Hub;

impl Transport for Hub {
    fn request(&self, _: &str, path: &str, _: &[(String, String)], _: &[u8]) -> Result<Response, String> {
        Ok(if path == "/v1/lookup" {
            Response { status: 200, body: LOOKUP.to_vec() }
        } else {
            Response { status: 404, body: Vec::new() }
        })
    }
}

fuzz_target!(|data: &[u8]| {
    let mut client = Client::restore(Arc::new(Hub), STATE, &KEY).unwrap();
    let mut rest = data;
    for _ in 0..16 {
        let Some((len, tail)) = rest.split_first_chunk::<4>() else { break };
        let len = (u32::from_be_bytes(*len) as usize).min(tail.len());
        let (envelope, tail) = tail.split_at(len);
        let _ = client.receive_for_fuzzing(envelope);
        rest = tail;
    }
    let _ = client.export(&KEY);
});
