//! HTTP/3 to the hub (its front advertises `alt-svc: h3`): one QUIC endpoint and connection per line (spec 1.1: the
//! lines share no socket, connection or TLS session), no 0-RTT, kept warm by QUIC PINGs as HTTP/2 is by its own.
//! UDP that does not get through costs one short try: the process then stays on HTTP/2 (`YO_HTTP3=0` from the
//! start).

use bytes::{Buf, Bytes};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// A QUIC handshake the hub's front answers in a round trip; past this, UDP is taken as blocked.
const HANDSHAKE: Duration = Duration::from_millis(1500);
/// QUIC PINGs keep the connection (and the NAT's mapping) for the first Yo after a pause.
const KEEPALIVE: Duration = Duration::from_secs(15);
/// Silence past this ends a connection (PINGs unanswered: the path is gone), so the next request makes a new one.
const IDLE: Duration = Duration::from_secs(40);

/// Off for the process: UDP did not get through once, or `YO_HTTP3=0`.
static OFF: AtomicBool = AtomicBool::new(false);

type Sender = h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>;

/// Why HTTP/3 gave no answer.
pub enum Failed {
    /// Nothing was sent: no connection came about. HTTP/2 may take the request.
    Unreachable(String),
    /// The request may have reached the hub: not sent again on another protocol.
    Broken(String),
}

/// One line's HTTP/3: its runtime, endpoint and connection, made on first use.
pub struct Quic {
    host: String,
    port: u16,
    tls: Arc<rustls::ClientConfig>,
    state: OnceLock<Result<Runtime, String>>,
    connection: Mutex<Option<(quinn::Connection, Sender)>>,
}

struct Runtime {
    runtime: tokio::runtime::Runtime,
    endpoint: Mutex<Option<quinn::Endpoint>>,
}

impl Quic {
    /// For an `https://` hub; `None` for any other (a test hub over plain HTTP) or when HTTP/3 is off.
    pub fn new(base: &str, tls: rustls::ClientConfig) -> Option<Quic> {
        if std::env::var("YO_HTTP3").is_ok_and(|v| v == "0") {
            OFF.store(true, Ordering::Relaxed);
        }
        let uri: http::Uri = base.parse().ok()?;
        if uri.scheme_str() != Some("https") {
            return None;
        }
        let mut tls = tls;
        tls.alpn_protocols = vec![b"h3".to_vec()];
        tls.enable_early_data = false;
        Some(Quic {
            host: uri.host()?.to_owned(),
            port: uri.port_u16().unwrap_or(443),
            tls: Arc::new(tls),
            state: OnceLock::new(),
            connection: Mutex::new(None),
        })
    }

