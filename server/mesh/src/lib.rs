mod tls;
pub use tls::{client_config, server_config, Certs};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{client::conn::http2::SendRequest, service::service_fn, Method, Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::ServerName;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinSet,
    time::{interval, sleep, timeout, MissedTickBehavior},
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const VERSION: &str = env!("CARGO_PKG_VERSION");
const SECOND: Duration = Duration::from_secs(1);

#[derive(Deserialize)]
pub struct Config {
    pub name: String,
    pub listen: SocketAddr,
    pub peers: BTreeMap<String, SocketAddr>,
    pub certs: Option<PathBuf>,
    pub status: Option<PathBuf>,
}

pub fn load(path: impl AsRef<Path>) -> Result<Config> {
    Ok(toml::from_str(&fs::read_to_string(path)?)?)
}

#[derive(Default, Serialize, Deserialize)]
struct Peer {
    online: bool,
    rtt_ms: Option<f64>,
    last_ok_ms: Option<u64>,
    error: Option<String>,
    /// The peer's own view of its peers, from its last answer: one status file shows the whole mesh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    view: Option<BTreeMap<String, Peer>>,
}

type Peers = Arc<Mutex<BTreeMap<String, Peer>>>;
type Body = Full<Bytes>;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn ticks() -> tokio::time::Interval {
    let mut timer = interval(SECOND);
    timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
    timer
}

async fn serve(name: Arc<str>, peers: Peers, stream: TcpStream, tls: TlsAcceptor) -> Result<()> {
    let stream = timeout(5 * SECOND, tls.accept(stream)).await??;
    if stream.get_ref().1.alpn_protocol() != Some(b"h2") {
        return Err("h2 required".into());
    }
    let service =
        service_fn(move |req: Request<hyper::body::Incoming>| {
            let response =
                if req.method() == Method::GET && req.uri().path() == "/ping" {
                    // Its own view only (no views of views): the answer stays small.
                    let view: BTreeMap<_, _> = peers.lock().unwrap().iter().map(|(n, p)| (n.clone(), json!({
                "online": p.online, "rtt_ms": p.rtt_ms, "last_ok_ms": p.last_ok_ms, "error": p.error,
            }))).collect();
                    let ping = json!({"node": &*name, "version": VERSION, "time_ms": now_ms(), "peers": view});
                    Response::builder().header("content-type", "application/json").body(Body::from(ping.to_string()))
                } else {
                    Response::builder().status(404).body(Body::default())
                };
            async move { response }
        });
    hyper::server::conn::http2::Builder::new(TokioExecutor::new())
        .serve_connection(TokioIo::new(stream), service)
        .await?;
    Ok(())
}

async fn server(addr: SocketAddr, name: Arc<str>, peers: Peers, tls: TlsAcceptor) {
    let listener = loop {
        match TcpListener::bind(addr).await {
            Ok(listener) => break listener,
            Err(e) => eprintln!("bind {addr}: {e}"),
        }
        sleep(2 * SECOND).await;
    };
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            Some(_) = connections.join_next() => {}
            accepted = listener.accept(), if connections.len() < 64 => match accepted {
                Ok((stream, peer)) => {
                    let (name, peers, tls) = (name.clone(), peers.clone(), tls.clone());
                    connections.spawn(async move {
                        if let Err(e) = serve(name, peers, stream, tls).await {
                            eprintln!("connection from {peer}: {e}");
                        }
                    });
                }
                Err(e) => {
                    eprintln!("accept: {e}");
                    sleep(SECOND).await;
                }
            }
        }
    }
}

type View = Option<BTreeMap<String, Peer>>;

async fn ping(
    sender: &mut Option<SendRequest<Body>>,
    tls: &TlsConnector,
    name: &str,
    addr: SocketAddr,
) -> Result<(f64, View)> {
    if sender.is_none() {
        let stream = tls.connect(ServerName::try_from(name.to_owned())?, TcpStream::connect(addr).await?).await?;
        if stream.get_ref().1.alpn_protocol() != Some(b"h2") {
            return Err("h2 required".into());
        }
        let (tx, connection) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream)).await?;
        tokio::spawn(connection);
        *sender = Some(tx);
    }
    let req = Request::get(format!("https://{name}/ping")).body(Body::default())?;
    let start = Instant::now();
    let response = sender.as_mut().unwrap().send_request(req).await?;
    if response.status() != 200 {
        return Err("unexpected ping status".into());
    }
    let bytes = Limited::new(response.into_body(), 4096).collect().await?.to_bytes();
    let rtt = (start.elapsed().as_secs_f64() * 10_000.0).round() / 10.0;
    #[derive(Deserialize)]
    struct Ping {
        node: String,
        #[serde(default)]
        peers: View,
    }
    let ping: Ping = serde_json::from_slice(&bytes)?;
    if ping.node != name {
        return Err("unexpected node name".into());
    }
    Ok((rtt, ping.peers))
}

async fn pinger(name: String, addr: SocketAddr, tls: TlsConnector, peers: Peers) {
    let mut timer = ticks();
    let mut sender = None;
    loop {
        timer.tick().await;
        let result = timeout(2 * SECOND, ping(&mut sender, &tls, &name, addr))
            .await
            .unwrap_or_else(|_| Err("ping timed out".into()));
        let mut peers = peers.lock().unwrap();
        let peer = peers.get_mut(&name).unwrap();
        match result {
            Ok((rtt, view)) => {
                *peer = Peer { online: true, rtt_ms: Some(rtt), last_ok_ms: Some(now_ms()), error: None, view }
            }
            Err(error) => {
                sender = None;
                peer.online = false;
                peer.rtt_ms = None;
                peer.view = None;
                let message = error.to_string();
                peer.error = Some(message.lines().next().unwrap_or("ping failed").chars().take(160).collect());
            }
        }
    }
}

fn write_status(path: &Path, name: &str, peers: &Peers) -> Result<()> {
    let status = json!({"node": name, "version": VERSION, "at_ms": now_ms(), "peers": *peers.lock().unwrap()});
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(&serde_json::to_vec(&status)?)?;
    file.set_permissions(fs::Permissions::from_mode(0o644))?;
    fs::rename(tmp, path)?;
    Ok(())
}

pub async fn run(config: Config) -> Result<()> {
    let dir = config
        .certs
        .or_else(|| env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from))
        .ok_or("set certs or CREDENTIALS_DIRECTORY")?;
    let certs = Certs::load(&dir)?;
    let acceptor = TlsAcceptor::from(server_config(&certs)?);
    let connector = TlsConnector::from(client_config(&certs)?);
    let status = config.status.unwrap_or_else(|| {
        env::var_os("RUNTIME_DIRECTORY").map(PathBuf::from).unwrap_or_else(|| "/run/yo-mesh".into()).join("status.json")
    });
    let peers: Peers = Arc::new(Mutex::new(config.peers.keys().map(|n| (n.clone(), Peer::default())).collect()));
    let mut tasks = JoinSet::new();
    tasks.spawn(server(config.listen, Arc::from(config.name.as_str()), peers.clone(), acceptor));
    for (name, addr) in config.peers {
        rustls::pki_types::DnsName::try_from(name.as_str())?;
        tasks.spawn(pinger(name, addr, connector.clone(), peers.clone()));
    }
    tasks.spawn(async move {
        let mut timer = ticks();
        loop {
            timer.tick().await;
            if let Err(e) = write_status(&status, &config.name, &peers) {
                eprintln!("write {}: {e}", status.display());
            }
        }
    });
    tasks.join_next().await.unwrap()?;
    Err("mesh task stopped".into())
}
