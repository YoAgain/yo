use super::*;
use hyper::{
    body::Incoming,
    header::{HeaderMap, HeaderName, HOST},
    http::uri::Authority,
    Uri, Version,
};
use hyper_util::client::legacy::{connect::HttpConnector, Client};

pub(super) const MARKER: &str = "yo-mesh-relay";
pub(super) type Payload = http_body_util::combinators::UnsyncBoxBody<Bytes, hyper::Error>;

pub(super) fn full(bytes: impl Into<Bytes>) -> Payload {
    Full::new(bytes.into()).map_err(|never| match never {}).boxed_unsync()
}

pub(super) fn empty(code: u16) -> Response<Payload> {
    Response::builder().status(code).body(full(Bytes::new())).unwrap()
}

fn strip(headers: &mut HeaderMap) {
    let named: Vec<_> = headers
        .get_all("connection")
        .iter()
        .flat_map(|v| v.as_bytes().split(|&b| b == b','))
        .filter_map(|s| HeaderName::from_bytes(s.trim_ascii()).ok())
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
        MARKER,
    ] {
        headers.remove(name);
    }
}

pub(super) fn rewrite(
    req: &mut Request<Incoming>,
    scheme: &'static str,
    addr: Authority,
    version: Version,
) -> Result<()> {
    let path = req.uri().path_and_query().cloned().ok_or("missing path")?;
    *req.uri_mut() = Uri::builder().scheme(scheme).authority(addr).path_and_query(path).build()?;
    *req.version_mut() = version;
    strip(req.headers_mut());
    Ok(())
}

pub(super) fn response(mut res: Response<Incoming>, version: Version) -> Response<Payload> {
    strip(res.headers_mut());
    *res.version_mut() = version;
    res.map(BodyExt::boxed_unsync)
}

#[derive(Clone)]
pub(super) struct Forward {
    default: Option<Authority>,
    by_host: Arc<BTreeMap<String, Authority>>,
    client: Client<HttpConnector, Incoming>,
}

fn authority(addr: SocketAddr) -> Authority {
    addr.to_string().parse().unwrap()
}

impl Forward {
    /// None when it forwards nowhere.
    pub(super) fn new(default: Option<SocketAddr>, by_host: BTreeMap<String, SocketAddr>) -> Option<Self> {
        if default.is_none() && by_host.is_empty() {
            return None;
        }
        Some(Self {
            default: default.map(authority),
            by_host: Arc::new(by_host.into_iter().map(|(h, a)| (h.to_ascii_lowercase(), authority(a))).collect()),
            client: Client::builder(TokioExecutor::new()).pool_max_idle_per_host(8).build_http(),
        })
    }

    pub(super) async fn send(&self, mut req: Request<Incoming>) -> Response<Payload> {
        let Some(host) = req.uri().authority().cloned() else { return empty(400) };
        let Some(addr) = self.by_host.get(&host.host().to_ascii_lowercase()).or(self.default.as_ref()).cloned() else {
            return empty(404);
        };
        let Ok(host) = host.as_str().parse() else { return empty(400) };
        if rewrite(&mut req, "http", addr, Version::HTTP_11).is_err() || !req.headers().contains_key("cf-connecting-ip")
        {
            return empty(400);
        }
        req.headers_mut().insert(HOST, host);
        match self.client.request(req).await {
            Ok(res) => response(res, Version::HTTP_2),
            Err(_) => empty(502),
        }
    }
}
