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
    addr: Authority,
    client: Client<HttpConnector, Incoming>,
}

impl Forward {
    pub(super) fn new(addr: SocketAddr) -> Self {
        Self {
            addr: addr.to_string().parse().unwrap(),
            client: Client::builder(TokioExecutor::new()).pool_max_idle_per_host(8).build_http(),
        }
    }

    pub(super) async fn send(&self, mut req: Request<Incoming>) -> Response<Payload> {
        let Some(host) = req.uri().authority().and_then(|a| a.as_str().parse().ok()) else { return empty(400) };
        if rewrite(&mut req, "http", self.addr.clone(), Version::HTTP_11).is_err()
            || !req.headers().contains_key("cf-connecting-ip")
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
