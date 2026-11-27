mod forward;
mod relay;
mod tls;
use forward::{Forward, Payload};
pub use relay::relay;
use std::future::{pending, Future};
pub use tls::{client_config, server_config, Certs};
use tokio::sync::watch;

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
    io::{BufReader, BufWriter, Write},
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
    pub forward: Option<SocketAddr>,
    /// Relayed requests by their host (no port): one mesh carries live and dev, the hub, its API and the admin.
    /// A host not named here goes to `forward`.
    #[serde(default)]
    pub forwards: BTreeMap<String, SocketAddr>,
    pub history: Option<PathBuf>,
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
type Sender = SendRequest<Payload>;

const MINUTES: usize = 1440;
const MINUTE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Default)]
struct Bucket {
    minute: u64,
    sum: f32,
    ok: u16,
    fail: u16,
}

struct History(Vec<Bucket>);

impl Default for History {
    fn default() -> Self {
        Self(vec![Bucket::default(); MINUTES])
    }
}

impl History {
    fn record<E>(&mut self, minute: u64, result: std::result::Result<f64, E>) {
        let slot = &mut self.0[(minute % MINUTES as u64) as usize];
        if slot.minute != minute {
            *slot = Bucket { minute, ..Bucket::default() };
        }
        match result {
            Ok(ms) if slot.ok < u16::MAX => {
                slot.ok += 1;
                slot.sum += ms as f32;
            }
            Ok(_) => {}
            Err(_) => slot.fail = slot.fail.saturating_add(1),
        }
    }

