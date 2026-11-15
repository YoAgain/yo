//! The Yo hub (spec 6): users and their devices, their prekeys, delivery to users, push. It holds no content key, and
//! the route that carries messages takes no identity: the hub never learns a sender.

pub mod blobs;
pub mod certificate;
pub mod config;
pub mod db;
pub mod error;
pub mod groups;
pub mod limiter;
pub mod push;

use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::Router;
use bytes::Bytes;
use config::Config;
use db::{Db, Delivered, Incoming, Key, NewDevice, Write};
use error::{refuse, Refusal, Res};
use limiter::{Limiter, ReplayCache, Requests};
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;
use yo_wire::api::{self, HEADER_BODY_HASH, HEADER_DEVICES};
use yo_wire::cert::DeviceCertificate;
use yo_wire::content::BlobId;
use yo_wire::handle::{Handle, KIND_USERNAME};
use yo_wire::hubsig;
use yo_wire::limits::{self, MAX_DEVICES, MAX_INBOX_FETCH, MAX_PREKEYS_PER_UPLOAD, PREKEY_TARGET};
use yo_wire::request::{HEADER_DEVICE, HEADER_SIGNATURE, HEADER_TIME};
use yo_wire::{decode, encode, sha256, unb64, DeviceId, UserId};

pub struct Hub {
    config: Config,
    db: Arc<Db>,
    deliveries: std::sync::mpsc::SyncSender<Delivery>,
    limiter: Limiter,
    /// Requests' buckets (spec 4.10), by the hub's clock as tests move it.
    requests: Requests,
    replay: ReplayCache,
    waiting: Mutex<HashMap<DeviceId, Arc<Notify>>>,
    /// Inbox requests under way, by device (`INBOX_AT_ONCE`).
    fetching: Mutex<HashMap<DeviceId, u32>>,
    /// The database disk's last answer (`db_room`): when it was asked (ms), and whether it was too full.
    disk_asked: AtomicU64,
    disk_full: AtomicBool,
    pusher: push::Pusher,
    blobs: Arc<blobs::Blobs>,
    /// Places for work off the async workers (`Hub::blocking`).
    blocking: Arc<tokio::sync::Semaphore>,
    /// Requests whose small body is being read and handled at once (`admit`).
    reading: Arc<tokio::sync::Semaphore>,
    /// Signs the devices' sender certificates (spec 6.1).
    signer: certificate::Signer,
    /// How far the clock of sender certificates and of envelopes' arrival is moved (tests: days gone by).
    shifted_ms: Arc<std::sync::atomic::AtomicI64>,
    /// The zkgroup keys of its groups (spec 4.9).
    zk: groups::Zk,
}

const HOUR: Duration = Duration::from_secs(3600);
const MINUTE: Duration = Duration::from_secs(60);
const DAY: Duration = Duration::from_secs(24 * 3600);

/// The hub's work on blocking threads at once (`Hub::blocking`): the database is one connection, so more would only
/// wait for it; the rest (signatures, sessions) is work for the cores.
const BLOCKING_AT_ONCE: usize = 32;

/// Requests with a body (all but a blob's, which streams) read and handled at once (`admit`): each holds up to
/// `MAX_BODY` while read; beyond, `503` before a byte of the body is read.
pub const READING_AT_ONCE: usize = 512;

/// The time a body may take to arrive: a client that sends it slower holds a place for nothing (`408`).
const BODY_DEADLINE: Duration = Duration::from_secs(20);

/// Inbox requests of one device at once: the app, its notification extension, a share. One answer holds up to
/// `MAX_INBOX_FETCH` envelopes of 64 KiB, and a wait holds a connection for a minute: a device that asks more often
/// at once is refused (`429`) rather than let it hold the hub's memory and connections.
pub const INBOX_AT_ONCE: u32 = 4;

/// An inbox request under way; its place is given back when dropped (`held`: once its answer has been sent).
struct Fetching {
    hub: Arc<Hub>,
    device: DeviceId,
}

impl Drop for Fetching {
    fn drop(&mut self) {
        let mut fetching = self.hub.fetching.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = fetching.get_mut(&self.device) {
            *n -= 1;
            if *n == 0 {
                fetching.remove(&self.device);
            }
        }
    }
}

/// A long poll waiting for mail (`Hub::waiter`).
struct Waiter<'a> {
    hub: &'a Hub,
    device: DeviceId,
    notify: Arc<Notify>,
}

impl Drop for Waiter<'_> {
    fn drop(&mut self) {
        let mut waiting = self.hub.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        // Two references: the map's and ours. Nobody else waits.
        if Arc::strong_count(&self.notify) == 2 {
            waiting.remove(&self.device);
        }
    }
}

impl Hub {
    /// For tests: the clock of sender certificates and of envelopes' arrival moved by `ms` (days gone by, or back).
    pub fn shift_clock(&self, ms: i64) {
        self.shifted_ms.store(ms, Ordering::Relaxed);
    }

