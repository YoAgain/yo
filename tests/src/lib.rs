//! The harness: a hub in-process on a real socket, clients over real HTTP.

use std::sync::{Arc, Mutex, PoisonError, RwLock};
use yo_core::{Client, Push, Response, Transport};
use yo_hub::config::Config;
use yo_hub::{push, Hub};

pub struct TestHub {
    pub hub: Arc<Hub>,
    pub push: Arc<push::Recorder>,
    pub addr: std::net::SocketAddr,
    runtime: Option<tokio::runtime::Runtime>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl TestHub {
    pub fn start() -> Self {
        Self::start_with(Config::for_tests(), "127.0.0.1:0")
    }

    pub fn start_with(config: Config, addr: &str) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
        let push = Arc::new(push::Recorder::default());
        let hub = Hub::new(config, push.clone()).unwrap();
        let listener = runtime.block_on(tokio::net::TcpListener::bind(addr)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let serving = hub.clone();
        runtime.spawn(async move {
            serving.serve(listener, async { let _ = stopped.await; }).await.unwrap();
        });
        TestHub { hub, push, addr, runtime: Some(runtime), stop: Some(stop) }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Stops serving and closes the database.
    pub fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(std::time::Duration::from_secs(5));
        }
    }
}

impl Drop for TestHub {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// The platform's HTTP, in Rust. The base URL can be moved, to follow a restarted hub.
pub struct Http {
    client: reqwest::blocking::Client,
    pub base: RwLock<String>,
    /// Every signed request, for replay tests.
    pub recorded: Mutex<Vec<(String, String, Vec<(String, String)>, Vec<u8>)>>,
}

impl Http {
    pub fn new(base: String) -> Arc<Self> {
        Arc::new(Http {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(90))
                .pool_max_idle_per_host(4)
                .build()
                .unwrap(),
            base: RwLock::new(base),
            recorded: Mutex::default(),
        })
    }
}

impl Transport for Http {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        if !headers.is_empty() {
            self.recorded
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((method.into(), path.into(), headers.to_vec(), body.to_vec()));
        }
        let url = format!("{}{}", self.base.read().unwrap(), path);
        let mut request = self.client.request(method.parse().map_err(|_| "method")?, url).body(body.to_vec());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let body = response.bytes().map_err(|e| e.to_string())?.to_vec();
        Ok(Response { status, body })
    }
}

/// A unique test phone number.
pub fn phone(i: u64) -> String {
    format!("+4915{:09}", i)
}

pub fn register(transport: Arc<Http>, phone: &str, name: Option<&str>) -> Client {
    Client::request_code(transport.as_ref(), phone).unwrap();
    Client::register(transport, phone, "000000", name.map(str::to_owned), Push::none()).unwrap()
}

pub fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).unwrap();
    u64::from_le_bytes(bytes)
}

/// A small deterministic generator for seeded scenarios (xorshift64*).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}
