use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{client::conn::http2, Request};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rcgen::{BasicConstraints, Certificate, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair};
use rustls::{crypto::aws_lc_rs, pki_types::ServerName, ClientConfig, RootCertStore};
use serde_json::Value;
use std::{
    env, fs,
    net::{SocketAddr, TcpListener},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpStream,
    time::{sleep, timeout},
};
use tokio_rustls::TlsConnector;
use yo_mesh::{client_config, run, Certs, Config, Result};

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn ca() -> (Certificate, Issuer<'static, KeyPair>) {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let key = KeyPair::generate().unwrap();
    (params.self_signed(&key).unwrap(), Issuer::new(params, key))
}

fn identity(dir: &Path, name: &str, issuer: &Issuer<'_, KeyPair>, trusted: &Certificate) -> PathBuf {
    let dir = dir.join(name);
    fs::create_dir(&dir).unwrap();
    let mut params = CertificateParams::new(vec![name.into()]).unwrap();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
    let key = KeyPair::generate().unwrap();
    fs::write(dir.join("ca.crt"), trusted.pem()).unwrap();
    fs::write(dir.join("node.crt"), params.signed_by(&key, issuer).unwrap().pem()).unwrap();
    fs::write(dir.join("node.key"), key.serialize_pem()).unwrap();
    dir
}

fn node(name: &str, addr: SocketAddr, peer: &str, other: SocketAddr, dir: &Path) -> Config {
    Config {
        name: name.into(),
        listen: addr,
        peers: [(peer.into(), other)].into(),
        certs: Some(dir.into()),
        status: Some(dir.join("status.json")),
        history: None,
        forward: None,
    }
}

async fn request(addr: SocketAddr, name: &'static str, tls: Arc<ClientConfig>, path: &str) -> Result<(u16, Bytes)> {
    let stream = TlsConnector::from(tls).connect(ServerName::try_from(name)?, TcpStream::connect(addr).await?).await?;
    let (mut sender, conn) = http2::handshake(TokioExecutor::new(), TokioIo::new(stream)).await?;
    tokio::spawn(conn);
    let req = Request::get(format!("https://{name}{path}")).body(Full::<Bytes>::default())?;
    let res = sender.send_request(req).await?;
    Ok((res.status().as_u16(), res.into_body().collect().await?.to_bytes()))
}

fn status(dir: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(dir.join("status.json")).ok()?).ok()
}