    /// The hub's clock as tests move it (`shift_clock`), ms.
    fn clock_ms(&self) -> u64 {
        now_ms().saturating_add_signed(self.shifted_ms.load(Ordering::Relaxed))
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn ok(body: Vec<u8>) -> (StatusCode, Vec<u8>) {
    (StatusCode::OK, body)
}

fn bad() -> Refusal {
    refuse(StatusCode::BAD_REQUEST)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// The time and signature of a signed request (spec 6.2): both there, and the time within the window of `now`.
fn signature_of(headers: &HeaderMap, now: u64) -> Option<(u64, [u8; 64])> {
    let time: u64 = header(headers, HEADER_TIME)?.parse().ok()?;
    let signature = unb64(header(headers, HEADER_SIGNATURE)?).ok()?.try_into().ok()?;
    (now.abs_diff(time) <= limits::MAX_SKEW_MS).then_some((time, signature))
}

/// The app a request names (`Yo-App`, spec 6.3), if it is one.
fn named_app(headers: &HeaderMap) -> Option<&str> {
    header(headers, api::HEADER_APP).filter(|a| config::app(a).is_some())
}

/// An answer whose body holds `guard` until it has been sent, or cut off: what it bounds (memory, connections) is
/// not free before.
fn held<G: Send + 'static>(body: Vec<u8>, guard: G) -> axum::response::Response {
    use futures_util::StreamExt;
    let len = body.len();
    let chunk = futures_util::stream::iter([Ok::<_, std::convert::Infallible>(Bytes::from(body))]);
    let body = chunk.map(move |chunk| {
        let _held = &guard;
        chunk
    });
    axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
        .header(axum::http::header::CONTENT_LENGTH, len)
        .body(axum::body::Body::from_stream(body))
        .unwrap_or_default()
}

/// An id in a path: base64url of exactly `N` bytes, or the route does not exist.
fn path_id<const N: usize>(text: &str) -> Res<[u8; N]> {
    unb64(text).ok().and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| refuse(StatusCode::NOT_FOUND))
}

/// An envelope on its way to the writer, which stores many in one transaction.
struct Delivery {
    user: UserId,
    devices_hash: [u8; 32],
    envelope: Bytes,
    stored: tokio::sync::oneshot::Sender<Result<Delivered, ()>>,
    /// Where its pushes are sent from.
    runtime: tokio::runtime::Handle,
}

/// The most envelopes one transaction stores.
const BATCH: usize = 512;

/// The most deliveries waiting for the writer (each holds its envelope, up to 64 KiB): beyond, `503`, sent again
/// later, rather than the hub's memory growing while the database is slow.
const QUEUED: usize = 2048;

/// Group commit: takes what has queued up, stores it in one transaction, wakes the devices and answers each. Under
/// load the batches grow by themselves; alone, an envelope is stored at once.
fn writer(
    db: Arc<Db>,
    queue: std::sync::mpsc::Receiver<Delivery>,
    most: u32,
    hub: Weak<Hub>,
    shifted_ms: Arc<std::sync::atomic::AtomicI64>,
) {
    while let Ok(first) = queue.recv() {
        let mut batch = vec![first];
        while batch.len() < BATCH {
            match queue.try_recv() {
                Ok(next) => batch.push(next),
                Err(_) => break,
            }
        }
        let items: Vec<Incoming<'_>> = batch
            .iter()
            .map(|d| Incoming { user: d.user, devices_hash: d.devices_hash, envelope: &d.envelope })
            .collect();
        let now = now_ms().saturating_add_signed(shifted_ms.load(Ordering::Relaxed));
        match db.deliver(&items, now, most) {
            Ok(outcomes) => {
                let hub = hub.upgrade();
                for (delivery, mut outcome) in batch.into_iter().zip(outcomes) {
                    // Here, not in the request: one cut off after its envelope was stored still wakes and pushes.
                    if let (Delivered::Stored(devices), Some(hub)) = (&mut outcome, &hub) {
                        hub.woken(std::mem::take(devices), &delivery.runtime);
                    }
                    let _ = delivery.stored.send(Ok(outcome));
                }
            }
            Err(error) => {
                eprintln!("yo-hub: database: {error}");
                for delivery in batch {
                    let _ = delivery.stored.send(Err(()));
                }
            }
        }
    }
}

/// A device that signed its request (spec 6.2).
struct Signed {
    device_id: DeviceId,
    user_id: UserId,
}

/// An app older than the hub's minimum for its platform (`YO_MIN_APP`) gets `426` on every request: it shows that it
/// must be updated (spec 6.3). Without `Yo-App`, or with a platform the hub has no minimum for, it passes.
async fn too_old(
    State(hub): State<Arc<Hub>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let app = request.headers().get(api::HEADER_APP).and_then(|v| v.to_str().ok()).and_then(config::app);
    if let Some((platform, build)) = app {
        if hub.config.min_app.iter().any(|(p, min)| *p == platform && build < *min) {
            return StatusCode::UPGRADE_REQUIRED.into_response();
        }
    }
    next.run(request).await
}

/// A route that is synchronous through and through (`Hub::router`), run off the async workers: its body is taken
/// here first, so a slow client holds no blocking thread.
/// Before a body is read (spec 6.6): one that says it is larger than any request may be is `413` at once, and only
/// `READING_AT_ONCE` requests read and are handled at once (`503` beyond): a flood of slow or large bodies holds no
/// more of the hub's memory than that.
async fn admit(
    State(hub): State<Arc<Hub>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let said = request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if said.is_some_and(|n| n > limits::MAX_BODY) {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let Ok(_place) = hub.reading.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    // The body here, within its time, for every route admitted (a delivery as much as the others).
    let (parts, body) = request.into_parts();
    let Ok(read) = tokio::time::timeout(BODY_DEADLINE, axum::body::to_bytes(body, limits::MAX_BODY)).await else {
        return StatusCode::REQUEST_TIMEOUT.into_response();
    };
    let Ok(body) = read else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(body));
    // Its place is held for at most this long in all.
    tokio::time::timeout(BODY_DEADLINE * 3, next.run(request))
        .await
        .unwrap_or_else(|_| StatusCode::REQUEST_TIMEOUT.into_response())
}

async fn off_workers(
    State(hub): State<Arc<Hub>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // Its body was read whole by `admit`, within its time: here it is taken as it is.
    let (parts, body) = request.into_parts();
    let Ok(body) = axum::body::to_bytes(body, limits::MAX_BODY).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(body));
    let runtime = tokio::runtime::Handle::current();
    hub.blocking(move |_| runtime.block_on(next.run(request))).await.unwrap_or_else(IntoResponse::into_response)
}

impl Hub {
    /// An answer that hands out keys, signed with the hub's key when the request asks so with a nonce (spec 6.10):
    /// the answer, then the signature. Without the header it goes as it is.
    fn signed_answer(&self, headers: &HeaderMap, path: &str, request: &[u8], mut answer: Vec<u8>) -> Vec<u8> {
        let nonce = headers.get(hubsig::NONCE_HEADER).and_then(|v| v.to_str().ok()).and_then(hubsig::nonce);
        if let Some(nonce) = nonce {
            let key = ed25519_dalek::SigningKey::from_bytes(&self.config.hub_key);
            let signature = ed25519_dalek::Signer::sign(&key, &hubsig::message(&nonce, path, request, &answer));
            answer.extend_from_slice(&signature.to_bytes());
        }
        answer
    }

    /// A username at least as long as this hub allows now (`YO_NAME_MIN`, spec 3.3); shorter is `400`, as a name of
    /// the wrong form.
    fn long_enough(&self, name: &[u8]) -> Res<()> {
        let name = std::str::from_utf8(name).map_err(|_| refuse(StatusCode::BAD_REQUEST))?;
        if yo_wire::handle::username_len(name) < self.config.name_min {
            return Err(refuse(StatusCode::BAD_REQUEST));
        }
        Ok(())
    }

    pub fn new(config: Config, push_transport: Arc<dyn push::Transport>) -> Result<Arc<Self>, String> {
        let db = Arc::new(Db::open(&config.db_path).map_err(|e| format!("database: {e}"))?);
        let added = db.seed_reserved(&config.reserved, now_ms()).map_err(|e| format!("reserved names: {e}"))?;
        eprintln!("yo-hub: reserved names: {} in the list, {added} new", config.reserved.len());
        let pusher = push::Pusher::new(push_transport, config.apns.as_ref(), config.fcm.as_ref())?;
        let blobs = blobs::Blobs::open(&config.blobs).map_err(|e| format!("{}: {e}", config.blobs.dir.display()))?;
        // What a crash left under way: fetches (given back) and blobs whose file was being moved into place.
        for blob in db.recover_blobs().map_err(|e| format!("database: {e}"))? {
            blobs.remove(&blob);
        }
        let signer = certificate::Signer::from_bytes(&config.server_certificate)
            .map_err(|e| format!("server certificate: {e}"))?;
        let zk = groups::Zk::from_bytes(&config.zk_params).map_err(|e| format!("zk params: {e}"))?;
        let shifted_ms = Arc::new(std::sync::atomic::AtomicI64::new(0));
        let (deliveries, queue) = std::sync::mpsc::sync_channel(QUEUED);
        let hub = Arc::new(Hub {
            zk,
            signer,
            shifted_ms: shifted_ms.clone(),
            db,
            deliveries,
            limiter: Limiter::default(),
            requests: Requests::new(
                config.limits.requests_per_pair,
                config.limits.request_recipients_per_sender,
                config.limits.requests_per_recipient,
                config.limits.request_hour_ms,
            ),
            replay: ReplayCache::default(),
            waiting: Mutex::default(),
            fetching: Mutex::default(),
            disk_asked: AtomicU64::new(0),
            disk_full: AtomicBool::new(false),
            pusher,
            blobs: Arc::new(blobs),
            blocking: Arc::new(tokio::sync::Semaphore::new(BLOCKING_AT_ONCE)),
            reading: Arc::new(tokio::sync::Semaphore::new(READING_AT_ONCE)),
            config,
        });
        // The writer ends with the hub: when the last sender of its queue goes.
        let (writing, woken, most) = (hub.db.clone(), Arc::downgrade(&hub), hub.config.limits.max_pending_per_device);
        std::thread::Builder::new()
            .name("yo-hub-writer".into())
            .spawn(move || writer(writing, queue, most, woken, shifted_ms))
            .map_err(|e| format!("writer: {e}"))?;
        Ok(hub)
    }

