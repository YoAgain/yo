//! Push (spec 6.7): the hub wakes a device through APNs or FCM, with nothing in the push but "there is mail".

use std::time::{Duration, Instant};
use yo_core::{Client, Push};
use yo_hub::config::{ApnsConfig, Config, FcmConfig};
use yo_hub::push::Request;
use yo_tests::{phone, random_u64, register, Http, TestHub};
use yo_wire::api::{PUSH_APNS, PUSH_FCM};

fn hub() -> TestHub {
    let mut config = Config::for_tests();
    config.apns = Some(ApnsConfig {
        key_pem: include_str!("../fixtures/test-apns.p8").into(),
        key_id: "KEYID12345".into(),
        team_id: "TEAMID1234".into(),
        topic: "co.justyo.app".into(),
    });
    let account = serde_json::json!({
        "project_id": "yo-test",
        "client_email": "push@yo-test.iam.gserviceaccount.com",
        "token_uri": "https://oauth2.googleapis.com/token",
        "private_key": include_str!("../fixtures/test-fcm.pem"),
    });
    config.fcm = Some(FcmConfig { service_account_json: account.to_string() });
    TestHub::start_with(config, "127.0.0.1:0")
}

fn with_push(hub: &TestHub, number: &str, platform: u8, token: &str) -> Client {
    let http = Http::new(hub.url());
    Client::request_code(http.as_ref(), number).unwrap();
    let push = Push { platform, token: token.as_bytes().to_vec().into() };
    Client::register(http, number, "000000", None, push).unwrap()
}

fn wait_for(hub: &TestHub, count: usize) -> Vec<Request> {
    let start = Instant::now();
    loop {
        let sent = hub.push.sent.lock().unwrap().clone();
        if sent.len() >= count || start.elapsed() > Duration::from_secs(3) {
            return sent;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn apns_and_fcm_are_woken_without_content() {
    let hub = hub();
    let n = random_u64() % 1_000_000_000;
    let mut a = register(Http::new(hub.url()), &phone(n), None);
    let _iphone = with_push(&hub, &phone(n + 1), PUSH_APNS, "a1b2c3");
    let _android = with_push(&hub, &phone(n + 2), PUSH_FCM, "fcm-token");
    a.send_yo(&phone(n + 1)).unwrap();
    a.send_yo(&phone(n + 2)).unwrap();
    // The iPhone gets two pushes (Welcome, Yo), the Android phone too, after one OAuth exchange.
    let sent = wait_for(&hub, 5);
    let apns: Vec<_> = sent.iter().filter(|r| r.url.contains("push.apple.com")).collect();
    let fcm: Vec<_> = sent.iter().filter(|r| r.url.contains("fcm.googleapis.com")).collect();
    let oauth: Vec<_> = sent.iter().filter(|r| r.url.contains("oauth2")).collect();
    assert_eq!((apns.len(), fcm.len(), oauth.len()), (2, 2, 1), "{sent:#?}");
    let first = apns[0];
    assert_eq!(first.url, "https://api.push.apple.com/3/device/a1b2c3");
    let header = |name: &str| first.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
    assert_eq!(header("apns-topic"), Some("co.justyo.app"));
    assert_eq!(header("apns-push-type"), Some("alert"));
    assert!(header("authorization").unwrap().starts_with("bearer ey"));
    // Every APNs push is the same bytes: nothing about sender, chat or content.
    assert!(apns.iter().all(|r| r.body == first.body));
    let body: serde_json::Value = serde_json::from_slice(&first.body).unwrap();
    assert_eq!(body["aps"]["mutable-content"], 1);
    assert!(fcm.iter().all(|r| r.body == fcm[0].body));
    let body: serde_json::Value = serde_json::from_slice(&fcm[0].body).unwrap();
    assert_eq!(body["message"]["token"], "fcm-token");
    assert_eq!(body["message"]["data"], serde_json::json!({ "yo": "1" }));
    assert!(fcm[0].url.ends_with("/v1/projects/yo-test/messages:send"));
}

#[test]
fn a_dead_token_is_forgotten() {
    let hub = hub();
    let n = random_u64() % 1_000_000_000;
    let mut a = register(Http::new(hub.url()), &phone(n), None);
    let _iphone = with_push(&hub, &phone(n + 1), PUSH_APNS, "dead");
    *hub.push.device_status.lock().unwrap() = Some(410);
    a.send_yo(&phone(n + 1)).unwrap();
    let before = wait_for(&hub, 2).len();
    std::thread::sleep(Duration::from_millis(200));
    a.send_yo(&phone(n + 1)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let after = hub.push.sent.lock().unwrap().len();
    // The first delivery's push was refused with 410, so the token was cleared before the next ones.
    assert!(before >= 1 && after <= before + 1, "before {before}, after {after}");
    assert_eq!(hub.push.sent.lock().unwrap().iter().filter(|r| r.url.contains("/device/dead")).count(), after);
}

#[test]
fn an_apns_token_that_is_not_hex_never_reaches_a_url() {
    let hub = hub();
    let n = random_u64() % 1_000_000_000;
    let mut a = register(Http::new(hub.url()), &phone(n), None);
    let _iphone = with_push(&hub, &phone(n + 1), PUSH_APNS, "../../../3/device/a1b2c3?x=");
    let _android = with_push(&hub, &phone(n + 2), PUSH_FCM, "fcm-token");
    a.send_yo(&phone(n + 1)).unwrap();
    a.send_yo(&phone(n + 2)).unwrap();
    // The Android phone's pushes come (OAuth, Welcome, Yo); the iPhone's token is refused before any request.
    let sent = wait_for(&hub, 3);
    std::thread::sleep(Duration::from_millis(200));
    let sent = sent.len().max(hub.push.sent.lock().unwrap().len());
    assert_eq!(sent, 3);
    assert!(!hub.push.sent.lock().unwrap().iter().any(|r| r.url.contains("push.apple.com")));
}
