//! Oblivious HTTP (spec 4.1, RFC 9458 with Binary HTTP, RFC 9292): a request wrapped to the hub's OHTTP key is
//! unwrapped, served by the same routes as if sent directly, and its answer wrapped back. Both padded to a bucket
//! (`limits::OHTTP_BUCKETS`), so the TLS proxy sees only IP, time and size. Media goes outside, by one-time tickets.

use crate::Hub;
use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use bytes::Bytes;
use ohttp::hpke::{Aead, Kdf, Kem};
use ohttp::{KeyConfig, Server, SymmetricSuite};
use std::collections::HashMap;
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use tower::ServiceExt;
use yoip::net::{api, limits};

/// The one suite (spec 4.1): DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-128-GCM.
const SUITE: SymmetricSuite = SymmetricSuite::new(Kdf::HkdfSha256, Aead::Aes128Gcm);
/// Where `enc` lies in a wrapped request: after key id (1), KEM, KDF and AEAD ids (2 each).
const ENC: std::ops::Range<usize> = 7..39;
/// The most `enc`s remembered, and tickets open, at once: beyond, `503`.
const MOST_SEEN: usize = 1 << 20;
const MOST_TICKETS: usize = 100_000;
/// Inner headers never taken from the client: the hub sets them, or they belong to the outer request.
const DROPPED: [&str; 7] =
    ["host", "content-length", "transfer-encoding", "connection", "cf-connecting-ip", "date", "te"];

/// A key's config (`application/ohttp-keys` item) from its key id and 32 bytes of key material.
pub fn key_config(id: u8, ikm: &[u8; 32]) -> Result<Vec<u8>, String> {
    let config = KeyConfig::derive(id, Kem::X25519Sha256, vec![SUITE], ikm).map_err(|e| format!("ohttp: {e}"))?;
    config.encode().map_err(|e| format!("ohttp: {e}"))
}

/// A blob transfer one ticket allows: the inner request it stands for, once, until `until` (ms).
struct Ticket {
    method: Method,
    uri: String,
    headers: HeaderMap,
    length: Option<u64>,
    until: u64,
}

pub struct Gateway {
    servers: Vec<(u8, Server)>,
    /// The key configs served at `/.well-known/yo-ohttp`, current first.
    configs: Vec<u8>,
    /// Each `enc` accepted, until its `Date` window is over (spec 4.1: replay).
    seen: Mutex<HashMap<[u8; 32], u64>>,
    tickets: Mutex<HashMap<[u8; 16], Ticket>>,
}

impl Gateway {
    pub fn new(keys: &[(u8, [u8; 32])]) -> Result<Option<Gateway>, String> {
        if keys.is_empty() {
            return Ok(None);
        }
        let mut servers = Vec::new();
        let mut list = Vec::new();
        for (id, ikm) in keys {
            let config =
                KeyConfig::derive(*id, Kem::X25519Sha256, vec![SUITE], ikm).map_err(|e| format!("ohttp: {e}"))?;
            list.push(config.clone());
            servers.push((*id, Server::new(config).map_err(|e| format!("ohttp: {e}"))?));
        }
        let configs = KeyConfig::encode_list(&list).map_err(|e| format!("ohttp: {e}"))?;
        Ok(Some(Gateway { servers, configs, seen: Mutex::default(), tickets: Mutex::default() }))
    }

    pub fn configs(&self) -> &[u8] {
        &self.configs
    }

