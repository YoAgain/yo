//! The Yo hub (spec 6): mailboxes, KeyPackages, the order of commits, push. It holds no content key, and the
//! routes that carry messages and commits take no identity: the hub never learns a sender.

pub mod config;
pub mod db;
pub mod error;
pub mod kp;
pub mod limiter;
pub mod push;
pub mod sms;

use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::routing::{get, post, put};
use axum::Router;
use bytes::Bytes;
use config::Config;
use db::{Commit, Db, Mailboxes, NewDevice, PhoneHmac, StoredKeyPackage, Write};
use error::{refuse, Refusal, Res};
use hmac::{Hmac, Mac};
use limiter::{Limiter, ReplayCache};
use openmls_rust_crypto::RustCrypto;
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;
use yo_wire::api::{self, status};
use yo_wire::cert::DeviceCertificate;
use yo_wire::limits::{self, KEY_PACKAGE_TARGET, MAX_DEVICES, MAX_INBOX_FETCH, MAX_KEY_PACKAGES_PER_UPLOAD};
use yo_wire::request::{HEADER_DEVICE, HEADER_SIGNATURE, HEADER_TIME};
use yo_wire::{decode, encode, phone, sha256, unb64, GroupId, Mailbox};

pub struct Hub {
    config: Config,
    db: Arc<Db>,
    deliveries: std::sync::mpsc::Sender<Delivery>,
    crypto: RustCrypto,
    limiter: Limiter,
    replay: ReplayCache,
    waiting: Mutex<HashMap<Mailbox, Arc<Notify>>>,
    sms: Box<dyn sms::Sms>,
    pusher: push::Pusher,
}

const HOUR: Duration = Duration::from_secs(3600);
const MINUTE: Duration = Duration::from_secs(60);
const DAY: Duration = Duration::from_secs(24 * 3600);

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn status_code(code: u16) -> StatusCode {
    StatusCode::from_u16(code).expect("a status of yo_wire::api::status")
}

fn ok(body: Vec<u8>) -> (StatusCode, Vec<u8>) {
    (StatusCode::OK, body)
}

/// An envelope on its way to the writer, which stores many in one transaction.
struct Delivery {
    mailbox: Mailbox,
    envelope: Bytes,
    stored: tokio::sync::oneshot::Sender<Result<Mailboxes, ()>>,
}

/// The most envelopes one transaction stores.
const BATCH: usize = 512;

