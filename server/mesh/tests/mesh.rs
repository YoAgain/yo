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
