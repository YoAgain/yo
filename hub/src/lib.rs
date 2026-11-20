//! The Yo hub (spec 6): it holds no content key, and the route that carries messages never learns a sender.
//!
//! `router` is the map of every route; each module serves one part of the spec: `account` (6.3, 6.4, 6.9, 7),
//! `mail` (4.4, 6.7), `groups` (4.9), `blobs` (6.8), `support` and `admin` (the operator). `request` holds what all
//! routes share: signatures (6.2) and limits (6.6).

mod account;
mod admin;
pub mod blobs;
pub mod certificate;
pub mod config;
pub mod db;
pub mod error;
pub mod groups;
mod help;
pub mod limiter;
mod mail;
pub mod push;
mod request;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::header::CONTENT_TYPE;
use axum::middleware::from_fn_with_state;
use axum::routing::{delete, get, post, put};
use axum::Router;
use config::Config;
use db::Db;
use limiter::{Limiter, Places, ReplayCache, Requests};
use mail::Delivery;
use request::{admit, off_workers, too_old, Hubs, DAY, DAY_MS, MINUTE};
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Notify, Semaphore};
use yo_wire::{api, hubsig, limits, DeviceId};

pub use mail::INBOX_AT_ONCE;
pub use request::READING_AT_ONCE;

// The in-file tests use these through `super::*`.
#[cfg(test)]
use {
    axum::extract::ConnectInfo,
    axum::http::StatusCode,
    bytes::Bytes,
    db::{NewDevice, Write},
    request::held,
    std::time::Duration,
};

/// What the hub counts per minute, without any identifier (`events`, spec 6.6).
#[derive(Clone, Copy)]
pub(crate) enum Event {
    /// A Yo accepted into an inbox.
    YoIn,
    /// A Yo a device fetched and acknowledged.
    YoOut,
    /// A blob uploaded.
    MediaIn,
    /// A blob fetched, each fetch handed on to its end.
    MediaOut,
    /// A group created.
    GroupCreate,
    /// A Yo accepted into a bot's inbox (every device of the recipient a bot's, `db::is_bot_app`). Who sent it the
    /// hub does not know (sealed), so a bot's Yo to a person is a `yo-in`.
    YoInBot,
    /// The bytes of the blobs uploaded (counted with each `MediaIn`).
    MediaInBytes,
    /// A Yo acknowledged by a device within a second of its being accepted (`latency`).
    Latency1s,
    /// Within five seconds.
    Latency5s,
    /// Within a minute.
    Latency60s,
    /// Later.
    LatencySlow,
    /// A push the service took (APNs or FCM answered 200).
    PushOk,
    /// A push that failed (an error answer, or none); a dead token is not counted, it is forgotten.
    PushFail,
    /// Every answer to a request, and those of a class: 4xx but 429, 429 apart, 5xx.
    Http,
    Http4xx,
    Http429,
    Http5xx,
    /// An account deleted (by its device, or by the operator).
    AccountGone,
}

impl Event {
    /// The kinds as stored, in the order of the variants.
    const KINDS: [&str; 18] = [
        "yo-in",
        "yo-out",
        "media-in",
        "media-out",
        "group-create",
        "yo-in-bot",
        "media-in-bytes",
        "latency-1s",
        "latency-5s",
        "latency-60s",
        "latency-slow",
        "push-ok",
        "push-fail",
        "http",
        "http-4xx",
        "http-429",
        "http-5xx",
        "account-gone",
    ];

    /// The latency bucket of a Yo acknowledged `ms` after it was accepted.
    pub(crate) fn latency(ms: u64) -> Event {
        match ms {
            0..1_000 => Event::Latency1s,
            1_000..5_000 => Event::Latency5s,
            5_000..60_000 => Event::Latency60s,
            _ => Event::LatencySlow,
        }
    }

    /// The class of an answer's status, beside `Http` (which counts them all).
    pub(crate) fn http(status: u16) -> Option<Event> {
        match status {
            429 => Some(Event::Http429),
            400..=428 | 430..=499 => Some(Event::Http4xx),
            500..=599 => Some(Event::Http5xx),
            _ => None,
        }
    }
}

