//! Delivery refusals get a second look at the authority. Other requests stream straight to the relay.

use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, Uri, Version},
    middleware::Next,
    response::{IntoResponse, Response},
};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use yoip::net::{api, limits};

#[derive(Clone)]
pub(crate) struct Edge {
    upstream: axum::http::uri::Authority,
    client: Client<HttpConnector, Body>,
    reading: std::sync::Arc<tokio::sync::Semaphore>,
}

/// A wrapped request to retry with its original ciphertext.
#[derive(Clone, Copy)]
pub(crate) struct Forward;

pub(crate) fn local(method: &Method, path: &str) -> bool {
    (method == Method::GET && path == api::INBOX)
        || (method == Method::POST
            && path.strip_prefix("/v1/inbox/").is_some_and(|user| !user.is_empty() && !user.contains('/')))
}

// TRANSITION(#15): remove when edges are equal hubs and the relay is gone.
pub(crate) fn retry(status: StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 403 | 404 | 409 | 410)
}

fn strip(headers: &mut HeaderMap) {
    let named: Vec<_> = headers
        .get_all("connection")
        .iter()
        .flat_map(|v| v.as_bytes().split(|b| *b == b','))
        .filter_map(|n| axum::http::HeaderName::from_bytes(n.trim_ascii()).ok())
        .collect();
    for name in named {
        headers.remove(name);
    }
    for name in [
        "connection",
        "keep-alive",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        headers.remove(name);
    }
}

impl Edge {
    pub fn new(upstream: &str) -> Result<Self, String> {
        let uri: Uri = upstream.parse().map_err(|_| "invalid edge upstream")?;
        if uri.scheme_str() != Some("http") || uri.path() != "/" || uri.query().is_some() {
            return Err("expected an HTTP relay origin".into());
        }
        Ok(Self {
            upstream: uri.authority().ok_or("missing relay authority")?.clone(),
            client: Client::builder(TokioExecutor::new()).build_http(),
            reading: std::sync::Arc::new(tokio::sync::Semaphore::new(crate::READING_AT_ONCE)),
        })
    }

    // TRANSITION(#15): remove when edges are equal hubs and the relay is gone.
    async fn send(&self, mut request: Request) -> Response {
        let path = request.uri().path_and_query().cloned().unwrap_or_else(|| "/".parse().unwrap());
        let Ok(uri) = Uri::builder().scheme("http").authority(self.upstream.clone()).path_and_query(path).build()
        else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        *request.uri_mut() = uri;
        *request.version_mut() = Version::HTTP_11;
        let client_ip = request.headers().get("cf-connecting-ip").cloned().or_else(|| {
            // cloudflared always names the client; without it (a local client) the peer is the client.
            let peer = request.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()?.0.ip();
            Some(peer.to_string().parse().expect("an IP address is a header value"))
        });
        strip(request.headers_mut());
        if let Some(ip) = client_ip {
            request.headers_mut().insert("cf-connecting-ip", ip);
        }
        match self.client.request(request).await {
            Ok(mut response) => {
                strip(response.headers_mut());
                response.map(Body::new)
            }
            Err(_) => StatusCode::BAD_GATEWAY.into_response(),
        }
    }
}

// TRANSITION(#15): remove when edges are equal hubs and the relay is gone.
pub(crate) async fn route(State(hub): State<std::sync::Arc<crate::Hub>>, request: Request, next: Next) -> Response {
    let edge = hub.edge.as_ref().unwrap();
    // TRANSITION(#15): remove when edges are equal hubs and the relay is gone.
    if hub.db.edge_off() {
        return edge.send(request).await;
    }
    let path = request.uri().path();
    if path.starts_with("/v1/peer/") || (request.method() == Method::GET && path == "/") {
        return next.run(request).await;
    }
    let wrapped = request.method() == Method::POST && path == api::OHTTP;
    if !wrapped && !local(request.method(), path) {
        return edge.send(request).await;
    }
    let limit = if wrapped {
        limits::MAX_OHTTP_REQUEST
    } else if path == api::INBOX_ACK {
        limits::MAX_BODY
    } else {
        limits::MAX_DELIVERY_BODY
    };
    let (parts, body) = request.into_parts();
    let Ok(place) = edge.reading.try_acquire() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(bytes) = to_bytes(body, limit).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    drop(place);
    let result = next.run(Request::from_parts(parts.clone(), Body::from(bytes.clone()))).await;
    if result.extensions().get::<Forward>().is_some() || (!wrapped && retry(result.status())) {
        edge.send(Request::from_parts(parts, Body::from(bytes))).await
    } else {
        result
    }
}
