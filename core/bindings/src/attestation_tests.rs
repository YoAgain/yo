use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

struct Hub(u16, Vec<u8>);

impl HttpTransport for Hub {
    fn request(&self, method: String, path: String, headers: Vec<Header>, body: Vec<u8>) -> HttpResult {
        assert!(headers.iter().any(|h| h.name.eq_ignore_ascii_case("yo-device")));
        match method.as_str() {
            "GET" => {
                assert_eq!(path, "/v1/attest/challenge");
                assert!(body.is_empty());
            }
            "POST" => {
                assert_eq!(path, "/v1/attest");
                assert_eq!(body, [2, 1, 42]); // Platform, VLBytes length, token.
            }
            _ => panic!("unexpected method"),
        }
        HttpResult { status: self.0, body: self.1.clone(), error: (self.0 == 0).then(|| "offline".into()) }
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

#[test]
fn attestation_schedule_survives_reopen() {
    const HOUR: u64 = 3_600_000;
    let path = std::env::temp_dir().join(format!("yo-attest-test-{}-{}", std::process::id(), now()));
    let key = [7; 32];
    let open = || StateDb(Arc::new(yo_core::statedb::StateDb::open_fresh(&path, key, Arc::new(|_| {})).unwrap()));
    let shared = Shared::new(&open(), None, None);
    let state = yo_core::state::State::default();
    let transport = Arc::new(Http(Arc::new(Hub(204, vec![]))));
    let (unregistered, _) = Client::from_snapshot(transport.clone(), state.clone(), Default::default(), false).unwrap();
    assert!(!unregistered.attest_due(u64::MAX));
    let mut state = state;
    state.user_id = "01".repeat(16);
    let (mut client, _) = Client::from_snapshot(transport, state, Default::default(), false).unwrap();
    client.persist_to(shared.store.clone(), key).unwrap();
    let reopen = |status, body| {
        YoClient::open_db(Arc::new(Hub(status, body)), Arc::new(open()), key.to_vec(), false, None).unwrap()
    };

    assert!(reopen(204, vec![]).attest_due(0));
    for status in [204, 404] {
        let before = now();
        assert_eq!(reopen(status, vec![]).attest_challenge().unwrap(), None);
        let client = reopen(status, vec![]);
        assert!(!client.attest_due(before + 24 * HOUR - 1));
        assert!(client.attest_due(now() + 24 * HOUR));
    }
    let challenge = vec![7; 32];
    let body = [vec![32], challenge.clone()].concat();
    assert_eq!(reopen(200, body).attest_challenge().unwrap(), Some(challenge));

    // Device failure, both negative verdicts and a lost reply each count once.
    for (attempt, hours) in [1, 2, 4, 8, 16, 32, 64, 128, 168, 168].into_iter().enumerate() {
        let before = now();
        match attempt {
            1 | 2 => {
                let verdict = attempt as u8 + 1;
                assert_eq!(reopen(200, vec![verdict]).attest_send(2, vec![42]).unwrap(), verdict);
            }
            3 => assert!(reopen(0, vec![]).attest_send(2, vec![42]).is_err()),
            _ => reopen(200, vec![]).attest_failed(before).unwrap(),
        }
        let client = reopen(200, vec![]);
        assert!(!client.attest_due(before + hours * HOUR - 1));
        assert!(client.attest_due(now() + hours * HOUR));
    }
    assert_eq!(reopen(200, vec![1]).attest_send(2, vec![42]).unwrap(), 1);
    assert!(!reopen(204, vec![]).attest_due(u64::MAX));
    std::fs::remove_file(path).unwrap();
}
