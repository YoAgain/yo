//! What a stranger tries at the hub: spending someone else's requests, filling the disk without an identity, the
//! routes of a service that is off (hub/SECURITY.md).

use yo_hub::config::{Config, SmsConfig};
use yo_tests::{devices_hash, phone, random_u64, register, user_id, Http, TestHub};
use yo_wire::api;

fn now_ms() -> u64 {
    yo_hub::now_ms()
}

/// Posts an envelope to a user as a stranger, with the device check and any extra headers; the status.
fn post_envelope(url: &str, user: &str, hash: &[u8; 32], extra: &[(&str, &str)]) -> u16 {
    let mut request = reqwest::blocking::Client::new()
        .post(format!("{url}{}", api::deliver_path(&user_id(user))))
        .header(api::HEADER_DEVICES, yo_wire::b64(hash))
        .body(yo_wire::envelope::seal(&[7; 100]).unwrap());
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    request.send().unwrap().status().as_u16()
}

#[test]
fn a_stranger_who_knows_a_device_id_cannot_spend_its_requests() {
    let mut config = Config::for_tests();
    config.limits.signed_per_device_minute = 5;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let mut a = register(Http::new(hub.url()), &phone(random_u64() % 1_000_000_000), None);
    let device = yo_wire::b64(&a.device_id());
    let client = reqwest::blocking::Client::new();
    for _ in 0..20 {
        let status = client
            .get(format!("{}{}", hub.url(), api::inbox_path(0)))
            .header(yo_wire::request::HEADER_DEVICE, &device)
            .header(yo_wire::request::HEADER_TIME, now_ms().to_string())
            .header(yo_wire::request::HEADER_SIGNATURE, yo_wire::b64(&[1; 64]))
            .send()
            .unwrap()
            .status()
            .as_u16();
        assert_eq!(status, 401, "a bad signature is no request of the device's");
    }
    a.sync(0).expect("the device's own requests still go through");
}

#[test]
fn bytes_stored_without_identity_are_limited_per_address_except_for_the_servers_own() {
    let mut config = Config::for_tests();
    // Three envelopes of 1 KiB an hour.
    config.limits.anonymous_bytes_per_ip_hour = 3 * 1024;
    config.trust_cloudflare = true;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let b = register(Http::new(hub.url()), &phone(random_u64() % 1_000_000_000), None);
    let url = hub.url();
    let hash = devices_hash(&url, b.user_id()).unwrap();
    // From 127.0.0.1 without CF-Connecting-IP: the server's own service (YoGPT), not limited.
    for _ in 0..5 {
        assert_eq!(post_envelope(&url, b.user_id(), &hash, &[]), 202);
    }
    // Through Cloudflare: by the client's address.
    let from = [("cf-connecting-ip", "203.0.113.9")];
    let statuses: Vec<u16> = (0..4).map(|_| post_envelope(&url, b.user_id(), &hash, &from)).collect();
    assert_eq!(statuses, [202, 202, 202, 429]);
    // Another address has its own budget; an address in the same IPv6 /64 shares one.
    assert_eq!(post_envelope(&url, b.user_id(), &hash, &[("cf-connecting-ip", "203.0.113.10")]), 202);
    let v6 = |ip: &'static str| [("cf-connecting-ip", ip)];
    let statuses: Vec<u16> = ["2001:db8:1:2::1", "2001:db8:1:2::2", "2001:db8:1:2::3", "2001:db8:1:2:ffff::4"]
        .into_iter()
        .map(|ip| post_envelope(&url, b.user_id(), &hash, &v6(ip)))
        .collect();
    assert_eq!(statuses, [202, 202, 202, 429]);
}

#[test]
fn nothing_is_stored_without_identity_while_the_disk_is_nearly_full() {
    let dir = std::env::temp_dir().join(format!("yo-hub-security-{}-{}", std::process::id(), random_u64()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = Config::for_tests();
    config.db_path = dir.join("hub.sqlite").to_string_lossy().into_owned();
    config.limits.db_min_free_bytes = u64::MAX;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    // Registration is signed and small, and goes on: an account can still be made and read its mail.
    let b = register(Http::new(hub.url()), &phone(random_u64() % 1_000_000_000), None);
    let url = hub.url();
    let probe = reqwest::blocking::Client::new()
        .post(format!("{url}{}", api::deliver_path(&user_id(b.user_id()))))
        .header(api::HEADER_DEVICES, yo_wire::b64(&[0; 32]))
        .body(yo_wire::envelope::seal(b"probe").unwrap())
        .send()
        .unwrap();
    assert_eq!(probe.status().as_u16(), 507);
    let group = yo_wire::encode(&api::GroupCreate { group_id: [3; 32], epoch: 1, token_hash: [4; 32] });
    let created = reqwest::blocking::Client::new().post(format!("{url}{}", api::GROUPS)).body(group).send().unwrap();
    assert_eq!(created.status().as_u16(), 507);
    hub.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_phone_routes_are_not_there_while_sms_is_off() {
    let mut config = Config::for_tests();
    config.sms = SmsConfig::Off;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let client = reqwest::blocking::Client::new();
    for path in [api::VERIFY_START, api::VERIFY_CHECK] {
        let status = client.post(format!("{}{path}", hub.url())).body(vec![0; 16]).send().unwrap().status();
        assert_eq!(status.as_u16(), 404, "{path}");
    }
}
