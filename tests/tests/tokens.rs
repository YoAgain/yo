//! Privacy Pass tokens for deliveries (spec 6.10): issued per device, spent once, a delivery with one past the limit
//! by address, one without as before, and a hub without them no trouble.

use std::sync::Arc;
use yo_core::{Client, Content, Event};
use yo_hub::config::Config;
use yo_tests::{random_u64, register, user_id, username, Fault, Http, TestHub};
use yo_wire::{api, token};

fn yos(events: &[Event], from: &Client) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::Message { user, content: Content::Yo, .. } if user == from.user_id()))
        .count()
}

/// A test hub's configuration with tokens (the tests' default has none: no limit by address to pass).
fn with_tokens() -> Config {
    let mut config = Config::for_tests();
    config.limits.tokens_per_device_day = 200;
    config
}

/// An account: its client, its transport, its username.
type Account = (Client, Arc<Http>, String);

/// Two accounts on a hub, each on its own transport.
fn pair(hub: &TestHub) -> (Account, Account) {
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let (ha, hb) = (Http::new(hub.url()), Http::new(hub.url()));
    ((register(ha.clone(), &pa, None), ha, pa), (register(hb.clone(), &pb, None), hb, pb))
}

/// A delivery as a stranger with a wrong device check: counted by the address like any, answered `409` (or `429`).
fn probe(url: &str, user: &str, extra: &[(&str, &str)]) -> u16 {
    let mut request = reqwest::blocking::Client::new()
        .post(format!("{url}{}", api::deliver_path(&user_id(user))))
        .header(api::HEADER_DEVICES, yo_wire::b64(&[0; 32]))
        .body(yo_wire::envelope::seal(b"probe").unwrap());
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    request.send().unwrap().status().as_u16()
}

/// Spends the address's deliveries until the hub says `429`.
fn exhaust(url: &str, user: &str) {
    for _ in 0..100 {
        if probe(url, user, &[]) == 429 {
            return;
        }
    }
    panic!("no limit");
}

fn issuer_key(url: &str) -> [u8; token::NE] {
    let directory = reqwest::blocking::get(format!("{url}{}", token::DIRECTORY)).unwrap();
    assert_eq!(directory.headers()["content-type"], "application/private-token-issuer-directory");
    yo_core::tokens::directory_key(&directory.bytes().unwrap(), 0).expect("the hub's key")
}

/// Asks for `n` tokens as the client's device; the status.
fn ask(client: &mut Client, key: &[u8; token::NE], n: usize) -> u16 {
    let batch = yo_core::tokens::request(key, n).unwrap();
    client.signed_for_tests("POST", api::TOKENS, &yo_wire::encode(&batch.request)).unwrap().status
}

/// The deliveries a transport sent to a user: their headers.
fn deliveries_to(http: &Http, user: &str) -> Vec<Vec<(String, String)>> {
    let path = api::deliver_path(&user_id(user));
    http.recorded.lock().unwrap().iter().filter(|(_, p, _, _)| *p == path).map(|(_, _, h, _)| h.clone()).collect()
}

fn has_token(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(name, value)| name == "authorization" && value.starts_with("PrivateToken token="))
}

#[test]
fn tokens_are_issued_per_device_and_day() {
    let mut config = with_tokens();
    config.limits.tokens_per_device_day = 150;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let ((mut a, _, _), (mut b, _, _)) = pair(&hub);
    a.sync(0).unwrap();
    assert_eq!(a.tokens_left(), 100, "a stash at the first sync");
    let key = issuer_key(&hub.url());
    assert_eq!(ask(&mut a, &key, 51), 429, "50 left of the day's 150");
    assert_eq!(ask(&mut a, &key, 50), 200);
    assert_eq!(ask(&mut a, &key, 1), 429);
    assert_eq!(ask(&mut b, &key, 101), 422, "at most 100 at once");
    b.sync(0).unwrap();
    assert_eq!(b.tokens_left(), 100, "another device has its own");
    let mut other = yo_core::tokens::request(&key, 1).unwrap();
    other.request.truncated_token_key_id ^= 1;
    let status = b.signed_for_tests("POST", api::TOKENS, &yo_wire::encode(&other.request)).unwrap().status;
    assert_eq!(status, 422, "a key the hub does not issue under");
    let unsigned = reqwest::blocking::Client::new()
        .post(format!("{}{}", hub.url(), api::TOKENS))
        .body(yo_wire::encode(&yo_core::tokens::request(&key, 1).unwrap().request))
        .send()
        .unwrap();
    assert_eq!(unsigned.status().as_u16(), 401, "for devices only");
}

