//! The harness: a hub in-process on a real socket, clients over real HTTP.

pub mod backup;

use std::sync::{Arc, Mutex, PoisonError, RwLock};
use yo_core::{Client, Push, Response, Transport};
use yo_hub::config::Config;
use yo_hub::{push, Hub};

/// Pins the calling thread to these CPUs (Linux).
fn set_affinity(cpus: &[usize]) {
    // SAFETY: a zeroed cpu_set_t is a valid empty set; CPU_SET writes inside it; sched_setaffinity reads it.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for &cpu in cpus {
            libc::CPU_SET(cpu, &mut set);
        }
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}

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

    /// `YO_HUB_CPU=n` (a measurement): every thread of the hub (its runtime and its database writer) runs on CPU n
    /// alone, and the calling thread (the load) on all the others, so the hub's throughput on one core shows.
    pub fn start_with(config: Config, addr: &str) -> Self {
        let pinned = std::env::var("YO_HUB_CPU").ok().and_then(|cpu| cpu.parse::<usize>().ok());
        if let Some(cpu) = pinned {
            set_affinity(&[cpu]); // threads made from here on inherit it
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(if pinned.is_some() { 1 } else { 4 })
            .enable_all()
            .build()
            .unwrap();
        let push = Arc::new(push::Recorder::default());
        let hub = Hub::new(config, push.clone()).unwrap();
        let listener = runtime.block_on(tokio::net::TcpListener::bind(addr)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let serving = hub.clone();
        runtime.spawn(async move {
            serving
                .serve(listener, async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        if let Some(cpu) = pinned {
            let all = std::thread::available_parallelism().map_or(1, |n| n.get());
            let others: Vec<usize> = (0..all).filter(|c| *c != cpu).collect();
            set_affinity(&others);
        }
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

/// A fault the transport injects once, for the first request whose path contains the pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The request reaches the hub, its answer is lost on the way back.
    LoseAnswer,
    /// The hub is not asked; this status comes back.
    Answer(u16),
}

/// A signed request as it was sent: method, path, headers, body.
pub type Recorded = (String, String, Vec<(String, String)>, Vec<u8>);

/// The platform's HTTP, in Rust. The base URL can be moved, to follow a restarted hub.
pub struct Http {
    client: reqwest::blocking::Client,
    pub base: RwLock<String>,
    /// Every signed request, for replay tests.
    pub recorded: Mutex<Vec<Recorded>>,
    pub faults: Mutex<Vec<(String, Fault)>>,
    /// Every request: method, path, body. Seeds for the fuzzers.
    pub sent: Mutex<Vec<(String, String, Vec<u8>)>>,
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
            faults: Mutex::default(),
            sent: Mutex::default(),
        })
    }

    /// Injects `fault` into the next request whose path contains `pattern`.
    pub fn fault(&self, pattern: &str, fault: Fault) {
        self.faults.lock().unwrap().push((pattern.to_owned(), fault));
    }
}

impl Transport for Http {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        if !headers.is_empty() {
            self.recorded.lock().unwrap_or_else(PoisonError::into_inner).push((
                method.into(),
                path.into(),
                headers.to_vec(),
                body.to_vec(),
            ));
        }
        self.sent.lock().unwrap_or_else(PoisonError::into_inner).push((method.into(), path.into(), body.to_vec()));
        let fault = {
            let mut faults = self.faults.lock().unwrap_or_else(PoisonError::into_inner);
            faults.iter().position(|(pattern, _)| path.contains(pattern.as_str())).map(|i| faults.remove(i).1)
        };
        if let Some(Fault::Answer(status)) = fault {
            return Ok(Response { status, body: Vec::new() });
        }
        let url = format!("{}{}", self.base.read().unwrap(), path);
        let mut request = self.client.request(method.parse().map_err(|_| "method")?, url).body(body.to_vec());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let body = response.bytes().map_err(|e| e.to_string())?.to_vec();
        if fault == Some(Fault::LoseAnswer) {
            return Err("answer lost (injected)".into());
        }
        Ok(Response { status, body })
    }
}

/// A unique test username: "u" and the number's digits, one skeleton per number.
pub fn username(i: u64) -> String {
    format!("u{i:09}")
}

/// Registers a new account with a free username.
pub fn register(transport: Arc<Http>, username: &str, name: Option<&str>) -> Client {
    let client = Client::register_username(transport, username, None, name.map(str::to_owned), Push::none()).unwrap();
    USERS.lock().unwrap_or_else(PoisonError::into_inner).insert(username.to_owned(), client.user_id().to_owned());
    client
}

/// A new phone of an account: the username registered again with the account key from the backup.
pub fn new_phone(transport: Arc<Http>, username: &str, account: &Client) -> Client {
    Client::register_username(transport, username, Some(account.account_secret()), None, Push::none()).unwrap()
}

static USERS: Mutex<std::collections::BTreeMap<String, String>> = Mutex::new(std::collections::BTreeMap::new());

/// The user id a username registered with (the same after a new phone).
pub fn user_of(username: &str) -> String {
    USERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(username)
        .cloned()
        .unwrap_or_else(|| panic!("{username} not registered"))
}

pub fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).unwrap();
    u64::from_le_bytes(bytes)
}

