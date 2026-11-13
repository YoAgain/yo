//! The hub's routes in-process: whatever comes in, never a panic and never a 5xx.
#![no_main]
use axum::body::Body;
use axum::http::Request;
use libfuzzer_sys::fuzz_target;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static ROUTER: OnceLock<axum::Router> = OnceLock::new();

const ROUTES: [(&str, &str); 27] = [
    ("POST", "/v1/verify/start"),
    ("POST", "/v1/verify/check"),
    ("PUT", "/v1/keypackages"),
    ("POST", "/v1/lookup"),
    ("PUT", "/v1/push"),
    ("PUT", "/v1/username"),
    ("GET", "/v1/inbox?wait=0"),
    ("POST", "/v1/inbox/ack"),
    ("POST", "/v1/inbox/AAAAAAAAAAAAAAAAAAAAAA"),
    ("POST", "/v1/groups"),
    ("POST", "/v1/groups/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA/commit"),
    ("GET", "/v1/groups/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA/commits/1"),
    ("POST", "/v1/register"),
    ("POST", "/v1/inbox/%%%"),
    ("GET", "/v1/nothing"),
    ("PUT", "/v1/backup"),
    ("GET", "/v1/backup/AAAAAAAAAAAAAAAAAAAAAA"),
    ("GET", "/v1/username/LEO"),
    ("GET", "/v1/username/%FF%00"),
    ("PUT", "/v1/blobs/AAAAAAAAAAAAAAAAAAAAAA?fetches=1"),
    ("GET", "/v1/blobs/AAAAAAAAAAAAAAAAAAAAAA"),
    // Appended, so the seeds' route indexes stay theirs.
    ("POST", "/v1/report"),
    ("POST", "/v1/feedback"),
    ("DELETE", "/v1/account"),
    ("GET", "/v1/inbox?wait=99999999999999999999&wait=1"),
    ("GET", "/v1/groups/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA/commits/18446744073709551615"),
    ("PUT", "/v1/blobs/AAAAAAAAAAAAAAAAAAAAAA?fetches=4294967296"),
];

fuzz_target!(|data: &[u8]| {
    let Some((&route, body)) = data.split_first() else { return };
    let (method, path) = ROUTES[route as usize % ROUTES.len()];
    let runtime = RUNTIME.get_or_init(|| tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap());
    let router = ROUTER.get_or_init(|| {
        let _guard = runtime.enter();
        let hub = yo_hub::Hub::new(yo_hub::config::Config::for_tests(), Arc::new(yo_hub::push::Recorder::default())).unwrap();
        hub.router()
    });
    let mut request = Request::builder().method(method).uri(path);
    if route % 2 == 1 {
        request = request.header("yo-device", "AAAAAAAAAAAAAAAAAAAAAA").header("yo-time", "1").header("yo-signature", "AAAA");
    }
    let mut request = request.body(Body::from(body.to_vec())).unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 9))));
    let response = runtime.block_on(router.clone().oneshot(request)).unwrap();
    assert!(response.status().as_u16() < 500, "{method} {path}: {}", response.status());
});