#[test]
fn a_delivery_with_a_token_passes_an_address_at_its_limit_and_one_without_waits() {
    let mut config = with_tokens();
    config.limits.deliver_per_ip_minute = 3;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let ((mut a, ha, _), (mut b, _, pb)) = pair(&hub);
    a.sync(0).unwrap();
    let before = a.tokens_left();
    exhaust(&hub.url(), b.user_id());
    // Many phones behind one address: the others have used it up, A's tokens go past it.
    for _ in 0..5 {
        a.send_yo(&pb).unwrap();
    }
    assert_eq!(a.waiting(), 0, "every delivery taken");
    assert_eq!(a.tokens_left(), before - 6, "one token each, the Welcome's too");
    assert!(deliveries_to(&ha, b.user_id()).iter().all(|headers| has_token(headers)));
    assert_eq!(yos(&b.sync(0).unwrap(), &a), 5);
    // A sender without tokens (none asked for yet) is counted by the address, as before: its Yo waits.
    let mut c = register(Http::new(hub.url()), &username(random_u64() % 1_000_000_000), None);
    c.send_yo(&pb).unwrap();
    assert!(c.waiting() > 0, "over the address's limit");
}

#[test]
fn without_a_token_deliveries_are_counted_by_address_as_before() {
    let mut config = with_tokens();
    config.limits.deliver_per_ip_minute = 3;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let ((mut a, ha, _), (mut b, _, pb)) = pair(&hub);
    // No sync, no tokens: a Welcome and two Yos are the address's three.
    a.send_yo(&pb).unwrap();
    a.send_yo(&pb).unwrap();
    assert_eq!(a.waiting(), 0);
    a.send_yo(&pb).unwrap();
    assert_eq!(a.waiting(), 1, "the fourth delivery waits for the address's limit");
    assert!(deliveries_to(&ha, b.user_id()).iter().all(|headers| !has_token(headers)));
    assert_eq!(yos(&b.sync(0).unwrap(), &a), 2);
    // A token that is no token is counted the same, and refused.
    assert_eq!(probe(&hub.url(), b.user_id(), &[("authorization", "PrivateToken token=\"AAAA\"")]), 429);
}

#[test]
fn a_spent_token_is_refused_and_the_delivery_goes_again_without() {
    let hub = TestHub::start_with(with_tokens(), "127.0.0.1:0");
    let ((mut a, ha, _), (mut b, _, pb)) = pair(&hub);
    a.sync(0).unwrap();
    a.send_yo(&pb).unwrap();
    let left = a.tokens_left();
    // The hub takes the Yo and spends its token; its answer is lost: the token goes back to the stash.
    ha.fault(&api::deliver_path(&user_id(b.user_id())), Fault::LoseAnswer);
    a.send_yo(&pb).unwrap();
    assert_eq!((a.waiting(), a.tokens_left()), (1, left));
    let spent = deliveries_to(&ha, b.user_id()).last().unwrap().clone();
    // Sent again with that token: refused, and at once again without it, which the hub takes.
    a.sync(0).unwrap();
    assert_eq!(a.waiting(), 0);
    let sent = deliveries_to(&ha, b.user_id());
    let (again, without) = (&sent[sent.len() - 2], &sent[sent.len() - 1]);
    assert_eq!(again, &spent, "the same token");
    assert!(!has_token(without), "then none: never a signed delivery");
    let signed = [yo_wire::request::HEADER_DEVICE, yo_wire::request::HEADER_SIGNATURE];
    assert!(!without.iter().any(|(name, _)| signed.contains(&name.as_str())), "nor a signed one");
    assert_eq!(yos(&b.sync(0).unwrap(), &a), 2, "each Yo once");
    // As anyone who copied it would find: spent.
    let authorization = spent.iter().find(|(name, _)| name == "authorization").unwrap().1.clone();
    assert_eq!(probe(&hub.url(), b.user_id(), &[("authorization", &authorization)]), 401);
}