    fn to_json(&self, end_min: u64) -> Series {
        let mut series = Series {
            rtt: vec![None; MINUTES].into_boxed_slice(),
            ok: vec![0; MINUTES].into_boxed_slice(),
            fail: vec![0; MINUTES].into_boxed_slice(),
        };
        for slot in &self.0 {
            if slot.ok == 0 && slot.fail == 0 {
                continue;
            }
            let Some(age) = end_min.checked_sub(slot.minute).filter(|&age| age < MINUTES as u64) else { continue };
            let i = MINUTES - 1 - age as usize;
            series.ok[i] = slot.ok;
            series.fail[i] = slot.fail;
            if slot.ok > 0 {
                series.rtt[i] = Some((slot.sum / f32::from(slot.ok) * 10.0).round() / 10.0);
            }
        }
        series
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Series {
    rtt: Box<[Option<f32>]>,
    ok: Box<[u16]>,
    fail: Box<[u16]>,
}

impl Series {
    fn valid(&self) -> bool {
        [self.rtt.len(), self.ok.len(), self.fail.len()] == [MINUTES; 3]
    }

    fn realign(&mut self, from: u64, to: u64) {
        fn shift<T: Copy + Default>(values: &mut [T], from: u64, to: u64) {
            let n = values.len();
            let by = from.abs_diff(to).min(n as u64) as usize;
            if from < to {
                values.copy_within(by.., 0);
                values[n - by..].fill(T::default());
            } else if from > to {
                values.copy_within(..n - by, by);
                values[..by].fill(T::default());
            }
        }
        shift(&mut self.rtt, from, to);
        shift(&mut self.ok, from, to);
        shift(&mut self.fail, from, to);
    }
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    node: String,
    end_min: u64,
    links: BTreeMap<String, Series>,
}

#[derive(Default)]
struct Histories {
    own: BTreeMap<String, History>,
    fetched: BTreeMap<String, (Instant, Snapshot)>,
}

impl Histories {
    fn seed(&mut self, path: &Path, node: &str, now: u64) {
        let Some(snapshot) =
            fs::File::open(path).ok().and_then(|f| serde_json::from_reader::<_, Snapshot>(BufReader::new(f)).ok())
        else {
            return;
        };
        if snapshot.node != node || snapshot.end_min.abs_diff(now) >= MINUTES as u64 {
            return;
        }
        for (peer, ring) in &mut self.own {
            let Some(series) = snapshot.links.get(&format!("{node}>{peer}")).filter(|s| s.valid()) else { continue };
            for i in 0..MINUTES {
                let Some(minute) = snapshot.end_min.checked_sub((MINUTES - 1 - i) as u64) else { continue };
                if minute > now || now - minute >= MINUTES as u64 {
                    continue;
                }
                ring.0[minute as usize % MINUTES] = Bucket {
                    minute,
                    ok: series.ok[i],
                    fail: series.fail[i],
                    sum: series.rtt[i].unwrap_or_default() * f32::from(series.ok[i]),
                };
            }
        }
    }

    fn snapshot(&self, node: &str, end_min: u64) -> Snapshot {
        let links = self.own.iter().map(|(peer, h)| (format!("{node}>{peer}"), h.to_json(end_min))).collect();
        Snapshot { node: node.into(), end_min, links }
    }

    fn merged(&mut self, node: &str, end_min: u64) -> Snapshot {
        self.fetched.retain(|_, (at, _)| at.elapsed() < 10 * MINUTE);
        let mut snapshot = self.snapshot(node, end_min);
        for (_, peer) in self.fetched.values() {
            for (key, values) in &peer.links {
                let mut values = values.clone();
                values.realign(peer.end_min, end_min);
                snapshot.links.insert(key.clone(), values);
            }
        }
        snapshot
    }
}

type SharedHistory = Arc<Mutex<Histories>>;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn ticks(period: Duration) -> tokio::time::Interval {
    let mut timer = interval(period);
    timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
    timer
}

async fn serve(
    name: Arc<str>,
    peers: Peers,
    history: SharedHistory,
    forward: Option<Forward>,
    stream: TcpStream,
    tls: TlsAcceptor,
) -> Result<()> {
    let stream = timeout(5 * SECOND, tls.accept(stream)).await??;
    if stream.get_ref().1.alpn_protocol() != Some(b"h2") {
        return Err("h2 required".into());
    }
    let service =
        service_fn(move |req: Request<hyper::body::Incoming>| {
            let forward = forward.clone();
            let (name, peers, history) = (name.clone(), peers.clone(), history.clone());
            async move {
                if req.headers().get(forward::MARKER).is_some_and(|v| v == "1") {
                    return Ok(match forward {
                        Some(f) => f.send(req).await,
                        None => forward::empty(404),
                    });
                }

                if req.method() == Method::GET && req.uri().path() == "/ping" {
                    // Its own view only (no views of views): the answer stays small.
                    let view: BTreeMap<_, _> = peers.lock().unwrap().iter().map(|(n, p)| (n.clone(), json!({
                "online": p.online, "rtt_ms": p.rtt_ms, "last_ok_ms": p.last_ok_ms, "error": p.error,
            }))).collect();
                    let ping = json!({"node": &*name, "version": VERSION, "time_ms": now_ms(), "peers": view});
                    Response::builder().header("content-type", "application/json").body(forward::full(ping.to_string()))
                } else if req.method() == Method::GET && req.uri().path() == "/history" {
                    let snapshot = history.lock().unwrap().snapshot(&name, now_ms() / 60_000);
                    Response::builder()
                        .header("content-type", "application/json")
                        .body(forward::full(serde_json::to_vec(&snapshot).unwrap()))
                } else {
                    Ok(forward::empty(404))
                }
            }
        });
    // Windows that grow with the link (BDP): a relayed photo over a 250 ms link is not held to 64 KiB per round trip.
    hyper::server::conn::http2::Builder::new(TokioExecutor::new())
        .adaptive_window(true)
        .serve_connection(TokioIo::new(stream), service)
        .await?;
    Ok(())
}

async fn server(
    addr: SocketAddr,
    name: Arc<str>,
    peers: Peers,
    history: SharedHistory,
    tls: TlsAcceptor,
    forward: Option<Forward>,
) {
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
                    let (name, peers, history, tls) = (name.clone(), peers.clone(), history.clone(), tls.clone());
                    let forward = forward.clone();
                    connections.spawn(async move {
                        if let Err(e) = serve(name, peers, history, forward, stream, tls).await {
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

async fn connect(tls: &TlsConnector, name: &str, addr: SocketAddr) -> Result<Sender> {
    let stream = tls.connect(ServerName::try_from(name.to_owned())?, TcpStream::connect(addr).await?).await?;
    if stream.get_ref().1.alpn_protocol() != Some(b"h2") {
        return Err("h2 required".into());
    }
    let (tx, connection) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .adaptive_window(true)
        .handshake(TokioIo::new(stream))
        .await?;
    tokio::spawn(connection);
    Ok(tx)
}

/// An outbound-only mesh connection pool. Concurrent requests share HTTP/2 streams.
pub struct Client {
    tls: TlsConnector,
    peers: BTreeMap<String, (SocketAddr, tokio::sync::Mutex<Option<Sender>>)>,
    status: PathBuf,
    host: Option<String>,
}

impl Client {
    pub fn new(config: &Config) -> Result<Self> {
        Self::configured(&config.name, &config.peers, config.certs.as_deref(), config.status.as_deref())
    }

    /// Read only the outbound fields. The same file may also configure a listening mesh node.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        #[derive(Deserialize)]
        struct Outbound {
            name: String,
            peers: BTreeMap<String, SocketAddr>,
            certs: Option<PathBuf>,
            status: Option<PathBuf>,
        }
        let config: Outbound = toml::from_str(&fs::read_to_string(path)?)?;
        Self::configured(&config.name, &config.peers, config.certs.as_deref(), config.status.as_deref())
    }

    fn configured(
        node: &str,
        addresses: &BTreeMap<String, SocketAddr>,
        dir: Option<&Path>,
        status: Option<&Path>,
    ) -> Result<Self> {
        let certs = load_certs(dir)?;
        let mut peers = BTreeMap::new();
        for (name, addr) in addresses {
            if name != node {
                rustls::pki_types::DnsName::try_from(name.as_str())?;
                peers.insert(name.clone(), (*addr, Default::default()));
            }
        }
        // The node on this server writes its heartbeat there (another process: its runtime directory, not ours).
        let status = status.map_or_else(|| PathBuf::from("/run/yo-mesh/status.json"), Path::to_owned);
        Ok(Self { tls: TlsConnector::from(client_config(&certs)?), peers, status, host: None })
    }

    /// The host the requests name (the peer's `forwards` pick the hub by it); the peer's node name without one.
    pub fn with_host(mut self, host: Option<String>) -> Self {
        self.host = host;
        self
    }

    pub fn peers(&self) -> impl Iterator<Item = &str> {
        self.peers.keys().map(String::as_str)
    }

    /// The peers the heartbeat of this server's node reports down. None without a fresh status file (no node, or a
    /// stopped one), and none the node has not pinged yet.
    pub fn down(&self) -> Vec<String> {
        #[derive(Deserialize)]
        struct Status {
            at_ms: u64,
            peers: BTreeMap<String, Peer>,
        }
        let Some(status) = fs::read(&self.status).ok().and_then(|b| serde_json::from_slice::<Status>(&b).ok()) else {
            return Vec::new();
        };
        if now_ms().abs_diff(status.at_ms) > 5_000 {
            return Vec::new();
        }
        status.peers.into_iter().filter(|(_, p)| !p.online && p.error.is_some()).map(|(name, _)| name).collect()
    }

    pub async fn send(&self, peer: &str, mut req: Request<Bytes>, deadline: Duration) -> Result<Response<Bytes>> {
        let (addr, cached) = self.peers.get(peer).ok_or("unknown mesh peer")?;
        timeout(deadline, async {
            let mut sender = {
                let mut slot = cached.lock().await;
                if slot.as_ref().is_none_or(Sender::is_closed) {
                    *slot = Some(connect(&self.tls, peer, *addr).await?);
                }
                slot.as_ref().unwrap().clone()
            };
            let path = req.uri().path_and_query().ok_or("missing path")?;
            *req.uri_mut() = format!("https://{}{path}", self.host.as_deref().unwrap_or(peer)).parse()?;
            req.headers_mut().insert(forward::MARKER, "1".parse()?);
            req.headers_mut().entry("cf-connecting-ip").or_insert("127.0.0.1".parse()?);
            let (parts, body) = sender.send_request(req.map(forward::full)).await?.into_parts();
            let bytes = Limited::new(body, 8 << 20).collect().await?.to_bytes();
            Ok(Response::from_parts(parts, bytes))
        })
        .await?
    }
}

async fn ping(sender: &mut Option<Sender>, tls: &TlsConnector, name: &str, addr: SocketAddr) -> Result<(f64, View)> {
    if sender.is_none() {
        *sender = Some(connect(tls, name, addr).await?);
    }
    let req = Request::get(format!("https://{name}/ping")).body(forward::full(Bytes::new()))?;
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

async fn fetch_history(mut sender: Sender, name: &str) -> Result<Snapshot> {
    let req = Request::get(format!("https://{name}/history")).body(forward::full(Bytes::new()))?;
    let response = sender.send_request(req).await?;
    if response.status() != 200 {
        return Err("unexpected history status".into());
    }
    let bytes = Limited::new(response.into_body(), 512 * 1024).collect().await?.to_bytes();
    let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
    if snapshot.node != name
        || !snapshot.links.iter().all(|(key, series)| {
            key.split_once('>').is_some_and(|(from, to)| {
                from == name && from != to && rustls::pki_types::DnsName::try_from(to).is_ok()
            }) && series.valid()
        })
    {
        return Err("unexpected peer history".into());
    }
    Ok(snapshot)
}

async fn pinger(
    name: String,
    addr: SocketAddr,
    tls: TlsConnector,
    peers: Peers,
    history: SharedHistory,
    upstream: Option<watch::Sender<relay::Upstream>>,
) {
    let mut timer = ticks(SECOND);
    let mut sender = None;
    let mut next_fetch = Instant::now();
    let mut fetch = JoinSet::new();
    loop {
        timer.tick().await;
        let _ = fetch.try_join_next();
        let result = timeout(2 * SECOND, ping(&mut sender, &tls, &name, addr))
            .await
            .unwrap_or_else(|_| Err("ping timed out".into()));
        let at = now_ms();
        history.lock().unwrap().own.get_mut(&name).unwrap().record(at / 60_000, result.as_ref().map(|(ms, _)| *ms));
        if result.is_ok() && Instant::now() >= next_fetch {
            next_fetch = Instant::now() + MINUTE;
            let sender = sender.as_ref().unwrap().clone();
            let (name, history) = (name.clone(), history.clone());
            fetch.spawn(async move {
                if let Ok(Ok(snapshot)) = timeout(5 * SECOND, fetch_history(sender, &name)).await {
                    history.lock().unwrap().fetched.insert(name, (Instant::now(), snapshot));
                }
            });
        }
        if let Some(upstream) = &upstream {
            upstream.send_modify(|u| match &result {
                Ok((rtt, _)) => {
                    u.last_ok = Some(Instant::now());
                    u.rtt = Some(*rtt);
                    u.sender = sender.clone();
                }
                Err(_) => u.sender = None,
            });
        }
        let mut peers = peers.lock().unwrap();
        let peer = peers.get_mut(&name).unwrap();
        match result {
            Ok((rtt, view)) => {
                *peer = Peer { online: true, rtt_ms: Some(rtt), last_ok_ms: Some(at), error: None, view }
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
    write_json(path, &status)
}

fn write_history(path: &Path, node: &str, history: &SharedHistory) -> Result<()> {
    #[derive(Serialize)]
    struct File {
        version: &'static str,
        at_ms: u64,
        minutes: usize,
        #[serde(flatten)]
        snapshot: Snapshot,
    }
    let at_ms = now_ms();
    let snapshot = history.lock().unwrap().merged(node, at_ms / 60_000);
    write_json(path, &File { version: VERSION, at_ms, minutes: MINUTES, snapshot })
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let file = fs::File::create(&tmp)?;
    let mut writer = BufWriter::new(&file);
    serde_json::to_writer(&mut writer, value)?;
    writer.flush()?;
    file.set_permissions(fs::Permissions::from_mode(0o644))?;
    fs::rename(tmp, path)?;
    Ok(())
}

pub async fn run(config: Config) -> Result<()> {
    run_with(config, pending()).await
}

/// Run the heartbeat until shutdown.
pub async fn run_with(config: Config, shutdown: impl Future<Output = ()>) -> Result<()> {
    let mut tasks = start(config, None)?;
    tokio::select! {
        _ = shutdown => Ok(()),
        result = tasks.join_next() => { result.unwrap()?; Err("mesh task stopped".into()) }
    }
}

fn load_certs(configured: Option<&Path>) -> Result<Certs> {
    let dir = configured
        .map(Path::to_owned)
        .or_else(|| env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from))
        .ok_or("set certs or CREDENTIALS_DIRECTORY")?;
    Certs::load(&dir)
}

fn start(config: Config, monitored: Option<(&str, watch::Sender<relay::Upstream>)>) -> Result<JoinSet<()>> {
    let certs = load_certs(config.certs.as_deref())?;
    let acceptor = TlsAcceptor::from(server_config(&certs)?);
    let connector = TlsConnector::from(client_config(&certs)?);
    let status = config.status.unwrap_or_else(|| {
        env::var_os("RUNTIME_DIRECTORY").map(PathBuf::from).unwrap_or_else(|| "/run/yo-mesh".into()).join("status.json")
    });
    let peers: Peers = Arc::new(Mutex::new(config.peers.keys().map(|n| (n.clone(), Peer::default())).collect()));
    let history_path = config.history.unwrap_or_else(|| status.with_file_name("history.json"));
    let history = Arc::new(Mutex::new(Histories {
        own: config.peers.keys().map(|name| (name.clone(), History::default())).collect(),
        ..Histories::default()
    }));
    history.lock().unwrap().seed(&history_path, &config.name, now_ms() / 60_000);
    let mut tasks = JoinSet::new();
    tasks.spawn(server(
        config.listen,
        Arc::from(config.name.as_str()),
        peers.clone(),
        history.clone(),
        acceptor,
        Forward::new(config.forward, config.forwards),
    ));
    for (name, addr) in config.peers {
        rustls::pki_types::DnsName::try_from(name.as_str())?;
        let upstream = monitored.as_ref().filter(|(peer, _)| *peer == name).map(|(_, tx)| tx.clone());
        tasks.spawn(pinger(name, addr, connector.clone(), peers.clone(), history.clone(), upstream));
    }
    let node = config.name.clone();
    tasks.spawn(async move {
        let mut timer = ticks(MINUTE);
        loop {
            timer.tick().await;
            if let Err(e) = write_history(&history_path, &node, &history) {
                eprintln!("write {}: {e}", history_path.display());
            }
        }
    });
    tasks.spawn(async move {
        let mut timer = ticks(SECOND);
        loop {
            timer.tick().await;
            if let Err(e) = write_status(&status, &config.name, &peers) {
                eprintln!("write {}: {e}", status.display());
            }
        }
    });
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minute_history() {
        let mut h = History::default();
        h.record(0, Ok::<_, ()>(10.0));
        h.record(0, Ok::<_, ()>(12.3));
        h.record(0, Err(()));
        h.record(2, Err(()));
        let s = h.to_json(2);
        assert_eq!(&s.rtt[MINUTES - 3..], &[Some(11.2), None, None]);
        assert_eq!(&s.ok[MINUTES - 3..], &[2, 0, 0]);
        assert_eq!(&s.fail[MINUTES - 3..], &[1, 0, 1]);

        h.record(MINUTES as u64, Ok::<_, ()>(4.0));
        let s = h.to_json(MINUTES as u64);
        assert_eq!(s.fail[1], 1);
        assert_eq!(s.ok[MINUTES - 1], 1);
        assert_eq!(s.fail[MINUTES - 1], 0);
        assert_eq!(s.rtt[MINUTES - 1], Some(4.0));
        assert!(h.to_json(3 * MINUTES as u64).rtt.iter().all(Option::is_none));
    }

    #[test]
    fn seed_local_minutes() {
        let end = MINUTES as u64 + 10;
        let mut saved = Histories::default();
        let ring = saved.own.entry("b".into()).or_default();
        ring.record(end, Ok::<_, ()>(6.0));
        ring.record(end, Err(()));
        let mut file = saved.snapshot("a", end);
        file.links.insert("b>a".into(), file.links["a>b"].clone());
        let id = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = env::temp_dir().join(format!("yo-mesh-seed-{}-{id}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("history.json");
        write_json(&path, &file).unwrap();

        let restore = |now| {
            let mut restored = Histories::default();
            restored.own.insert("b".into(), History::default());
            restored.seed(&path, "a", now);
            restored
        };
        let mut restored = restore(end + 1);
        restored.own.get_mut("b").unwrap().record(end, Ok::<_, ()>(10.0));
        let links = restored.merged("a", end + 1).links;
        assert_eq!(links.len(), 1);
        let series = &links["a>b"];
        assert_eq!(series.ok[MINUTES - 2], 2);
        assert_eq!(series.fail[MINUTES - 2], 1);
        assert_eq!(series.rtt[MINUTES - 2], Some(8.0));
        let stale = restore(end + MINUTES as u64);
        assert_eq!(stale.snapshot("a", end).links["a>b"].ok[MINUTES - 1], 0);
        fs::write(&path, b"{").unwrap();
        assert_eq!(restore(end).snapshot("a", end).links["a>b"].ok[MINUTES - 1], 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn merge_until_expired() {
        let mut local = Histories::default();
        local.own.insert("b".into(), History::default());
        let mut remote = Histories::default();
        remote.own.entry("a".into()).or_default().record(100, Ok::<_, ()>(12.0));
        local.fetched.insert("b".into(), (Instant::now(), remote.snapshot("b", 100)));
        let snapshot = local.merged("a", 101);
        assert_eq!(snapshot.links.len(), 2);
        assert_eq!(snapshot.links["b>a"].rtt[MINUTES - 2], Some(12.0));
        local.fetched.get_mut("b").unwrap().0 = Instant::now() - 10 * MINUTE;
        assert_eq!(local.merged("a", 110).links.len(), 1);
        assert!(local.fetched.is_empty());
    }

    #[test]
    fn align_peer_minutes() {
        let end = MINUTES as u64;
        let mut h = History::default();
        h.record(end - 1, Ok::<_, ()>(7.0));
        h.record(end, Err(()));
        for to in [end - 1, end, end + 2, 0, u64::MAX] {
            let mut s = h.to_json(end);
            s.realign(end, to);
            let expected = h.to_json(to);
            assert_eq!(s.rtt, expected.rtt);
            assert_eq!(s.ok, expected.ok);
            assert_eq!(s.fail, expected.fail);
        }
    }
}
