//! The harness: a hub in-process on a real socket, clients over real HTTP.

pub mod backup;

use std::sync::{Arc, Mutex, PoisonError, RwLock};
use yo_core::{Client, Push, Response, Transport};
use yo_hub::config::Config;
use yo_hub::{push, Hub};

/// Pins the calling thread to these CPUs (Linux; elsewhere it stays as it is).
#[cfg(not(target_os = "linux"))]
pub fn set_affinity(_cpus: &[usize]) {}

#[cfg(not(target_os = "linux"))]
pub fn affinity() -> Vec<usize> {
    (0..std::thread::available_parallelism().map_or(1, |n| n.get())).collect()
}

/// Read before pinning: available_parallelism after pinning sees only the hub's CPU.
#[cfg(target_os = "linux")]
pub fn affinity() -> Vec<usize> {
    // SAFETY: sched_getaffinity initializes the set; CPU_ISSET reads indices within CPU_SETSIZE.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        assert_eq!(libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set), 0);
        (0..libc::CPU_SETSIZE as usize).filter(|&cpu| libc::CPU_ISSET(cpu, &set)).collect()
    }
}

/// Pins the calling thread to these CPUs (Linux).
#[cfg(target_os = "linux")]
pub fn set_affinity(cpus: &[usize]) {
    // SAFETY: a zeroed cpu_set_t is a valid empty set; CPU_SET writes inside it; sched_setaffinity reads it.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for &cpu in cpus {
            libc::CPU_SET(cpu, &mut set);
        }
        assert_eq!(libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set), 0);
    }
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn load_affinity_can_be_restored_after_pinning_the_hub() {
    std::thread::spawn(|| {
        let all = affinity();
        if all.len() < 2 {
            return;
        }
        let cpu = all[0];
        let others: Vec<_> = all.into_iter().filter(|c| *c != cpu).collect();
        set_affinity(&[cpu]);
        assert_eq!(affinity(), [cpu]);
        set_affinity(&others);
        assert_eq!(affinity(), others);
    })
    .join()
    .unwrap();
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
        let others = pinned.map(|cpu| {
            let all = affinity();
            assert!(all.contains(&cpu), "YO_HUB_CPU must be in the allowed CPU set");
            let others: Vec<_> = all.into_iter().filter(|c| *c != cpu).collect();
            assert!(!others.is_empty(), "YO_HUB_CPU needs another CPU for the load");
            others
        });
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
        if let Some(others) = others {
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
    /// The hub is not asked; this status and body come back (a `429` naming its limit and seconds).
    Body(u16, &'static [u8]),
}

/// A signed request as it was sent: method, path, headers, body.
pub type Recorded = (String, String, Vec<(String, String)>, Vec<u8>);

/// The platform's HTTP, in Rust. The base URL can be moved, to follow a restarted hub.
pub struct Http {
    client: reqwest::blocking::Client,
    pub base: RwLock<String>,
    record: bool,
    /// Every signed request, for replay tests.
    pub recorded: Mutex<Vec<Recorded>>,
    pub faults: Mutex<Vec<(String, Fault)>>,
    /// Every request: method, path, body. Seeds for the fuzzers.
    pub sent: Mutex<Vec<(String, String, Vec<u8>)>>,
}

impl Http {
    pub fn new(base: String) -> Arc<Self> {
        Self::with_recording(base, true)
    }

    /// The same transport without request history, for sustained traffic.
    pub fn unrecorded(base: String) -> Arc<Self> {
        Self::with_recording(base, false)
    }

    fn with_recording(base: String, record: bool) -> Arc<Self> {
        Arc::new(Http {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(90))
                .pool_max_idle_per_host(4)
                .build()
                .unwrap(),
            base: RwLock::new(base),
            record,
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
        if self.record && !headers.is_empty() {
            self.recorded.lock().unwrap_or_else(PoisonError::into_inner).push((
                method.into(),
                path.into(),
                headers.to_vec(),
                body.to_vec(),
            ));
        }
        if self.record {
            // A delivery of one copy per device is kept as its envelopes, each as a body of its own (the
            // single-envelope body the hub still takes), so tests look at and post again what one device got.
            let copies = headers.iter().any(|(name, _)| name.eq_ignore_ascii_case(yoip::net::api::HEADER_COPIES));
            let bodies = match copies.then(|| yoip::decode::<yoip::net::api::Copies>(body).ok()).flatten() {
                Some(c) => c.copies.into_iter().map(|c| c.envelope.as_slice().to_vec()).collect(),
                None => vec![body.to_vec()],
            };
            let mut sent = self.sent.lock().unwrap_or_else(PoisonError::into_inner);
            sent.extend(bodies.into_iter().map(|b| (method.to_owned(), path.to_owned(), b)));
        }
        let fault = {
            let mut faults = self.faults.lock().unwrap_or_else(PoisonError::into_inner);
            faults.iter().position(|(pattern, _)| path.contains(pattern.as_str())).map(|i| faults.remove(i).1)
        };
        match fault {
            Some(Fault::Answer(status)) => return Ok(Response { status, body: Vec::new() }),
            Some(Fault::Body(status, body)) => return Ok(Response { status, body: body.to_vec() }),
            _ => {}
        }
        let url = format!("{}{}", self.base.read().unwrap(), path);
        let mut request = self.client.request(method.parse().map_err(|_| "method")?, url).body(body.to_vec());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let limited = status == 429;
        let body = response.bytes().map_err(|e| e.to_string())?.to_vec();
        if limited && std::env::var_os("YO_SEND_LIMITS").is_some() {
            eprintln!("429 {method} {path}: {}", String::from_utf8_lossy(&body));
        }
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

/// The account key of an account secret (any 32 bytes are one): its libsignal identity key's 32 bytes (spec 2).
pub fn account_key(secret: &[u8; 32]) -> [u8; 32] {
    yo_core::identity::account_key(&yo_core::identity::account_pair(secret).unwrap())
}

/// The public key of the test hubs' signing key (`Config::for_tests`), as an app ships its hub's (spec 4.1).
pub fn hub_key() -> [u8; 32] {
    yo_hub::config::hub_public_key(&yo_hub::config::TEST_HUB_KEY)
}

/// The public key of the test hubs' trust root (`yoip::protocol::envelope::TEST_TRUST_ROOT`), as an app pins its hub's.
pub fn trust_root() -> [u8; 32] {
    yo_hub::certificate::Signer::from_bytes(&TEST_SIGNER).unwrap().root()
}

/// The test hubs' zkgroup public params (spec 3.4), as an app pins them.
/// The test hubs' OHTTP key config (`Config::for_tests`), as apps pin it.
pub fn ohttp_config() -> Vec<u8> {
    yo_hub::ohttp::key_config(yo_hub::config::TEST_OHTTP_KEY_ID, &yo_hub::config::TEST_OHTTP_KEY).unwrap()
}

pub fn zk_params() -> Vec<u8> {
    yo_hub::groups::Zk::from_bytes(&yo_hub::groups::test_params()).expect("params").public().to_vec()
}

static TEST_SIGNER: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(yo_hub::certificate::test_file);

/// A sender certificate as a test hub issues it, for a device of a user, good for a day from `now_ms`.
pub fn sender_certificate(user: &[u8; 16], device: u8, account_key: &[u8; 32], now_ms: u64) -> Vec<u8> {
    let signer = yo_hub::certificate::Signer::from_bytes(&TEST_SIGNER).unwrap();
    signer.issue(user, device, account_key, now_ms).unwrap()
}

/// Registers a new account with a free username; answers handing out keys checked against the test hub's key.
pub fn register(transport: Arc<Http>, username: &str, name: Option<&str>) -> Client {
    let mut client =
        Client::register_username(transport, username, None, name.map(str::to_owned), Push::none()).unwrap();
    client.set_hub_keys(vec![hub_key()]);
    USERS.lock().unwrap_or_else(PoisonError::into_inner).insert(username.to_owned(), client.user_id().to_owned());
    client
}

/// A new phone of an account: the username registered again with the account key from the backup.
pub fn new_phone(transport: Arc<Http>, username: &str, account: &Client) -> Client {
    let mut client =
        Client::register_username(transport, username, Some(account.account_secret()), None, Push::none()).unwrap();
    client.set_hub_keys(vec![hub_key()]);
    client
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
    getrandom::fill(&mut bytes).unwrap();
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
pub fn user_id(user: &str) -> yoip::UserId {
    (0..16).map(|i| u8::from_str_radix(&user[2 * i..2 * i + 2], 16).unwrap()).collect::<Vec<_>>().try_into().unwrap()
}

/// A well-formed envelope as a stranger can make one: a bucket, sealed to nobody (spec 4.1).
pub fn junk(fill: u8) -> Vec<u8> {
    let mut envelope = vec![fill; yoip::protocol::envelope::BUCKETS[0]];
    envelope[0] = yoip::protocol::envelope::SEALED;
    envelope
}

/// The device check for a user, as any stranger can learn it: from the hub's 409 to a delivery with a wrong one.
/// `None` for a user the hub does not know, or one that takes no delivery without its profile key (spec 3.3).
pub fn devices_hash(url: &str, user: &str) -> Option<[u8; 32]> {
    devices_hash_with(url, user, None)
}

/// `devices_hash` showing a profile key (`Yo-Access`, as sent).
pub fn devices_hash_with(url: &str, user: &str, access: Option<&str>) -> Option<[u8; 32]> {
    let mut probe = reqwest::blocking::Client::new()
        .post(format!("{url}{}", yoip::net::api::deliver_path(&user_id(user))))
        .header(yoip::net::api::HEADER_DEVICES, yoip::b64(&[0; 32]));
    if let Some(access) = access {
        probe = probe.header(yoip::net::api::HEADER_ACCESS, access);
    }
    let probe = probe.body(junk(0)).send().unwrap();
    if probe.status().as_u16() != 409 {
        return None;
    }
    let devices: yoip::net::api::Devices = yoip::decode(&probe.bytes().unwrap()).unwrap();
    Some(yoip::net::devices::hash(&devices.account_key, &devices.keys()))
}

/// Delivers raw bytes to a user as any stranger could (sealed sender), with the device check learned from the hub.
/// Returns the status.
pub fn deliver_raw(url: &str, user: &str, body: Vec<u8>) -> u16 {
    deliver_raw_with(url, user, None, body)
}

/// `deliver_raw` as a contact: showing the recipient's profile key as a contact's delivery did (`access`).
pub fn deliver_raw_with(url: &str, user: &str, access: Option<&str>, body: Vec<u8>) -> u16 {
    let Some(hash) = devices_hash_with(url, user, access) else { return 404 };
    let mut request = reqwest::blocking::Client::new()
        .post(format!("{url}{}", yoip::net::api::deliver_path(&user_id(user))))
        .header(yoip::net::api::HEADER_DEVICES, yoip::b64(&hash));
    if let Some(access) = access {
        request = request.header(yoip::net::api::HEADER_ACCESS, access);
    }
    request.body(body).send().unwrap().status().as_u16()
}

/// The last delivery `http`'s client posted to `path`, posted again as it was (its headers too: a request's signature,
/// a contact's key): one a fault held on its way. Returns the status.
pub fn deliver_held(url: &str, http: &Http, path: &str) -> u16 {
    let (_, _, headers, body) =
        http.recorded.lock().unwrap().iter().rev().find(|(m, p, _, _)| m == "POST" && p == path).cloned().unwrap();
    let mut request = reqwest::blocking::Client::new().post(format!("{url}{path}"));
    for (name, value) in headers {
        request = request.header(name, value);
    }
    request.body(body).send().unwrap().status().as_u16()
}

/// The profile key `http`'s client showed in its last delivery to `to` (`Yo-Access`), if it did.
pub fn access_shown(http: &Http, to: &str) -> Option<String> {
    let path = yoip::net::api::deliver_path(&user_id(to));
    let recorded = http.recorded.lock().unwrap();
    recorded.iter().rev().filter(|(_, p, _, _)| *p == path).find_map(|(_, _, headers, _)| {
        headers.iter().find(|(n, _)| n == yoip::net::api::HEADER_ACCESS).map(|(_, v)| v.clone())
    })
}

/// A hub that lies to one client (spec 1.2), once told to: asked for `user`, it answers with the keys and devices of
/// another account, the stand-in, under the user's id and name, and hands what is sent to the user to the stand-in.
/// What a hub that gives out a key not the user's would do; the client must notice.
pub struct Lying {
    pub http: Arc<Http>,
    lie: Mutex<Option<Lie>>,
}

struct Lie {
    user: yoip::UserId,
    name: String,
    stand_in: yoip::UserId,
    stand_in_name: String,
    /// The stand-in's profile key, if it made one: the hub takes what goes to it as a contact's.
    stand_in_key: Option<String>,
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
            stand_in_key: stand_in.access_key_for_tests(),
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
            (path == yoip::net::api::LOOKUP).then(|| yoip::decode::<yoip::net::api::Lookup>(body).ok()).flatten();
        let response = match lookup {
            Some(mut lookup)
                if lookup.handle.user_id() == Some(lie.user)
                    || (lookup.handle.kind == yoip::net::handle::KIND_USERNAME
                        && yoip::net::handle::same_name(&lookup.handle.display(), &lie.name)) =>
            {
                lookup.handle = yoip::net::handle::Handle::user(&lie.stand_in);
                let body = yoip::encode(&lookup);
                lie.asker.signed_for_tests("POST", yoip::net::api::LOOKUP, &body).map_err(|e| e.to_string())?
            }
            // The hub itself hands it on: a request's signature (over the user's path) and a key are no check of it.
            _ if path == yoip::net::api::deliver_path(&lie.user) => {
                let mut headers: Vec<_> = headers
                    .iter()
                    .filter(|(n, _)| {
                        ![
                            yoip::net::request::HEADER_DEVICE,
                            yoip::net::request::HEADER_TIME,
                            yoip::net::request::HEADER_SIGNATURE,
                            yoip::net::api::HEADER_ACCESS,
                        ]
                        .contains(&n.as_str())
                    })
                    .cloned()
                    .collect();
                if let Some(key) = &lie.stand_in_key {
                    headers.push((yoip::net::api::HEADER_ACCESS.to_owned(), key.clone()));
                }
                self.http.request(method, &yoip::net::api::deliver_path(&lie.stand_in), &headers, body)?
            }
            _ => self.http.request(method, path, headers, body)?,
        };
        let body = replaced(&response.body, &lie.stand_in, &lie.user);
        let body = replaced(&body, lie.stand_in_name.as_bytes(), lie.name.as_bytes());
        Ok(Response { status: response.status, body })
    }
}

/// The state shared by processes (the iPhone's app and its extensions in the App Group): one database in a folder of
/// its own (gone when dropped), and the turn between the processes for writing.
pub struct Disk {
    dir: std::path::PathBuf,
    held: Mutex<bool>,
    free: std::sync::Condvar,
    /// Full: every write fails (`Alone` gives the turn back before it).
    pub full: std::sync::atomic::AtomicBool,
    /// What the files held when the process died (`freeze`): a restart opens that.
    frozen: Mutex<Option<Files>>,
}

/// The database's files as they were at one moment.
pub type Files = Vec<(std::path::PathBuf, Vec<u8>)>;

struct StateDiagnostics;

impl yo::StateDiagnostics for StateDiagnostics {
    fn failure(&self, kind: String) {
        eprintln!("StateDb {kind}");
    }
}

impl Disk {
    pub fn new() -> Arc<Disk> {
        let dir = std::env::temp_dir().join(format!("yo-shared-{}", random_u64()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(Disk {
            dir,
            held: Mutex::new(false),
            free: std::sync::Condvar::new(),
            full: Default::default(),
            frozen: Mutex::default(),
        })
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub fn path(&self) -> String {
        self.dir.join("state.sqlite").to_string_lossy().into_owned()
    }

    /// A process's own opening of the database.
    pub fn db(&self) -> Arc<yo::StateDb> {
        yo::StateDb::open_fresh(self.path(), vec![7; 32], Arc::new(StateDiagnostics)).unwrap()
    }

    /// The word of the last write (none: no state).
    pub fn word(&self) -> Option<String> {
        yo_core::statedb::StateDb::open_fresh(self.path(), [7; 32], Arc::new(|_| {})).unwrap().word().unwrap()
    }

    /// The files as they are now.
    pub fn snapshot(&self) -> Files {
        ["", "-wal"]
            .iter()
            .map(|end| std::path::PathBuf::from(format!("{}{end}", self.path())))
            .filter_map(|path| std::fs::read(&path).ok().map(|bytes| (path, bytes)))
            .collect()
    }

    /// The files as they were at `snapshot`.
    pub fn put_back(&self, files: Files) {
        for end in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{end}", self.path()));
        }
        for (path, bytes) in files {
            std::fs::write(path, bytes).unwrap();
        }
    }

    /// The process dies now: what it writes from here on is lost at its restart (`thaw`).
    pub fn freeze(&self) {
        let mut frozen = self.frozen.lock().unwrap();
        if frozen.is_none() {
            *frozen = Some(self.snapshot());
        }
    }

    /// Before a restart: the files as the process left them when it died.
    pub fn thaw(&self) {
        if let Some(files) = self.frozen.lock().unwrap().take() {
            self.put_back(files);
        }
    }

    /// Whether a process has the turn.
    pub fn is_held(&self) -> bool {
        *self.held.lock().unwrap()
    }
}

impl Drop for Disk {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// One process's turn on a `Disk`, as the app implements `StateLock`.
pub struct Process {
    disk: Arc<Disk>,
}

impl Process {
    pub fn new(disk: &Arc<Disk>) -> Arc<Self> {
        Arc::new(Process { disk: disk.clone() })
    }

    pub fn hold(&self) {
        let mut held = self.disk.held.lock().unwrap();
        while *held {
            held = self.disk.free.wait(held).unwrap();
        }
        *held = true;
    }

    pub fn release(&self) {
        *self.disk.held.lock().unwrap() = false;
        self.disk.free.notify_all();
    }
}

impl yo::StateLock for Process {
    fn lock(&self) -> std::result::Result<(), yo::YoError> {
        self.hold();
        Ok(())
    }

    fn held(&self) -> bool {
        assert!(self.disk.is_held(), "written only in a turn");
        true
    }

    fn unlock(&self, _saved: bool) {
        self.release();
    }
}

/// The turn of the one process on a `Disk`: always its own, given back before each write while the disk is full.
pub struct Alone(pub Arc<Disk>);

impl yo::StateLock for Alone {
    fn lock(&self) -> std::result::Result<(), yo::YoError> {
        Ok(())
    }

    fn held(&self) -> bool {
        !self.0.full.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn unlock(&self, _saved: bool) {}
}

/// A Yo told apart from others by a short tag: a photo whose key and hash carry it (no blob is ever fetched). The id
/// is new each time, so the same tag twice is two messages.
pub fn tagged(tag: String) -> yo_core::Content {
    assert!(tag.len() <= 64, "a tag of at most 64 bytes");
    let mut bytes = [0u8; 64];
    bytes[..tag.len()].copy_from_slice(tag.as_bytes());
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&random_u64().to_be_bytes());
    id[8..].copy_from_slice(TAG_MARK);
    let (key, sha256) = (bytes[..32].try_into().unwrap(), bytes[32..].try_into().unwrap());
    yo_core::Content::Photo(yo_core::Media { id, key, sha256, size: 1, width: 1, height: 1 })
}

const TAG_MARK: &[u8; 8] = b"yo-tag\0\0";

/// The tag of a `tagged` Yo.
pub fn tag_of(content: &yo_core::Content) -> Option<String> {
    let yo_core::Content::Photo(media) = content.clone() else { return None };
    if &media.id[8..] != TAG_MARK {
        return None;
    }
    let bytes = [&media.key[..], &media.sha256[..]].concat();
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8(bytes[..end].to_vec()).ok()
}

/// A saved state as bytes: what a crashed process left, or a parked account keeps on disk.
pub fn saved_bytes(state: &yo_core::state::State, records: yo_core::signal::Entries) -> Vec<u8> {
    serde_json::to_vec(&yo_core::Saved::of(state, &records)).expect("a state encodes")
}

/// The client a restart opens from `saved_bytes`.
pub fn restore_bytes(transport: Arc<dyn Transport>, bytes: &[u8]) -> yo_core::Result<Client> {
    let saved: yo_core::Saved = serde_json::from_slice(bytes).map_err(|_| yo_core::Error::State)?;
    Client::restore(transport, &saved)
}

/// `YoClient::register_username_db` on `disk`, the one process on it (`Alone`).
#[allow(clippy::too_many_arguments)]
pub fn register_on(
    transport: Arc<dyn yo::HttpTransport>,
    disk: Arc<Disk>,
    key: Vec<u8>,
    username: String,
    secret: Option<Vec<u8>>,
    legacy: Option<Vec<u8>>,
    name: Option<String>,
    platform: u8,
    token: String,
) -> std::result::Result<Arc<yo::YoClient>, yo::YoError> {
    let lock: Arc<dyn yo::StateLock> = Arc::new(Alone(disk.clone()));
    yo::YoClient::register_username_db(
        transport,
        disk.db(),
        key,
        Some(lock),
        username,
        secret,
        legacy,
        None,
        name,
        platform,
        token,
        None,
    )
}

/// The app started again on `disk`: from what the files held when it died (`freeze`), if it did.
pub fn reopen(
    transport: Arc<dyn yo::HttpTransport>,
    disk: Arc<Disk>,
    key: Vec<u8>,
) -> std::result::Result<Arc<yo::YoClient>, yo::YoError> {
    disk.thaw();
    let lock: Arc<dyn yo::StateLock> = Arc::new(Alone(disk.clone()));
    yo::YoClient::open_db(transport, disk.db(), key, false, Some(lock))
}

/// The app layer's sends and acknowledgement under the short names the tests use.
pub trait AppClient {
    fn send_yo(&self, to: impl Into<String>) -> std::result::Result<Vec<yo::YoEvent>, yo::YoError>;
    fn send_photo(
        &self,
        to: impl Into<String>,
        media: yo::YoMedia,
    ) -> std::result::Result<Vec<yo::YoEvent>, yo::YoError>;
    fn send_group_yo(&self, group: impl Into<String>) -> std::result::Result<Vec<yo::YoEvent>, yo::YoError>;
    /// Every event handed out so far is shown: none comes again after a crash.
    fn acknowledge_events(&self);
    /// The rest of the last `sync` now rather than with the next one (`Client::after_sync`).
    fn after_sync(&self) -> std::result::Result<(), yo::YoError>;
}

impl AppClient for yo::YoClient {
    fn send_yo(&self, to: impl Into<String>) -> std::result::Result<Vec<yo::YoEvent>, yo::YoError> {
        self.send(yo::SendTarget::Contact { id: to.into() }, yo::MessageKind::Yo)
    }

    fn send_photo(
        &self,
        to: impl Into<String>,
        media: yo::YoMedia,
    ) -> std::result::Result<Vec<yo::YoEvent>, yo::YoError> {
        self.send(yo::SendTarget::Contact { id: to.into() }, yo::MessageKind::Photo { media })
    }

    fn send_group_yo(&self, group: impl Into<String>) -> std::result::Result<Vec<yo::YoEvent>, yo::YoError> {
        self.send(yo::SendTarget::Group { id: group.into() }, yo::MessageKind::Yo)
    }

    fn acknowledge_events(&self) {
        self.core_for_tests(|c| c.acknowledge_events());
    }

    fn after_sync(&self) -> std::result::Result<(), yo::YoError> {
        Ok(self.core_for_tests(|c| c.after_sync())?)
    }
}