/// Group commit: takes what has queued up, stores it in one transaction, answers each. Under load the batches
/// grow by themselves; alone, an envelope is stored at once.
fn writer(db: Arc<Db>, queue: std::sync::mpsc::Receiver<Delivery>) {
    while let Ok(first) = queue.recv() {
        let mut batch = vec![first];
        while batch.len() < BATCH {
            match queue.try_recv() {
                Ok(next) => batch.push(next),
                Err(_) => break,
            }
        }
        let items: Vec<(Mailbox, &[u8])> = batch.iter().map(|d| (d.mailbox, &d.envelope[..])).collect();
        match db.deliver(&items, now_ms()) {
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
    mailbox: Mailbox,
    phone: PhoneHmac,
}

impl Hub {
    pub fn new(config: Config, push_transport: Arc<dyn push::Transport>) -> Result<Arc<Self>, String> {
        let db = Arc::new(Db::open(&config.db_path).map_err(|e| format!("database: {e}"))?);
        let pusher = push::Pusher::new(push_transport, config.apns.as_ref(), config.fcm.as_ref())?;
        let (deliveries, queue) = std::sync::mpsc::channel();
        let writing = db.clone();
        std::thread::Builder::new()
            .name("yo-hub-writer".into())
            .spawn(move || writer(writing, queue))
            .map_err(|e| format!("writer: {e}"))?;
        Ok(Arc::new(Hub {
            sms: sms::from_config(&config.sms),
            db,
            deliveries,
            crypto: RustCrypto::default(),
            limiter: Limiter::default(),
            replay: ReplayCache::default(),
            waiting: Mutex::default(),
            pusher,
            config,
        }))
    }

    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route(api::VERIFY_START, post(verify_start))
            .route(api::VERIFY_CHECK, post(verify_check))
            .route(api::KEY_PACKAGES, put(key_packages))
            .route(api::LOOKUP, post(lookup))
            .route(api::PUSH, put(set_push))
            .route(api::INBOX, get(inbox))
            .route(api::INBOX_ACK, post(ack))
            .route("/v1/inbox/{mailbox}", post(deliver))
            .route(api::GROUPS, post(create_group))
            .route("/v1/groups/{group}/commit", post(commit))
            .layer(DefaultBodyLimit::max(limits::MAX_BODY))
            .with_state(self.clone())
    }

    /// Serves until `shutdown` resolves; prunes old envelopes every hour meanwhile.
    pub async fn serve(
        self: Arc<Self>,
        listener: tokio::net::TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        let pruning = self.clone();
        let pruner = tokio::spawn(async move {
            let mut every = tokio::time::interval(HOUR);
            loop {
                every.tick().await;
                let ttl = pruning.config.limits.envelope_ttl_days * 24 * 3600 * 1000;
                if let Err(error) = pruning.db.prune(now_ms(), ttl) {
                    eprintln!("yo-hub: prune: {error}");
                }
            }
        });
        let app = self.router().into_make_service_with_connect_info::<SocketAddr>();
        let served = axum::serve(listener, app).with_graceful_shutdown(shutdown).await;
        pruner.abort();
        served
    }

    fn phone_hmac(&self, digits: &[u8]) -> PhoneHmac {
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&self.config.phone_secret).expect("any key length");
        mac.update(digits);
        mac.finalize().into_bytes().into()
    }

    fn ip(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
        if self.config.trust_cloudflare {
            if let Some(ip) = headers.get("cf-connecting-ip").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok())
            {
                return ip;
            }
        }
        peer.ip()
    }

    fn limit(&self, key: String, per: u32, window: Duration) -> Res<()> {
        if self.limiter.allow(key, per, window) {
            Ok(())
        } else {
            Err(refuse(StatusCode::TOO_MANY_REQUESTS))
        }
    }

    fn authenticate(&self, method: &Method, uri: &Uri, headers: &HeaderMap, body: &[u8]) -> Res<Signed> {
        let unauthorized = || refuse(StatusCode::UNAUTHORIZED);
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).ok_or_else(unauthorized);
        let mailbox: Mailbox =
            unb64(header(HEADER_DEVICE)?).ok().and_then(|m| m.try_into().ok()).ok_or_else(unauthorized)?;
        let time: u64 = header(HEADER_TIME)?.parse().map_err(|_| unauthorized())?;
        let signature: [u8; 64] =
            unb64(header(HEADER_SIGNATURE)?).ok().and_then(|s| s.try_into().ok()).ok_or_else(unauthorized)?;
        let now = now_ms();
        if now.abs_diff(time) > limits::MAX_SKEW_MS {
            return Err(unauthorized());
        }
        let Some(device) = self.db.device(&mailbox)? else {
            // A replaced device learns that it was replaced.
            return Err(if self.db.is_gone(&mailbox)? { refuse(StatusCode::GONE) } else { unauthorized() });
        };
        let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        yo_wire::request::verify(&device.auth_key, method.as_str(), path, time, body, &signature)
            .map_err(|_| unauthorized())?;
        if !self.replay.first_time(signature, now, time + limits::MAX_SKEW_MS + 1) {
            return Err(unauthorized());
        }
        self.db.touch(&mailbox, now)?;
        Ok(Signed { mailbox, phone: device.phone })
    }

    /// Checks uploaded KeyPackages against the uploader; the last resort must say it is one, the others not.
    fn check_key_packages(
        &self,
        uploads: &[yo_wire::VLBytes],
        last_resort: Option<&yo_wire::VLBytes>,
        phone: &[u8],
        account_key: &[u8; 32],
        mailbox: &Mailbox,
    ) -> Res<(Vec<StoredKeyPackage>, Option<StoredKeyPackage>)> {
        if uploads.len() > MAX_KEY_PACKAGES_PER_UPLOAD {
            return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
        }
        let check = |bytes: &yo_wire::VLBytes, want_last_resort: bool| {
            kp::check(&self.crypto, bytes.as_slice(), phone, account_key, mailbox)
                .filter(|checked| checked.last_resort == want_last_resort)
                .map(|checked| StoredKeyPackage { hash: checked.hash, data: bytes.as_slice().to_vec() })
                .ok_or_else(|| refuse(StatusCode::BAD_REQUEST))
        };
        let stored = uploads.iter().map(|k| check(k, false)).collect::<Res<Vec<_>>>()?;
        let last = last_resort.map(|k| check(k, true)).transpose()?;
        Ok((stored, last))
    }

    fn waiter(&self, mailbox: &Mailbox) -> Arc<Notify> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner).entry(*mailbox).or_default().clone()
    }

    fn wake_waiters(&self, mailbox: &Mailbox) {
        if let Some(notify) = self.waiting.lock().unwrap_or_else(PoisonError::into_inner).get(mailbox) {
            notify.notify_waiters();
        }
    }

    fn forget_waiter(&self, mailbox: &Mailbox, notify: Arc<Notify>) {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        // Two references: the map's and ours. Nobody else waits.
        if Arc::strong_count(&notify) == 2 {
            waiting.remove(mailbox);
        }
    }
}

