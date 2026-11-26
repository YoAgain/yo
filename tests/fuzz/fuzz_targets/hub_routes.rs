//! The hub's routes in-process: whatever comes in, never a panic and never a 5xx.
#![no_main]
use axum::body::Body;
use axum::http::Request;
use libfuzzer_sys::fuzz_target;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static ROUTER: OnceLock<axum::Router> = OnceLock::new();

const ROUTES: [(&str, &str); 45] = [
    ("PUT", "/v1/prekeys"),
    ("POST", "/v1/lookup"),
    ("PUT", "/v1/push"),
    ("PUT", "/v1/username"),
    ("GET", "/v1/inbox?wait=0"),
    ("POST", "/v1/inbox/ack"),
    ("POST", "/v1/inbox/AAAAAAAAAAAAAAAAAAAAAA"),
    ("PUT", "/v1/prekeys"),
    ("POST", "/v1/lookup"),
    ("GET", "/v1/inbox?wait=1"),
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
    ("POST", "/v1/help/report"),
    ("POST", "/v1/help/messages"),
    ("DELETE", "/v1/account"),
    ("GET", "/v1/inbox?wait=99999999999999999999&wait=1"),
    ("PUT", "/v1/prekeys"),
    ("PUT", "/v1/blobs/AAAAAAAAAAAAAAAAAAAAAA?fetches=4294967296"),
    ("GET", "/v1/storage"),
    ("PUT", "/v1/storage"),
    // Odd: signed, a fetch only a backup door's key may make.
    ("GET", "/v1/backup/AAAAAAAAAAAAAAAAAAAAAA"),
    ("PUT", "/v1/access-key"),
    ("POST", "/v1/help/crash"),
    ("GET", "/.well-known/yo-hub-key"),
    ("GET", "/.well-known/yo-trust-root"),
    ("GET", "/v1/certificate"),
    ("GET", "/v1/group-auth"),
    ("POST", "/v1/group-credential"),
    ("PUT", "/v1/groups/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
    ("POST", "/v1/groups/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
    ("GET", "/.well-known/yo-zk-params"),
    ("GET", "/"),
    ("GET", "/.well-known/apple-app-site-association"),
    ("POST", "/v1/admin/todos"),
    ("GET", "/v1/admin/reserved"),
    ("POST", "/v1/admin/reserved"),
    ("PUT", "/v1/admin/reserved"),
    ("DELETE", "/v1/admin/reserved/ADMIN"),
];

fuzz_target!(|data: &[u8]| {
    let Some((&route, body)) = data.split_first() else { return };
    let (method, path) = ROUTES[route as usize % ROUTES.len()];
    let runtime = RUNTIME.get_or_init(|| tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap());
    let router = ROUTER.get_or_init(|| {
        let _guard = runtime.enter();
        let mut config = yo_hub::config::Config::for_tests();
        config.admin_token = Some("fuzz-only".into());
        config.apple_app_ids = vec!["TEST.invalid.yo".into()];
        let hub = yo_hub::Hub::new(config, Arc::new(yo_hub::push::Recorder::default())).unwrap();
        hub.router()
    });
    let mut request = Request::builder().method(method).uri(path);
    if path.starts_with("/v1/admin/") {
        request = request.header("yo-admin-token", "fuzz-only");
    }
    if route % 2 == 1 {
        request =
            request.header("yo-device", "AAAAAAAAAAAAAAAAAAAAAA").header("yo-time", "1").header("yo-signature", "AAAA");
    }
    let mut request = request.body(Body::from(body.to_vec())).unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 9))));
    let response = runtime.block_on(router.clone().oneshot(request)).unwrap();
    assert!(response.status().as_u16() < 500, "{method} {path}: {}", response.status());
});
