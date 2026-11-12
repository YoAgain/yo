//! Writes the fuzzers' seeds: a device's state before a chat, and the envelopes that make the chat (Welcome,
//! profile, Yos, commits), framed as the core_receive target reads them. Run when the formats change:
//!   dev/cargo test -p yo-tests --test fuzz_corpus -- --ignored

use std::sync::{Arc, Mutex};
use yo_core::Store;
use yo_tests::{phone, random_u64, register, Http, TestHub};
use yo_wire::api;

#[derive(Default)]
struct Saved(Mutex<Vec<u8>>);

impl Store for Saved {
    fn save(&self, blob: Vec<u8>) {
        *self.0.lock().unwrap() = blob;
    }
}

/// The key the fuzz target opens the state with.
const KEY: [u8; 32] = [0x42; 32];

fn write(dir: &str, name: &str, bytes: &[u8]) {
    let dir = format!(
        "{}/../fuzz/seeds/{}",
        env!("CARGO_MANIFEST_DIR"),
        dir.trim_start_matches("corpus/").trim_start_matches("seeds")
    );
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(format!("{dir}/{name}"), bytes).unwrap();
}

#[test]
#[ignore]
fn write_fuzz_corpus() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (phone(n), phone(n + 1));
    let ha = Http::new(hub.url());
    let mut a = register(ha.clone(), &pa, Some("ALICE"));
    let mut b = register(Http::new(hub.url()), &pb, Some("BOB"));
    // B's state before anything arrives.
    let saved = Arc::new(Saved::default());
    b.persist_to(saved.clone(), KEY);
    write("seeds", "core_state.bin", &saved.0.lock().unwrap());
    // What B's check of A's identity answers.
    let (account_key, device_key) = ha
        .sent
        .lock()
        .unwrap()
        .iter()
        .find(|(_, path, _)| path == api::VERIFY_CHECK)
        .map(|(_, _, body)| {
            let check: api::VerifyCheck = yo_wire::decode(body).unwrap();
            let device = check.device;
            let certificate =
                yo_wire::cert::DeviceCertificate::verify(device.certificate.as_slice(), &device.device_key).unwrap();
            (certificate.account_key, device.device_key)
        })
        .unwrap();
    let lookup = api::LookupResult {
        user_id: yo_tests::user_id(a.user_id()),
        account_key,
        device_keys: vec![device_key],
        key_packages: vec![],
    };
    write("seeds", "core_lookup.bin", &yo_wire::encode(&lookup));
    // A chat: Welcome, profile, Yo; an update; more Yos.
    a.send_yo(&pb).unwrap();
    let later = yo_hub::now_ms() + yo_core::UPDATE_AFTER_MS + 1;
    a.set_clock(Arc::new(move || later));
    a.maintenance().unwrap();
    a.send_yo(&pb).unwrap();
    a.send(&pb, yo_core::Content::Text("hello".into())).unwrap();
    let to_b = api::deliver_path(&yo_tests::user_id(b.user_id()));
    let envelopes: Vec<Vec<u8>> = ha
        .sent
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, path, _)| method == "POST" && *path == to_b)
        .map(|(_, _, body)| body.clone())
        .collect();
    assert!(envelopes.len() >= 5, "{}", envelopes.len());
    // core_receive: a sequence of envelopes, each as u32 length + bytes.
    let mut framed = Vec::new();
    for envelope in &envelopes {
        framed.extend_from_slice(&(envelope.len() as u32).to_be_bytes());
        framed.extend_from_slice(envelope);
    }
    write("corpus/core_receive", "chat", &framed);
    for (i, envelope) in envelopes.iter().enumerate() {
        write("corpus/wire_envelope", &format!("envelope-{i}"), envelope);
    }
    // The other targets: requests as sent, KeyPackages, certificates, contents.
    for (i, (_, path, body)) in ha.sent.lock().unwrap().iter().enumerate().filter(|(_, (_, _, b))| !b.is_empty()) {
        if path == api::VERIFY_CHECK {
            let check: api::VerifyCheck = yo_wire::decode(body).unwrap();
            let device = check.device;
            let mut cert_input = device.device_key.to_vec();
            cert_input.extend_from_slice(device.certificate.as_slice());
            write("corpus/wire_cert", "certificate", &cert_input);
            for (k, key_package) in device.key_packages.iter().take(3).enumerate() {
                write("corpus/hub_key_package", &format!("kp-{k}"), key_package.as_slice());
            }
            write("corpus/hub_key_package", "last-resort", device.last_resort.as_slice());
        }
        write("corpus/wire_api", &format!("request-{i}"), body);
        write("corpus/hub_routes", &format!("request-{i}"), &route_input(path, body));
    }
    for content in [yo_core::Content::Yo, yo_core::Content::Text("hi".into())] {
        let bytes = yo_wire::content::Message { sent_at_ms: 1, content }.encode().unwrap();
        write("corpus/wire_content", &format!("content-{}", bytes.len()), &bytes);
    }
}

/// hub_routes input: route index (1 byte), then the body.
fn route_input(path: &str, body: &[u8]) -> Vec<u8> {
    let index = [
        api::VERIFY_START,
        api::VERIFY_CHECK,
        api::KEY_PACKAGES,
        api::LOOKUP,
        api::PUSH,
        api::INBOX,
        api::INBOX_ACK,
        "/v1/inbox/",
        api::GROUPS,
        "/commit",
    ]
    .iter()
    .position(|p| path.contains(p))
    .unwrap_or(0) as u8;
    let mut out = vec![index];
    out.extend_from_slice(body);
    out
}