#[tokio::test]
async fn mutual_heartbeat() -> Result<()> {
    let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let dir = TempDir(env::temp_dir().join(format!("yo-mesh-{}-{id}", std::process::id())));
    fs::DirBuilder::new().mode(0o700).create(&dir.0)?;
    let (root, issuer) = ca();
    let tokyo = identity(&dir.0, "yo-tokyo", &issuer, &root);
    let ohio = identity(&dir.0, "yo-ohio", &issuer, &root);
    let (_, foreign_issuer) = ca();
    // The foreign client trusts the server, so only the client identity is wrong.
    let foreign = identity(&dir.0, "yo-foreign", &foreign_issuer, &root);

    let a = TcpListener::bind("127.0.0.1:0")?;
    let b = TcpListener::bind("127.0.0.1:0")?;
    let (addr, other) = (a.local_addr()?, b.local_addr()?);
    drop((a, b));
    let server = tokio::spawn(run(node("yo-tokyo", addr, "yo-ohio", other, &tokyo)));
    let peer = tokio::spawn(run(node("yo-ohio", other, "yo-tokyo", addr, &ohio)));
    timeout(Duration::from_secs(6), async {
        loop {
            let online =
                [(&tokyo, "yo-ohio", "yo-tokyo"), (&ohio, "yo-tokyo", "yo-ohio")].iter().all(|(dir, name, own)| {
                    status(dir).is_some_and(|s| {
                        let p = &s["peers"][*name];
                        p["online"] == true
                            && p["rtt_ms"].as_f64().is_some()
                            && p["last_ok_ms"].as_u64().is_some()
                            && p["view"][*own]["online"] == true
                    })
                });
            if online {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    assert_eq!(fs::metadata(tokyo.join("status.json"))?.permissions().mode() & 0o777, 0o644);

    let valid = client_config(&Certs::load(&ohio)?)?;
    let (code, body) = request(addr, "yo-tokyo", valid.clone(), "/ping").await?;
    assert_eq!(code, 200);
    let ping: Value = serde_json::from_slice(&body)?;
    assert_eq!(ping["node"], "yo-tokyo");
    assert_eq!(ping["version"], env!("CARGO_PKG_VERSION"));
    assert!(ping["time_ms"].is_u64());
    assert_eq!(ping["peers"]["yo-ohio"]["online"], true);
    assert_eq!(request(addr, "yo-tokyo", valid.clone(), "/other").await?.0, 404);

    let path = tokyo.join("history.json");
    let file: Value = serde_json::from_slice(&fs::read(&path)?)?;
    assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o644);
    assert_eq!(file["node"], "yo-tokyo");
    assert_eq!(file["minutes"], 1440);
    assert_eq!(file["end_min"], file["at_ms"].as_u64().unwrap() / 60_000);
    assert_eq!(file["version"], env!("CARGO_PKG_VERSION"));
    let (code, body) = request(addr, "yo-tokyo", valid.clone(), "/history").await?;
    assert_eq!(code, 200);
    let live: Value = serde_json::from_slice(&body)?;
    assert_eq!(live["node"], "yo-tokyo");
    assert_eq!(live["links"].as_object().unwrap().len(), 1);
    for snapshot in [&file, &live] {
        let link = &snapshot["links"]["yo-tokyo>yo-ohio"];
        for field in ["rtt", "ok", "fail"] {
            assert_eq!(link[field].as_array().unwrap().len(), 1440);
        }
    }
    assert!(live["links"]["yo-tokyo>yo-ohio"]["ok"].as_array().unwrap().iter().any(|n| n.as_u64().unwrap() > 0));

    let mut roots = RootCertStore::empty();
    roots.add(root.der().clone())?;
    let mut anonymous = ClientConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_no_client_auth();
    anonymous.alpn_protocols = vec![b"h2".to_vec()];
    for (name, client) in
        [("yo-tokyo", client_config(&Certs::load(&foreign)?)?), ("yo-ohio", valid), ("yo-tokyo", Arc::new(anonymous))]
    {
        assert!(timeout(Duration::from_secs(2), request(addr, name, client, "/ping")).await?.is_err());
    }
    server.abort();
    peer.abort();
    Ok(())
}

async fn edge_request(addr: SocketAddr, req: Request<Full<Bytes>>) -> Result<hyper::Response<hyper::body::Incoming>> {
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(TokioIo::new(TcpStream::connect(addr).await?)).await?;
    tokio::spawn(conn);
    Ok(sender.send_request(req).await?)
}

fn get(path: &str) -> Request<Full<Bytes>> {
    Request::get(path)
        .header("host", "dev.example")
        .header("cf-connecting-ip", "192.0.2.1")
        .body(Full::default())
        .unwrap()
}

async fn edge_status(addr: SocketAddr) -> Result<Value> {
    let res = edge_request(addr, get("/status")).await?;
    Ok(serde_json::from_slice(&res.collect().await?.to_bytes())?)
}

async fn echo(req: Request<hyper::body::Incoming>) -> Result<hyper::Response<hyper::body::Incoming>> {
    if req.uri().path() == "/slow" {
        sleep(Duration::from_secs(2)).await;
    }
    assert_eq!(req.version(), hyper::Version::HTTP_11);
    assert!(req.uri().authority().is_none());
    assert!(!req.headers().contains_key("yo-mesh-relay"));
    assert!(!req.headers().contains_key("x-hop"));
    let mut response = hyper::Response::builder()
        .status(201)
        .header("x-method", req.method().as_str())
        .header("x-path", req.uri().to_string())
        .header("x-host", &req.headers()["host"])
        .header("x-ip", &req.headers()["cf-connecting-ip"])
        .header("connection", "x-return-hop")
        .header("x-return-hop", "drop");
    if let Some(value) = req.headers().get("yo-rtt") {
        response = response.header("yo-rtt", value);
    }
    Ok(response.body(req.into_body())?)
}

#[tokio::test]
async fn relay_round_trip() -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::oneshot;
    let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let dir = TempDir(env::temp_dir().join(format!("yo-relay-{}-{id}", std::process::id())));
    fs::DirBuilder::new().mode(0o700).create(&dir.0)?;
    let (root, issuer) = ca();
    let a = identity(&dir.0, "node-a", &issuer, &root);
    let b = identity(&dir.0, "node-b", &issuer, &root);
    let sockets: Vec<_> = (0..4).map(|_| TcpListener::bind("127.0.0.1:0")).collect::<std::io::Result<_>>()?;
    let (addr, other, listen, status) =
        (sockets[0].local_addr()?, sockets[1].local_addr()?, sockets[2].local_addr()?, sockets[3].local_addr()?);
    let hub = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let mut config = node("node-a", addr, "node-b", other, &a);
    config.forward = Some(hub.local_addr()?);
    let echo_server = tokio::spawn(async move {
        loop {
            let stream = hub.accept().await.unwrap().0;
            tokio::spawn(
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), hyper::service::service_fn(echo)),
            );
        }
    });
    drop(sockets);
    let start_edge = || {
        let (stop, stopped) = oneshot::channel::<()>();
        let edge = tokio::spawn(yo_mesh::relay(
            node("node-b", other, "node-a", addr, &b),
            "node-a".into(),
            listen,
            status,
            async {
                let _ = stopped.await;
            },
        ));
        (edge, stop)
    };
    let (edge, stop) = start_edge();
    timeout(Duration::from_secs(4), async {
        while edge_status(status).await.is_err() {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    assert_eq!(edge_request(listen, get("/")).await?.status(), 502);
    let server = tokio::spawn(run(config));
    timeout(Duration::from_secs(6), async {
        while edge_status(status).await.unwrap()["upstream_ok"] != true {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    let health = edge_status(status).await?;
    assert_eq!(health["upstream"], "node-a");
    assert!(health["since_ok_ms"].is_u64() && health["upstream_rtt_ms"].is_number());

    for req in [
        Request::get("/").header("host", "dev.example"),
        Request::get("/")
            .header("host", "dev.example")
            .header("cf-connecting-ip", "192.0.2.1")
            .header("connection", "cf-connecting-ip"),
    ] {
        assert_eq!(edge_request(listen, req.body(Full::default())?).await?.status(), 400);
    }

    // The echo starts before the upload finishes, through both proxies.
    let mut stream = TcpStream::connect(listen).await?;
    stream.write_all(b"PUT /blob?q=1 HTTP/1.1\r\nHost: dev.example:443\r\nCF-Connecting-IP: 192.0.2.1\r\nYo-Rtt: 3\r\nConnection: x-hop, close\r\nX-Hop: drop\r\nContent-Length: 1048576\r\n\r\n").await?;
    let payload: Vec<u8> = (0..1048576).map(|i| (i % 251) as u8).collect();
    stream.write_all(&payload[..1024]).await?;
    let mut received = Vec::new();
    let mut chunk = [0u8; 4096];
    let body_start = timeout(Duration::from_secs(3), async {
        loop {
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            received.extend_from_slice(&chunk[..n]);
            if let Some(i) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                if received.len() > i + 4 {
                    break i + 4;
                }
            }
        }
    })
    .await?;
    let headers = std::str::from_utf8(&received[..body_start])?.to_ascii_lowercase();
    for expected in [
        "http/1.1 201",
        "x-method: put",
        "x-path: /blob?q=1",
        "x-host: dev.example:443",
        "x-ip: 192.0.2.1",
        "yo-rtt: 3",
        "content-length: 1048576",
    ] {
        assert!(headers.contains(expected), "{headers}");
    }
    assert!(!headers.contains("x-return-hop"));
    let (mut reader, mut writer) = stream.into_split();
    let (written, read) = timeout(Duration::from_secs(5), async {
        tokio::join!(writer.write_all(&payload[1024..]), reader.read_to_end(&mut received))
    })
    .await?;
    written?;
    read?;
    assert_eq!(&received[body_start..], payload.as_slice());

    let slow = tokio::spawn(edge_request(listen, get("/slow")));
    timeout(Duration::from_secs(1), async {
        while edge_status(status).await.unwrap()["in_flight"] != 1 {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    stop.send(()).unwrap();
    timeout(Duration::from_secs(1), async {
        while TcpStream::connect(listen).await.is_ok() {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    assert_eq!(slow.await??.status(), 201);
    timeout(Duration::from_secs(1), edge).await???;

    let (edge, _stop) = start_edge();
    timeout(Duration::from_secs(6), async {
        while !edge_status(status).await.is_ok_and(|s| s["upstream_ok"] == true) {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    server.abort();
    let _ = server.await;
    assert_eq!(timeout(Duration::from_secs(1), edge_request(listen, get("/slow"))).await??.status(), 502);
    assert_eq!(timeout(Duration::from_secs(13), edge).await??.unwrap_err().to_string(), "upstream unreachable");
    echo_server.abort();
    Ok(())
}