#[test]
fn a_hub_without_tokens_gets_every_delivery_as_before() {
    let hub = TestHub::start_with(with_tokens(), "127.0.0.1:0");
    let ((mut a, ha, _), (mut b, _, pb)) = pair(&hub);
    // A hub from before tokens has no issuer directory.
    ha.fault(token::DIRECTORY, Fault::Answer(404));
    a.sync(0).unwrap();
    assert_eq!(a.tokens_left(), 0);
    a.send_yo(&pb).unwrap();
    a.sync(0).unwrap();
    assert_eq!(a.waiting(), 0);
    assert_eq!(yos(&b.sync(0).unwrap(), &a), 1);
    let sent = ha.sent.lock().unwrap();
    assert_eq!(sent.iter().filter(|(_, path, _)| path == token::DIRECTORY).count(), 1, "asked once, not at every sync");
    assert!(!sent.iter().any(|(_, path, _)| path == api::TOKENS));
    // A hub whose operator issues none answers the same.
    let off = TestHub::start();
    assert_eq!(reqwest::blocking::get(format!("{}{}", off.url(), token::DIRECTORY)).unwrap().status().as_u16(), 404);
}

#[test]
fn the_hub_keeps_nothing_that_ties_a_redeemed_token_to_its_issuance() {
    let dir = std::env::temp_dir().join(format!("yo-hub-tokens-{}-{}", std::process::id(), random_u64()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hub.sqlite");
    let mut config = with_tokens();
    config.db_path = path.to_string_lossy().into_owned();
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let ((mut a, ha, _), (mut b, _, pb)) = pair(&hub);
    a.sync(0).unwrap();
    for _ in 0..3 {
        a.send_yo(&pb).unwrap();
    }
    b.sync(0).unwrap();
    // What the device sent to be issued: its blinded elements.
    let blinded: Vec<[u8; token::NE]> = ha
        .sent
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, path, _)| path == api::TOKENS)
        .flat_map(|(_, _, body)| yo_wire::decode::<token::BatchTokenRequest>(body).unwrap().blinded_elements)
        .collect();
    assert_eq!(blinded.len(), 100);
    let db = rusqlite::Connection::open(&path).unwrap();
    let columns: Vec<String> = db
        .prepare("SELECT name FROM pragma_table_info('spent_tokens')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(columns, ["nonce", "key_id"], "a spent token is its nonce and its key, nothing about who");
    let spent: i64 = db.query_row("SELECT count(*) FROM spent_tokens", [], |row| row.get(0)).unwrap();
    assert_eq!(spent, 4, "the Welcome and three Yos");
    // Not a byte of what was blinded is anywhere in the database.
    let tables: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut values = Vec::new();
    for table in &tables {
        let mut statement = db.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let n = statement.column_count();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for i in 0..n {
                if let Ok(rusqlite::types::ValueRef::Blob(bytes)) = row.get_ref(i) {
                    values.push(bytes.to_vec());
                }
            }
        }
    }
    for element in &blinded {
        assert!(!values.iter().any(|v| v.windows(token::NE).any(|w| w == element)), "a blinded element kept");
    }
    drop(db);
    hub.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
