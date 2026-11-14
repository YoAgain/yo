//! The app's version (spec 6.3): `Yo-App` kept per device, and an app older than the hub's minimum for its platform
//! answered `426`, so it asks to be updated.

use std::sync::Arc;
use yo_core::{Client, Error, Push, Response, Transport};
use yo_hub::config::Config;
use yo_tests::{random_u64, Http, TestHub};
use yo_wire::api::{status, HEADER_APP};

/// The platform's HTTP with the app's header on every request, as the apps send it.
struct App(Arc<Http>, &'static str);

impl Transport for App {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        let mut headers = headers.to_vec();
        headers.push((HEADER_APP.into(), self.1.into()));
        self.0.request(method, path, &headers, body)
    }
}

fn register(hub: &TestHub, app: &'static str) -> Result<Client, Error> {
    let name = format!("App{}", random_u64() % 1_000_000);
    Client::register_username(Arc::new(App(Http::new(hub.url()), app)), &name, None, None, Push::none())
}

#[test]
fn the_app_is_kept_per_device() {
    let hub = TestHub::start();
    let mut a = register(&hub, "ios/202611140619").unwrap();
    a.sync(0).unwrap();
    let mut b = register(&hub, "android/12").unwrap();
    b.sync(0).unwrap();
    assert_eq!(hub.hub.apps().unwrap(), [("android/12".into(), 1), ("ios/202611140619".into(), 1)]);
}

/// The app's header on the registration alone.
struct Registering(Arc<Http>);

impl Transport for Registering {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        let mut headers = headers.to_vec();
        if path == yo_wire::api::REGISTER {
            headers.push((HEADER_APP.into(), "ios/202611140619".into()));
        }
        self.0.request(method, path, &headers, body)
    }
}

#[test]
fn the_app_is_kept_from_the_registration_on() {
    let hub = TestHub::start();
    let name = format!("App{}", random_u64() % 1_000_000);
    let mut a = Client::register_username(Arc::new(Registering(Http::new(hub.url()))), &name, None, None, Push::none())
        .unwrap();
    a.sync(0).unwrap();
    assert_eq!(hub.hub.apps().unwrap(), [("ios/202611140619".into(), 1)]);
}

#[test]
fn an_app_older_than_the_minimum_is_told_to_update() {
    let config = Config { min_app: vec![("ios".into(), 202611140619)], ..Config::for_tests() };
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    assert!(matches!(register(&hub, "ios/202611130000"), Err(Error::Refused(status::UPGRADE_REQUIRED))));
    // The minimum itself, a newer build, another platform and no header at all pass.
    for app in ["ios/202611140619", "ios/202611150000", "android/1"] {
        register(&hub, app).unwrap().sync(0).unwrap();
    }
    Client::register_username(
        Http::new(hub.url()),
        &format!("NoApp{}", random_u64() % 1_000_000),
        None,
        None,
        Push::none(),
    )
    .unwrap();
}