pub struct Hub {
    config: Config,
    db: Arc<Db>,
    blobs: Arc<blobs::Blobs>,
    pusher: push::Pusher,
    /// Signs the devices' sender certificates (spec 6.1).
    signer: certificate::Signer,
    /// The zkgroup keys of its groups (spec 4.9).
    zk: groups::Zk,
    /// To the writer, which stores deliveries in group commits (`mail::writer`).
    deliveries: std::sync::mpsc::SyncSender<Delivery>,
    /// Events since the last flush, a count per kind only (6.6): into `events` (and `yo-in`, `yo-in-bot` also into
    /// `deliveries_hourly`) once a minute.
    events: [AtomicU64; Event::KINDS.len()],
    /// Each Yo (`yo-in`, `media-in`; bots left out) the moment it is counted, for the operator's live strip
    /// (`/v1/admin/live`): its kind only, nothing of who. Bounded: a stream that falls behind skips what it missed.
    live: tokio::sync::broadcast::Sender<Event>,
    /// Set when the hub shuts down: the live streams end, so a graceful shutdown does not wait for them.
    closing: tokio::sync::watch::Sender<bool>,
    /// Long polls waiting for mail, by device.
    waiting: Mutex<HashMap<DeviceId, Arc<Notify>>>,
    /// Inbox requests under way, by device (`INBOX_AT_ONCE`).
    fetching: Arc<Places>,
    limiter: Limiter,
    /// Requests' buckets (spec 4.10), by the hub's clock as tests move it.
    requests: Requests,
    replay: ReplayCache,
    /// The database disk's last answer (`db_room`): when (ms), and whether it was too full.
    disk_asked: AtomicU64,
    disk_full: AtomicBool,
    /// Places for work off the async workers (`Hub::blocking`).
    blocking: Arc<Semaphore>,
    /// Requests whose small body is being read and handled at once (`admit`).
    reading: Arc<Semaphore>,
    /// How far tests move the clock of sender certificates and of envelopes' arrival.
    shifted_ms: AtomicI64,
}

/// Yos a live stream may fall behind by before it skips them (`Hub::live`).
const LIVE_QUEUE: usize = 256;

/// Blocking work at once (`Hub::blocking`): the database is one connection, more would only wait for it.
const BLOCKING_AT_ONCE: usize = 32;

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

impl Hub {
    pub fn new(config: Config, push_transport: Arc<dyn push::Transport>) -> Result<Arc<Self>, String> {
        let db = Arc::new(Db::open(&config.db_path).map_err(|e| format!("database: {e}"))?);
        if !config.reserved.is_empty() {
            db.add_reserved(&config.reserved, now_ms()).map_err(|e| format!("database: {e}"))?;
        }
        let pusher = push::Pusher::new(push_transport, config.apns.as_ref(), config.fcm.as_ref())?;
        let blobs = blobs::Blobs::open(&config.blobs).map_err(|e| format!("{}: {e}", config.blobs.dir.display()))?;
        // What a crash left under way: fetches (given back) and blobs whose file was being moved into place.
        for blob in db.recover_blobs().map_err(|e| format!("database: {e}"))? {
            blobs.remove(&blob);
        }
        let signer = certificate::Signer::from_bytes(&config.server_certificate)
            .map_err(|e| format!("server certificate: {e}"))?;
        let zk = groups::Zk::from_bytes(&config.zk_params).map_err(|e| format!("zk params: {e}"))?;
        let (deliveries, queue) = std::sync::mpsc::sync_channel(mail::QUEUED);
        let limits = &config.limits;
        let requests = Requests::new(
            limits.requests_per_pair,
            limits.request_recipients_per_sender,
            limits.requests_per_recipient,
            limits.request_hour_ms,
        );
        let hub = Arc::new(Hub {
            db,
            blobs: Arc::new(blobs),
            pusher,
            signer,
            zk,
            deliveries,
            events: Default::default(),
            live: tokio::sync::broadcast::channel(LIVE_QUEUE).0,
            closing: tokio::sync::watch::channel(false).0,
            waiting: Mutex::default(),
            fetching: Arc::default(),
            limiter: Limiter::default(),
            requests,
            replay: ReplayCache::default(),
            disk_asked: AtomicU64::new(0),
            disk_full: AtomicBool::new(false),
            blocking: Arc::new(Semaphore::new(BLOCKING_AT_ONCE)),
            reading: Arc::new(Semaphore::new(READING_AT_ONCE)),
            shifted_ms: AtomicI64::new(0),
            config,
        });
        // The writer ends with the hub: when the last sender of its queue goes.
        let (db, most, woken) = (hub.db.clone(), hub.config.limits.max_pending_per_device, Arc::downgrade(&hub));
        std::thread::Builder::new()
            .name("yo-hub-writer".into())
            .spawn(move || mail::writer(db, queue, most, woken))
            .map_err(|e| format!("writer: {e}"))?;
        Ok(hub)
    }