/// A small deterministic generator for seeded scenarios (xorshift64*).
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// A user's id from its hex form.
pub fn user_id(user: &str) -> yo_wire::UserId {
    (0..16).map(|i| u8::from_str_radix(&user[2 * i..2 * i + 2], 16).unwrap()).collect::<Vec<_>>().try_into().unwrap()
}

/// The device check for a user, as any stranger can learn it: from the hub's 409 to a delivery with a wrong one.
/// `None` for a user the hub does not know.
pub fn devices_hash(url: &str, user: &str) -> Option<[u8; 32]> {
    let probe = reqwest::blocking::Client::new()
        .post(format!("{url}{}", yo_wire::api::deliver_path(&user_id(user))))
        .header(yo_wire::api::HEADER_DEVICES, yo_wire::b64(&[0; 32]))
        .body(yo_wire::envelope::seal(b"probe").unwrap())
        .send()
        .unwrap();
    if probe.status().as_u16() != 409 {
        return None;
    }
    let devices: yo_wire::api::Devices = yo_wire::decode(&probe.bytes().unwrap()).unwrap();
    Some(yo_wire::devices::hash(&devices.account_key, &devices.device_keys))
}

/// Delivers raw bytes to a user as any stranger could (sealed sender), with the device check learned from the hub.
/// Returns the status.
pub fn deliver_raw(url: &str, user: &str, body: Vec<u8>) -> u16 {
    let Some(hash) = devices_hash(url, user) else { return 404 };
    reqwest::blocking::Client::new()
        .post(format!("{url}{}", yo_wire::api::deliver_path(&user_id(user))))
        .header(yo_wire::api::HEADER_DEVICES, yo_wire::b64(&hash))
        .body(body)
        .send()
        .unwrap()
        .status()
        .as_u16()
}

/// A hub that lies to one client (spec 3.4), once told to: asked for `user`, it answers with the keys and devices of
/// another account, the stand-in, under the user's id and name, and hands what is sent to the user to the stand-in.
/// What a hub that gives out a key not the user's would do; the client must notice.
pub struct Lying {
    pub http: Arc<Http>,
    lie: Mutex<Option<Lie>>,
}

struct Lie {
    user: yo_wire::UserId,
    name: String,
    stand_in: yo_wire::UserId,
    stand_in_name: String,
    /// Another account, whose lookups of the stand-in become the lies.
    asker: Client,
}

impl Lying {
    pub fn new(base: String) -> Arc<Self> {
        Arc::new(Lying { http: Http::new(base), lie: Mutex::default() })
    }

    /// From now on, `user` (known by `name`) is `stand_in` (known by `stand_in_name`, as long as `name`).
    pub fn lie(&self, user: &Client, name: &str, stand_in: &Client, stand_in_name: &str, asker: Client) {
        assert_eq!(name.len(), stand_in_name.len(), "a lie of the same length keeps the encoding");
        *self.lie.lock().unwrap() = Some(Lie {
            user: user_id(user.user_id()),
            name: name.to_owned(),
            stand_in: user_id(stand_in.user_id()),
            stand_in_name: stand_in_name.to_owned(),
            asker,
        });
    }
}

/// `body` with every `from` replaced by `to`, of the same length.
fn replaced(body: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = body.to_vec();
    let mut i = 0;
    while i + from.len() <= out.len() {
        if out[i..i + from.len()] == *from {
            out[i..i + from.len()].copy_from_slice(to);
            i += from.len();
        } else {
            i += 1;
        }
    }
    out
}

impl Transport for Lying {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        let mut lie = self.lie.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(lie) = lie.as_mut() else { return self.http.request(method, path, headers, body) };
        let lookup =
            (path == yo_wire::api::LOOKUP).then(|| yo_wire::decode::<yo_wire::api::Lookup>(body).ok()).flatten();
        let response = match lookup {
            Some(mut lookup)
                if lookup.handle.user_id() == Some(lie.user)
                    || (lookup.handle.kind == yo_wire::handle::KIND_USERNAME
                        && yo_wire::handle::same_name(&lookup.handle.display(), &lie.name)) =>
            {
                lookup.handle = yo_wire::handle::Handle::user(&lie.stand_in);
                let body = yo_wire::encode(&lookup);
                lie.asker.signed_for_tests("POST", yo_wire::api::LOOKUP, &body).map_err(|e| e.to_string())?
            }
            _ if path == yo_wire::api::deliver_path(&lie.user) => {
                self.http.request(method, &yo_wire::api::deliver_path(&lie.stand_in), headers, body)?
            }
            _ => self.http.request(method, path, headers, body)?,
        };
        let body = replaced(&response.body, &lie.stand_in, &lie.user);
        let body = replaced(&body, lie.stand_in_name.as_bytes(), lie.name.as_bytes());
        Ok(Response { status: response.status, body })
    }
}