    /// Remembers `enc` until `until`: false if it was seen within its window (a replay) or too many are.
    fn first_time(&self, enc: [u8; 32], now: u64, until: u64) -> Result<bool, StatusCode> {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        if seen.len() >= MOST_SEEN {
            seen.retain(|_, until| *until > now);
            if seen.len() >= MOST_SEEN {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
        }
        if seen.get(&enc).is_some_and(|was| *was > now) {
            return Ok(false);
        }
        seen.insert(enc, until);
        Ok(true)
    }

    fn issue(&self, ticket: Ticket, now: u64) -> Result<String, StatusCode> {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let mut tickets = self.tickets.lock().unwrap_or_else(PoisonError::into_inner);
        if tickets.len() >= MOST_TICKETS {
            tickets.retain(|_, t| t.until > now);
            if tickets.len() >= MOST_TICKETS {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
        }
        tickets.insert(id, ticket);
        Ok(yoip::b64(&id))
    }

    /// Takes a ticket (once), if it is open and for this direction.
    fn redeem(&self, ticket: &str, method: &Method, now: u64) -> Option<Ticket> {
        let id: [u8; 16] = yoip::unb64(ticket).ok()?.try_into().ok()?;
        let ticket = self.tickets.lock().unwrap_or_else(PoisonError::into_inner).remove(&id)?;
        (ticket.until > now && ticket.method == method).then_some(ticket)
    }

    /// Drops what is over: run every minute.
    pub fn forget_expired(&self, now: u64) {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).retain(|_, until| *until > now);
        self.tickets.lock().unwrap_or_else(PoisonError::into_inner).retain(|_, t| t.until > now);
    }
}

/// The inner routes OHTTP serves (spec 4.1): YoIP's, not admin, support, discovery, tickets or OHTTP itself.
pub fn served_inside(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    path.starts_with("/v1/")
        && !path.starts_with("/v1/admin")
        && !path.starts_with("/v1/help/")
        && path != api::OHTTP
        && !path.starts_with(&format!("{}/", api::TICKETS))
}

/// Marks a request served from inside OHTTP or by a ticket: never `426` by `direct_gone`.
#[derive(Clone, Copy)]
struct Inside;

/// After the cutoff (`Config::direct_gone`): a direct YoIP request is answered `426`, as an app below the minimum,
/// so an app before OHTTP learns it must update. Discovery, admin and support stay direct.
pub(crate) async fn direct_gone(
    axum::extract::State(hub): crate::request::Hubs,
    request: Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    let path = request.uri().path();
    let stays = !path.starts_with("/v1/") || path.starts_with("/v1/admin") || path.starts_with("/v1/help/");
    if hub.config.direct_gone && !stays && request.extensions().get::<Inside>().is_none() {
        return outer(StatusCode::UPGRADE_REQUIRED);
    }
    next.run(request).await
}

fn outer(status: StatusCode) -> Response {
    let mut response = status.into_response();
    response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `POST /v1/ohttp`: limited per address and by size before it is unwrapped; once unwrapped, every answer goes back
/// wrapped in a `200`.
pub(crate) async fn post(hub: Arc<Hub>, inner: Router, peer: SocketAddr, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(refusal) = hub.limit_ohttp(peer, &headers) {
        return refusal.into_response();
    }
    let Some(gateway) = hub.ohttp.as_ref() else { return outer(StatusCode::NOT_FOUND) };
    if headers.get(CONTENT_TYPE).is_none_or(|t| t.as_bytes() != api::OHTTP_REQUEST_TYPE.as_bytes()) {
        return outer(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    if body.len() > limits::MAX_OHTTP_REQUEST || body.len() <= ENC.end {
        return outer(StatusCode::BAD_REQUEST);
    }
    // An unknown key id: `422`, which a client may try with its other pinned key (nothing was processed).
    let Some((_, server)) = gateway.servers.iter().find(|(id, _)| *id == body[0]) else {
        return outer(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let Ok((plain, wrap)) = server.decapsulate(&body) else { return outer(StatusCode::BAD_REQUEST) };
    let now = hub.clock_ms();
    let answer = match unwrapped(gateway, inner, peer, &headers, &body, &plain, now).await {
        Ok(answer) => answer,
        Err(status) => (status, Vec::new(), Vec::new()),
    };
    let Some(encoded) = encode_answer(answer) else { return outer(StatusCode::INTERNAL_SERVER_ERROR) };
    match wrap.encapsulate(&encoded) {
        Ok(wrapped) => {
            let mut response = (StatusCode::OK, wrapped).into_response();
            let h = response.headers_mut();
            h.insert(CONTENT_TYPE, HeaderValue::from_static(api::OHTTP_RESPONSE_TYPE));
            h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(_) => outer(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// An inner answer: status, headers, body.
type Answer = (StatusCode, Vec<(String, Vec<u8>)>, Vec<u8>);

/// The answer as Binary HTTP, zero padded so that wrapped it fills a bucket; too large for any: `None`.
fn encode_answer((status, headers, body): Answer) -> Option<Vec<u8>> {
    let encode = |status: u16, headers: &[(String, Vec<u8>)], body: &[u8]| {
        let mut message = bhttp::Message::response(bhttp::StatusCode::try_from(status).ok()?);
        for (name, value) in headers {
            message.put_header(name.as_bytes(), value.clone());
        }
        message.write_content(body);
        let mut out = Vec::new();
        message.write_bhttp(bhttp::Mode::KnownLength, &mut out).ok()?;
        Some(out)
    };
    let mut out = encode(status.as_u16(), &headers, &body)?;
    let padded = match limits::ohttp_padded(out.len(), limits::OHTTP_RESPONSE_OVERHEAD) {
        Some(padded) => padded,
        None => {
            out = encode(StatusCode::INSUFFICIENT_STORAGE.as_u16(), &[], &[])?;
            limits::ohttp_padded(out.len(), limits::OHTTP_RESPONSE_OVERHEAD)?
        }
    };
    out.resize(padded, 0);
    Some(out)
}

/// Parses, checks and serves an unwrapped request.
#[allow(clippy::too_many_arguments)]
async fn unwrapped(
    gateway: &Gateway,
    inner: Router,
    peer: SocketAddr,
    outer_headers: &HeaderMap,
    wrapped: &[u8],
    plain: &[u8],
    now: u64,
) -> Result<Answer, StatusCode> {
    let mut cursor = Cursor::new(plain);
    let message = bhttp::Message::read_bhttp(&mut cursor).map_err(|_| StatusCode::BAD_REQUEST)?;
    // Only zeros after the message (RFC 9292 padding).
    if plain[cursor.position() as usize..].iter().any(|b| *b != 0) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let control = message.control();
    let method = control.method().and_then(|m| Method::from_bytes(m).ok()).ok_or(StatusCode::BAD_REQUEST)?;
    let path = control.path().and_then(|p| std::str::from_utf8(p).ok()).ok_or(StatusCode::BAD_REQUEST)?;
    // Replay (spec 4.1): its `Date` within 5 minutes, its `enc` once in that window.
    let date = message.header().get(b"date").and_then(|d| std::str::from_utf8(d).ok());
    let date = date.and_then(|d| httpdate::parse_http_date(d).ok()).ok_or(StatusCode::BAD_REQUEST)?;
    let date_ms = date.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    if date_ms.abs_diff(now) > limits::OHTTP_SKEW_MS {
        return Err(StatusCode::BAD_REQUEST);
    }
    let enc: [u8; 32] = wrapped[ENC].try_into().map_err(|_| StatusCode::BAD_REQUEST)?;
    if !gateway.first_time(enc, now, date_ms + limits::OHTTP_SKEW_MS + 1)? {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !served_inside(path) {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut headers = HeaderMap::new();
    for field in message.header().iter() {
        let name = HeaderName::from_bytes(field.name()).map_err(|_| StatusCode::BAD_REQUEST)?;
        if DROPPED.contains(&name.as_str()) {
            continue;
        }
        headers.append(name, HeaderValue::from_bytes(field.value()).map_err(|_| StatusCode::BAD_REQUEST)?);
    }
    // The client's address as the outer request had it, so per-address limits count as before.
    if let Some(ip) = outer_headers.get("cf-connecting-ip") {
        headers.insert("cf-connecting-ip", ip.clone());
    }
    // A ticket for a blob transfer outside OHTTP (spec 4.1): one blob, one direction, once, 5 minutes.
    if headers.remove(api::HEADER_TICKET).is_some() {
        let blob = path.starts_with(&format!("{}/", api::BLOBS));
        let length = match method {
            Method::PUT => {
                let length = headers.remove(api::HEADER_BLOB_LENGTH);
                let length = length.and_then(|l| l.to_str().ok()?.parse::<u64>().ok());
                Some(length.filter(|l| *l <= limits::MAX_BLOB_LEN as u64).ok_or(StatusCode::BAD_REQUEST)?)
            }
            Method::GET => None,
            _ => return Err(StatusCode::BAD_REQUEST),
        };
        if !blob {
            return Err(StatusCode::BAD_REQUEST);
        }
        let ticket = Ticket { method, uri: path.to_owned(), headers, length, until: now + limits::TICKET_MS };
        let id = gateway.issue(ticket, now)?;
        return Ok((StatusCode::OK, vec![("cache-control".into(), b"no-store".to_vec())], id.into_bytes()));
    }
    let content = message.content().to_vec();
    headers.insert(CONTENT_LENGTH, HeaderValue::from(content.len()));
    let mut request =
        Request::builder().method(method).uri(path).body(Body::from(content)).map_err(|_| StatusCode::BAD_REQUEST)?;
    *request.headers_mut() = headers;
    request.extensions_mut().insert(ConnectInfo(peer));
    request.extensions_mut().insert(Inside);
    let response = inner.oneshot(request).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "content-length" | "transfer-encoding" | "connection"))
        .map(|(name, value)| (name.as_str().to_owned(), value.as_bytes().to_vec()))
        .collect();
    let body = to_bytes(response.into_body(), limits::MAX_OHTTP_RESPONSE)
        .await
        .map_err(|_| StatusCode::INSUFFICIENT_STORAGE)?;
    Ok((status, headers, body.to_vec()))
}

/// `PUT` or `GET /v1/blobs/t/{ticket}`: the transfer a ticket allows, direct and streamed; never cached.
pub(crate) async fn transfer(hub: Arc<Hub>, inner: Router, ticket: String, request: Request<Body>) -> Response {
    let Some(gateway) = hub.ohttp.as_ref() else { return outer(StatusCode::NOT_FOUND) };
    let Some(open) = gateway.redeem(&ticket, request.method(), hub.clock_ms()) else {
        return outer(StatusCode::NOT_FOUND);
    };
    if let Some(length) = open.length {
        let given = request.headers().get(CONTENT_LENGTH).and_then(|l| l.to_str().ok()?.parse::<u64>().ok());
        if given != Some(length) {
            return outer(StatusCode::BAD_REQUEST);
        }
    }
    // A request of its own: only the connection's address carried over, not the outer route's path parameters.
    let (parts, body) = request.into_parts();
    let Ok(mut request) = Request::builder().method(parts.method).uri(&open.uri).body(body) else {
        return outer(StatusCode::BAD_REQUEST);
    };
    if let Some(peer) = parts.extensions.get::<ConnectInfo<SocketAddr>>() {
        request.extensions_mut().insert(*peer);
    }
    request.extensions_mut().insert(Inside);
    let length = parts.headers.get(CONTENT_LENGTH).cloned();
    let ip = parts.headers.get("cf-connecting-ip").cloned();
    let mut headers = open.headers;
    headers.remove("cf-connecting-ip");
    if let Some(ip) = ip {
        headers.insert("cf-connecting-ip", ip);
    }
    if let Some(length) = length {
        headers.insert(CONTENT_LENGTH, length);
    }
    *request.headers_mut() = headers;
    let mut response = match inner.oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    };
    response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, TEST_OHTTP_KEY, TEST_OHTTP_KEY_ID};

    fn hub(config: Config) -> Arc<Hub> {
        Hub::new(config, Arc::new(crate::push::Recorder::default())).unwrap()
    }

    /// A wrapped request as a client makes it: Binary HTTP with `date`, padded, sealed to the test key.
    fn wrap(
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        date: std::time::SystemTime,
    ) -> (Vec<u8>, ohttp::ClientResponse) {
        let mut message = bhttp::Message::request(method.into(), b"https".to_vec(), Vec::new(), path.into());
        for (name, value) in headers {
            message.put_header(*name, *value);
        }
        message.put_header("date", httpdate::fmt_http_date(date));
        let mut plain = Vec::new();
        message.write_bhttp(bhttp::Mode::KnownLength, &mut plain).unwrap();
        plain.resize(limits::ohttp_padded(plain.len(), limits::OHTTP_REQUEST_OVERHEAD).unwrap(), 0);
        let config = key_config(TEST_OHTTP_KEY_ID, &TEST_OHTTP_KEY).unwrap();
        let (wrapped, opener) =
            ohttp::ClientRequest::from_encoded_config(&config).unwrap().encapsulate(&plain).unwrap();
        assert!(limits::OHTTP_BUCKETS.contains(&wrapped.len()));
        (wrapped, opener)
    }

    async fn post(hub: &Arc<Hub>, body: Vec<u8>, ip: [u8; 4]) -> Response {
        let mut request =
            Request::post(api::OHTTP).header(CONTENT_TYPE, api::OHTTP_REQUEST_TYPE).body(Body::from(body)).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from((ip, 1))));
        hub.router().oneshot(request).await.unwrap()
    }

    /// The inner status and body of a wrapped answer, which must fill a bucket.
    async fn open(response: Response, opener: ohttp::ClientResponse) -> (u16, Vec<u8>) {
        assert_eq!(response.status(), StatusCode::OK);
        let wrapped = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(limits::OHTTP_BUCKETS.contains(&wrapped.len()), "{}", wrapped.len());
        let plain = opener.decapsulate(&wrapped).unwrap();
        let message = bhttp::Message::read_bhttp(&mut Cursor::new(&plain[..])).unwrap();
        (message.control().status().unwrap().code(), message.content().to_vec())
    }

    #[tokio::test]
    async fn a_wrapped_request_is_served_by_the_same_route_and_answered_padded() {
        let hub = hub(Config::for_tests());
        let (wrapped, opener) = wrap("GET", "/v1/username/Somebody", &[], std::time::SystemTime::now());
        let (status, _) = open(post(&hub, wrapped, [192, 0, 2, 1]).await, opener).await;
        let direct = Request::get("/v1/username/Somebody").body(Body::empty()).unwrap();
        let mut direct = direct;
        direct.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 1], 1))));
        assert_eq!(status, hub.router().oneshot(direct).await.unwrap().status().as_u16());
        // The keys, for tools.
        let keys = hub.router().oneshot(Request::get(api::OHTTP_KEYS_PATH).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(keys.headers()[CONTENT_TYPE], api::OHTTP_KEYS_TYPE);
        let keys = to_bytes(keys.into_body(), usize::MAX).await.unwrap();
        assert!(KeyConfig::decode_list(&keys).is_ok());
    }

    #[tokio::test]
    async fn replays_old_dates_and_routes_not_served_inside_are_refused_inside() {
        let hub = hub(Config::for_tests());
        let now = std::time::SystemTime::now();
        let (wrapped, opener) = wrap("GET", "/v1/username/Somebody", &[], now);
        assert_ne!(open(post(&hub, wrapped, [192, 0, 2, 1]).await, opener).await.0, 400);
        let old = now - std::time::Duration::from_secs(6 * 60);
        let (wrapped, opener) = wrap("GET", "/v1/username/Somebody", &[], old);
        assert_eq!(open(post(&hub, wrapped, [192, 0, 2, 1]).await, opener).await.0, 400);
        for path in ["/v1/admin/users", "/v1/help/messages", "/.well-known/yo-hub-key", "/v1/ohttp", "/v1/blobs/t/x"] {
            let (wrapped, opener) = wrap("GET", path, &[], now);
            assert_eq!(open(post(&hub, wrapped, [192, 0, 2, 1]).await, opener).await.0, 404, "{path}");
        }
    }

    #[tokio::test]
    async fn a_replay_is_answered_400_inside() {
        let hub = hub(Config::for_tests());
        let gateway = hub.ohttp.as_ref().unwrap();
        assert!(gateway.first_time([1; 32], 10, 100).unwrap());
        assert!(!gateway.first_time([1; 32], 50, 140).unwrap());
        assert!(gateway.first_time([1; 32], 100, 400).unwrap(), "its window over");
    }

    #[tokio::test]
    async fn outer_failures_before_unwrapping() {
        let mut config = Config::for_tests();
        config.limits.ohttp_per_ip_minute = 2;
        let hub = hub(config);
        let (mut wrapped, _) = wrap("GET", "/v1/username/Somebody", &[], std::time::SystemTime::now());
        wrapped[0] = 9;
        assert_eq!(post(&hub, wrapped, [192, 0, 2, 2]).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(post(&hub, vec![1; 100], [192, 0, 2, 2]).await.status(), StatusCode::BAD_REQUEST);
        assert_eq!(post(&hub, vec![1; 100], [192, 0, 2, 2]).await.status(), StatusCode::TOO_MANY_REQUESTS);
        let mut typed = Request::post(api::OHTTP).body(Body::from(vec![1; 100])).unwrap();
        typed.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 3], 1))));
        assert_eq!(hub.router().oneshot(typed).await.unwrap().status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let too_large = vec![1; limits::MAX_OHTTP_REQUEST + 1];
        assert_ne!(post(&hub, too_large, [192, 0, 2, 4]).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_ticket_works_once_in_its_direction() {
        let hub = hub(Config::for_tests());
        let (wrapped, opener) =
            wrap("GET", "/v1/blobs/AAAAAAAAAAAAAAAAAAAAAA", &[(api::HEADER_TICKET, "1")], std::time::SystemTime::now());
        let (status, ticket) = open(post(&hub, wrapped, [192, 0, 2, 1]).await, opener).await;
        assert_eq!(status, 200);
        let path = api::ticket_path(std::str::from_utf8(&ticket).unwrap());
        let transfer = |method: Method| {
            let mut request = Request::builder().method(method).uri(&path).body(Body::empty()).unwrap();
            request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 1], 1))));
            hub.router().oneshot(request)
        };
        assert_eq!(transfer(Method::PUT).await.unwrap().status(), StatusCode::NOT_FOUND, "the other direction");
        let (wrapped, opener) =
            wrap("GET", "/v1/blobs/AAAAAAAAAAAAAAAAAAAAAA", &[(api::HEADER_TICKET, "1")], std::time::SystemTime::now());
        let (_, ticket) = open(post(&hub, wrapped, [192, 0, 2, 1]).await, opener).await;
        let path = api::ticket_path(std::str::from_utf8(&ticket).unwrap());
        let mut request = Request::get(&path).body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 1], 1))));
        let first = hub.router().oneshot(request).await.unwrap();
        assert_eq!(first.headers()[CACHE_CONTROL], "no-store");
        assert!(hub.ohttp.as_ref().unwrap().tickets.lock().unwrap().is_empty(), "taken once");
    }

    #[tokio::test]
    async fn after_the_cutoff_direct_routes_answer_426_and_wrapped_ones_are_served() {
        let mut config = Config::for_tests();
        config.direct_gone = true;
        let hub = hub(config);
        let mut direct = Request::get("/v1/username/Somebody").body(Body::empty()).unwrap();
        direct.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 1], 1))));
        assert_eq!(hub.router().oneshot(direct).await.unwrap().status(), StatusCode::UPGRADE_REQUIRED);
        let keys = hub.router().oneshot(Request::get(api::OHTTP_KEYS_PATH).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(keys.status(), StatusCode::OK);
        let (wrapped, opener) = wrap("GET", "/v1/username/Somebody", &[], std::time::SystemTime::now());
        assert_ne!(open(post(&hub, wrapped, [192, 0, 2, 1]).await, opener).await.0, 426);
    }

    #[test]
    fn padding_fills_buckets() {
        for len in [0, 100, 968, 969, 5000, 70_000] {
            let padded = limits::ohttp_padded(len, limits::OHTTP_REQUEST_OVERHEAD).unwrap();
            assert!(padded >= len && limits::OHTTP_BUCKETS.contains(&(padded + limits::OHTTP_REQUEST_OVERHEAD)));
        }
        assert_eq!(limits::ohttp_padded(limits::MAX_OHTTP_RESPONSE, 32), None);
    }
}