    /// Every route (spec 6.3). Those that only work the database run off the async workers (`off_workers`); those
    /// that wait (a long poll, the writer, a blob's bytes) run their database work through `Hub::blocking`.
    pub fn router(self: &Arc<Self>) -> Router {
        let synchronous = Router::new()
            .route(api::REGISTER, post(account::post_register))
            .route("/v1/username/{name}", get(account::get_username))
            .route(api::USERNAME, put(account::put_username))
            .route(api::PREKEYS, put(account::put_prekeys))
            .route(api::LOOKUP, post(account::post_lookup))
            .route(api::CERTIFICATE, get(account::get_certificate))
            .route(api::PUSH, put(account::put_push))
            .route(api::ACCESS_KEY, put(account::put_access_key))
            .route(api::ACCOUNT, delete(account::delete_account))
            .route(api::STORAGE, get(account::get_storage).put(account::put_storage))
            .route(api::BACKUP, put(account::put_backup))
            .route("/v1/backup/{id}", get(account::get_backup))
            .route(api::INBOX_ACK, post(mail::post_ack))
            .route(api::GROUP_AUTH, get(groups::get_group_auth))
            .route(api::GROUP_CREDENTIAL, post(groups::post_group_credential))
            .route("/v1/groups/{id}", put(groups::put_group).post(groups::post_group))
            .route(api::HELP_MESSAGES, get(help::get_messages).post(help::post_message))
            .route(api::HELP_READ, post(help::post_read))
            .route(api::HELP_REPORT, post(help::post_report))
            .route(api::HELP_CRASH, post(help::post_crash))
            .route(api::HELP_ERROR, post(help::post_error))
            .route(hubsig::KEY_PATH, get(hub_key))
            .route(api::TRUST_ROOT_PATH, get(trust_root))
            .route(api::ZK_PARAMS_PATH, get(zk_params))
            .merge(admin::routes(self))
            .route_layer(from_fn_with_state(self.clone(), off_workers))
            .route_layer(from_fn_with_state(self.clone(), admit));
        let delivering = Router::new()
            .route("/v1/inbox/{user}", post(mail::post_envelope))
            .route_layer(from_fn_with_state(self.clone(), admit));
        let mut router = Router::new()
            .route("/", get(|| async { axum::response::Html(include_str!("index.html")) }))
            .route(api::INBOX, get(mail::get_inbox))
            .route("/v1/blobs/{id}", put(blobs::put_blob).get(blobs::get_blob).delete(blobs::delete_blob))
            .merge(delivering)
            .merge(synchronous)
            .merge(admin::live_route(self));
        if !self.config.apple_app_ids.is_empty() {
            // Which apps may make passkeys for this domain (spec 7).
            let file = serde_json::json!({ "webcredentials": { "apps": self.config.apple_app_ids } }).to_string();
            let answer = move || async move { ([(CONTENT_TYPE, "application/json")], file.clone()) };
            router = router.route("/.well-known/apple-app-site-association", get(answer));
        }
        if let Some(statements) = self.config.asset_links.clone() {
            router = router.route("/.well-known/assetlinks.json", get(move || async move { axum::Json(statements) }));
        }
        router
            .layer(from_fn_with_state(self.clone(), too_old))
            .layer(from_fn_with_state(self.clone(), request::counted))
            .layer(DefaultBodyLimit::max(limits::MAX_BODY))
            .with_state(self.clone())
    }

    /// Serves until `shutdown` resolves; cleans up meanwhile.
    pub async fn serve(
        self: Arc<Self>,
        listener: tokio::net::TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        let pruner = tokio::spawn(self.clone().every_minute());
        let app = self.router().into_make_service_with_connect_info::<SocketAddr>();
        let hub = self.clone();
        let shutdown = async move {
            shutdown.await;
            hub.closing.send_replace(true);
        };
        let served = axum::serve(listener, app).with_graceful_shutdown(shutdown).await;
        pruner.abort();
        served
    }

