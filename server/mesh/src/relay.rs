use super::*;
use forward::{empty, full, Payload};
use hyper::{body::Incoming, header::HOST, server::conn::http1, Version};
use std::{
    convert::Infallible,
    os::unix::{ffi::OsStrExt, net::UnixDatagram},
    sync::atomic::{AtomicUsize, Ordering},
};

const UPSTREAM_DEAD: Duration = Duration::from_secs(10);

#[derive(Default, Clone)]
pub(super) struct Upstream {
    pub last_ok: Option<Instant>,
    pub rtt: Option<f64>,
    pub sender: Option<Sender>,
}

impl Upstream {
    fn ok(&self) -> bool {
        self.sender.as_ref().is_some_and(|s| !s.is_closed())
            && self.last_ok.is_some_and(|at| at.elapsed() <= 3 * SECOND)
    }
}

struct Flight(Arc<AtomicUsize>);

impl Flight {
    fn start(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::Relaxed);
        Self(count.clone())
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

struct Edge {
    name: String,
    upstream: watch::Receiver<Upstream>,
    flights: Arc<AtomicUsize>,
}

impl Edge {
    fn status(&self, req: Request<Incoming>) -> Response<Payload> {
        if req.method() != Method::GET || req.uri().path() != "/status" {
            return empty(404);
        }
        let u = self.upstream.borrow();
        let json = json!({"in_flight": self.flights.load(Ordering::Relaxed), "upstream": self.name,
            "upstream_ok": u.ok(), "upstream_rtt_ms": u.rtt, "since_ok_ms": u.last_ok.map(|at| at.elapsed().as_millis())});
        Response::builder().header("content-type", "application/json").body(full(json.to_string())).unwrap()
    }

    async fn send(&self, mut req: Request<Incoming>) -> Response<Payload> {
        let mut changes = self.upstream.clone();
        let mut sender = {
            let upstream = changes.borrow_and_update();
            if !upstream.ok() {
                return empty(502);
            }
            upstream.sender.clone().unwrap()
        };
        let Some(host) = req.headers_mut().remove(HOST).and_then(|v| v.to_str().ok()?.parse().ok()) else {
            return empty(400);
        };
        if forward::rewrite(&mut req, "https", host, Version::HTTP_2).is_err() {
            return empty(400);
        }
        req.headers_mut().insert(forward::MARKER, "1".parse().unwrap());
        let flight = Flight::start(&self.flights);
        let head = sender.send_request(req.map(BodyExt::boxed_unsync));
        let res = tokio::select! {
            res = timeout(75 * SECOND, head) => match res {
                Ok(Ok(res)) => res,
                _ => return empty(502),
            },
            _ = changes.wait_for(|u| !u.ok()) => return empty(502),
        };
        forward::response(res, Version::HTTP_11).map(|body| {
            // Count the response through its last frame or cancellation.
            body.map_frame(move |frame| {
                let _ = &flight;
                frame
            })
            .boxed_unsync()
        })
    }
}

fn notify_ready() -> Result<()> {
    let Some(path) = env::var_os("NOTIFY_SOCKET") else { return Ok(()) };
    let socket = UnixDatagram::unbound()?;
    #[cfg(target_os = "linux")]
    if let Some(name) = path.as_bytes().strip_prefix(b"@") {
        use std::os::{linux::net::SocketAddrExt, unix::net::SocketAddr};
        socket.send_to_addr(b"READY=1", &SocketAddr::from_abstract_name(name)?)?;
        return Ok(());
    }
    socket.send_to(b"READY=1", path)?;
    Ok(())
}

async fn connection(stream: TcpStream, edge: Arc<Edge>, status: bool, mut stop: watch::Receiver<bool>) {
    let service = service_fn(move |req| {
        let edge = edge.clone();
        async move { Ok::<_, Infallible>(if status { edge.status(req) } else { edge.send(req).await }) }
    });
    let conn = http1::Builder::new().serve_connection(TokioIo::new(stream), service);
    tokio::pin!(conn);
    tokio::select! {
        _ = &mut conn => {}
        _ = stop.changed() => {
            conn.as_mut().graceful_shutdown();
            let _ = conn.await;
        }
    }
}

/// Serve the loopback relay and heartbeat over the same peer connection.
pub async fn relay(
    config: Config,
    upstream: String,
    listen: SocketAddr,
    status: SocketAddr,
    shutdown: impl Future<Output = ()>,
) -> Result<()> {
    if !listen.ip().is_loopback() || !status.ip().is_loopback() {
        return Err("relay listeners require loopback".into());
    }
    if !config.peers.contains_key(&upstream) {
        return Err("unknown upstream peer".into());
    }
    let listener = TcpListener::bind(listen).await?;
    let status = TcpListener::bind(status).await?;
    let (tx, mut health) = watch::channel(Upstream::default());
    let mut mesh = start(config, Some((&upstream, tx)))?;
    let edge = Arc::new(Edge { name: upstream, upstream: health.clone(), flights: Arc::default() });
    let (stop, stopping) = watch::channel(false);
    let mut connections = JoinSet::new();
    let mut ready = false;
    tokio::pin!(shutdown);
    let dead = loop {
        let last_ok = health.borrow_and_update().last_ok;
        if let Some(at) = last_ok {
            if !ready {
                notify_ready()?;
                ready = true;
            }
            if at.elapsed() > UPSTREAM_DEAD {
                break true;
            }
        }
        tokio::select! {
            biased;
            _ = &mut shutdown => break false,
            _ = sleep(last_ok.map_or(UPSTREAM_DEAD, |at| UPSTREAM_DEAD.saturating_sub(at.elapsed()))) => {}
            result = health.changed() => {result?;}
            result = mesh.join_next() => { result.unwrap()?; return Err("mesh task stopped".into()); }
            Some(_) = connections.join_next() => {}
            stream = status.accept() => { connections.spawn(connection(stream?.0, edge.clone(), true, stopping.clone())); }
            stream = listener.accept() => { connections.spawn(connection(stream?.0, edge.clone(), false, stopping.clone())); }
        }
    };
    drop((listener, status));
    stop.send_replace(true);
    let grace = if dead { UPSTREAM_DEAD } else { 30 * SECOND };
    let _ = timeout(grace, async { while connections.join_next().await.is_some() {} }).await;
    if dead {
        Err("upstream unreachable".into())
    } else {
        Ok(())
    }
}