    /// The status, the body (at most `limit` bytes), or why not. A connection that cannot come about turns HTTP/3 off
    /// for the process.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
        timeout: Duration,
        limit: u64,
    ) -> Result<(u16, Vec<u8>), Failed> {
        if OFF.load(Ordering::Relaxed) {
            return Err(Failed::Unreachable("off".into()));
        }
        let state = self
            .state
            .get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .thread_name("yo-quic")
                    .enable_all()
                    .build()
                    .map(|runtime| Runtime { runtime, endpoint: Mutex::new(None) })
                    .map_err(|e| format!("quic: {e}"))
            })
            .as_ref()
            .map_err(|e| Failed::Unreachable(e.clone()))?;
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{}:{}{path}", self.host, self.port))
            .body(())
            .map_err(|e| Failed::Unreachable(format!("quic: {e}")))?;
        let (mut parts, ()) = request.into_parts();
        for (name, value) in headers {
            let (Ok(name), Ok(value)) =
                (http::HeaderName::try_from(name.as_str()), http::HeaderValue::try_from(value.as_str()))
            else {
                return Err(Failed::Unreachable("quic: header".into()));
            };
            parts.headers.append(name, value);
        }
        let request = http::Request::from_parts(parts, ());
        let result = state.runtime.block_on(async {
            let mut sender = match self.sender(state).await {
                Ok(sender) => sender,
                Err(error) => {
                    OFF.store(true, Ordering::Relaxed);
                    return Err(Failed::Unreachable(error));
                }
            };
            let exchange = async {
                let mut stream = sender.send_request(request).await.map_err(|e| e.to_string())?;
                if !body.is_empty() {
                    stream.send_data(Bytes::copy_from_slice(body)).await.map_err(|e| e.to_string())?;
                }
                stream.finish().await.map_err(|e| e.to_string())?;
                let response = stream.recv_response().await.map_err(|e| e.to_string())?;
                let mut answer = Vec::new();
                while let Some(mut chunk) = stream.recv_data().await.map_err(|e| e.to_string())? {
                    if answer.len() as u64 + chunk.remaining() as u64 > limit {
                        return Err("answer too large".to_owned());
                    }
                    while chunk.has_remaining() {
                        let part = chunk.chunk();
                        answer.extend_from_slice(part);
                        let n = part.len();
                        chunk.advance(n);
                    }
                }
                Ok((response.status().as_u16(), answer))
            };
            match tokio::time::timeout(timeout, exchange).await {
                Ok(Ok(answer)) => Ok(answer),
                Ok(Err(error)) => Err(Failed::Broken(error)),
                Err(_) => Err(Failed::Broken("no answer in time".into())),
            }
        });
        if let Err(Failed::Broken(_)) = &result {
            // The next request on a new connection, not on one that may be gone.
            if let Some((connection, _)) = self.connection.lock().unwrap_or_else(|e| e.into_inner()).take() {
                connection.close(0u32.into(), b"");
            }
        }
        result
    }

    /// The line's connection, made anew when there is none or it closed.
    async fn sender(&self, state: &Runtime) -> Result<Sender, String> {
        {
            let held = self.connection.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((connection, sender)) = held.as_ref() {
                if connection.close_reason().is_none() {
                    return Ok(sender.clone());
                }
            }
        }
        let address = tokio::net::lookup_host((self.host.as_str(), self.port))
            .await
            .map_err(|e| format!("quic: {e}"))?
            .next()
            .ok_or("quic: no address")?;
        let endpoint = self.endpoint(state, address)?;
        let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(self.tls.clone()).map_err(|e| e.to_string())?;
        let mut config = quinn::ClientConfig::new(Arc::new(crypto));
        let mut transport = quinn::TransportConfig::default();
        transport.keep_alive_interval(Some(KEEPALIVE));
        transport.max_idle_timeout(Some(IDLE.try_into().map_err(|_| "quic: idle")?));
        config.transport_config(Arc::new(transport));
        let started = std::time::Instant::now();
        let connecting = endpoint.connect_with(config, address, &self.host).map_err(|e| format!("quic: {e}"))?;
        let connection = tokio::time::timeout(HANDSHAKE, connecting)
            .await
            .map_err(|_| "quic: no handshake in time".to_owned())?
            .map_err(|e| format!("quic: {e}"))?;
        let (mut driver, sender) =
            h3::client::new(h3_quinn::Connection::new(connection.clone())).await.map_err(|e| format!("h3: {e}"))?;
        tokio::spawn(async move {
            let _ = driver.wait_idle().await;
        });
        yo_core::timing::note(&format!("quic: connected {:?}", started.elapsed()));
        *self.connection.lock().unwrap_or_else(|e| e.into_inner()) = Some((connection, sender.clone()));
        Ok(sender)
    }

    /// One UDP socket per line, bound once for its family.
    fn endpoint(&self, state: &Runtime, to: SocketAddr) -> Result<quinn::Endpoint, String> {
        let mut held = state.endpoint.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(endpoint) = held.as_ref().filter(|e| e.local_addr().is_ok_and(|a| a.is_ipv6() == to.is_ipv6())) {
            return Ok(endpoint.clone());
        }
        let local: SocketAddr = if to.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }.parse().map_err(|_| "quic: bind")?;
        let endpoint = quinn::Endpoint::client(local).map_err(|e| format!("quic: {e}"))?;
        *held = Some(endpoint.clone());
        Ok(endpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tls() -> rustls::ClientConfig {
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth()
    }

    #[test]
    fn only_an_https_hub_gets_http3_without_0rtt() {
        assert!(Quic::new("http://127.0.0.1:8080", tls()).is_none());
        let quic = Quic::new("https://hub.example:8443", tls()).unwrap();
        assert_eq!((quic.host.as_str(), quic.port), ("hub.example", 8443));
        assert_eq!(quic.tls.alpn_protocols, [b"h3".to_vec()]);
        assert!(!quic.tls.enable_early_data);
    }

    #[test]
    fn no_quic_answer_turns_http3_off_with_nothing_sent() {
        // Nobody listens on this UDP port: the handshake gets no answer.
        let port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let quic = Quic::new(&format!("https://127.0.0.1:{port}"), tls()).unwrap();
        let started = std::time::Instant::now();
        let result = quic.request("GET", "/", &[], &[], Duration::from_secs(5), 1024);
        assert!(matches!(result, Err(Failed::Unreachable(_))));
        assert!(started.elapsed() < HANDSHAKE + Duration::from_secs(1));
        assert!(OFF.load(Ordering::Relaxed));
        assert!(matches!(
            quic.request("GET", "/", &[], &[], Duration::from_secs(5), 1024),
            Err(Failed::Unreachable(_))
        ));
    }
}
