//! The Yo hub (spec 6): users and their devices, KeyPackages, delivery to users, the order of commits, push. It holds
//! no content key, and the routes that carry messages and commits take no identity: the hub never learns a sender.

pub mod blobs;
pub mod config;
pub mod db;
pub mod error;
pub mod kp;
pub mod limiter;
pub mod push;
pub mod sms;

use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::routing::{delete, get, post, put};
use axum::Router;
use bytes::Bytes;
use config::Config;
use db::{Commit, Db, Delivered, Key, NewDevice, StoredKeyPackage, Write};
use error::{refuse, Refusal, Res};
use hmac::{Hmac, Mac};
use limiter::{Limiter, ReplayCache};
use openmls_rust_crypto::RustCrypto;
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;
use yo_wire::api::{self, HEADER_BODY_HASH, HEADER_DEVICES};
use yo_wire::cert::DeviceCertificate;
use yo_wire::content::BlobId;
use yo_wire::handle::{Handle, KIND_PHONE, KIND_USERNAME};
use yo_wire::limits::{self, KEY_PACKAGE_TARGET, MAX_DEVICES, MAX_INBOX_FETCH, MAX_KEY_PACKAGES_PER_UPLOAD};
use yo_wire::request::{HEADER_DEVICE, HEADER_SIGNATURE, HEADER_TIME};
use yo_wire::{decode, encode, phone, sha256, unb64, DeviceId, GroupId, UserId};

pub struct Hub {
    config: Config,
    db: Arc<Db>,
    deliveries: std::sync::mpsc::Sender<Delivery>,
    crypto: RustCrypto,
    limiter: Limiter,
    replay: ReplayCache,
    waiting: Mutex<HashMap<DeviceId, Arc<Notify>>>,
    /// Inbox requests under way, by device (`INBOX_AT_ONCE`).
    fetching: Mutex<HashMap<DeviceId, u32>>,
    /// The database disk's last answer (`db_room`): when it was asked (ms), and whether it was too full.
    disk_asked: AtomicU64,
    disk_full: AtomicBool,
    sms: Box<dyn sms::Sms>,
    pusher: push::Pusher,
    blobs: Arc<blobs::Blobs>,
}

const HOUR: Duration = Duration::from_secs(3600);
const MINUTE: Duration = Duration::from_secs(60);
const DAY: Duration = Duration::from_secs(24 * 3600);

/// Inbox requests of one device at once: the app, its notification extension, a share. One answer holds up to
/// `MAX_INBOX_FETCH` envelopes of 64 KiB, and a wait holds a connection for a minute: a device that asks more often
/// at once is refused (`429`) rather than let it hold the hub's memory and connections.
pub const INBOX_AT_ONCE: u32 = 4;

/// An inbox request under way; its place is given back when dropped.
struct Fetching<'a> {
    hub: &'a Hub,
    device: DeviceId,
}

impl Drop for Fetching<'_> {
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
}

/// The most envelopes one transaction stores.
const BATCH: usize = 512;