    pub fn router(self: &Arc<Self>) -> Router {
        // Synchronous through and through: off the async workers (`off_workers`).
        let synchronous = Router::new()
            .route(api::REGISTER, post(register))
            .route(api::BACKUP, put(put_backup))
            .route("/v1/backup/{id}", get(fetch_backup))
            .route(api::PREKEYS, put(prekeys))
            .route(api::LOOKUP, post(lookup))
            .route(api::PUSH, put(set_push))
            .route(api::USERNAME, put(set_username))
            .route(api::ACCESS_KEY, put(set_access_key))
            .route(api::ACCOUNT, delete(delete_account))
            .route(api::REPORT, post(report))
            .route(api::FEEDBACK, post(feedback))
            .route(api::CRASH, post(crash))
            .route(api::STORAGE, get(get_storage).put(put_storage))
            .route(hubsig::KEY_PATH, get(hub_key))
            .route(api::TRUST_ROOT_PATH, get(trust_root))
            .route(api::CERTIFICATE, get(sender_certificate))
            .route(api::GROUP_AUTH, get(group_auth))
            .route(api::GROUP_CREDENTIAL, post(group_credential))
            .route("/v1/groups/{id}", put(create_group).post(group_request))
            .route(api::ZK_PARAMS_PATH, get(zk_params))
            .route("/v1/username/{name}", get(username_free))
            .route(api::INBOX_ACK, post(ack))
            .route_layer(axum::middleware::from_fn_with_state(self.clone(), off_workers))
            .route_layer(axum::middleware::from_fn_with_state(self.clone(), admit));
        let delivering = Router::new()
            .route("/v1/inbox/{user}", post(deliver))
            .route_layer(axum::middleware::from_fn_with_state(self.clone(), admit));
        // These wait (a long poll, the writer, a blob's bytes) and run their database work through `blocking`.
        let mut router = Router::new()
            .route("/", get(|| async { axum::response::Html(include_str!("index.html")) }))
            .route(api::INBOX, get(inbox))
            .route("/v1/blobs/{id}", put(put_blob).get(get_blob))
            .merge(delivering)
            .merge(synchronous);
        if !self.config.apple_app_ids.is_empty() {
            // Which apps may make passkeys for this domain (Apple reads it through its CDN; spec 7).
            let file = serde_json::json!({ "webcredentials": { "apps": self.config.apple_app_ids } }).to_string();
            router = router.route(
                "/.well-known/apple-app-site-association",
                get(move || {
                    let file = file.clone();
                    async move { ([(axum::http::header::CONTENT_TYPE, "application/json")], file) }
                }),
            );
        }
        router
            .layer(axum::middleware::from_fn_with_state(self.clone(), too_old))
            .layer(DefaultBodyLimit::max(limits::MAX_BODY))
            .with_state(self.clone())
    }

    /// The hourly cleanup at `now`: envelopes and commits past their time (6.6), names not secured past theirs (6.4),
    /// blobs past theirs and files no blob owns (6.8).
    pub fn prune(&self, now: u64) -> rusqlite::Result<()> {
        let limits = &self.config.limits;
        self.db.prune(now, limits.envelope_ttl_days * DAY.as_millis() as u64)?;
        for blob in self.db.prune_blobs(now, limits.blob_ttl_days * DAY.as_millis() as u64)? {
            self.blobs.remove(&blob);
        }
        match self.blobs.sweep(&self.db) {
            Ok(0) => {}
            Ok(n) => eprintln!("yo-hub: {n} stray blob files removed"),
            Err(error) => eprintln!("yo-hub: blob sweep in {}: {error}", self.blobs.dir().display()),
        }
        let released = self.db.release_unsecured(now, limits.unsecured_idle_days * DAY.as_millis() as u64)?;
        if released > 0 {
            eprintln!("yo-hub: {released} names not secured released");
        }
        Ok(())
    }

    /// What should never be (`Db::audit`), and blobs whose file is missing: for tests and the operator; reads only.
    /// Files no blob owns are not counted: the hourly sweep takes them (`prune`).
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

