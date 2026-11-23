//! What every route shares: admission of a request, its signature (spec 6.2), its limits (spec 6.6), small parsers.

use crate::error::{self, refuse, Refusal, Res};
use crate::{config, Hub};
use axum::extract::{ConnectInfo, FromRequest, Request, State};
use axum::http::header::{AsHeaderName, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use yo_wire::request::{HEADER_DEVICE, HEADER_SIGNATURE, HEADER_TIME};
use yo_wire::{api, b64, hubsig, limits, sha256, unb64, DeviceId, UserId};

pub(crate) type Hubs = State<Arc<Hub>>;
pub(crate) type Peer = ConnectInfo<SocketAddr>;

pub(crate) const MINUTE: Duration = Duration::from_secs(60);
pub(crate) const HOUR: Duration = Duration::from_secs(3600);
pub(crate) const DAY: Duration = Duration::from_secs(24 * 3600);
pub(crate) const DAY_MS: u64 = 24 * 3600 * 1000;

/// Requests with a body read and handled at once (`admit`), each up to `MAX_BODY`: beyond, `503` before reading.
pub const READING_AT_ONCE: usize = 512;

/// The time a body may take to arrive: a slower client holds a place for nothing (`408`).
const BODY_DEADLINE: Duration = Duration::from_secs(20);

// ---- middleware ----

/// An app older than `YO_MIN_APP` for its platform gets `426` on every request (spec 6.3).
/// Every answer counted, and its class (4xx, 429, 5xx): counts only, nothing of the request (6.6).
pub(crate) async fn counted(State(hub): Hubs, request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    hub.count(crate::Event::Http, 1);
    if let Some(class) = crate::Event::http(response.status().as_u16()) {
        hub.count(class, 1);
    }
    response
}

pub(crate) async fn too_old(State(hub): Hubs, request: Request, next: Next) -> Response {
    if let Some((platform, build)) = header(request.headers(), api::HEADER_APP).and_then(config::app) {
        if hub.config.min_app.iter().any(|(p, min)| *p == platform && build < *min) {
            return StatusCode::UPGRADE_REQUIRED.into_response();
        }
    }
    next.run(request).await
}

/// Before a body is read (spec 6.6): too large is `413` at once, beyond `Limits::reading_per_ip` or `READING_AT_ONCE` `503`,
/// too slow `408`.
pub(crate) async fn admit(State(hub): Hubs, request: Request, next: Next) -> Response {
    let said = header(request.headers(), CONTENT_LENGTH).and_then(|v| v.parse::<usize>().ok());
    if said.is_some_and(|n| n > limits::MAX_BODY) {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    // One address's first: slow bodies from one place must not hold everyone's.
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(peer)| *peer);
    let by_ip = match peer.filter(|peer| !hub.own_service(*peer, request.headers())) {
        Some(peer) => {
            let key = match hub.ip(peer, request.headers()) {
                IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
                IpAddr::V6(v6) => v6.octets(),
            };
            match crate::limiter::take_place(&hub.reading_by_ip, key, hub.config.limits.reading_per_ip, None) {
                Some(place) => Some(place),
                None => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            }
        }
        None => None,
    };
    let Ok(_place) = hub.reading.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let _by_ip = by_ip;
    let (parts, body) = request.into_parts();
    let Ok(read) = tokio::time::timeout(BODY_DEADLINE, axum::body::to_bytes(body, limits::MAX_BODY)).await else {
        return StatusCode::REQUEST_TIMEOUT.into_response();
    };
    let Ok(body) = read else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let request = Request::from_parts(parts, axum::body::Body::from(body));
    // Its place is held for at most this long in all.
    let handled = tokio::time::timeout(BODY_DEADLINE * 3, next.run(request)).await;
    handled.unwrap_or_else(|_| StatusCode::REQUEST_TIMEOUT.into_response())
}

/// A synchronous route, run off the async workers (`Hub::blocking`); its body was read whole by `admit`.
pub(crate) async fn off_workers(State(hub): Hubs, request: Request, next: Next) -> Response {
    let runtime = tokio::runtime::Handle::current();
    let handled = hub.blocking(move |_| runtime.block_on(next.run(request))).await;
    handled.unwrap_or_else(IntoResponse::into_response)
}

// ---- signed requests (spec 6.2) ----

/// A device that signed its request.
pub(crate) struct Signed {
    pub device_id: DeviceId,
    pub user_id: UserId,
}

/// A signed request's device, and its body.
pub(crate) struct Auth(pub Signed, pub Bytes);

impl FromRequest<Arc<Hub>> for Auth {
    type Rejection = Refusal;

    async fn from_request(request: Request, hub: &Arc<Hub>) -> Res<Self> {
        let (parts, body) = request.into_parts();
        let body =
            axum::body::to_bytes(body, limits::MAX_BODY).await.map_err(|_| refuse(StatusCode::PAYLOAD_TOO_LARGE))?;
        let device = hub.authenticate(&parts.method, &parts.uri, &parts.headers, &body)?;
        Ok(Auth(device, body))
    }
}

/// The time and signature of a signed request: both there, and the time within the window of `now`.
pub(crate) fn signature_of(headers: &HeaderMap, now: u64) -> Option<(u64, [u8; 64])> {
    let time: u64 = header(headers, HEADER_TIME)?.parse().ok()?;
    let signature = b64_array(header(headers, HEADER_SIGNATURE)?)?;
    (now.abs_diff(time) <= limits::MAX_SKEW_MS).then_some((time, signature))
}

/// `401`; for a time outside the window with the hub's time (spec 6.2), so the device corrects its clock by it.
fn out_of_window(headers: &HeaderMap, now: u64) -> Refusal {
    match header(headers, HEADER_TIME).and_then(|t| t.parse::<u64>().ok()) {
        Some(time) if now.abs_diff(time) > limits::MAX_SKEW_MS => {
            Refusal(StatusCode::UNAUTHORIZED, yo_wire::request::clock_body(now))
        }
        _ => refuse(StatusCode::UNAUTHORIZED),
    }
}

impl Hub {
    pub(crate) fn authenticate(&self, method: &Method, uri: &Uri, headers: &HeaderMap, body: &[u8]) -> Res<Signed> {
        self.authenticate_hash(method, uri, headers, &sha256(body))
    }

    /// `authenticate` by the body's hash, before the body is there (a blob, spec 6.8).
    pub(crate) fn authenticate_hash(
        &self,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        body_hash: &[u8; 32],
    ) -> Res<Signed> {
        let unauthorized = || refuse(StatusCode::UNAUTHORIZED);
        let device_id: DeviceId = header(headers, HEADER_DEVICE).and_then(b64_array).ok_or_else(unauthorized)?;
        let now = crate::now_ms();
        let (time, signature) = signature_of(headers, now).ok_or_else(|| out_of_window(headers, now))?;
        let Some(device) = self.db.device(&device_id)? else {
            // A replaced device learns that it was replaced.
            return Err(if self.db.is_gone(&device_id)? { refuse(StatusCode::GONE) } else { unauthorized() });
        };
        let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        yo_wire::request::verify_hash(&device.auth_key, method.as_str(), path, time, body_hash, &signature)
            .map_err(|_| unauthorized())?;
        // A replay is refused before it is counted: whoever repeats a device's request must not spend its budget.
        if !self.first_time(&signature, time, now) {
            return Err(unauthorized());
        }
        // Counted only once the signature holds: a stranger who knows a device id must not spend its requests.
        let signed = Signed { device_id, user_id: device.user_id };
        self.limit_signed(&signed, "signed", &device_id, self.config.limits.signed_per_device_minute, MINUTE)
            .map_err(|_| error::limited("device", None))?;
        // Last seen counts only to the day (spec 6.4): one write a day; the app it names, when it changes.
        let app = named_app(headers);
        let new_app = app.is_some_and(|a| device.app.as_deref() != Some(a));
        if new_app || now.saturating_sub(device.last_seen_at) >= DAY_MS {
            self.db.touch(&device_id, now, app)?;
        }
        Ok(signed)
    }

    /// Each signature once (the replay cache), for as long as its time is within the window.
    pub(crate) fn first_time(&self, signature: &[u8; 64], time: u64, now: u64) -> bool {
        self.replay.first_time(*signature, now, time + limits::MAX_SKEW_MS + 1)
    }

    /// An answer that hands out keys, signed with the hub's key when the request asks with a nonce (spec 6.10).
    pub(crate) fn signed_answer(
        &self,
        headers: &HeaderMap,
        path: &str,
        request: &[u8],
        mut answer: Vec<u8>,
    ) -> Vec<u8> {
        if let Some(nonce) = header(headers, hubsig::NONCE_HEADER).and_then(hubsig::nonce) {
            let key = libsignal_protocol::PrivateKey::deserialize(&self.config.hub_key).expect("32 bytes");
            let message = hubsig::message(&nonce, path, request, &answer);
            let signature = key.calculate_signature(&message, &mut rand::rng()).expect("a Curve25519 key signs");
            answer.extend_from_slice(&signature);
        }
        answer
    }

    // ---- limits (spec 6.6) ----

    pub(crate) fn limit(&self, key: String, per: u32, window: Duration) -> Res<()> {
        match self.limiter.allow(key, per, window) {
            true => Ok(()),
            false => Err(refuse(StatusCode::TOO_MANY_REQUESTS)),
        }
    }

    /// A limit per device or user of a signed request, `id` one of `signed`'s; none for the operator's bot accounts.
    pub(crate) fn limit_signed(&self, signed: &Signed, kind: &str, id: &[u8], per: u32, window: Duration) -> Res<()> {
        if self.bot(&signed.user_id).is_some() {
            return Ok(());
        }
        self.limit_by(kind, id, per, window)
    }

    /// A limit per device or user: `kind` and the id name its bucket.
    pub(crate) fn limit_by(&self, kind: &str, id: &[u8], per: u32, window: Duration) -> Res<()> {
        self.limit(format!("{kind}:{}", b64(id)), per, window)
    }

    /// A limit per address, except for the server's own services (YoGPT: one address, many users).
    pub(crate) fn limit_ip(
        &self,
        kind: &str,
        peer: SocketAddr,
        headers: &HeaderMap,
        per: u32,
        window: Duration,
    ) -> Res<()> {
        if self.own_service(peer, headers) {
            return Ok(());
        }
        self.limit(format!("{kind}:ip:{}", self.ip(peer, headers)), per, window)
    }

    /// Bytes stored without identity, per address (deliveries stay up to `envelope_ttl_days`).
    pub(crate) fn limit_ip_bytes(&self, peer: SocketAddr, headers: &HeaderMap, bytes: usize) -> Res<()> {
        self.limit_ip_bytes_as("bytes", peer, headers, bytes, self.config.limits.anonymous_bytes_per_ip_hour)
    }

    /// Bytes per address an hour in the bucket `kind`, `per` of them.
    pub(crate) fn limit_ip_bytes_as(
        &self,
        kind: &str,
        peer: SocketAddr,
        headers: &HeaderMap,
        bytes: usize,
        per: u64,
    ) -> Res<()> {
        if self.own_service(peer, headers) {
            return Ok(());
        }
        let key = format!("{kind}:ip:{}", self.ip(peer, headers));
        match self.limiter.allow_n(key, bytes as u64, per, HOUR) {
            true => Ok(()),
            false => Err(error::limited("bytes", None)),
        }
    }

    /// Behind Cloudflare every request carries `CF-Connecting-IP`: one from 127.0.0.1 without it is the server itself.
    pub(crate) fn own_service(&self, peer: SocketAddr, headers: &HeaderMap) -> bool {
        self.config.trust_cloudflare && peer.ip().is_loopback() && !headers.contains_key("cf-connecting-ip")
    }

    /// A test robot's request (`robot_ranges`, by the client's address): its help is answered `204` and dropped.
    pub(crate) fn robot(&self, peer: SocketAddr, headers: &HeaderMap) -> bool {
        crate::config::in_ranges(&self.config.robot_ranges, self.client_ip(peer, headers))
    }

    /// The client's address; `CF-Connecting-IP` only if trusted: else anyone forges it.
    fn client_ip(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
        let ip = match header(headers, "cf-connecting-ip").and_then(|v| v.parse().ok()) {
            Some(ip) if self.config.trust_cloudflare => ip,
            _ => peer.ip(),
        };
        ip.to_canonical()
    }

    /// The client's address as limits count it, IPv6 by /64.
    fn ip(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
        match self.client_ip(peer, headers) {
            IpAddr::V6(v6) => {
                let s = v6.segments();
                IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
            v4 => v4,
        }
    }

    /// `507` while the database's disk is below `db_min_free_bytes`, asked once a second (an error counts as room).
    pub(crate) fn db_room(&self) -> Res<()> {
        let floor = self.config.limits.db_min_free_bytes;
        if floor == 0 || self.config.db_path == ":memory:" {
            return Ok(());
        }
        let now = crate::now_ms();
        if now.saturating_sub(self.disk_asked.load(Ordering::Relaxed)) >= 1000 {
            self.disk_asked.store(now, Ordering::Relaxed);
            let path = std::path::Path::new(&self.config.db_path);
            let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
            let full = rustix::fs::statvfs(dir).is_ok_and(|stat| stat.f_bavail.saturating_mul(stat.f_frsize) < floor);
            self.disk_full.store(full, Ordering::Relaxed);
        }
        match self.disk_full.load(Ordering::Relaxed) {
            true => Err(refuse(StatusCode::INSUFFICIENT_STORAGE)),
            false => Ok(()),
        }
    }
}

// ---- parsing and answering ----

pub(crate) fn bad() -> Refusal {
    refuse(StatusCode::BAD_REQUEST)
}

pub(crate) fn header(headers: &HeaderMap, name: impl AsHeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Base64url of exactly `N` bytes.
pub(crate) fn b64_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    unb64(text).ok()?.try_into().ok()
}

/// An id in a path: base64url of exactly `N` bytes, or the route does not exist.
pub(crate) fn path_id<const N: usize>(text: &str) -> Res<[u8; N]> {
    b64_array(text).ok_or_else(|| refuse(StatusCode::NOT_FOUND))
}

pub(crate) fn query<T: std::str::FromStr>(uri: &Uri, name: &str) -> Option<T> {
    uri.query()?.split('&').find_map(|kv| kv.strip_prefix(name)?.strip_prefix('='))?.parse().ok()
}

/// The app a request names (`Yo-App`, spec 6.3), if it is one.
pub(crate) fn named_app(headers: &HeaderMap) -> Option<&str> {
    header(headers, api::HEADER_APP).filter(|a| config::app(a).is_some())
}

/// A body of `len` bytes that streams.
pub(crate) fn octets(len: u64, body: axum::body::Body) -> Response {
    let answer = Response::builder().header(CONTENT_TYPE, "application/octet-stream").header(CONTENT_LENGTH, len);
    answer.body(body).unwrap_or_default()
}

/// An answer whose body holds `guard` until it has been sent or cut off: what it bounds is not free before.
pub(crate) fn held<G: Send + 'static>(body: Vec<u8>, guard: G) -> Response {
    use futures_util::StreamExt;
    let len = body.len() as u64;
    let chunk = futures_util::stream::iter([Ok::<_, std::convert::Infallible>(Bytes::from(body))]);
    let guarded = chunk.map(move |chunk| {
        let _held = &guard;
        chunk
    });
    octets(len, axum::body::Body::from_stream(guarded))
}