/// Group commit: takes what has queued up, stores it in one transaction, answers each. Under load the batches grow by
/// themselves; alone, an envelope is stored at once.
fn writer(db: Arc<Db>, queue: std::sync::mpsc::Receiver<Delivery>, most: u32) {
    while let Ok(first) = queue.recv() {
        let mut batch = vec![first];
        while batch.len() < BATCH {
            match queue.try_recv() {
                Ok(next) => batch.push(next),
                Err(_) => break,
            }
        }
        let items: Vec<(UserId, [u8; 32], &[u8])> =
            batch.iter().map(|d| (d.user, d.devices_hash, &d.envelope[..])).collect();
        match db.deliver(&items, now_ms(), most) {
            Ok(outcomes) => {
                for (delivery, outcome) in batch.into_iter().zip(outcomes) {
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
    device_key: Key,
}

impl Hub {
    pub fn new(config: Config, push_transport: Arc<dyn push::Transport>) -> Result<Arc<Self>, String> {
        let db = Arc::new(Db::open(&config.db_path).map_err(|e| format!("database: {e}"))?);
        let added = db.seed_reserved(&config.reserved, now_ms()).map_err(|e| format!("reserved names: {e}"))?;
        eprintln!("yo-hub: reserved names: {} in the list, {added} new", config.reserved.len());
        let pusher = push::Pusher::new(push_transport, config.apns.as_ref(), config.fcm.as_ref())?;
        let blobs = blobs::Blobs::open(&config.blobs).map_err(|e| format!("{}: {e}", config.blobs.dir.display()))?;
        let (deliveries, queue) = std::sync::mpsc::channel();
        let writing = db.clone();
        let most = config.limits.max_pending_per_device;
        std::thread::Builder::new()
            .name("yo-hub-writer".into())
            .spawn(move || writer(writing, queue, most))
            .map_err(|e| format!("writer: {e}"))?;
        Ok(Arc::new(Hub {
            sms: sms::from_config(&config.sms),
            db,
            deliveries,
            crypto: RustCrypto::default(),
            limiter: Limiter::default(),
            replay: ReplayCache::default(),
            waiting: Mutex::default(),
            fetching: Mutex::default(),
            disk_asked: AtomicU64::new(0),
            disk_full: AtomicBool::new(false),
            pusher,
            blobs: Arc::new(blobs),
            config,
        }))
    }

    pub fn router(self: &Arc<Self>) -> Router {
        let mut router = Router::new()
            .route("/", get(|| async { axum::response::Html(include_str!("index.html")) }))
            .route(api::REGISTER, post(register))
            .route(api::BACKUP, put(put_backup))
            .route("/v1/backup/{id}", get(fetch_backup))
            .route(api::KEY_PACKAGES, put(key_packages))
            .route(api::LOOKUP, post(lookup))
            .route(api::PUSH, put(set_push))
            .route(api::USERNAME, put(set_username))
            .route(api::ACCOUNT, delete(delete_account))
            .route(api::REPORT, post(report))
            .route(api::FEEDBACK, post(feedback))
            .route(api::STORAGE, get(get_storage).put(put_storage))
            .route("/v1/username/{name}", get(username_free))
            .route(api::INBOX, get(inbox))
            .route(api::INBOX_ACK, post(ack))
            .route("/v1/inbox/{user}", post(deliver))
            .route(api::GROUPS, post(create_group))
            .route("/v1/groups/{group}/commit", post(commit))
            .route("/v1/groups/{group}/commits/{epoch}", get(fetch_commit))
            .route("/v1/blobs/{id}", put(put_blob).get(get_blob));
        // Phone numbers (spec 6.3: kept for later), only where SMS is configured: off, there is no code to guess.
        if !matches!(self.config.sms, config::SmsConfig::Off) {
            router = router.route(api::VERIFY_START, post(verify_start)).route(api::VERIFY_CHECK, post(verify_check));
        }
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
        router.layer(DefaultBodyLimit::max(limits::MAX_BODY)).with_state(self.clone())
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

    fn phone_hmac(&self, digits: &[u8]) -> [u8; 32] {
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&self.config.phone_secret).expect("any key length");
        mac.update(digits);
        mac.finalize().into_bytes().into()
    }

    /// A handle's value as the hub keeps it: a number as its HMAC only, everything else as it is.
    fn stored_handle(&self, handle: &Handle) -> Vec<u8> {
        match handle.kind {
            KIND_PHONE => self.phone_hmac(handle.value.as_slice()).to_vec(),
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
        self.limit(
            format!("signed:{}", yo_wire::b64(&device_id)),
            self.config.limits.signed_per_device_minute,
            MINUTE,
        )?;
        if !self.first_time(&signature, time, now) {
            return Err(unauthorized());
        }
        // When a device was last seen counts only to the day (spec 6.4): one write a day, not one per request.
        if now.saturating_sub(device.last_seen_at) >= DAY.as_millis() as u64 {
            self.db.touch(&device_id, now)?;
        }
        Ok(Signed { device_id, user_id: device.user_id, device_key: device.device_key })
    }

    /// Each signature once (the replay cache), for as long as its time is within the window.
    fn first_time(&self, signature: &[u8; 64], time: u64, now: u64) -> bool {
        self.replay.first_time(*signature, now, time + limits::MAX_SKEW_MS + 1)
    }

    /// Checks uploaded KeyPackages against the uploading device and its account; the last resort must say it is one,
    /// the others not.
    fn check_key_packages(
        &self,
        uploads: &[yo_wire::VLBytes],
        last_resort: Option<&yo_wire::VLBytes>,
        account_key: &Key,
        device_key: &Key,
    ) -> Res<(Vec<StoredKeyPackage>, Option<StoredKeyPackage>)> {
        if uploads.len() > MAX_KEY_PACKAGES_PER_UPLOAD {
            return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
        }
        let check = |bytes: &yo_wire::VLBytes, want_last_resort: bool| {
            kp::check(&self.crypto, bytes.as_slice(), account_key, device_key)
                .filter(|checked| checked.last_resort == want_last_resort)
                .map(|checked| StoredKeyPackage { hash: checked.hash, data: bytes.as_slice().to_vec() })
                .ok_or_else(bad)
        };
        let stored = uploads.iter().map(|k| check(k, false)).collect::<Res<Vec<_>>>()?;
        let last = last_resort.map(|k| check(k, true)).transpose()?;
        Ok((stored, last))
    }

    /// Everything about a registering device that can be checked before an SMS code is spent (spec 6.4): its
    /// certificate, the account key's proof of possession over this handle and device, its KeyPackages.
    fn check_new_device(
        &self,
        handle: &Handle,
        device: &api::NewDevice,
    ) -> Res<(Key, NewDevice, Vec<StoredKeyPackage>, StoredKeyPackage)> {
        if device.push.token.as_slice().len() > limits::MAX_PUSH_TOKEN_LEN {
            return Err(bad());
        }
        let certificate = DeviceCertificate::verify(device.certificate.as_slice(), &device.device_key)?;
        let account_key = certificate.account_key;
        let signed = api::registration_to_be_signed(handle, &device.device_id, &device.device_key, &device.auth_key);
        let key = ed25519_dalek::VerifyingKey::from_bytes(&account_key).map_err(|_| bad())?;
        key.verify_strict(&signed, &ed25519_dalek::Signature::from_bytes(&device.proof)).map_err(|_| bad())?;
        let (key_packages, last_resort) =
            self.check_key_packages(&device.key_packages, Some(&device.last_resort), &account_key, &device.device_key)?;
        let new = NewDevice {
            device_id: device.device_id,
            device_key: device.device_key,
            auth_key: device.auth_key,
            push_platform: device.push.platform,
            push_token: device.push.token.as_slice().to_vec(),
        };
        Ok((account_key, new, key_packages, last_resort.expect("asked for")))
    }

    /// Stores a checked registration and answers the user id.
    fn register_checked(
        &self,
        handle: &Handle,
        by_key: bool,
        checked: (Key, NewDevice, Vec<StoredKeyPackage>, StoredKeyPackage),
    ) -> Res<(StatusCode, Vec<u8>)> {
        let (account_key, device, key_packages, last_resort) = checked;
        let mut fresh: UserId = [0; 16];
        getrandom::getrandom(&mut fresh).map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
        let spelling = match handle.kind {
            KIND_USERNAME => std::str::from_utf8(handle.value.as_slice()).ok(),
            _ => None,
        };
        let written = self.db.register(
            handle.kind,
            &self.stored_handle(handle),
            spelling,
            by_key,
            &fresh,
            &account_key,
            &device,
            &key_packages,
            &last_resort,
            now_ms(),
        )?;
        match written {
            Write::Done(user_id) => Ok(ok(encode(&api::Registered { user_id }))),
            Write::Refused(_) => Err(refuse(StatusCode::CONFLICT)),
        }
    }

    /// A place for one more inbox request of a device, if it has fewer than `INBOX_AT_ONCE` under way.
    fn fetching(&self, device: DeviceId) -> Option<Fetching<'_>> {
        let mut fetching = self.fetching.lock().unwrap_or_else(PoisonError::into_inner);
        let n = fetching.entry(device).or_default();
        if *n >= INBOX_AT_ONCE {
            return None;
        }
        *n += 1;
        Some(Fetching { hub: self, device })
    }

    fn waiter(&self, device: &DeviceId) -> Arc<Notify> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner).entry(*device).or_default().clone()
    }

    fn wake_waiters(&self, device: &DeviceId) {
        if let Some(notify) = self.waiting.lock().unwrap_or_else(PoisonError::into_inner).get(device) {
            notify.notify_waiters();
        }
    }

    fn forget_waiter(&self, device: &DeviceId, notify: Arc<Notify>) {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        // Two references: the map's and ours. Nobody else waits.
        if Arc::strong_count(&notify) == 2 {
            waiting.remove(device);
        }
    }

    fn push(self: &Arc<Self>, wake: db::Wake) {
        if wake.push_token.is_empty() {
            return;
        }
        let hub = self.clone();
        tokio::spawn(async move {
            match hub.pusher.wake(wake.push_platform, &wake.push_token).await {
                push::Outcome::TokenInvalid => {
                    if let Err(error) = hub.db.clear_push(&wake.device_id, &wake.push_token) {
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

async fn verify_start(State(hub): Hubs, ConnectInfo(peer): Peer, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let request: api::VerifyStart = decode(&body)?;
    phone::check(request.phone.as_slice())?;
    let limits = &hub.config.limits;
    let phone_key = yo_wire::b64(&hub.phone_hmac(request.phone.as_slice()));
    hub.limit_ip("start", peer, &headers, limits.verify_per_ip_hour, HOUR)?;
    hub.limit(format!("start:phone:{phone_key}"), limits.verify_per_phone_hour, HOUR)?;
    let digits = String::from_utf8(request.phone.as_slice().to_vec()).map_err(|_| bad())?;
    hub.sms.start(digits).await.map_err(|error| {
        eprintln!("yo-hub: sms: {error}");
        refuse(StatusCode::BAD_GATEWAY)
    })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn verify_check(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    body: Bytes,
) -> Res<(StatusCode, Vec<u8>)> {
    let request: api::VerifyCheck = decode(&body)?;
    let digits = String::from_utf8(request.phone.as_slice().to_vec()).map_err(|_| bad())?;
    phone::check(digits.as_bytes())?;
    let handle = Handle::phone(&digits);
    let limits = &hub.config.limits;
    hub.limit_ip("check", peer, &headers, limits.verify_per_ip_hour.saturating_mul(3), HOUR)?;
    hub.limit(format!("check:phone:{}", yo_wire::b64(&hub.stored_handle(&handle))), limits.check_per_phone_hour, HOUR)?;
    let code = String::from_utf8(request.code.as_slice().to_vec()).map_err(|_| bad())?;
    if code.is_empty() || code.len() > limits::MAX_CODE_LEN {
        return Err(bad());
    }
    // Everything is checked before the code is spent.
    let checked = hub.check_new_device(&handle, &request.device)?;
    let approved = hub.sms.check(digits, code).await.map_err(|error| {
        eprintln!("yo-hub: sms: {error}");
        refuse(StatusCode::BAD_GATEWAY)
    })?;
    if !approved {
        return Err(refuse(StatusCode::FORBIDDEN));
    }
    hub.register_checked(&handle, false, checked)
}

/// A username is all it takes (spec 6.4): first come, first served, limited per address. The name's user again with
/// its current account key (a new phone restored from the backup), else `409`.
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
    let checked = hub.check_new_device(&handle, &request.device)?;
    hub.register_checked(&handle, true, checked)
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
    // Keys a signature can be checked against: a key that is no point is refused here, not at every fetch.
    let (passkey, code) = backup.auth_keys();
    if passkey.into_iter().chain(code).any(|key| ed25519_dalek::VerifyingKey::from_bytes(&key).is_err()) {
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
static NO_KEY: std::sync::LazyLock<Key> =
    std::sync::LazyLock::new(|| ed25519_dalek::SigningKey::from_bytes(&[0; 32]).verifying_key().to_bytes());

/// A backup record by its user id or locator, to a request signed by one of its doors' auth keys (spec 7): only who
/// holds a door learns that there is one. Anything else is `404`, as for no record. A record stored before the auth
/// keys is handed out without a signature until `legacy_backups_until_ms`, to no one after.
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
    if let Some((record, auth)) = &found {
        if auth.code.is_none() && now < hub.config.limits.legacy_backups_until_ms {
            return Ok(ok(record.clone()));
        }
    }
    let not_found = || refuse(StatusCode::NOT_FOUND);
    let (time, signature) = signature_of(&headers, now).ok_or_else(not_found)?;
    let auth = found.as_ref().map(|(_, auth)| auth.clone()).unwrap_or_default();
    let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let empty = sha256(&[]);
    let signed = [auth.passkey, auth.code]
        .map(|key| {
            let verified = yo_wire::request::verify_hash(
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

async fn key_packages(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::KeyPackages = decode(&body)?;
    let account_key = hub.db.account_key(&device.user_id)?.ok_or_else(|| refuse(StatusCode::UNAUTHORIZED))?;
    let (stored, last) =
        hub.check_key_packages(&request.key_packages, request.last_resort.as_ref(), &account_key, &device.device_key)?;
    match hub.db.add_key_packages(&device.device_id, &stored, last.as_ref(), KEY_PACKAGE_TARGET * 2)? {
        Write::Done(()) => Ok(StatusCode::NO_CONTENT),
        Write::Refused(_) => Err(refuse(StatusCode::CONFLICT)),
    }
}

async fn lookup(
    State(hub): Hubs,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Res<(StatusCode, Vec<u8>)> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::Lookup = decode(&body)?;
    request.handle.check()?;
    if request.except.len() > MAX_DEVICES {
        return Err(bad());
    }
    let limits = &hub.config.limits;
    let take = request.key_packages & api::LOOKUP_KEY_PACKAGES != 0;
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
        device_keys: found.device_keys.into_iter().take(MAX_DEVICES).collect(),
        key_packages: found.key_packages.into_iter().take(MAX_DEVICES).map(Into::into).collect(),
    };
    // The username only to who asks for it: an app from before spellings reads a `LookupResult` and nothing after.
    if request.key_packages & api::LOOKUP_USERNAME == 0 {
        return Ok(ok(encode(&result)));
    }
    let username = found.username.map(|name| name.into_bytes().into());
    Ok(ok(encode(&api::LookupNamed { found: result, username })))
}

/// Claims or gives up the signing device's account's username.
async fn set_username(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::SetUsername = decode(&body)?;
    if let Some(name) = &request.username {
        Handle { kind: KIND_USERNAME, value: name.clone() }.check()?;
    }
    // Checked above: UTF-8.
    let spelling = request.username.as_ref().and_then(|n| std::str::from_utf8(n.as_slice()).ok());
    let skeleton = spelling.map(yo_wire::handle::skeleton);
    match hub.db.set_username(&device.user_id, skeleton.as_deref().zip(spelling).map(|(s, n)| (s.as_bytes(), n)))? {
        Write::Done(()) => Ok(StatusCode::NO_CONTENT),
        Write::Refused(_) => Err(refuse(StatusCode::CONFLICT)),
    }
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
    hub.db.set_push(&device.device_id, request.platform, request.token.as_slice())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn inbox(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap) -> Res<(StatusCode, Vec<u8>)> {
    let device = hub.authenticate(&method, &uri, &headers, &[])?;
    let _fetching = hub.fetching(device.device_id).ok_or_else(|| refuse(StatusCode::TOO_MANY_REQUESTS))?;
    let wait_s = uri
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("wait=")))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        .min(limits::MAX_WAIT_S);
    let mut items = hub.db.fetch(&device.device_id, MAX_INBOX_FETCH)?;
    if items.is_empty() && wait_s > 0 {
        let notify = hub.waiter(&device.device_id);
        let fetched = async {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            // Mail that came between the first fetch and `enable` is found here.
            let items = hub.db.fetch(&device.device_id, MAX_INBOX_FETCH)?;
            if !items.is_empty() {
                return Ok::<_, Refusal>(items);
            }
            let _ = tokio::time::timeout(Duration::from_secs(wait_s), notified).await;
            Ok(hub.db.fetch(&device.device_id, MAX_INBOX_FETCH)?)
        }
        .await;
        hub.forget_waiter(&device.device_id, notify);
        items = fetched?;
    }
    let inbox = api::Inbox {
        key_packages_left: hub.db.key_packages_left(&device.device_id)?,
        items: items.into_iter().map(|(id, envelope)| api::InboxItem { id, envelope: envelope.into() }).collect(),
    };
    Ok(ok(encode(&inbox)))
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

// ---- sealed sender: no identity on these routes ----

/// Delivers an envelope to every device of a user, if the sender's view of the user's devices is the hub's (4.6).
async fn deliver(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
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
    yo_wire::envelope::check(&body)?;
    hub.limit_ip("deliver", peer, &headers, hub.config.limits.deliver_per_ip_minute, MINUTE)?;
    hub.limit_ip_bytes(peer, &headers, body.len())?;
    hub.db_room()?;
    let (stored, outcome) = tokio::sync::oneshot::channel();
    hub.deliveries
        .send(Delivery { user, devices_hash, envelope: body, stored })
        .map_err(|_| refuse(StatusCode::SERVICE_UNAVAILABLE))?;
    let outcome = outcome.await.ok().and_then(Result::ok).ok_or_else(|| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
    match outcome {
        Delivered::Stored(devices) => {
            for wake in devices {
                hub.wake_waiters(&wake.device_id);
                hub.push(wake);
            }
            Ok((StatusCode::ACCEPTED, Vec::new()))
        }
        Delivered::Mismatch(roster) => Err(Refusal(
            StatusCode::CONFLICT,
            encode(&api::Devices { account_key: roster.account_key, device_keys: roster.device_keys }),
        )),
        Delivered::Full => Err(refuse(StatusCode::TOO_MANY_REQUESTS)),
        Delivered::Unknown => Err(refuse(StatusCode::NOT_FOUND)),
    }
}

async fn create_group(State(hub): Hubs, ConnectInfo(peer): Peer, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    hub.limit_ip("groups", peer, &headers, hub.config.limits.groups_per_ip_hour, HOUR)?;
    let request: api::GroupCreate = decode(&body)?;
    if request.epoch == 0 || request.epoch > u32::MAX as u64 {
        return Err(bad());
    }
    hub.db_room()?;
    if hub.db.create_group(&request.group_id, request.epoch, &request.token_hash, now_ms())? {
        Ok(StatusCode::CREATED)
    } else {
        Err(refuse(StatusCode::CONFLICT))
    }
}

async fn commit(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(group): Path<String>,
    body: Bytes,
) -> Res<StatusCode> {
    hub.limit_ip("commit", peer, &headers, hub.config.limits.commit_per_ip_minute, MINUTE)?;
    let group: GroupId = path_id(&group)?;
    let request: api::Commit = decode(&body)?;
    yo_wire::envelope::check(request.envelope.as_slice())?;
    hub.limit_ip_bytes(peer, &headers, request.envelope.as_slice().len())?;
    hub.db_room()?;
    let ordered = hub.db.commit(
        &group,
        request.epoch,
        &sha256(&request.token),
        &request.next_token_hash,
        request.envelope.as_slice(),
        now_ms(),
    )?;
    match ordered {
        Commit::Accepted => Ok(StatusCode::OK),
        Commit::Stale(epoch) => Err(Refusal(StatusCode::CONFLICT, encode(&api::Epoch { epoch }))),
        Commit::BadToken => Err(refuse(StatusCode::FORBIDDEN)),
        Commit::Unknown => Err(refuse(StatusCode::NOT_FOUND)),
    }
}

/// The commit accepted on an epoch, for a member that missed it (spec 4.3).
async fn fetch_commit(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path((group, epoch)): Path<(String, u64)>,
) -> Res<(StatusCode, Vec<u8>)> {
    hub.limit_ip("commit", peer, &headers, hub.config.limits.commit_per_ip_minute, MINUTE)?;
    let group: GroupId = path_id(&group)?;
    match hub.db.commit_envelope(&group, epoch)? {
        Ok(envelope) => Ok(ok(envelope)),
        // Accepted once, kept no more (past its 30 days): a member that needs it cannot follow the group (4.9).
        Err(true) => Err(refuse(StatusCode::GONE)),
        Err(false) => Err(refuse(StatusCode::NOT_FOUND)),
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
    let device = hub.authenticate_hash(&method, &uri, &headers, &body_hash)?;
    let quota = limits.blob_quota();
    let now = now_ms();
    if hub.db.blob_exists(&id)? {
        return Err(refuse(StatusCode::CONFLICT));
    }
    if !hub.db.blob_quota_left(&device.user_id, size, now, &quota)? {
        return Err(refuse(StatusCode::TOO_MANY_REQUESTS));
    }
    // A few uploads of one user at once, no more: none can hold the room of everyone's.
    let _slot = hub.blobs.uploading(device.user_id).ok_or_else(|| refuse(StatusCode::TOO_MANY_REQUESTS))?;
    let _room =
        hub.blobs.reserve(size, || hub.db.blobs_stored())?.map_err(|_| refuse(StatusCode::INSUFFICIENT_STORAGE))?;
    let failed = |error: std::io::Error| {
        eprintln!("yo-hub: blob upload: {error}");
        refuse(StatusCode::INTERNAL_SERVER_ERROR)
    };
    let (mut file, incoming) = hub.blobs.incoming().map_err(failed)?;
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
    let stored_at = now_ms();
    let put = hub.db.put_blob(&device.device_id, &id, size, fetches, stored_at, &quota)?;
    if let Some(status) = blobs::refused(&put) {
        return Err(refuse(status));
    }
    if let Err(error) = hub.blobs.keep(incoming, &(id, stored_at)) {
        let _ = hub.db.delete_blob(&id);
        return Err(failed(error));
    }
    Ok(StatusCode::CREATED)
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
    let (size, created_at) = hub.db.blob(&id, now_ms(), ttl)?.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    let file = hub.blobs.read(&(id, created_at), size).await.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    let (db, store) = (hub.db.clone(), hub.blobs.clone());
    let stream = blobs::Counted {
        inner: tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024),
        left: size,
        done: Some(Box::new(move || blobs::fetched(&db, &store, id, created_at))),
    };
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
    fn a_device_has_a_few_inbox_requests_under_way_at_most() {
        let hub = Hub::new(Config::for_tests(), Arc::new(push::Recorder::default())).unwrap();
        let held: Vec<_> = (0..INBOX_AT_ONCE).map(|_| hub.fetching([1; 16]).expect("a place")).collect();
        assert!(hub.fetching([1; 16]).is_none(), "one more is refused");
        assert!(hub.fetching([2; 16]).is_some(), "another device has its own");
        drop(held);
        assert!(hub.fetching([1; 16]).is_some(), "places are given back");
        assert!(hub.fetching.lock().unwrap().is_empty(), "and forgotten");
    }
}