    /// Serves until `shutdown` resolves; prunes meanwhile.
    pub async fn serve(
        self: Arc<Self>,
        listener: tokio::net::TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        let pruning = self.clone();
        let pruner = tokio::spawn(async move {
            let mut every = tokio::time::interval(MINUTE);
            let mut minutes = 0u64;
            loop {
                every.tick().await;
                pruning.replay.forget_expired(now_ms());
                if minutes.is_multiple_of(60) {
                    pruning.limiter.forget_idle(DAY);
                    pruning.requests.forget_full(pruning.clock_ms());
                    // Off the request threads: the sweep lists every blob file.
                    let hub = pruning.clone();
                    match tokio::task::spawn_blocking(move || hub.prune(now_ms())).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => eprintln!("yo-hub: prune: {error}"),
                        Err(error) => eprintln!("yo-hub: prune: {error}"),
                    }
                }
                minutes += 1;
            }
        });
        let app = self.router().into_make_service_with_connect_info::<SocketAddr>();
        let served = axum::serve(listener, app).with_graceful_shutdown(shutdown).await;
        pruner.abort();
        served
    }

    /// A handle's value as the hub keeps it.
    fn stored_handle(&self, handle: &Handle) -> Vec<u8> {
        match handle.kind {
            // A username by its skeleton (spec 3.3): one name per look, and LE0 finds LEO.
            KIND_USERNAME => std::str::from_utf8(handle.value.as_slice())
                .map(|name| yo_wire::handle::skeleton(name).into_bytes())
                .unwrap_or_default(),
            _ => handle.value.as_slice().to_vec(),
        }
    }

    /// The client's address as rate limits count it: IPv6 by its /64, which one subscriber usually holds whole.
    /// `CF-Connecting-IP` is believed only when configured: the hub must then be reachable through Cloudflare alone
    /// (a tunnel, listening on 127.0.0.1), or anyone could name any address.
    fn ip(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
        let forwarded = headers.get("cf-connecting-ip").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
        let ip = match forwarded {
            Some(ip) if self.config.trust_cloudflare => ip,
            _ => peer.ip(),
        };
        match ip {
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => IpAddr::V4(v4),
                None => {
                    let s = v6.segments();
                    IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
                }
            },
            v4 => v4,
        }
    }

    /// A per-address limit. Not for the server's own services (YoGPT): behind Cloudflare every request carries
    /// `CF-Connecting-IP`, so one that comes straight from 127.0.0.1 without it is the server itself, which speaks for
    /// many users at once from one address.
    fn limit_ip(&self, kind: &str, peer: SocketAddr, headers: &HeaderMap, per: u32, window: Duration) -> Res<()> {
        if self.own_service(peer, headers) {
            return Ok(());
        }
        self.limit(format!("{kind}:ip:{}", self.ip(peer, headers)), per, window)
    }

    /// Whether a request comes from the server's own services (`limit_ip`).
    fn own_service(&self, peer: SocketAddr, headers: &HeaderMap) -> bool {
        self.config.trust_cloudflare && peer.ip().is_loopback() && !headers.contains_key("cf-connecting-ip")
    }

    /// Bytes stored without identity (deliveries, commits), counted per address like `limit_ip`: they stay for
    /// `envelope_ttl_days`, so what one address may store is bounded by bytes, not by requests alone.
    fn limit_ip_bytes(&self, peer: SocketAddr, headers: &HeaderMap, bytes: usize) -> Res<()> {
        if self.own_service(peer, headers) {
            return Ok(());
        }
        let key = format!("bytes:ip:{}", self.ip(peer, headers));
        if self.limiter.allow_n(key, bytes as u64, self.config.limits.anonymous_bytes_per_ip_hour, HOUR) {
            Ok(())
        } else {
            Err(refuse(StatusCode::TOO_MANY_REQUESTS))
        }
    }

    /// `507` while the database's disk has less than `db_min_free_bytes` free: deliveries, commits and new groups
    /// wait (the apps try again) rather than fill it. The disk is asked at most once a second; an error asking
    /// counts as room (the database itself fails then, as it would anyway).
    fn db_room(&self) -> Res<()> {
        let floor = self.config.limits.db_min_free_bytes;
        if floor == 0 || self.config.db_path == ":memory:" {
            return Ok(());
        }
        let now = now_ms();
        if now.saturating_sub(self.disk_asked.load(Ordering::Relaxed)) >= 1000 {
            self.disk_asked.store(now, Ordering::Relaxed);
            let path = std::path::Path::new(&self.config.db_path);
            let dir = match path.parent() {
                Some(dir) if !dir.as_os_str().is_empty() => dir,
                _ => std::path::Path::new("."),
            };
            let full = rustix::fs::statvfs(dir).is_ok_and(|stat| stat.f_bavail.saturating_mul(stat.f_frsize) < floor);
            self.disk_full.store(full, Ordering::Relaxed);
        }
        if self.disk_full.load(Ordering::Relaxed) {
            return Err(refuse(StatusCode::INSUFFICIENT_STORAGE));
        }
        Ok(())
    }

    fn limit(&self, key: String, per: u32, window: Duration) -> Res<()> {
        if self.limiter.allow(key, per, window) {
            Ok(())
        } else {
            Err(refuse(StatusCode::TOO_MANY_REQUESTS))
        }
    }

    fn authenticate(&self, method: &Method, uri: &Uri, headers: &HeaderMap, body: &[u8]) -> Res<Signed> {
        self.authenticate_hash(method, uri, headers, &sha256(body))
    }

    /// `authenticate` by the body's hash, before the body is there (a blob, 6.8).
    fn authenticate_hash(&self, method: &Method, uri: &Uri, headers: &HeaderMap, body_hash: &[u8; 32]) -> Res<Signed> {
        let unauthorized = || refuse(StatusCode::UNAUTHORIZED);
        let device_id: DeviceId = header(headers, HEADER_DEVICE)
            .and_then(|d| unb64(d).ok())
            .and_then(|d| d.try_into().ok())
            .ok_or_else(unauthorized)?;
        let now = now_ms();
        let (time, signature) = signature_of(headers, now).ok_or_else(unauthorized)?;
        let Some(device) = self.db.device(&device_id)? else {
            // A replaced device learns that it was replaced.
            return Err(if self.db.is_gone(&device_id)? { refuse(StatusCode::GONE) } else { unauthorized() });
        };
        let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        yo_wire::request::verify_hash(&device.auth_key, method.as_str(), path, time, body_hash, &signature)
            .map_err(|_| unauthorized())?;
        // Counted only once the signature holds: a stranger who knows a device id must not spend its requests.
        self.limit(format!("signed:{}", yo_wire::b64(&device_id)), self.config.limits.signed_per_device_minute, MINUTE)
            .map_err(|_| error::limited("device", None))?;
        if !self.first_time(&signature, time, now) {
            return Err(unauthorized());
        }
        // When a device was last seen counts only to the day (spec 6.4): one write a day, not one per request; the
        // app it names is written when it changes.
        let app = named_app(headers);
        let new_app = app.is_some_and(|a| device.app.as_deref() != Some(a));
        if new_app || now.saturating_sub(device.last_seen_at) >= DAY.as_millis() as u64 {
            self.db.touch(&device_id, now, app)?;
        }
        Ok(Signed { device_id, user_id: device.user_id })
    }

    /// Each signature once (the replay cache), for as long as its time is within the window.
    fn first_time(&self, signature: &[u8; 64], time: u64, now: u64) -> bool {
        self.replay.first_time(*signature, now, time + limits::MAX_SKEW_MS + 1)
    }

    /// Checks an upload of prekeys (spec 4.1): their number and sizes. What they are, the devices that fetch them
    /// check (against the account's identity key); the hub stores them as they came.
    fn check_prekeys(prekeys: &api::Prekeys, first: bool) -> Res<()> {
        use limits::{MAX_PREKEY_LEN, MAX_PREKEY_SIGNATURE_LEN};
        if prekeys.one_time.len() > MAX_PREKEYS_PER_UPLOAD || prekeys.kyber.len() > MAX_PREKEYS_PER_UPLOAD {
            return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
        }
        if first && (prekeys.signed.is_none() || prekeys.last_resort.is_none()) {
            return Err(bad());
        }
        let key = |k: &yo_wire::VLBytes| !k.as_slice().is_empty() && k.as_slice().len() <= MAX_PREKEY_LEN;
        let signed = |p: &api::SignedPrekey| {
            key(&p.key)
                && !p.signature.as_slice().is_empty()
                && p.signature.as_slice().len() <= MAX_PREKEY_SIGNATURE_LEN
        };
        let good = prekeys.signed.iter().chain(&prekeys.last_resort).chain(&prekeys.kyber).all(signed)
            && prekeys.one_time.iter().all(|p| key(&p.key));
        good.then_some(()).ok_or_else(bad)
    }

    /// A registration id in libsignal's range (14 bits, not 0) and an auth key that is an Ed25519 point (spec 4.1,
    /// 6.2): else the device's bundles and signed requests could never work.
    fn device_keys_usable(registration_id: u32, auth_key: &Key) -> bool {
        (1..=0x3fff).contains(&registration_id) && ed25519_dalek::VerifyingKey::from_bytes(auth_key).is_ok()
    }

    /// Everything about a registering device that is checked before it is stored (spec 6.4): its certificate, the
    /// account key's proof of possession over this handle and device, its prekeys.
    fn check_new_device(&self, handle: &Handle, device: &api::NewDevice, app: Option<&str>) -> Res<(Key, NewDevice)> {
        if device.push.token.as_slice().len() > limits::MAX_PUSH_TOKEN_LEN
            || !Self::device_keys_usable(device.registration_id, &device.auth_key)
        {
            return Err(bad());
        }
        let certificate = DeviceCertificate::verify(device.certificate.as_slice(), &device.device_key)?;
        let account_key = certificate.account_key;
        let signed = api::registration_to_be_signed(handle, &device.device_id, &device.device_key, &device.auth_key);
        if !yo_wire::xeddsa_verify(&account_key, &signed, &device.proof) {
            return Err(bad());
        }
        Self::check_prekeys(&device.prekeys, true)?;
        let new = NewDevice {
            device_id: device.device_id,
            device_key: device.device_key,
            auth_key: device.auth_key,
            registration_id: device.registration_id,
            push_platform: device.push.platform,
            push_token: device.push.token.as_slice().to_vec(),
            app: app.map(str::to_owned),
        };
        Ok((account_key, new))
    }

    /// Stores a checked registration for a username and answers the user id.
    fn register_checked(
        &self,
        handle: &Handle,
        checked: (Key, NewDevice),
        prekeys: &api::Prekeys,
    ) -> Res<(StatusCode, Vec<u8>)> {
        let (account_key, device) = checked;
        let mut fresh: UserId = [0; 16];
        getrandom::getrandom(&mut fresh).map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
        let spelling = std::str::from_utf8(handle.value.as_slice()).map_err(|_| bad())?;
        let written = self.db.register(
            &self.stored_handle(handle),
            spelling,
            &fresh,
            &account_key,
            &device,
            prekeys,
            PREKEY_TARGET * 2,
            now_ms(),
        )?;
        match written {
            Write::Done((user_id, username, device_id)) => {
                let username = username.map(|name| name.into_bytes().into());
                Ok(ok(encode(&api::Registered { user_id, username, device_id })))
            }
            Write::Refused(_) => Err(refuse(StatusCode::CONFLICT)),
        }
    }

    /// Runs `f` on a blocking thread, off the async workers, with at most `BLOCKING_AT_ONCE` of the hub's at once:
    /// what waits for the database (one connection behind a lock) or works the CPU must not hold a worker. The place
    /// goes with `f`, which runs to its end even if the request is cut off meanwhile.
    async fn blocking<T: Send + 'static>(self: &Arc<Self>, f: impl FnOnce(&Hub) -> T + Send + 'static) -> Res<T> {
        let place = self.blocking.clone().acquire_owned().await.map_err(|_| refuse(StatusCode::SERVICE_UNAVAILABLE))?;
        let hub = self.clone();
        tokio::task::spawn_blocking(move || {
            let _place = place;
            f(&hub)
        })
        .await
        .map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))
    }

    /// A place for one more inbox request of a device, if it has fewer than `INBOX_AT_ONCE` under way.
    fn fetching(self: &Arc<Self>, device: DeviceId) -> Option<Fetching> {
        let mut fetching = self.fetching.lock().unwrap_or_else(PoisonError::into_inner);
        let n = fetching.entry(device).or_default();
        if *n >= INBOX_AT_ONCE {
            return None;
        }
        *n += 1;
        Some(Fetching { hub: self.clone(), device })
    }

    /// A long poll's wait for mail to `device`; the device's entry goes with its last waiter, also when the request is
    /// cut off.
    fn waiter(&self, device: DeviceId) -> Waiter<'_> {
        let notify = self.waiting.lock().unwrap_or_else(PoisonError::into_inner).entry(device).or_default().clone();
        Waiter { hub: self, device, notify }
    }

    fn wake_waiters(&self, device: &DeviceId) {
        if let Some(notify) = self.waiting.lock().unwrap_or_else(PoisonError::into_inner).get(device) {
            notify.notify_waiters();
        }
    }

    /// What follows a stored delivery: its devices' long polls answer, the others are pushed.
    fn woken(self: &Arc<Self>, devices: Vec<db::Wake>, runtime: &tokio::runtime::Handle) {
        for wake in devices {
            self.wake_waiters(&wake.device_id);
            self.push(wake, runtime);
        }
    }

    fn push(self: &Arc<Self>, wake: db::Wake, runtime: &tokio::runtime::Handle) {
        if wake.push_token.is_empty() {
            return;
        }
        let hub = self.clone();
        runtime.spawn(async move {
            match hub.pusher.wake(wake.push_platform, &wake.push_token).await {
                push::Outcome::TokenInvalid => {
                    let cleared = hub.blocking(move |hub| hub.db.clear_push(&wake.device_id, &wake.push_token));
                    if let Ok(Err(error)) = cleared.await {
                        eprintln!("yo-hub: clear push: {error}");
                    }
                }
                push::Outcome::Failed(error) => eprintln!("yo-hub: push: {error}"),
                push::Outcome::Sent | push::Outcome::Skipped => {}
            }
        });
    }
}

