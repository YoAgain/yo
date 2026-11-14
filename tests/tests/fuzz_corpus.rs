//! Writes the fuzzers' seeds: a device's state before anyone wrote to it, and the payloads of what arrives (a
//! session's first message, a profile, Yos, a text), framed as the core_receive target reads them. Run when the
//! formats change:
//!   dev/cargo test -p yo-tests --test fuzz_corpus -- --ignored

use std::sync::{Arc, Mutex};
use yo_core::Store;
use yo_tests::{random_u64, register, username, Http, TestHub};
use yo_wire::api;

#[derive(Default)]
struct Saved(Mutex<Vec<u8>>);

impl Store for Saved {
    fn save(&self, blob: Vec<u8>) -> Result<(), String> {
        *self.0.lock().unwrap() = blob;
        Ok(())
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
    let (pa, pb) = (username(n), username(n + 1));
    let ha = Http::new(hub.url());
    let mut a = register(ha.clone(), &pa, Some("ALICE"));
    let mut b = register(Http::new(hub.url()), &pb, Some("BOB"));
    // B's state before anything arrives.
    let saved = Arc::new(Saved::default());
    b.persist_to(saved.clone(), KEY).unwrap();
    write("seeds", "core_state.bin", &saved.0.lock().unwrap());
    // What B's check of A's identity answers.
    let (account_key, device_key) = ha
        .sent
        .lock()
        .unwrap()
        .iter()
        .find(|(_, path, _)| path == api::REGISTER)
        .map(|(_, _, body)| {
            let device = yo_wire::decode::<api::Register>(body).unwrap().device;
            let certificate =
                yo_wire::cert::DeviceCertificate::verify(device.certificate.as_slice(), &device.device_key).unwrap();
            (certificate.account_key, device.device_key)
        })
        .unwrap();
    let lookup = api::LookupResult {
        user_id: yo_tests::user_id(a.user_id()),
        account_key,
        devices: vec![api::Device { key: device_key, id: 1 }],
        bundles: vec![],
    };
    let named = api::LookupNamed { found: lookup, username: Some(pa.as_bytes().to_vec().into()) };
    write("seeds", "core_lookup.bin", &yo_wire::encode(&named));
    // A's first message (with its profile), then more.
    a.send_yo(&pb).unwrap();
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
    assert!(envelopes.len() >= 3, "{}", envelopes.len());
    // core_receive: a sequence of payloads, out of their seals, each as u32 length + bytes.
    let mut framed = Vec::new();
    for envelope in &envelopes {
        let payload = yo_core::seal::open(&b.account_secret(), envelope).unwrap();
        framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        framed.extend_from_slice(&payload);
    }
    write("corpus/core_receive", "chat", &framed);
    for (i, envelope) in envelopes.iter().enumerate() {
        write("corpus/wire_envelope", &format!("envelope-{i}"), envelope);
    }
    // The other targets: requests as sent, certificates, contents.
    for (i, (_, path, body)) in ha.sent.lock().unwrap().iter().enumerate().filter(|(_, (_, _, b))| !b.is_empty()) {
        if path == api::REGISTER {
            let device = yo_wire::decode::<api::Register>(body).unwrap().device;
            let mut cert_input = device.device_key.to_vec();
            cert_input.extend_from_slice(device.certificate.as_slice());
            write("corpus/wire_cert", "certificate", &cert_input);
        }
        write("corpus/wire_api", &format!("request-{i}"), body);
        write("corpus/hub_routes", &format!("request-{i}"), &route_input(path, body));
    }
    for content in [yo_core::Content::Yo, yo_core::Content::Text("hi".into())] {
        let bytes = yo_wire::content::Message { sent_at_ms: 1, content }.encode().unwrap();
        write("corpus/wire_content", &format!("content-{}", bytes.len()), &bytes);
    }
}

/// hub_routes input: route index (1 byte, the route's place in its `ROUTES`), then the body. Only requests with a
/// body come here; the first pattern the path contains names the route.
fn route_input(path: &str, body: &[u8]) -> Vec<u8> {
    let index = [
        (api::INBOX_ACK, 5),
        ("/v1/inbox/", 6),
        (api::PREKEYS, 9),
        (api::LOOKUP, 1),
        (api::PUSH, 2),
        (api::USERNAME, 3),
        (api::REGISTER, 10),
        (api::BACKUP, 13),
        ("/v1/blobs/", 17),
        (api::REPORT, 19),
        (api::FEEDBACK, 20),
        (api::STORAGE, 26),
    ]
    .iter()
    .find(|(p, _)| path.contains(p))
    .map_or(0, |(_, i)| *i);
    let mut out = vec![index];
    out.extend_from_slice(body);
    out
}

/// A backup record, for wire_api, and as the body of `PUT /v1/backup` and the signed `GET`, for
/// hub_routes. Run when the formats change, as the one above.
#[test]
#[ignore]
fn write_backup_seeds() {
    use yo_core::backup::{self, PasskeyDoor};
    let code = yo_tests::backup::words([1; 16]);
    let record = backup::make(&[2; 32], b"{}", &code, PasskeyDoor::New(&[3; 32])).unwrap();
    write("corpus/wire_api", "backup-2", &yo_wire::encode(&record));
    // Route 13 is `PUT /v1/backup`, 27 the signed `GET /v1/backup/{id}` (fuzz_targets/hub_routes.rs).
    write("corpus/hub_routes", "backup-put", &[&[13u8][..], &yo_wire::encode(&record)].concat());
    write("corpus/hub_routes", "backup-get", &[27]);
}