type Hubs = State<Arc<Hub>>;
type Peer = ConnectInfo<SocketAddr>;

async fn verify_start(State(hub): Hubs, ConnectInfo(peer): Peer, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let request: api::VerifyStart = decode(&body)?;
    phone::check(request.phone.as_slice())?;
    let limits = &hub.config.limits;
    let phone_key = yo_wire::b64(&hub.phone_hmac(request.phone.as_slice()));
    hub.limit(format!("start:ip:{}", hub.ip(peer, &headers)), limits.verify_per_ip_hour, HOUR)?;
    hub.limit(format!("start:phone:{phone_key}"), limits.verify_per_phone_hour, HOUR)?;
    let digits = String::from_utf8(request.phone.as_slice().to_vec()).map_err(|_| refuse(StatusCode::BAD_REQUEST))?;
    hub.sms.start(digits).await.map_err(|error| {
        eprintln!("yo-hub: sms: {error}");
        refuse(StatusCode::BAD_GATEWAY)
    })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn verify_check(State(hub): Hubs, ConnectInfo(peer): Peer, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let request: api::VerifyCheck = decode(&body)?;
    let digits = request.phone.as_slice();
    phone::check(digits)?;
    let phone = hub.phone_hmac(digits);
    let limits = &hub.config.limits;
    hub.limit(format!("check:ip:{}", hub.ip(peer, &headers)), limits.verify_per_ip_hour.saturating_mul(3), HOUR)?;
    hub.limit(format!("check:phone:{}", yo_wire::b64(&phone)), limits.check_per_phone_hour, HOUR)?;
    let code = String::from_utf8(request.code.as_slice().to_vec()).map_err(|_| refuse(StatusCode::BAD_REQUEST))?;
    if code.is_empty()
        || code.len() > limits::MAX_CODE_LEN
        || request.push.token.as_slice().len() > limits::MAX_PUSH_TOKEN_LEN
    {
        return Err(refuse(StatusCode::BAD_REQUEST));
    }
    let certificate = DeviceCertificate::verify(request.certificate.as_slice())?;
    let body = &certificate.body;
    if body.phone() != digits {
        return Err(refuse(StatusCode::BAD_REQUEST));
    }
    // Everything is checked before the code is spent.
    let (key_packages, last_resort) = hub.check_key_packages(
        &request.key_packages,
        Some(&request.last_resort),
        digits,
        &body.account_key,
        &body.mailbox,
    )?;
    let last_resort = last_resort.expect("asked for");
    let approved = hub.sms.check(String::from_utf8_lossy(digits).into_owned(), code).await.map_err(|error| {
        eprintln!("yo-hub: sms: {error}");
        refuse(StatusCode::BAD_GATEWAY)
    })?;
    if !approved {
        return Err(refuse(StatusCode::FORBIDDEN));
    }
    let device = NewDevice {
        mailbox: body.mailbox,
        auth_key: request.auth_key,
        push_platform: request.push.platform,
        push_token: request.push.token.as_slice().to_vec(),
    };
    match hub.db.register(&phone, &body.account_key, &device, &key_packages, &last_resort, now_ms())? {
        Write::Done(()) => Ok(StatusCode::NO_CONTENT),
        Write::Refused(_) => Err(refuse(StatusCode::CONFLICT)),
    }
}

async fn key_packages(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::KeyPackages = decode(&body)?;
    let account_key = hub.db.account_key(&device.phone)?.ok_or_else(|| refuse(StatusCode::UNAUTHORIZED))?;
    // The hub knows the number only as its HMAC: the certificate names it, the HMAC must match.
    let first =
        request.key_packages.first().or(request.last_resort.as_ref()).ok_or_else(|| refuse(StatusCode::BAD_REQUEST))?;
    let digits = phone_of(first.as_slice()).ok_or_else(|| refuse(StatusCode::BAD_REQUEST))?;
    if hub.phone_hmac(&digits) != device.phone {
        return Err(refuse(StatusCode::BAD_REQUEST));
    }
    let (stored, last) = hub.check_key_packages(
        &request.key_packages,
        request.last_resort.as_ref(),
        &digits,
        &account_key,
        &device.mailbox,
    )?;
    match hub.db.add_key_packages(&device.mailbox, &stored, last.as_ref(), KEY_PACKAGE_TARGET * 2)? {
        Write::Done(()) => Ok(StatusCode::NO_CONTENT),
        Write::Refused(_) => Err(refuse(StatusCode::CONFLICT)),
    }
}

/// The phone number a KeyPackage's certificate names, unchecked: the caller checks the whole KeyPackage after.
fn phone_of(key_package: &[u8]) -> Option<Vec<u8>> {
    use openmls::prelude::{BasicCredential, KeyPackageIn};
    use tls_codec::Deserialize as _;
    let key_package = KeyPackageIn::tls_deserialize_exact(key_package).ok()?;
    let leaf = key_package.unverified_credential();
    let credential = BasicCredential::try_from(leaf.credential).ok()?;
    let certificate: DeviceCertificate = decode(credential.identity()).ok()?;
    Some(certificate.body.phone().to_vec())
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
    phone::check(request.phone.as_slice())?;
    hub.limit(format!("lookup:{}", yo_wire::b64(&device.mailbox)), hub.config.limits.lookup_per_device_day, DAY)?;
    let phone = hub.phone_hmac(request.phone.as_slice());
    let (account_key, key_packages) =
        hub.db.lookup(&phone, request.key_packages == 1)?.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    let key_packages = key_packages.into_iter().take(MAX_DEVICES).map(Into::into).collect();
    Ok(ok(encode(&api::LookupResult { account_key, key_packages })))
}

async fn set_push(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    let device = hub.authenticate(&method, &uri, &headers, &body)?;
    let request: api::Push = decode(&body)?;
    if request.token.as_slice().len() > limits::MAX_PUSH_TOKEN_LEN {
        return Err(refuse(StatusCode::BAD_REQUEST));
    }
    hub.db.set_push(&device.mailbox, request.platform, request.token.as_slice())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn inbox(State(hub): Hubs, method: Method, uri: Uri, headers: HeaderMap) -> Res<(StatusCode, Vec<u8>)> {
    let device = hub.authenticate(&method, &uri, &headers, &[])?;
    let wait_s = uri
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("wait=")))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        .min(limits::MAX_WAIT_S);
    let mut items = hub.db.fetch(&device.mailbox, MAX_INBOX_FETCH)?;
    if items.is_empty() && wait_s > 0 {
        let notify = hub.waiter(&device.mailbox);
        let fetched = async {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            // Mail that came between the first fetch and `enable` is found here.
            let items = hub.db.fetch(&device.mailbox, MAX_INBOX_FETCH)?;
            if !items.is_empty() {
                return Ok::<_, Refusal>(items);
            }
            let _ = tokio::time::timeout(Duration::from_secs(wait_s), notified).await;
            Ok(hub.db.fetch(&device.mailbox, MAX_INBOX_FETCH)?)
        }
        .await;
        hub.forget_waiter(&device.mailbox, notify);
        items = fetched?;
    }
    let inbox = api::Inbox {
        key_packages_left: hub.db.key_packages_left(&device.mailbox)?,
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
    hub.db.ack(&device.mailbox, &request.ids)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Sealed sender: no identity, only the envelope and where it goes.
async fn deliver(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(mailbox): Path<String>,
    body: Bytes,
) -> Res<StatusCode> {
    let mailbox: Mailbox =
        unb64(&mailbox).ok().and_then(|m| m.try_into().ok()).ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    let limits = &hub.config.limits;
    hub.limit(format!("deliver:ip:{}", hub.ip(peer, &headers)), limits.deliver_per_ip_minute, MINUTE)?;
    hub.limit(format!("deliver:to:{}", yo_wire::b64(&mailbox)), limits.deliver_per_mailbox_minute, MINUTE)?;
    yo_wire::envelope::check(&body)?;
    let (stored, outcome) = tokio::sync::oneshot::channel();
    hub.deliveries
        .send(Delivery { mailbox, envelope: body, stored })
        .map_err(|_| refuse(StatusCode::SERVICE_UNAVAILABLE))?;
    let outcome = outcome.await.ok().and_then(Result::ok).ok_or_else(|| refuse(StatusCode::INTERNAL_SERVER_ERROR))?;
    match outcome {
        Mailboxes::Live { push_platform, push_token } => {
            hub.wake_waiters(&mailbox);
            if !push_token.is_empty() {
                let hub = hub.clone();
                tokio::spawn(async move {
                    match hub.pusher.wake(push_platform, &push_token).await {
                        push::Outcome::TokenInvalid => {
                            if let Err(error) = hub.db.clear_push(&mailbox, &push_token) {
                                eprintln!("yo-hub: clear push: {error}");
                            }
                        }
                        push::Outcome::Failed(error) => eprintln!("yo-hub: push: {error}"),
                        push::Outcome::Sent | push::Outcome::Skipped => {}
                    }
                });
            }
            Ok(StatusCode::ACCEPTED)
        }
        Mailboxes::Gone => Err(refuse(status_code(status::GONE))),
        Mailboxes::Unknown => Err(refuse(StatusCode::NOT_FOUND)),
    }
}

async fn create_group(State(hub): Hubs, ConnectInfo(peer): Peer, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    hub.limit(format!("groups:ip:{}", hub.ip(peer, &headers)), hub.config.limits.groups_per_ip_hour, HOUR)?;
    let request: api::GroupCreate = decode(&body)?;
    if request.epoch == 0 || request.epoch > u32::MAX as u64 {
        return Err(refuse(StatusCode::BAD_REQUEST));
    }
    if hub.db.create_group(&request.group_id, request.epoch, &request.token_hash, now_ms())? {
        Ok(StatusCode::CREATED)
    } else {
        Err(refuse(StatusCode::CONFLICT))
    }
}

async fn commit(State(hub): Hubs, Path(group): Path<String>, body: Bytes) -> Res<StatusCode> {
    let group: GroupId =
        unb64(&group).ok().and_then(|g| g.try_into().ok()).ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    let request: api::Commit = decode(&body)?;
    match hub.db.commit(&group, request.epoch, &sha256(&request.token), &request.next_token_hash, now_ms())? {
        Commit::Accepted => Ok(StatusCode::OK),
        Commit::Stale(epoch) => Err(Refusal(StatusCode::CONFLICT, encode(&api::Epoch { epoch }))),
        Commit::BadToken => Err(refuse(StatusCode::FORBIDDEN)),
        Commit::Unknown => Err(refuse(StatusCode::NOT_FOUND)),
    }
}