type Hubs = State<Arc<Hub>>;
type Peer = ConnectInfo<SocketAddr>;

// ---- registration ----

/// A username is all it takes (spec 6.4): first come, first served, limited per address. A user's current account
/// key registers that user again, with the name it has now (a new phone restored from the backup); a name taken is
/// `409`.
async fn register(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    body: Bytes,
) -> Res<(StatusCode, Vec<u8>)> {
    hub.limit_ip("register", peer, &headers, hub.config.limits.register_per_ip_hour, HOUR)?;
    let request: api::Register = decode(&body)?;
    let handle = Handle { kind: KIND_USERNAME, value: request.username.clone() };
    handle.check()?;
    hub.long_enough(request.username.as_slice())?;
    let checked = hub.check_new_device(&handle, &request.device, named_app(&headers))?;
    hub.register_checked(&handle, checked, &request.device.prekeys)
}

/// Whether a username is free, for the app while it is typed (spec 6.3): `204` free, `409` taken by anyone or
/// reserved (to the app both are a name it cannot have), `400` no username. Compared by skeleton, as registration
/// compares, so another spelling or a lookalike of a taken name is taken. Nothing about the user is told.
async fn username_free(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Res<StatusCode> {
    hub.limit_ip("name", peer, &headers, hub.config.limits.name_check_per_ip_minute, MINUTE)?;
    yo_wire::handle::check_username_form(&name)?;
    hub.long_enough(name.as_bytes())?;
    if hub.db.username_taken(yo_wire::handle::skeleton(&name).as_bytes())? {
        return Err(refuse(StatusCode::CONFLICT));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- backups (spec 7) ----

/// Stores the signing device's account's backup record, replacing the one before.
async fn put_backup(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let backup: api::Backup = decode(&body)?;
    backup.check()?;
    // Keys a signature can be checked against (Curve25519, canonical): any other is refused here, not at every fetch.
    let (passkey, code) = backup.auth_keys();
    if passkey.into_iter().chain(code).any(|key| !yo_wire::curve_key_usable(&key)) {
        return Err(bad());
    }
    let auth = db::BackupAuth { passkey, code };
    let locator = backup.locator();
    match hub.db.put_backup(&device.device_id, locator.as_ref().map(|l| &l[..]), &body, &auth, now_ms())? {
        Write::Done(()) => Ok(StatusCode::NO_CONTENT),
        // Replaced between its authentication and now (a new phone restored the account).
        Write::Refused(db::Conflict::DeviceGone) => Err(refuse(StatusCode::GONE)),
        Write::Refused(_) => Err(refuse(StatusCode::CONFLICT)),
    }
}

/// What a signature is checked against where a record has no key (and never counts): every fetch checks its
/// signature twice, so the time does not tell whether there is a record, or a passkey.
static NO_KEY: std::sync::LazyLock<Key> = std::sync::LazyLock::new(|| {
    let mut key = [0; 32];
    key[0] = 9; // Curve25519's base point
    key
});

/// A backup record by its user id or locator, to a request signed by one of its doors' auth keys (spec 7): only who
/// holds a door learns that there is one. Anything else is `404`, as for no record.
async fn fetch_backup(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<(StatusCode, Vec<u8>)> {
    hub.limit_ip("backup", peer, &headers, hub.config.limits.backup_per_ip_hour, HOUR)?;
    let id: [u8; api::LOCATOR_LEN] = path_id(&id)?;
    let now = now_ms();
    let found = hub.db.backup(&id)?;
    let not_found = || refuse(StatusCode::NOT_FOUND);
    let (time, signature) = signature_of(&headers, now).ok_or_else(not_found)?;
    let auth = found.as_ref().map(|(_, auth)| auth.clone()).unwrap_or_default();
    let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let empty = sha256(&[]);
    let signed = [auth.passkey, auth.code]
        .map(|key| {
            let verified = yo_wire::request::verify_door_hash(
                key.as_ref().unwrap_or(&NO_KEY),
                method.as_str(),
                path,
                time,
                &empty,
                &signature,
            );
            key.is_some() && verified.is_ok()
        })
        .contains(&true);
    match found {
        Some((record, _)) if signed && hub.first_time(&signature, time, now) => Ok(ok(record)),
        _ => Err(not_found()),
    }
}

// ---- what a device asks for itself ----

async fn prekeys(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::Prekeys = decode(&body)?;
    Hub::check_prekeys(&request, false)?;
    match hub.db.add_prekeys(&device.device_id, &request, PREKEY_TARGET * 2)? {
        Write::Done(()) => Ok(StatusCode::NO_CONTENT),
        Write::Refused(db::Conflict::DeviceGone) => Err(refuse(StatusCode::GONE)),
        Write::Refused(_) => Err(refuse(StatusCode::CONFLICT)),
    }
}

/// The hub's public key (spec 6.10), for tools: apps ship their own copy and trust only that.
/// The public key of the trust root the hub's sender certificates go back to, 32 bytes (spec 6.1): for tools.
async fn trust_root(State(hub): Hubs) -> Vec<u8> {
    hub.signer.root().to_vec()
}

/// A sender certificate for the signing device, good for a day (spec 6.1). The hub learns that this device asked,
/// as of any signed request.
async fn sender_certificate(
    State(hub): Hubs,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Res<(StatusCode, Vec<u8>)> {
    let device = hub.authenticate(&method, &uri, &headers, &[])?;
    let number = hub.db.device_number(&device.device_id)?.ok_or_else(|| refuse(StatusCode::UNAUTHORIZED))?;
    let account_key = hub.db.account_key(&device.user_id)?.ok_or_else(|| refuse(StatusCode::UNAUTHORIZED))?;
    let now = now_ms().saturating_add_signed(hub.shifted_ms.load(Ordering::Relaxed));
    let certificate = hub.signer.issue(&device.user_id, number, &account_key, now).map_err(|error| {
        eprintln!("yo-hub: sender certificate: {error}");
        refuse(StatusCode::INTERNAL_SERVER_ERROR)
    })?;
    Ok(ok(certificate))
}

async fn hub_key(State(hub): Hubs) -> Vec<u8> {
    ed25519_dalek::SigningKey::from_bytes(&hub.config.hub_key).verifying_key().to_bytes().to_vec()
}

async fn lookup(
    State(hub): Hubs,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Res<(StatusCode, Vec<u8>)> {
    let answer = lookup_answer(&hub, &method, &uri, &headers, &body)?;
    Ok(ok(hub.signed_answer(&headers, uri.path(), &body, answer)))
}

/// Who a handle is (spec 6.3), before the hub signs it.
fn lookup_answer(hub: &Hub, method: &Method, uri: &Uri, headers: &HeaderMap, body: &Bytes) -> Res<Vec<u8>> {
    let device = hub.authenticate(method, uri, headers, body)?;
    let request: api::Lookup = decode(body)?;
    request.handle.check()?;
    if request.except.len() > MAX_DEVICES {
        return Err(bad());
    }
    let limits = &hub.config.limits;
    let take = request.flags & api::LOOKUP_PREKEYS != 0;
    let (kind, per_day) = match take {
        true => ("lookup", limits.lookup_per_device_day),
        false => ("identity", limits.identity_lookup_per_device_day),
    };
    hub.limit(format!("{kind}:{}", yo_wire::b64(&device.device_id)), per_day, DAY)?;
    let found = hub
        .db
        .lookup(request.handle.kind, &hub.stored_handle(&request.handle), take, &request.except)?
        .ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    let result = api::LookupResult {
        user_id: found.user_id,
        account_key: found.account_key,
        devices: found.devices.into_iter().take(MAX_DEVICES).collect(),
        bundles: found.bundles.into_iter().take(MAX_DEVICES).collect(),
    };
    if request.flags & api::LOOKUP_USERNAME == 0 {
        return Ok(encode(&result));
    }
    let username = found.username.map(|name| name.into_bytes().into());
    Ok(encode(&api::LookupNamed { found: result, username }))
}

/// Claims or gives up the signing device's account's username.
async fn set_username(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::SetUsername = decode(&body)?;
    if let Some(name) = &request.username {
        Handle { kind: KIND_USERNAME, value: name.clone() }.check()?;
        hub.long_enough(name.as_slice())?;
    }
    // Checked above: UTF-8.
    let spelling = request.username.as_ref().and_then(|n| std::str::from_utf8(n.as_slice()).ok());
    let skeleton = spelling.map(yo_wire::handle::skeleton);
    match hub.db.set_username(&device.device_id, skeleton.as_deref().zip(spelling).map(|(s, n)| (s.as_bytes(), n)))? {
        Write::Done(()) => Ok(StatusCode::NO_CONTENT),
        Write::Refused(db::Conflict::DeviceGone) => Err(refuse(StatusCode::GONE)),
        Write::Refused(_) => Err(refuse(StatusCode::CONFLICT)),
    }
}

/// Sets the signing device's account's profile key check (spec 4.10); whether it takes requests is kept as told, and
/// decides nothing here: the device drops strangers' requests itself.
async fn set_access_key(
    State(hub): Hubs,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::AccessKey = decode(&body)?;
    let commitment = request.commitment.as_slice();
    if request.requests_open > 1 || zkgroup::deserialize::<zkgroup::profiles::ProfileKeyCommitment>(commitment).is_err()
    {
        return Err(refuse(StatusCode::BAD_REQUEST));
    }
    hub.db.set_access_key(&device.user_id, &request.check, request.requests_open == 1, commitment)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- groups (spec 4.9) ----

async fn zk_params(State(hub): Hubs) -> Vec<u8> {
    hub.zk.public().to_vec()
}

/// Today's auth credential for the signing device's account.
async fn group_auth(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap) -> Res<(StatusCode, Vec<u8>)> {
    let device = hub.authenticate(&method, &uri, &headers, &[])?;
    Ok(ok(encode(&hub.group_auth(&device.user_id))))
}

/// A profile key credential for a user whose key the asker holds.
async fn group_credential(
    State(hub): Hubs,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Res<(StatusCode, Vec<u8>)> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    hub.limit(format!("credential:{}", yo_wire::b64(&device.device_id)), groups::CREDENTIALS_PER_DAY, DAY)?;
    let request: api::ProfileCredentialRequest = decode(&body)?;
    Ok(ok(hub.profile_credential(&request)?))
}

async fn create_group(
    State(hub): Hubs,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Res<(StatusCode, Vec<u8>)> {
    hub.authenticate(&method, &uri, &headers, &body)?;
    let id = path_id(&id)?;
    let request: api::NewGroup = decode(&body)?;
    Ok(ok(encode(&hub.create_group(&id, &request)?)))
}

async fn group_request(
    State(hub): Hubs,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Res<(StatusCode, Vec<u8>)> {
    hub.authenticate(&method, &uri, &headers, &body)?;
    let id = path_id(&id)?;
    let request: api::GroupRequest = decode(&body)?;
    Ok(ok(encode(&hub.group_request(&id, &request)?)))
}

/// Deletes the signing device's account with everything the hub holds about it; its other devices answer `410`.
async fn delete_account(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &[])?;
    let blobs = hub.db.blobs_of(&device.device_id)?;
    match hub.db.delete_account(&device.device_id, now_ms())? {
        Write::Done(()) => {
            // Its photos and videos with it, not yet fetched or not. One uploaded meanwhile goes with the sweep.
            for blob in &blobs {
                hub.blobs.remove(blob);
            }
            Ok(StatusCode::NO_CONTENT)
        }
        Write::Refused(_) => Err(refuse(StatusCode::GONE)),
    }
}

/// A user reported by the signing device's user, for the operator to review: who, whom, why, when.
async fn report(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::Report = decode(&body)?;
    if !(api::REPORT_SPAM..=api::REPORT_OTHER).contains(&request.reason) || request.user_id == device.user_id {
        return Err(bad());
    }
    hub.limit(format!("report:{}", yo_wire::b64(&device.user_id)), hub.config.limits.report_per_user_day, DAY)?;
    match hub.db.report(&device.device_id, &request.user_id, request.reason, now_ms())? {
        Write::Done(true) => Ok(StatusCode::NO_CONTENT),
        Write::Done(false) => Err(refuse(StatusCode::NOT_FOUND)),
        Write::Refused(_) => Err(refuse(StatusCode::GONE)),
    }
}

/// Feedback on the app from the signing device's user, for the operator to read: the text as written (not end-to-end
/// encrypted, spec 1), the app's version, who, when.
async fn feedback(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::Feedback = decode(&body)?;
    let (text, app_version) = request.check().map_err(|_| bad())?;
    hub.limit(format!("feedback:{}", yo_wire::b64(&device.user_id)), hub.config.limits.feedback_per_user_day, DAY)?;
    match hub.db.feedback(&device.device_id, text, app_version, now_ms())? {
        Write::Done(()) => Ok(StatusCode::NO_CONTENT),
        Write::Refused(_) => Err(refuse(StatusCode::GONE)),
    }
}

/// A crash report (spec 6.4): unsigned, no account; kept as feedback marked `auto`, counted by the address.
async fn crash(State(hub): Hubs, ConnectInfo(peer): Peer, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    hub.limit_ip("crash", peer, &headers, hub.config.limits.crash_per_ip_day, DAY)?;
    let request: api::Feedback = decode(&body)?;
    let (text, app_version) = request.check().map_err(|_| bad())?;
    hub.db.crash(text, app_version, now_ms())?;
    Ok(StatusCode::NO_CONTENT)
}

/// The signing device's account's storage (spec 6.9): `404` when there is none yet.
async fn get_storage(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap) -> Res<(StatusCode, Vec<u8>)> {
    let device = hub.authenticate(&method, &uri, &headers, &[])?;
    hub.limit(format!("storage:{}", yo_wire::b64(&device.user_id)), hub.config.limits.storage_per_user_hour, HOUR)?;
    let (version, data) = hub.db.storage(&device.user_id)?.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    Ok(ok(encode(&api::Storage { version, data: data.into() })))
}

/// Replaces the storage if the request names the version stored now; else `409` with that one.
async fn put_storage(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::Storage = decode(&body)?;
    if request.data.as_slice().len() > limits::MAX_STORAGE_LEN {
        return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
    }
    if request.data.as_slice().is_empty() {
        return Err(bad());
    }
    hub.limit(format!("storage:{}", yo_wire::b64(&device.user_id)), hub.config.limits.storage_per_user_hour, HOUR)?;
    match hub.db.put_storage(&device.device_id, request.version, request.data.as_slice(), now_ms())? {
        Write::Done(Ok(())) => Ok(StatusCode::NO_CONTENT),
        Write::Done(Err((version, data))) => {
            Err(Refusal(StatusCode::CONFLICT, encode(&api::Storage { version, data: data.into() })))
        }
        Write::Refused(_) => Err(refuse(StatusCode::GONE)),
    }
}

async fn set_push(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::Push = decode(&body)?;
    if request.token.as_slice().len() > limits::MAX_PUSH_TOKEN_LEN {
        return Err(bad());
    }
    match hub.db.set_push(&device.device_id, request.platform, request.token.as_slice())? {
        Write::Done(()) => Ok(StatusCode::NO_CONTENT),
        Write::Refused(_) => Err(refuse(StatusCode::GONE)),
    }
}

async fn inbox(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap) -> Res<axum::response::Response> {
    let wait_s = uri
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("wait=")))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        .min(limits::MAX_WAIT_S);
    let device = hub.blocking(move |hub| hub.authenticate(&method, &uri, &headers, &[])).await??;
    let fetching = hub.fetching(device.device_id).ok_or_else(|| refuse(StatusCode::TOO_MANY_REQUESTS))?;
    let id = device.device_id;
    let read =
        move |hub: &Hub| Ok::<_, rusqlite::Error>((hub.db.fetch(&id, MAX_INBOX_FETCH)?, hub.db.prekeys_left(&id)?));
    let (mut items, mut prekeys_left) = hub.blocking(read).await??;
    if items.is_empty() && wait_s > 0 {
        let waiter = hub.waiter(id);
        let notified = waiter.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        // Mail that came between the first fetch and `enable` is found here.
        (items, prekeys_left) = hub.blocking(read).await??;
        if items.is_empty() {
            let _ = tokio::time::timeout(Duration::from_secs(wait_s), notified).await;
            (items, prekeys_left) = hub.blocking(read).await??;
        }
    }
    let inbox = api::Inbox {
        prekeys_left,
        items: items
            .into_iter()
            .map(|(id, envelope, received_ms)| api::InboxItem { id, received_ms, envelope: envelope.into() })
            .collect(),
    };
    Ok(held(encode(&inbox), fetching))
}

async fn ack(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::Ack = decode(&body)?;
    if request.ids.len() > limits::MAX_ACK {
        return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
    }
    hub.db.ack(&device.device_id, &request.ids)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- sealed sender: no identity on these routes, but for a request (spec 4.10) ----

/// Delivers an envelope to every device of a user, if the sender's view of the user's devices is the hub's (4.6).
async fn deliver(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Path(user): Path<String>,
    body: Bytes,
) -> Res<(StatusCode, Vec<u8>)> {
    let user: UserId = path_id(&user)?;
    let devices_hash: [u8; 32] = headers
        .get(HEADER_DEVICES)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| unb64(v).ok())
        .and_then(|v| v.try_into().ok())
        .ok_or_else(bad)?;
    yo_wire::envelope::check(&body, headers.contains_key(HEADER_DEVICE))?;
    // Sealed: limited by its address only (6.6). An `Authorization` header (an old client's token) is ignored.
    hub.limit_ip("deliver", peer, &headers, hub.config.limits.deliver_per_ip_minute, MINUTE)
        .map_err(|_| error::limited("ip", None))?;
    hub.limit_ip_bytes(peer, &headers, body.len()).map_err(|_| error::limited("bytes", None))?;
    // A contact's, or a request (spec 4.10): the recipient's profile key shown names no sender, every contact has the
    // same. A request is signed by its sending device, so the hub knows whose it is and counts it in its buckets: past
    // them it waits with the sender (`429`, this recipient only, `Retry-After`); unsigned it is `403`. Past the
    // limits above; counted only once stored.
    let sender = match headers.contains_key(HEADER_DEVICE) {
        true => {
            let (uri, headers, body) = (uri.clone(), headers.clone(), body.clone());
            Some(hub.blocking(move |hub| hub.authenticate(&method, &uri, &headers, &body)).await??.user_id)
        }
        false => None,
    };
    let shown: Option<[u8; 32]> = headers
        .get(api::HEADER_ACCESS)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| unb64(v).ok())
        .and_then(|v| <[u8; api::ACCESS_KEY_LEN]>::try_from(v).ok())
        .map(|key| api::access_check(&key));
    // A group send token for the recipient (spec 4.9): a fellow member's, as a contact's; one that does not hold is
    // as none.
    let endorsed = header(&headers, api::HEADER_GROUP_SEND).is_some_and(|token| hub.endorsed(&user, token));
    let by = sender;
    let request = match endorsed {
        true => None,
        false => match hub.blocking(move |hub| hub.db.admit(&user, shown, by.as_ref())).await?? {
            db::Admission::Contact => None,
            db::Admission::Request => sender,
            db::Admission::Closed => return Err(refuse(StatusCode::FORBIDDEN)),
        },
    };
    // Past its buckets: `429` with the time until a token, for this recipient only; the sender holds it till then.
    if let Some(sender) = &request {
        hub.requests.check(sender, &user, hub.clock_ms()).map_err(|(scope, ms)| error::limited(scope, Some(ms)))?;
    }
    hub.db_room()?;
    let (stored, outcome) = tokio::sync::oneshot::channel();
    let runtime = tokio::runtime::Handle::current();
    // Full (the writer behind) or gone: "later", the envelope stays with its sender. Never waits here.
    hub.deliveries
        .try_send(Delivery { user, devices_hash, envelope: body.clone(), stored, runtime })
        .map_err(|_| refuse(StatusCode::SERVICE_UNAVAILABLE))?;
    let outcome = outcome.await.ok().and_then(Result::ok).ok_or_else(|| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
    match outcome {
        // Its devices are woken by the writer (`writer`).
        Delivered::Stored(_) => {
            if let Some(sender) = request {
                hub.requests.take(&sender, &user, hub.clock_ms());
            }
            Ok((StatusCode::ACCEPTED, Vec::new()))
        }
        Delivered::Mismatch(roster) => {
            let answer = encode(&api::Devices { account_key: roster.account_key, devices: roster.devices });
            Err(Refusal(StatusCode::CONFLICT, hub.signed_answer(&headers, uri.path(), &body, answer)))
        }
        // This recipient has no room now, not the sender too many: `507`, so the sender holds back this one's
        // deliveries only, not everyone's (a `429` stops its whole round).
        Delivered::Full => Err(refuse(StatusCode::INSUFFICIENT_STORAGE)),
        Delivered::Unknown => Err(refuse(StatusCode::NOT_FOUND)),
    }
}

// ---- blobs (spec 6.8) ----

/// The longest an upload may take: a whole blob over a slow line, and no longer.
const UPLOAD_TIME: Duration = Duration::from_secs(300);

fn query_u32(uri: &Uri, name: &str) -> Option<u32> {
    uri.query()?.split('&').find_map(|kv| kv.strip_prefix(name)?.strip_prefix('='))?.parse().ok()
}

/// Stores a photo or video for `fetches` fetches (spec 6.8). Signed by the body's hash, named in a header: the request,
/// its quota and the room for it are checked before the body is taken; the body goes to disk as it comes and is kept
/// only if its hash is the signed one. `201` stored, `409` the id is taken, `413` too large, `429` the uploader's
/// quota or a rate, `507` no room at the hub now.
#[allow(clippy::too_many_arguments)]
async fn put_blob(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Body,
) -> Res<StatusCode> {
    use futures_util::StreamExt;
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;
    let id: BlobId = path_id(&id)?;
    let size: u64 = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| refuse(StatusCode::LENGTH_REQUIRED))?;
    if size > limits::MAX_BLOB_LEN as u64 {
        return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
    }
    // A ChaCha20-Poly1305 ciphertext has its 16-byte tag at least.
    if size < 16 {
        return Err(bad());
    }
    let fetches = query_u32(&uri, "fetches").filter(|n| (1..=limits::MAX_BLOB_FETCHES).contains(n)).ok_or_else(bad)?;
    let body_hash: [u8; 32] = headers
        .get(HEADER_BODY_HASH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| unb64(v).ok())
        .and_then(|v| v.try_into().ok())
        .ok_or_else(bad)?;
    let limits = &hub.config.limits;
    hub.limit_ip("blob:put", peer, &headers, limits.blob_put_per_ip_minute, MINUTE)?;
    let quota = limits.blob_quota();
    let (device, _claim) = hub
        .blocking(move |hub| {
            let device = hub.authenticate_hash(&method, &uri, &headers, &body_hash)?;
            // Held before the body comes, so a second upload of the same id is "later" at once (spec 6.8).
            let claim = hub.blobs.claim(id).ok_or_else(|| refuse(StatusCode::SERVICE_UNAVAILABLE))?;
            match hub.db.blob_ready_state(&id)? {
                Some(true) => return Err(refuse(StatusCode::CONFLICT)),
                // Another upload of it is not done: "later", not "there" (spec 6.6).
                Some(false) => return Err(refuse(StatusCode::SERVICE_UNAVAILABLE)),
                None => {}
            }
            if !hub.db.blob_quota_left(&device.user_id, size, now_ms(), &quota)? {
                return Err(refuse(StatusCode::TOO_MANY_REQUESTS));
            }
            Ok((device, claim))
        })
        .await??;
    // A few uploads of one user at once, no more: none can hold the room of everyone's.
    let _slot = hub.blobs.uploading(device.user_id).ok_or_else(|| refuse(StatusCode::TOO_MANY_REQUESTS))?;
    let failed = |error: std::io::Error| {
        eprintln!("yo-hub: blob upload: {error}");
        refuse(StatusCode::INTERNAL_SERVER_ERROR)
    };
    let (room, mut file, incoming) = hub
        .blocking(move |hub| {
            let room = hub.blobs.reserve(size, || hub.db.blobs_stored())?;
            let room = room.map_err(|_| refuse(StatusCode::INSUFFICIENT_STORAGE))?;
            let (file, incoming) = hub.blobs.incoming().map_err(failed)?;
            Ok::<_, Refusal>((room, file, incoming))
        })
        .await??;
    let taken = async {
        let mut stream = body.into_data_stream();
        let mut hash = sha2::Sha256::new();
        let mut got = 0u64;
        while let Some(chunk) = stream.next().await {
            // The client went away, or sent something that is no body.
            let chunk = chunk.map_err(|_| bad())?;
            got += chunk.len() as u64;
            if got > size {
                return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
            }
            hash.update(&chunk);
            file.write_all(&chunk).await.map_err(failed)?;
        }
        file.flush().await.map_err(failed)?;
        if got != size || <[u8; 32]>::from(hash.finalize()) != body_hash {
            return Err(bad());
        }
        Ok(())
    };
    tokio::time::timeout(UPLOAD_TIME, taken).await.map_err(|_| refuse(StatusCode::REQUEST_TIMEOUT))??;
    drop(file);
    hub.blocking(move |hub| {
        // The room is held until the blob's row counts it.
        let _room = room;
        let stored_at = now_ms();
        let put = hub.db.put_blob(&device.device_id, &id, size, fetches, stored_at, &quota)?;
        if let Some(status) = blobs::refused(&put) {
            return Err(refuse(status));
        }
        if let Err(error) = hub.blobs.keep(incoming, &(id, stored_at)) {
            let _ = hub.db.delete_blob(&id);
            return Err(failed(error));
        }
        // Handed out from now on. Left not ready (a crash just before), it goes at the next start or within the hour.
        match hub.db.blob_ready(&id, stored_at) {
            Ok(true) => Ok(StatusCode::CREATED),
            // Its row went meanwhile (the account deleted while it came): no file left behind, and no 201.
            Ok(false) => {
                hub.blobs.remove(&(id, stored_at));
                Err(refuse(StatusCode::NOT_FOUND))
            }
            Err(error) => {
                let _ = hub.db.delete_blob(&id);
                hub.blobs.remove(&(id, stored_at));
                Err(error.into())
            }
        }
    })
    .await?
}

/// A blob, to whoever names its id (no identity: the hub does not learn who fetches what), streamed from disk. A fetch
/// counts when its last byte is handed on; after the last of its fetches the blob is gone. `404`: unknown, fetched by
/// all, or past its time.
async fn get_blob(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<axum::response::Response> {
    hub.limit_ip("blob:get", peer, &headers, hub.config.limits.blob_get_per_ip_minute, MINUTE)?;
    let id: BlobId = path_id(&id)?;
    let ttl = hub.config.limits.blob_ttl_days * DAY.as_millis() as u64;
    // One of its fetches is taken now, and given back if this one is cut off before its last byte (`Taken`).
    let taking = hub.blocking(move |hub| hub.db.take_fetch(&id, now_ms(), ttl)).await??;
    let (size, created_at) = taking.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    let (db, store) = (hub.db.clone(), hub.blobs.clone());
    let taken = blobs::Taken::new(move |complete| blobs::fetched(&db, &store, id, created_at, complete));
    let file = hub.blobs.read(&(id, created_at), size).await.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    let stream = blobs::Counted::new(tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024), size, taken);
    axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
        .header(axum::http::header::CONTENT_LENGTH, size)
        .body(axum::body::Body::from_stream(stream))
        .map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registration_id_out_of_range_or_an_auth_key_off_the_curve_is_refused() {
        let good = ed25519_dalek::SigningKey::from_bytes(&[3; 32]).verifying_key().to_bytes();
        assert!(Hub::device_keys_usable(1, &good) && Hub::device_keys_usable(0x3fff, &good));
        assert!(!Hub::device_keys_usable(0, &good) && !Hub::device_keys_usable(0x4000, &good));
        // About half of all y have no x on the curve.
        let off = (2u8..=255)
            .map(|y| [&[y][..], &[0; 31]].concat().try_into().unwrap())
            .find(|key: &Key| ed25519_dalek::VerifyingKey::from_bytes(key).is_err())
            .unwrap();
        assert!(!Hub::device_keys_usable(1, &off), "no point");
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
        assert!(hub.db.take_fetch(&blob.0, 5, u64::MAX).unwrap().is_none(), "not handed out before it is ready");
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
        hub.deliveries.send(Delivery { user: [1; 16], devices_hash, envelope, stored, runtime }).unwrap();
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