    async fn every_minute(self: Arc<Self>) {
        let mut every = tokio::time::interval(MINUTE);
        let mut minutes = 0u64;
        loop {
            every.tick().await;
            self.replay.forget_expired(now_ms());
            // The minute's events, a count per kind (nothing of who or what): the operator's page.
            let _ = self.blocking(|hub| hub.flush_events(now_ms())).await;
            if minutes.is_multiple_of(60) {
                self.limiter.forget_idle(DAY);
                self.requests.forget_full(self.clock_ms());
                // Off the request threads: the sweep lists every blob file.
                let hub = self.clone();
                match tokio::task::spawn_blocking(move || hub.prune(now_ms())).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => eprintln!("yo-hub: prune: {error}"),
                    Err(error) => eprintln!("yo-hub: prune: {error}"),
                }
            }
            minutes += 1;
        }
    }

    /// One more event of a kind, counted in memory until the next minute's flush.
    pub(crate) fn count(&self, event: Event, n: u64) {
        self.events[event as usize].fetch_add(n, Ordering::Relaxed);
        if matches!(event, Event::YoIn | Event::MediaIn) && self.live.receiver_count() > 0 {
            for _ in 0..n.min(LIVE_QUEUE as u64) {
                let _ = self.live.send(event);
            }
        }
    }

    /// Yos acknowledged at `now`, accepted at `accepted` (ms): `yo-out`, and how long each took in latency buckets.
    pub(crate) fn count_acked(&self, accepted: &[u64], now: u64) {
        self.count(Event::YoOut, accepted.len() as u64);
        for at in accepted {
            self.count(Event::latency(now.saturating_sub(*at)), 1);
        }
    }

    /// A blob of `size` bytes uploaded: one more `media-in`, and its bytes into `media-in-bytes`.
    pub(crate) fn count_upload(&self, size: u64) {
        self.count(Event::MediaIn, 1);
        self.count(Event::MediaInBytes, size);
    }

    /// The events counted since the last flush into `events` at `now`'s minute; deliveries also into their hour.
    pub fn flush_events(&self, now: u64) -> rusqlite::Result<()> {
        let counts: Vec<_> = Event::KINDS
            .iter()
            .zip(&self.events)
            .map(|(kind, n)| (*kind, n.swap(0, Ordering::Relaxed)))
            .filter(|(_, n)| *n > 0)
            .collect();
        // Deliveries per hour count every Yo, a bot's too.
        let delivered: u64 = counts
            .iter()
            .filter(|(kind, _)| [Event::YoIn, Event::YoInBot].iter().any(|e| *kind == Event::KINDS[*e as usize]))
            .map(|(_, n)| n)
            .sum();
        if delivered > 0 {
            self.db.count_deliveries(now, delivered)?;
        }
        self.db.count_events(now, &counts)
    }

    /// The hourly cleanup at `now`: envelopes past their time (6.6), blobs past theirs and files no blob owns (6.8).
    pub fn prune(&self, now: u64) -> rusqlite::Result<()> {
        let limits = &self.config.limits;
        self.db.prune(now, limits.envelope_ttl_days * DAY_MS)?;
        for blob in self.db.prune_blobs(now, limits.blob_ttl_days * DAY_MS)? {
            self.blobs.remove(&blob);
        }
        match self.blobs.sweep(&self.db) {
            Ok(0) => {}
            Ok(n) => eprintln!("yo-hub: {n} stray blob files removed"),
            Err(error) => eprintln!("yo-hub: blob sweep in {}: {error}", self.blobs.dir().display()),
        }
        Ok(())
    }

    /// What should never be (`Db::audit`), and blobs whose file is missing: for tests and the operator; reads only.
    pub fn audit(&self) -> rusqlite::Result<Vec<(&'static str, i64)>> {
        let mut failed = self.db.audit()?;
        let missing = self.db.blob_ids()?.into_iter().filter(|blob| !self.blobs.has(blob)).count() as i64;
        if missing > 0 {
            failed.push(("blobs without their file", missing));
        }
        Ok(failed)
    }

    /// Devices with a long poll waiting, and with inbox requests under way: for tests and the operator.
    pub fn under_way(&self) -> (usize, usize) {
        let waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner).len();
        (waiting, self.fetching.lock().unwrap_or_else(PoisonError::into_inner).len())
    }

    /// How many devices name each app (`Yo-App`; "" for none): for tests and the operator.
    pub fn apps(&self) -> rusqlite::Result<Vec<(String, i64)>> {
        self.db.apps()
    }

    /// For tests: the clock of sender certificates and of envelopes' arrival moved by `ms` (days gone by, or back).
    pub fn shift_clock(&self, ms: i64) {
        self.shifted_ms.store(ms, Ordering::Relaxed);
    }

    /// The hub's clock as tests move it (`shift_clock`), ms.
    fn clock_ms(&self) -> u64 {
        now_ms().saturating_add_signed(self.shifted_ms.load(Ordering::Relaxed))
    }

    /// Runs `f` on a blocking thread, `BLOCKING_AT_ONCE` at most; it runs to its end even if the request is cut off.
    async fn blocking<T: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&Hub) -> T + Send + 'static,
    ) -> error::Res<T> {
        use axum::http::StatusCode;
        let place = self.blocking.clone().acquire_owned().await;
        let place = place.map_err(|_| error::refuse(StatusCode::SERVICE_UNAVAILABLE))?;
        let hub = self.clone();
        let done = tokio::task::spawn_blocking(move || {
            let _place = place;
            f(&hub)
        });
        done.await.map_err(|_| error::refuse(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

// ---- the keys apps pin: for tools, apps ship their own copies ----

/// `GET /.well-known/yo-hub-key` (spec 6.10).
async fn hub_key(State(hub): Hubs) -> Vec<u8> {
    config::hub_public_key(&hub.config.hub_key).to_vec()
}

/// `GET /.well-known/yo-trust-root` (spec 6.1).
async fn trust_root(State(hub): Hubs) -> Vec<u8> {
    hub.signer.root().to_vec()
}

/// `GET` the zkgroup public params (spec 4.9).
async fn zk_params(State(hub): Hubs) -> Vec<u8> {
    hub.zk.public().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn asset_links_are_served_only_when_configured() {
        use axum::body::{to_bytes, Body};
        use axum::http::Request;
        use tower::ServiceExt;

        let statements = vec![serde_json::json!({
            "relation": ["delegate_permission/common.get_login_creds"],
            "target": { "namespace": "android_app", "package_name": "app.example" }
        })];
        for asset_links in [None, Some(statements.clone())] {
            let configured = asset_links.is_some();
            let hub =
                Hub::new(Config { asset_links, ..Config::for_tests() }, Arc::new(push::Recorder::default())).unwrap();
            let response = hub
                .router()
                .oneshot(Request::get("/.well-known/assetlinks.json").body(Body::empty()).unwrap())
                .await
                .unwrap();
            if configured {
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(response.headers()[axum::http::header::CONTENT_TYPE], "application/json");
                let body = to_bytes(response.into_body(), 4096).await.unwrap();
                assert_eq!(serde_json::from_slice::<Vec<serde_json::Value>>(&body).unwrap(), statements);
            } else {
                assert_eq!(response.status(), StatusCode::NOT_FOUND);
            }
        }
    }

    #[tokio::test]
    async fn admin_user_deletion_requires_admin_and_removes_the_account() {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;
        let mut config = Config::for_tests();
        config.admin_token = Some("test-token".into());
        let hub = Hub::new(config, Arc::new(push::Recorder::default())).unwrap();
        let user = [7; 16];
        let device = db::NewDevice {
            device_id: [8; 16],
            device_key: [8; 32],
            auth_key: [8; 32],
            registration_id: 8,
            push_platform: 0,
            push_token: Vec::new(),
            app: None,
        };
        let prekeys = api::Prekeys { signed: None, last_resort: None, one_time: Vec::new(), kyber: Vec::new() };
        assert!(matches!(
            hub.db.register(b"deletable", "Deletable", &user, &[7; 32], &device, &prekeys, 100, 1).unwrap(),
            Write::Done(_)
        ));
        let path = format!("/v1/admin/users/{}", "07".repeat(16));
        for (token, target, expected) in [
            ("", path.as_str(), StatusCode::NOT_FOUND),
            ("wrong", path.as_str(), StatusCode::NOT_FOUND),
            ("test-token", "/v1/admin/users/invalid", StatusCode::BAD_REQUEST),
            ("test-token", path.as_str(), StatusCode::NO_CONTENT),
            ("test-token", path.as_str(), StatusCode::NOT_FOUND),
        ] {
            let request = Request::delete(target).header("yo-admin-token", token).body(Body::empty()).unwrap();
            let response = hub.router().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected);
            if token != "test-token" || target.ends_with("invalid") {
                assert!(hub.db.account_key(&user).unwrap().is_some());
            }
        }
        assert!(hub.db.account_key(&user).unwrap().is_none());
        assert!(!hub.db.username_taken(b"deletable").unwrap());
        hub.flush_events(120_000).unwrap();
        let gone: i64 =
            hub.db.conn().query_row("SELECT sum(n) FROM events WHERE kind = 'account-gone'", [], |r| r.get(0)).unwrap();
        assert_eq!(gone, 1, "counted once, not for the refused ones");
        assert!(matches!(hub.db.delete_account(&device.device_id, 2).unwrap(), Write::Refused(_)));
        let mut config = Config::for_tests();
        config.admin_token = None;
        let disabled = Hub::new(config, Arc::new(push::Recorder::default())).unwrap();
        let response = disabled
            .router()
            .oneshot(Request::delete(path).header("yo-admin-token", "test-token").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn admin_reserved_routes_authenticate_validate_and_manage_names() {
        use axum::{
            body::{to_bytes, Body},
            http::Request,
        };
        use tower::ServiceExt;
        for enabled in [false, true] {
            let mut config = Config::for_tests();
            config.admin_token = enabled.then(|| "test-token".into());
            config.name_min = 3;
            let hub = Hub::new(config, Arc::new(push::Recorder::default())).unwrap();
            for (method, path, body, expected) in [
                ("GET", "/v1/admin/reserved", "", StatusCode::OK),
                ("POST", "/v1/admin/reserved", "[\"Admin\",\"Adm1n\",\"Support\"]", StatusCode::OK),
                ("POST", "/v1/admin/reserved", "{}", StatusCode::BAD_REQUEST),
                ("POST", "/v1/admin/reserved", "[\" \"]", StatusCode::BAD_REQUEST),
                ("POST", "/v1/admin/reserved", "[{\"name\":\"Support\",\"note\":\"role\"}]", StatusCode::OK),
                ("POST", "/v1/admin/reserved", "[{\"name\":\"Support\",\"note\":1}]", StatusCode::BAD_REQUEST),
                ("POST", "/v1/admin/reserved", "[{\"name\":\"Support\",\"x\":1}]", StatusCode::BAD_REQUEST),
                ("POST", "/v1/admin/reserved", "[{\"note\":\"role\"}]", StatusCode::BAD_REQUEST),
                ("POST", "/v1/admin/reserved", "[1]", StatusCode::BAD_REQUEST),
                ("PUT", "/v1/admin/reserved", "{\"enabled\":false}", StatusCode::OK),
                ("PUT", "/v1/admin/reserved", "{\"enabled\":1}", StatusCode::BAD_REQUEST),
                ("PUT", "/v1/admin/reserved", "{\"enabled\":true,\"extra\":1}", StatusCode::BAD_REQUEST),
                ("DELETE", "/v1/admin/reserved/Adm1n", "", StatusCode::NO_CONTENT),
                ("PUT", "/v1/admin/reserved", "{\"enabled\":true}", StatusCode::OK),
            ] {
                for token in ["", "wrong", "test-token"] {
                    let request = Request::builder()
                        .method(method)
                        .uri(path)
                        .header("yo-admin-token", token)
                        .body(Body::from(body))
                        .unwrap();
                    let response = hub.router().oneshot(request).await.unwrap();
                    assert_eq!(
                        response.status(),
                        if enabled && token == "test-token" { expected } else { StatusCode::NOT_FOUND },
                    );
                    if enabled && token == "test-token" && method == "GET" {
                        let body = to_bytes(response.into_body(), 65536).await.unwrap();
                        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
                        assert_eq!(value["name_min"], 3);
                        assert_eq!(value["name_max"], yo_wire::handle::USERNAME_MAX);
                        assert_eq!(value["name_comparison"], "skeleton");
                        assert!(value["name_form"].as_str().unwrap().contains("NFC"));
                        continue;
                    }
                    if enabled && token == "test-token" && method == "POST" && expected == StatusCode::OK {
                        let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
                        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                        assert_eq!(value["names"], serde_json::json!(["Admin", "Support"]));
                        let notes = if body.contains("note") {
                            serde_json::json!({"Support": "role"})
                        } else {
                            serde_json::json!({})
                        };
                        assert_eq!(value["notes"], notes);
                    }
                    if enabled && token == "test-token" && method == "PUT" && body == "{\"enabled\":false}" {
                        assert!(!hub.db.username_taken(b"ADMIN").unwrap());
                        assert_eq!(hub.db.reserved_state().unwrap()["names"].as_array().unwrap().len(), 2);
                    }
                }
            }
            if enabled {
                assert_eq!(
                    hub.db.reserved_state().unwrap(),
                    serde_json::json!({"enabled":true,"names":["Support"],"notes":{"Support":"role"}}),
                );
            }
        }
    }

    #[tokio::test]
    async fn admin_todos_require_a_token_and_validate_text() {
        use axum::{
            body::{to_bytes, Body},
            http::Request,
        };
        use tower::ServiceExt;
        let path = std::env::temp_dir().join(format!("yo-todo-{}-{}.sqlite", std::process::id(), now_ms()));
        let mut config = Config { db_path: path.to_string_lossy().into(), ..Config::for_tests() };
        config.limits.todo_per_ip_day = 5;
        for enabled in [false, true] {
            config.admin_token = enabled.then(|| "test-token".into());
            let hub = Hub::new(config.clone(), Arc::new(push::Recorder::default())).unwrap();
            for (token, body, expected) in [
                ("", b"fix".to_vec(), StatusCode::NOT_FOUND),
                ("wrong", b"fix".to_vec(), StatusCode::NOT_FOUND),
                ("test-token", vec![], StatusCode::BAD_REQUEST),
                ("test-token", vec![255], StatusCode::BAD_REQUEST),
                ("test-token", vec![b'a'; 4001], StatusCode::BAD_REQUEST),
                ("test-token", "fix 🎨".as_bytes().to_vec(), StatusCode::CREATED),
                ("test-token", vec![b'a'; 4000], StatusCode::CREATED),
                ("test-token", b"limit".to_vec(), StatusCode::TOO_MANY_REQUESTS),
            ] {
                let mut request =
                    Request::post("/v1/admin/todos").header("yo-admin-token", token).body(Body::from(body)).unwrap();
                request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9))));
                let response = hub.router().oneshot(request).await.unwrap();
                assert_eq!(response.status(), if enabled { expected } else { StatusCode::NOT_FOUND });
                if response.status() == StatusCode::CREATED {
                    let body = to_bytes(response.into_body(), 1024).await.unwrap();
                    let id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["id"].as_i64().unwrap();
                    let conn = rusqlite::Connection::open(&path).unwrap();
                    let kind: String =
                        conn.query_row("SELECT text FROM todos WHERE id = ?1", [id], |r| r.get(0)).unwrap();
                    assert!(!kind.is_empty());
                }
            }
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_registration_id_out_of_range_or_an_auth_key_off_the_curve_is_refused() {
        let good = config::hub_public_key(&[3; 32]);
        assert!(Hub::device_keys_usable(1, &good) && Hub::device_keys_usable(0x3fff, &good));
        assert!(!Hub::device_keys_usable(0, &good) && !Hub::device_keys_usable(0x4000, &good));
        // Not canonical: its top bit set.
        let mut off = good;
        off[31] |= 0x80;
        assert!(!Hub::device_keys_usable(1, &off), "not canonical");
    }

    /// The events table's rows, `(kind, n)` summed over the minutes, by kind.
    fn counted(hub: &Hub) -> Vec<(String, i64)> {
        hub.flush_events(120_000).unwrap();
        hub.db
            .conn()
            .prepare("SELECT kind, sum(n) FROM events GROUP BY kind ORDER BY kind")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn kinds(rows: &[(&str, i64)]) -> Vec<(String, i64)> {
        rows.iter().map(|(k, n)| (k.to_string(), *n)).collect()
    }

    #[test]
    fn acknowledged_yos_count_in_latency_buckets() {
        let hub = Hub::new(Config::for_tests(), Arc::new(push::Recorder::default())).unwrap();
        let now = 1_000_000;
        hub.count_acked(&[now - 10, now - 999, now - 1_000, now - 4_999, now - 59_999, now - 60_000, now + 5], now);
        let rows = [("latency-1s", 3), ("latency-5s", 2), ("latency-60s", 1), ("latency-slow", 1), ("yo-out", 7)];
        assert_eq!(counted(&hub), kinds(&rows), "a clock behind counts as at once");
    }

    #[tokio::test]
    async fn answers_are_counted_by_class() {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;
        let hub = Hub::new(Config::for_tests(), Arc::new(push::Recorder::default())).unwrap();
        for path in ["/", "/nowhere", "/v1/inbox/ack"] {
            hub.router().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
        }
        assert_eq!(counted(&hub), kinds(&[("http", 3), ("http-4xx", 2)]));
        assert_eq!(Event::http(429).map(|e| Event::KINDS[e as usize]), Some("http-429"));
        assert_eq!(Event::http(503).map(|e| Event::KINDS[e as usize]), Some("http-5xx"));
        assert!(Event::http(204).is_none() && Event::http(304).is_none());
    }

    /// A push the service refuses: counted, and an error of the hub's own in diagnostics (domain "push", by reason and service,
    /// nothing of the device); one it takes is counted as such; a dead token is neither.
    #[tokio::test]
    async fn pushes_are_counted_and_a_failed_one_is_a_diagnostic() {
        let apns = config::ApnsConfig {
            key_pem: include_str!("../../tests/fixtures/test-apns.p8").into(),
            key_id: "KEYID12345".into(),
            team_id: "TEAMID1234".into(),
            topic: "co.justyo.app".into(),
        };
        let recorder = Arc::new(push::Recorder::default());
        let hub = Hub::new(Config { apns: Some(apns), ..Config::for_tests() }, recorder.clone()).unwrap();
        let wake = || db::Wake {
            device_id: [1; 16],
            push_platform: api::PUSH_APNS,
            push_token: b"a1b2c3".to_vec(),
            bot: false,
        };
        hub.clone().push(wake(), true).await;
        *recorder.device_status.lock().unwrap() = Some(500);
        hub.clone().push(wake(), true).await;
        hub.clone().push(wake(), false).await;
        *recorder.device_status.lock().unwrap() = Some(410);
        hub.clone().push(wake(), true).await;
        assert_eq!(counted(&hub), kinds(&[("push-fail", 2), ("push-ok", 1)]));
        let diagnostics = hub.db.diagnostics().unwrap();
        let rows = diagnostics.as_array().unwrap();
        assert_eq!(rows.len(), 1, "one entry for the same service and reason");
        assert_eq!((rows[0]["kind"].as_str(), rows[0]["count"].as_i64()), (Some("error"), Some(2)));
        assert_eq!(rows[0]["app"].as_str(), Some("hub"));
        let payload: serde_json::Value = serde_json::from_str(rows[0]["payload"].as_str().unwrap()).unwrap();
        assert_eq!((payload["domain"].as_str(), payload["code"].as_str()), (Some("push"), Some("500")));
        assert_eq!(payload["place"].as_str(), Some("APNs"));
        assert!(!rows[0]["payload"].as_str().unwrap().contains("a1b2c3"), "no token");
    }

    #[test]
    fn an_upload_counts_one_blob_and_its_bytes() {
        let hub = Hub::new(Config::for_tests(), Arc::new(push::Recorder::default())).unwrap();
        hub.count_upload(1000);
        hub.count_upload(24);
        hub.flush_events(120_000).unwrap();
        let counted: Vec<(String, i64, i64)> = hub
            .db
            .conn()
            .prepare("SELECT kind, minute, n FROM events ORDER BY kind")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(counted, [("media-in".to_owned(), 2, 2), ("media-in-bytes".to_owned(), 2, 1024)]);
    }

    #[test]
    fn a_device_has_a_few_inbox_requests_under_way_at_most() {
        let hub = Hub::new(Config::for_tests(), Arc::new(push::Recorder::default())).unwrap();
        let held: Vec<_> = (0..INBOX_AT_ONCE).map(|_| hub.fetching([1; 16]).expect("a place")).collect();
        assert!(hub.fetching([1; 16]).is_none(), "one more is refused");
        assert!(hub.fetching([2; 16]).is_some(), "another device has its own");
        drop(held);
        assert!(hub.fetching([1; 16]).is_some(), "places are given back");
        assert!(hub.fetching.lock().unwrap().is_empty(), "and forgotten");
    }

    /// A crash between a blob's row and its being ready (the upload's last steps, done here by hand): the next start
    /// removes it, row and file, and the upload's retry stores it anew instead of finding its id taken.
    #[test]
    fn a_blob_left_not_ready_by_a_crash_is_gone_at_the_next_start() {
        let dir = std::env::temp_dir().join(format!("yo-hub-crash-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = Config { db_path: dir.join("hub.sqlite").to_string_lossy().into(), ..Config::for_tests() };
        let hub = Hub::new(config.clone(), Arc::new(push::Recorder::default())).unwrap();
        let device = NewDevice {
            device_id: [1; 16],
            device_key: [1; 32],
            auth_key: [1; 32],
            registration_id: 1,
            push_platform: 0,
            push_token: Vec::new(),
            app: None,
        };
        let prekeys = api::Prekeys { signed: None, last_resort: None, one_time: Vec::new(), kyber: Vec::new() };
        hub.db.register(b"LEO", "LEO", &[1; 16], &[1; 32], &device, &prekeys, 10, 0).unwrap();
        let (quota, blob) = (config.limits.blob_quota(), ([7; 16], 5));
        assert_eq!(hub.db.put_blob(&[1; 16], &blob.0, 16, 1, blob.1, &quota).unwrap(), db::BlobPut::Stored);
        let (file, incoming) = hub.blobs.incoming().unwrap();
        drop(file);
        std::fs::write(&incoming.path, [0; 16]).unwrap();
        hub.blobs.keep(incoming, &blob).unwrap();
        assert!(hub.db.take_blob_get(&blob.0, 5, u64::MAX).unwrap().is_none(), "not handed out before it is ready");
        drop(hub);
        let hub = Hub::new(config.clone(), Arc::new(push::Recorder::default())).unwrap();
        assert!(!hub.db.blob_exists(&blob.0).unwrap() && !hub.blobs.has(&blob));
        assert_eq!(hub.audit().unwrap(), []);
        assert_eq!(hub.db.put_blob(&[1; 16], &blob.0, 16, 1, 6, &quota).unwrap(), db::BlobPut::Stored);
        drop(hub);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&config.blobs.dir);
    }

    /// The request is cut off once its envelope is queued (nobody waits for the writer's answer): the envelope is
    /// stored, and its device is woken and pushed all the same.
    #[tokio::test]
    async fn a_delivery_whose_request_is_gone_still_wakes_and_pushes() {
        let apns = config::ApnsConfig {
            key_pem: include_str!("../../tests/fixtures/test-apns.p8").into(),
            key_id: "KEYID12345".into(),
            team_id: "TEAMID1234".into(),
            topic: "co.justyo.app".into(),
        };
        let recorder = Arc::new(push::Recorder::default());
        let hub = Hub::new(Config { apns: Some(apns), ..Config::for_tests() }, recorder.clone()).unwrap();
        let device = NewDevice {
            device_id: [1; 16],
            device_key: [1; 32],
            auth_key: [1; 32],
            registration_id: 1,
            push_platform: api::PUSH_APNS,
            push_token: b"a1b2c3".to_vec(),
            app: None,
        };
        let prekeys = api::Prekeys { signed: None, last_resort: None, one_time: Vec::new(), kyber: Vec::new() };
        hub.db.register(b"LEO", "LEO", &[1; 16], &[1; 32], &device, &prekeys, 10, 0).unwrap();
        let waiter = hub.waiter([1; 16]);
        let notified = waiter.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let (stored, outcome) = tokio::sync::oneshot::channel();
        drop(outcome);
        let mut sealed = vec![0; 1024];
        sealed[0] = yo_wire::envelope::SEALED;
        let envelope = Bytes::from(sealed);
        let devices_hash = yo_wire::devices::hash(&[1; 32], &[[1; 32]]);
        let runtime = tokio::runtime::Handle::current();
        hub.deliveries.send(Delivery { user: [1; 16], devices_hash, envelope, urgent: true, stored, runtime }).unwrap();
        tokio::time::timeout(Duration::from_secs(5), notified).await.expect("its long poll answers");
        assert_eq!(hub.db.fetch(&[1; 16], 10).unwrap().len(), 1);
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while recorder.sent.lock().unwrap().is_empty() {
            assert!(std::time::Instant::now() < until, "pushed");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(recorder.sent.lock().unwrap()[0].url.ends_with("/3/device/a1b2c3"));
    }

    #[tokio::test]
    async fn an_inbox_answer_holds_its_place_until_it_is_sent() {
        let hub = Hub::new(Config::for_tests(), Arc::new(push::Recorder::default())).unwrap();
        let answer = held(vec![7; 1000], hub.fetching([1; 16]).unwrap());
        assert_eq!(hub.under_way(), (0, 1), "made, not sent");
        let body = axum::body::to_bytes(answer.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.len(), 1000);
        assert_eq!(hub.under_way(), (0, 0), "sent");
    }
}
