//! yo-scale: ten thousand accounts and millions of messages through the real core and a hub on this machine, with
//! the hub killed (SIGKILL) mid-write and every fault a phone meets; every message accounted for.
//!
//!   yo-scale [--accounts 10000] [--messages 5000000] [--threads 16] [--seed n] [--faults 15] [--crashes 10]
//!            [--kills 10] [--media 6] [--hot 24] [--inbox 300] [--dir ~/.cache/yo-scale] [--db-dir /dev/shm/…]
//!            [--blob-quota-mb 128] [--blob-budget-mb 6144] [--trace-file f] [--out file.json]
//!
//! The hub is a child process of this one (the same binary, `--serve-hub`) on a SQLite file (`--db-dir`: elsewhere
//! than `--dir`, such as a tmpfs, where a kill loses what a process crash would and fsyncs cost nothing), killed with
//! SIGKILL `--kills` times during the run and started again on the same database, then opened in-process at the end
//! for its audit and a month jumped for the blobs' expiry. Requests carry a `CF-Connecting-IP` per phone (some phones
//! share one, as behind a carrier's NAT) and the hub believes it, so its limits by address hold as in production.
//!
//! Accounts form a small world: friends mostly among neighbours, some far, a few accounts everybody knows (their
//! one-time prekeys run out while they are away). Each account is owned by one worker thread; a thread keeps `--hot`
//! of its accounts running (a `Client` in memory, its sealed state saved after every change), and the others are on
//! disk as their sealed state, as an app the system has stopped: taken up again, each catches up on what waited.
//! Some sleep through most of the run (their inboxes fill: `507` for those who write to them, held in the outbox).
//!
//! Faults: requests dropped, answers lost, requests duplicated or delayed, deliveries arriving again much later (out
//! of order), `503`, `429`; uploads cut off halfway or stalled, fetches cut off, two fetches of one blob at once;
//! a process dying at a request with its saved state as it was then; a disk that is full for a while (saves fail);
//! a phone's clock off by minutes, or by hours for a while; new phones (the old device must get `410`); accounts
//! deleted and names taken again; blocks; a photo Yo whose blob never came; the blob quota and budget met.
//!
//! Groups of 2–20 under the same chaos: made, Yos and photos to them (one blob for every member's fetch), members
//! added and removed at once by different members, members leaving. Group Yos carry nothing to name them, so they are
//! counted per (group, sender, receiver): shown only to members as the sender had the group, never more often than
//! sent, never out of order from one device; and every one sent to a member no change, block, new phone or
//! deletion concerned arrived.
//!
//! Messages are texts that name themselves (a Yo with a tag), photos and videos of real sizes (0.2–6 MB, videos up to
//! the 10 MiB blob). The ledger checks every one received: from whom, to whom, once, in order; every one that must
//! arrive did; nothing from someone blocked. At the end: empty outboxes, a message each way between friends (no
//! session wedged), every saved state opens, the hub's database is consistent, its memory and disk stayed bounded.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::io::{Read as _, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};
use yo_core::{media, Backup, Client, Content, Error, Event, Push, Response, Store, Transport};
use yo_hub::config::{BlobConfig, Config, Limits};
use yo_tests::{Rng, TestHub};

const MIB: u64 = 1024 * 1024;
const STATE_KEY: [u8; 32] = [7; 32];

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- arguments ----

#[derive(Clone)]
struct Args {
    seed: u64,
    accounts: usize,
    messages: u64,
    threads: usize,
    /// Faults per thousand requests.
    faults: u64,
    /// Crashes per thousand operations.
    crashes: u64,
    kills: usize,
    /// Photos and videos per thousand operations.
    media: u64,
    hot: usize,
    inbox: u32,
    dir: String,
    out: Option<String>,
    /// The child: serve the hub on this address and nothing else.
    serve: Option<String>,
    /// Hours at most, whatever was sent by then.
    hours: f64,
    /// Lines of trace kept per user.
    trace: usize,
    trace_file: Option<String>,
    /// The hub's database elsewhere than `dir` (`/dev/shm`: no fsync wait; a kill loses what a crash of the process
    /// would, not what a power cut would).
    db_dir: Option<String>,
    /// Blob bytes waiting per user, and all blobs together, at most (MiB): small, to meet the quota and the budget.
    blob_quota_mb: u64,
    blob_budget_mb: u64,
    /// Memory the machine keeps free (available and swap, MiB): below it the chaos stops where it is.
    min_free_mb: u64,
}

impl Args {
    fn db(&self) -> String {
        format!("{}/hub.sqlite", self.db_dir.as_ref().unwrap_or(&self.dir))
    }
}

fn args() -> Args {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let mut args = Args {
        seed: yo_tests::random_u64() % 1_000_000,
        accounts: 10_000,
        messages: 5_000_000,
        threads: std::thread::available_parallelism().map_or(8, |n| n.get()),
        faults: 15,
        crashes: 10,
        kills: 10,
        media: 6,
        hot: 24,
        inbox: 300,
        dir: format!("{home}/.cache/yo-scale"),
        out: None,
        serve: None,
        hours: 12.0,
        trace: 40,
        trace_file: None,
        db_dir: None,
        blob_quota_mb: 128,
        blob_budget_mb: 6144,
        min_free_mb: 6144,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().unwrap_or_default();
        let number = || value.parse::<u64>().unwrap_or_else(|_| panic!("{flag}: a number"));
        match flag.as_str() {
            "--seed" => args.seed = number(),
            "--accounts" => args.accounts = number() as usize,
            "--messages" => args.messages = number(),
            "--threads" => args.threads = number().max(1) as usize,
            "--faults" => args.faults = number(),
            "--crashes" => args.crashes = number(),
            "--kills" => args.kills = number() as usize,
            "--media" => args.media = number(),
            "--hot" => args.hot = number().max(2) as usize,
            "--inbox" => args.inbox = number() as u32,
            "--dir" => args.dir = value,
            "--out" => args.out = Some(value),
            "--hours" => args.hours = value.parse().expect("--hours"),
            "--trace" => args.trace = number() as usize,
            "--trace-file" => args.trace_file = Some(value),
            "--db-dir" => args.db_dir = Some(value),
            "--blob-quota-mb" => args.blob_quota_mb = number(),
            "--blob-budget-mb" => args.blob_budget_mb = number(),
            "--min-free-mb" => args.min_free_mb = number(),
            "--serve-hub" => args.serve = Some(value),
            other => panic!("unknown flag {other}"),
        }
    }
    args
}

// ---- counting ----

#[derive(Default)]
struct Stats {
    counts: Mutex<HashMap<String, u64>>,
    violations: AtomicUsize,
}

impl Stats {
    fn count(&self, what: impl Into<String>) {
        *lock(&self.counts).entry(what.into()).or_default() += 1;
    }

    fn violation(&self, what: String) {
        let kind: Vec<&str> = what
            .split_whitespace()
            .take(10)
            .map(|w| if w.chars().any(|c| c.is_ascii_digit()) { "#" } else { w })
            .collect();
        self.count(format!("VIOLATION {}", kind.join(" ")));
        if self.violations.fetch_add(1, Relaxed) < 300 {
            println!("VIOLATION: {what}");
        }
    }
}

/// Latencies in microseconds, in buckets 4% apart: percentiles without keeping every value.
struct Hist(Vec<AtomicU64>);

impl Hist {
    fn new() -> Self {
        Hist((0..640).map(|_| AtomicU64::new(0)).collect())
    }

    fn add(&self, micros: u64) {
        let i = ((micros as f64 + 1.0).ln() / 1.04f64.ln()) as usize;
        self.0[i.min(self.0.len() - 1)].fetch_add(1, Relaxed);
    }

    fn total(&self) -> u64 {
        self.0.iter().map(|b| b.load(Relaxed)).sum()
    }

    fn percentile_ms(&self, p: f64) -> f64 {
        let total = self.total();
        if total == 0 {
            return 0.0;
        }
        let want = ((total as f64) * p).ceil().max(1.0) as u64;
        let mut seen = 0;
        for (i, b) in self.0.iter().enumerate() {
            seen += b.load(Relaxed);
            if seen >= want {
                return 1.04f64.powi(i as i32 + 1) / 1000.0;
            }
        }
        f64::INFINITY
    }

    fn json(&self) -> serde_json::Value {
        let r = |x: f64| (x * 100.0).round() / 100.0;
        serde_json::json!({
            "n": self.total(),
            "p50_ms": r(self.percentile_ms(0.50)),
            "p90_ms": r(self.percentile_ms(0.90)),
            "p99_ms": r(self.percentile_ms(0.99)),
            "p999_ms": r(self.percentile_ms(0.999)),
            "max_ms": r(self.percentile_ms(1.0)),
        })
    }
}

fn rss_mb(pid: Option<u32>) -> u64 {
    let path = pid.map_or("/proc/self/status".to_owned(), |p| format!("/proc/{p}/status"));
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).map(str::to_owned))
        .and_then(|l| l.split_whitespace().nth(1).and_then(|kb| kb.parse::<u64>().ok()))
        .map_or(0, |kb| kb / 1024)
}

/// What the machine has free: available memory and swap, MiB.
fn free_mb() -> u64 {
    let info = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let field = |name: &str| {
        info.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<u64>().ok())
            .unwrap_or(0)
    };
    (field("MemAvailable:") + field("SwapFree:")) / 1024
}

fn file_mb(path: &str) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len() / MIB)
}

fn dir_bytes(path: &std::path::Path) -> (u64, u64) {
    let (mut bytes, mut files) = (0, 0);
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                let (b, f) = dir_bytes(&entry.path());
                bytes += b;
                files += f;
            } else {
                bytes += meta.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

// ---- names for users ----

/// User ids and names as small numbers: the ledger holds millions of messages.
#[derive(Default)]
struct Interner {
    ids: HashMap<String, u32>,
    all: Vec<String>,
}

impl Interner {
    fn id(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.ids.get(s) {
            return id;
        }
        self.all.push(s.to_owned());
        let id = self.all.len() as u32; // 0 is nobody
        self.ids.insert(s.to_owned(), id);
        id
    }
}

// ---- the network, with faults ----

struct Net {
    http: reqwest::blocking::Client,
    base: String,
    addr: String,
}

impl Net {
    fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
        ip: &str,
    ) -> Result<Response, String> {
        let mut request = self
            .http
            .request(method.parse().map_err(|_| "method")?, format!("{}{path}", self.base))
            .header("cf-connecting-ip", ip)
            .body(body.to_vec());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        Ok(Response { status, body: response.bytes().map_err(|e| e.to_string())?.to_vec() })
    }

    /// A request whose body stops after `keep` bytes, the connection held `hold` and then closed: an upload cut off
    /// by a lost network.
    fn cut(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
        ip: &str,
        cut: (usize, Duration),
    ) {
        let (keep, hold) = cut;
        let Ok(mut s) = std::net::TcpStream::connect(&self.addr) else { return };
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nhost: {}\r\ncontent-length: {}\r\ncf-connecting-ip: {ip}\r\n",
            self.addr,
            body.len()
        );
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        let _ = s.write_all(head.as_bytes());
        let _ = s.write_all(&body[..keep.min(body.len())]);
        let _ = s.flush();
        if hold.is_zero() {
            return;
        }
        std::thread::spawn(move || {
            std::thread::sleep(hold);
            drop(s);
        });
    }

    /// A fetch that stops reading after the first bytes of the answer, then closes.
    fn cut_fetch(&self, path: &str, ip: &str) {
        let Ok(mut s) = std::net::TcpStream::connect(&self.addr) else { return };
        let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
        let head = format!("GET {path} HTTP/1.1\r\nhost: {}\r\ncf-connecting-ip: {ip}\r\n\r\n", self.addr);
        let _ = s.write_all(head.as_bytes());
        let mut buf = vec![0u8; 16 * 1024];
        let _ = s.read(&mut buf);
    }
}

type Log = Arc<Mutex<std::io::BufWriter<std::fs::File>>>;

/// A delivery held back to arrive again much later (after newer ones): (method, path, headers, body).
type Held = (String, String, Vec<(String, String)>, Vec<u8>);

/// One process of one phone: its network (with faults, from its address) and its saved state.
struct Proc {
    net: Arc<Net>,
    stats: Arc<Stats>,
    ip: String,
    faults: u64,
    rng: Mutex<Rng>,
    crash_in: AtomicU64,
    crashed: AtomicBool,
    disk_full: AtomicBool,
    saved: Mutex<Vec<u8>>,
    held: Mutex<VecDeque<Held>>,
    /// Where its requests are written down (`--trace-file`): the log, whose they are, since when.
    log: Mutex<Option<(Log, String, Instant)>>,
}

impl Proc {
    fn new(net: &Arc<Net>, stats: &Arc<Stats>, ip: &str, faults: u64, seed: u64, saved: Vec<u8>) -> Arc<Self> {
        Arc::new(Proc {
            net: net.clone(),
            stats: stats.clone(),
            ip: ip.to_owned(),
            faults,
            rng: Mutex::new(Rng(seed.max(1))),
            crash_in: AtomicU64::new(0),
            crashed: AtomicBool::new(false),
            disk_full: AtomicBool::new(false),
            saved: Mutex::new(saved),
            held: Mutex::default(),
            log: Mutex::default(),
        })
    }

    fn note(&self, what: std::fmt::Arguments<'_>) {
        if let Some((log, who, since)) = lock(&self.log).as_ref() {
            let _ = writeln!(lock(log), "{} {who} net {what}", since.elapsed().as_millis());
        }
    }

    fn roll(&self, n: u64) -> u64 {
        lock(&self.rng).below(n)
    }

    fn saved(&self) -> Vec<u8> {
        lock(&self.saved).clone()
    }
}

impl Transport for Proc {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        let result = self.request_inner(method, path, headers, body);
        if lock(&self.log).is_some()
            && !(method == "GET" && path.starts_with("/v1/inbox") && matches!(&result, Ok(r) if r.body.len() < 8))
        {
            let what = match &result {
                Ok(r) => r.status.to_string(),
                Err(e) => e.clone(),
            };
            self.note(format_args!("{method} {} {} → {what}", path.split('?').next().unwrap_or(path), body.len()));
        }
        result
    }
}

impl Proc {
    fn request_inner(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<Response, String> {
        if self.crashed.load(Relaxed) {
            return Err("process gone".into());
        }
        if self.crash_in.load(Relaxed) > 0 && self.crash_in.fetch_sub(1, Relaxed) == 1 {
            self.crashed.store(true, Relaxed);
            self.stats.count("fault: crash");
            if self.roll(2) == 0 {
                let _ = self.net.send(method, path, headers, body, &self.ip);
            }
            return Err("process gone".into());
        }
        // Now and then a delivery held back long ago arrives first: out of order, and again.
        if self.roll(3) == 0 {
            let late = lock(&self.held).pop_front();
            if let Some((m, p, h, b)) = late {
                self.stats.count("fault: late delivery arrives");
                self.note(format_args!("late {m} {p}"));
                let _ = self.net.send(&m, &p, &h, &b, &self.ip);
            }
        }
        let upload = method == "PUT" && path.starts_with(yo_wire::api::BLOBS);
        if upload && self.roll(1000) < 120 {
            // Cut off halfway; now and then the connection hangs on a while (the hub has it "under way").
            let keep = self.roll(body.len() as u64 + 1) as usize;
            let hold = if self.roll(3) == 0 { Duration::from_millis(500 + self.roll(4000)) } else { Duration::ZERO };
            self.stats.count(if hold.is_zero() { "fault: upload cut" } else { "fault: upload stalled" });
            self.net.cut(method, path, headers, body, &self.ip, (keep, hold));
            return Err("upload cut (injected)".into());
        }
        if self.roll(1000) >= self.faults {
            return self.net.send(method, path, headers, body, &self.ip);
        }
        let delivery = method == "POST" && path.starts_with("/v1/inbox/") && path != yo_wire::api::INBOX_ACK;
        let kind = self.roll(if delivery { 8 } else { 6 });
        self.note(format_args!("fault {kind} on {method} {path}"));
        self.stats.count(format!(
            "fault: {}",
            ["drop", "lose answer", "duplicate", "delay", "503", "429", "held back", "held back, sent too"]
                [kind as usize]
        ));
        let hold = || {
            let mut held = lock(&self.held);
            if held.len() < 16 {
                held.push_back((method.to_owned(), path.to_owned(), headers.to_vec(), body.to_vec()));
            }
        };
        match kind {
            0 => Err("dropped (injected)".into()),
            1 => {
                let _ = self.net.send(method, path, headers, body, &self.ip);
                Err("answer lost (injected)".into())
            }
            2 => {
                let first = self.net.send(method, path, headers, body, &self.ip);
                let _ = self.net.send(method, path, headers, body, &self.ip);
                first
            }
            3 => {
                std::thread::sleep(Duration::from_millis(self.roll(300)));
                self.net.send(method, path, headers, body, &self.ip)
            }
            4 => Ok(Response { status: 503, body: Vec::new() }),
            5 => Ok(Response { status: 429, body: Vec::new() }),
            6 => {
                hold();
                Err("held back (injected)".into())
            }
            _ => {
                hold();
                self.net.send(method, path, headers, body, &self.ip)
            }
        }
    }
}

impl Store for Proc {
    fn save(&self, blob: Vec<u8>) -> Result<(), String> {
        if self.crashed.load(Relaxed) {
            return Ok(());
        }
        if self.disk_full.load(Relaxed) {
            return Err("no space left on device (injected)".into());
        }
        *lock(&self.saved) = blob;
        Ok(())
    }
}

// ---- the ledger ----

#[derive(Clone, Copy)]
struct Msg {
    from: u32,
    to: u32,
    seq: u32,
    /// Ms since the run started.
    sent_ms: u32,
    /// Sent by this name (an interned name, 0: by user id).
    name: u32,
    got_by: u16,
    got: u8,
    must: bool,
    /// The receiver was running when it was sent: its latency is the system's, not the receiver's absence.
    hot: bool,
}

/// What a photo's tag is, its size, and whether its blob was ever uploaded.
#[derive(Clone, Copy)]
struct MediaInfo {
    key: u64,
    size: usize,
    uploaded: bool,
    from: u32,
}

#[derive(Default)]
struct Ledger {
    msgs: HashMap<u64, Msg>,
    /// Not yet received, by receiver and by sender.
    open_to: HashMap<u32, HashSet<u64>>,
    open_from: HashMap<u32, HashSet<u64>>,
    last: HashMap<(u32, u32), u32>,
    media: HashMap<[u8; 16], MediaInfo>,
    blocks: HashSet<(u32, u32)>,
}

impl Ledger {
    fn insert(&mut self, key: u64, msg: Msg) {
        self.open_to.entry(msg.to).or_default().insert(key);
        self.open_from.entry(msg.from).or_default().insert(key);
        self.msgs.insert(key, msg);
    }

    fn arrived(&mut self, key: u64) {
        if let Some(m) = self.msgs.get(&key) {
            let (to, from) = (m.to, m.from);
            if let Some(s) = self.open_to.get_mut(&to) {
                s.remove(&key);
            }
            if let Some(s) = self.open_from.get_mut(&from) {
                s.remove(&key);
            }
        }
    }

    fn relax_keys(&mut self, keys: Vec<u64>) {
        for k in keys {
            if let Some(m) = self.msgs.get_mut(&k) {
                m.must = false;
            }
        }
    }

    /// What is on its way between `a` and `b`, either way, may now be lost.
    fn relax_pair(&mut self, a: u32, b: u32) {
        let mut keys = Vec::new();
        for (side, other) in [(a, b), (b, a)] {
            if let Some(open) = self.open_to.get(&side) {
                keys.extend(open.iter().filter(|k| self.msgs.get(k).is_some_and(|m| m.from == other)));
            }
        }
        self.relax_keys(keys);
    }

    fn relax_to(&mut self, user: u32) {
        let keys = self.open_to.get(&user).map(|s| s.iter().copied().collect()).unwrap_or_default();
        self.relax_keys(keys);
    }

    fn relax_from(&mut self, user: u32) {
        let keys = self.open_from.get(&user).map(|s| s.iter().copied().collect()).unwrap_or_default();
        self.relax_keys(keys);
    }
}

fn msg_key(idx: usize, gen: u32, sent: u32) -> u64 {
    ((idx as u64) << 40) | (((gen as u64) & 0xFFF) << 28) | (sent as u64 & 0x0FFF_FFFF)
}

fn parse_tag(run: &str, tag: &str) -> Option<u64> {
    let mut parts = tag.split(':');
    if parts.next()? != run {
        return None;
    }
    let idx = parts.next()?.parse().ok()?;
    let gen = parts.next()?.parse().ok()?;
    let sent = parts.next()?.parse().ok()?;
    Some(msg_key(idx, gen, sent))
}

// ---- the world ----

#[derive(Clone)]
struct Entry {
    name: String,
    user: String,
    alive: bool,
    reused: bool,
}

struct World {
    args: Args,
    net: Arc<Net>,
    stats: Arc<Stats>,
    ledger: Mutex<Ledger>,
    groups: Mutex<GroupLedger>,
    users: RwLock<Interner>,
    names: RwLock<Interner>,
    dir: RwLock<Vec<Entry>>,
    by_user: RwLock<HashMap<String, usize>>,
    /// Whether each account runs now (in a worker's hot set).
    hot: Vec<AtomicBool>,
    run: String,
    started: Instant,
    sent: AtomicU64,
    received: AtomicU64,
    stop: AtomicBool,
    /// The machine ran low on memory (`--min-free-mb`): the chaos ends early, everything after it runs.
    low_memory: AtomicBool,
    send_latency: Hist,
    delivery: Hist,
    delivery_hot: Hist,
    traces: Mutex<HashMap<u32, VecDeque<String>>>,
    /// Every trace line, when asked for (`--trace-file`).
    trace_file: Option<Log>,
    progress: Vec<AtomicU64>,
    states: std::path::PathBuf,
    hub: Mutex<HubProc>,
}

impl World {
    fn ledger(&self) -> MutexGuard<'_, Ledger> {
        lock(&self.ledger)
    }

    fn uid(&self, user: &str) -> u32 {
        if let Some(&id) = self.users.read().unwrap_or_else(PoisonError::into_inner).ids.get(user) {
            return id;
        }
        self.users.write().unwrap_or_else(PoisonError::into_inner).id(user)
    }

    fn name_id(&self, name: &str) -> u32 {
        if let Some(&id) = self.names.read().unwrap_or_else(PoisonError::into_inner).ids.get(name) {
            return id;
        }
        self.names.write().unwrap_or_else(PoisonError::into_inner).id(name)
    }

    fn user_of(&self, uid: u32) -> String {
        let users = self.users.read().unwrap_or_else(PoisonError::into_inner);
        users.all.get(uid as usize - 1).cloned().unwrap_or_default()
    }

    fn now_ms(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    fn trace(&self, uid: u32, what: String) {
        if let Some(file) = &self.trace_file {
            let _ = writeln!(lock(file), "{} {} {what}", self.now_ms(), self.user_of(uid));
        }
        let mut traces = lock(&self.traces);
        let trace = traces.entry(uid).or_default();
        trace.push_back(format!("{} {what}", self.now_ms()));
        if trace.len() > self.args.trace {
            trace.pop_front();
        }
    }

    fn dump(&self, uid: u32) {
        let traces = lock(&self.traces);
        println!("  trace of {}:", self.user_of(uid));
        for line in traces.get(&uid).into_iter().flatten() {
            println!("    {line}");
        }
    }

    fn entry(&self, idx: usize) -> Entry {
        self.dir.read().unwrap_or_else(PoisonError::into_inner)[idx].clone()
    }

    fn idx_of(&self, user: &str) -> Option<usize> {
        self.by_user.read().unwrap_or_else(PoisonError::into_inner).get(user).copied()
    }

    fn alive(&self, user: &str) -> bool {
        self.idx_of(user).is_some_and(|i| self.entry(i).alive && self.entry(i).user == user)
    }

    fn done(&self) -> bool {
        self.stop.load(Relaxed)
            || self.low_memory.load(Relaxed)
            || self.sent.load(Relaxed) >= self.args.messages
            || self.started.elapsed().as_secs_f64() > self.args.hours * 3600.0
    }
}

/// A photo or video sealed and waiting for its upload.
struct Pending {
    to: usize,
    media: yo_core::Media,
    sealed: Vec<u8>,
    key: u64,
    tag: String,
    video: bool,
}

struct Acct {
    idx: usize,
    name: String,
    secret: [u8; 32],
    user: String,
    uid: u32,
    alive: bool,
    client: Option<Client>,
    proc_: Arc<Proc>,
    gen: u32,
    blocked: BTreeSet<String>,
    seq: HashMap<u32, u32>,
    sent: u32,
    friends: Vec<usize>,
    bot: bool,
    celeb: bool,
    /// Asleep (no app running) until this many messages were sent in all.
    sleeps_until: u64,
    /// Several phones over the run, one after the other.
    phones: bool,
    ip: String,
    /// The phone's clock, off by this much (within the hub's window), and for a while by much more.
    skew: i64,
    wrong_clock: u32,
    disk_full: u32,
    backed_up: Option<bool>,
    uploads: Vec<Pending>,
    /// Group photos and videos waiting for their upload: (group, media, ciphertext, video).
    group_uploads: Vec<(String, yo_core::Media, Vec<u8>, bool)>,
    drops_traced: usize,
    last_used: u64,
}

// ---- names ----

fn name_for(run: &str, i: usize, style: u64) -> String {
    match style % 6 {
        0 => format!("Leo{run}{i}"),
        1 => format!("Zoë{run}{i}"),
        2 => format!("José_{run}{i}"),
        3 => format!("🔥{run}{i}"),
        4 => format!("Mia_{run}{i}"),
        _ => format!("yOgPt{run}{i}"),
    }
}

fn ip_for(i: usize, rng: &mut Rng) -> String {
    // One phone in twenty behind a carrier's NAT, with others.
    if rng.below(20) == 0 {
        format!("100.64.0.{}", rng.below(16))
    } else {
        format!("10.{}.{}.{}", (i >> 16) & 255, (i >> 8) & 255, i & 255)
    }
}

// ---- the hub, a process of its own ----

struct HubProc {
    child: Option<std::process::Child>,
    addr: String,
    kills: usize,
    rss_max: u64,
    wal_max: u64,
    db_max: u64,
    blobs_max: u64,
    pending_max: i64,
    drained_max: i64,
}

fn hub_config(args: &Args) -> Config {
    let mut config = Config::for_tests();
    config.db_path = args.db();
    config.trust_cloudflare = true;
    config.limits = Limits {
        lookup_per_device_day: 1000,
        identity_lookup_per_device_day: 10_000,
        register_per_ip_hour: 1000,
        name_check_per_ip_minute: u32::MAX,
        backup_per_ip_hour: u32::MAX,
        max_pending_per_device: args.inbox,
        blob_bytes_stored_per_user: args.blob_quota_mb * MIB,
        ..Limits::default()
    };
    config.blobs = BlobConfig {
        dir: format!("{}/blobs", args.dir).into(),
        budget_bytes: args.blob_budget_mb * MIB,
        min_free_bytes: 4096 * MIB,
    };
    config.reserved = yo_hub::config::parse_reserved("YoGPT\nSupport\n");
    config
}

/// The child's life: the hub on its database, until killed.
/// As `yo-hub` serves (a worker per core), with the test's push recorder.
fn serve(args: &Args, addr: &str) -> ! {
    // Gone with the run that started it (PR_SET_PDEATHSIG would follow the thread that spawned it, not the process).
    let parent = std::os::unix::process::parent_id();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        if std::os::unix::process::parent_id() != parent {
            std::process::exit(0);
        }
    });
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let hub = yo_hub::Hub::new(hub_config(args), Arc::new(yo_hub::push::Recorder::default())).unwrap();
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        println!("ready {addr}");
        hub.serve(listener, std::future::pending::<()>()).await.unwrap();
    });
    std::process::exit(0)
}

fn spawn_hub(args: &Args, addr: &str) -> std::process::Child {
    let exe = std::env::current_exe().unwrap();
    for _ in 0..100 {
        let mut child = std::process::Command::new(&exe)
            .args(std::env::args().skip(1))
            .args(["--serve-hub", addr])
            .stdout(std::process::Stdio::piped())
            .stderr(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(format!("{}/hub.log", args.dir))
                    .map(std::process::Stdio::from)
                    .unwrap_or_else(|_| std::process::Stdio::null()),
            )
            .spawn()
            .expect("the hub starts");
        let mut line = String::new();
        let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
        let _ = std::io::BufRead::read_line(&mut out, &mut line);
        if line.starts_with("ready") {
            return child;
        }
        // The port may not be free again yet.
        let _ = child.kill();
        let _ = child.wait();
        std::thread::sleep(Duration::from_millis(300));
    }
    panic!("the hub does not start on {addr}");
}

fn kill_hub(w: &World, restart: bool) {
    let mut hub = lock(&w.hub);
    if let Some(mut child) = hub.child.take() {
        // SIGKILL: no shutdown, whatever it was writing.
        let _ = child.kill();
        let _ = child.wait();
        hub.kills += 1;
        w.stats.count("hub killed");
        println!("hub killed at {} ms", w.now_ms());
    }
    if restart {
        std::thread::sleep(Duration::from_millis(200 + yo_tests::random_u64() % 2000));
        let addr = hub.addr.clone();
        hub.child = Some(spawn_hub(&w.args, &addr));
    }
}

/// What the hub holds and takes, every few seconds: its memory, its files, its waiting envelopes.
fn watch_hub(w: &World, db: &Option<rusqlite::Connection>) -> String {
    let pid = lock(&w.hub).child.as_ref().map(|c| c.id());
    let rss = pid.map_or(0, |p| rss_mb(Some(p)));
    let path = w.args.db();
    let (db_mb, wal_mb) = (file_mb(&path), file_mb(&format!("{path}-wal")));
    let (blob_bytes, _) = dir_bytes(&std::path::PathBuf::from(format!("{}/blobs", w.args.dir)));
    let (mut pending, mut drained) = (-1, -1);
    if let Some(db) = db {
        pending = db.query_row("SELECT count(*) FROM messages", [], |r| r.get(0)).unwrap_or(-1);
        drained = db
            .query_row(
                "SELECT count(*) FROM devices d WHERE NOT EXISTS
                 (SELECT 1 FROM prekeys p WHERE p.device_id = d.id AND p.kind = 3)",
                [],
                |r| r.get(0),
            )
            .unwrap_or(-1);
    }
    let mut hub = lock(&w.hub);
    hub.rss_max = hub.rss_max.max(rss);
    hub.wal_max = hub.wal_max.max(wal_mb);
    hub.db_max = hub.db_max.max(db_mb);
    hub.blobs_max = hub.blobs_max.max(blob_bytes / MIB);
    hub.pending_max = hub.pending_max.max(pending);
    hub.drained_max = hub.drained_max.max(drained);
    format!(
        "hub rss {rss} MB, db {db_mb} MB, wal {wal_mb} MB, blobs {} MB, waiting {pending}, devices without one-time \
         prekeys {drained}, kills {}",
        blob_bytes / MIB,
        hub.kills
    )
}

// ---- accounts ----

fn state_path(w: &World, idx: usize) -> std::path::PathBuf {
    w.states.join(idx.to_string())
}

fn new_proc(w: &World, a: &Acct, seed: u64, saved: Vec<u8>) -> Arc<Proc> {
    let proc_ = Proc::new(&w.net, &w.stats, &a.ip, w.args.faults, seed, saved);
    if let Some(log) = &w.trace_file {
        *lock(&proc_.log) = Some((log.clone(), a.user.clone(), w.started));
    }
    proc_
}

fn register(w: &World, name: &str, secret: Option<[u8; 32]>, proc_: &Arc<Proc>) -> Result<Client, Error> {
    for attempt in 0.. {
        match Client::register_username(proc_.clone(), name, secret, None, Push::none()) {
            Err(Error::Refused(429)) if attempt < 100 => {
                w.stats.count("register: 429, waiting");
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(Error::Network(_)) | Err(Error::Refused(503)) if attempt < 100 => {
                std::thread::sleep(Duration::from_millis(300));
            }
            other => return other,
        }
    }
    unreachable!()
}

fn new_acct(w: &World, idx: usize, rng: &mut Rng) -> Option<Acct> {
    let name = name_for(&w.run, idx, rng.below(6));
    let ip = ip_for(idx, rng);
    let proc_ = Proc::new(&w.net, &w.stats, &ip, w.args.faults / 3, rng.next_u64(), Vec::new());
    let mut client = match register(w, &name, None, &proc_) {
        Ok(client) => client,
        Err(error) => {
            w.stats.violation(format!("registering {name}: {error}"));
            return None;
        }
    };
    let skew = if rng.below(3) == 0 { rng.below(480_000) as i64 - 240_000 } else { 0 };
    client.set_clock_skew(skew);
    if let Err(error) = client.persist_to(proc_.clone(), STATE_KEY) {
        w.stats.violation(format!("{name}: first save: {error}"));
    }
    let user = client.user_id().to_owned();
    let uid = w.uid(&user);
    w.stats.count("registered");
    let a = Acct {
        idx,
        name,
        secret: client.account_secret(),
        user,
        uid,
        alive: true,
        client: None,
        proc_: proc_.clone(),
        gen: 0,
        blocked: BTreeSet::new(),
        seq: HashMap::new(),
        sent: 0,
        friends: Vec::new(),
        bot: idx % 400 == 7,
        celeb: idx % 300 == 11,
        sleeps_until: if rng.below(20) == 0 { w.args.messages * (30 + rng.below(50)) / 100 } else { 0 },
        phones: rng.below(5) == 0,
        ip,
        skew,
        wrong_clock: 0,
        disk_full: 0,
        backed_up: Some(false),
        uploads: Vec::new(),
        group_uploads: Vec::new(),
        drops_traced: 0,
        last_used: 0,
    };
    // Stopped as soon as registered: on disk until its turn.
    drop(client);
    std::fs::write(state_path(w, idx), proc_.saved()).unwrap();
    Some(a)
}

/// The app started (again) from its state on disk.
fn activate(w: &World, a: &mut Acct, rng: &mut Rng) -> bool {
    if a.client.is_some() {
        return true;
    }
    if !a.alive {
        return false;
    }
    let blob = match std::fs::read(state_path(w, a.idx)) {
        Ok(blob) => blob,
        Err(error) => {
            w.stats.violation(format!("{}: no state on disk: {error}", a.name));
            return false;
        }
    };
    let proc_ = new_proc(w, a, rng.next_u64(), blob.clone());
    match Client::restore(proc_.clone(), &blob, &STATE_KEY) {
        Ok(mut client) => {
            client.set_clock_skew(a.skew);
            if let Err(error) = client.persist_to(proc_.clone(), STATE_KEY) {
                w.stats.count(format!("activate: {}", short(&error)));
            }
            a.client = Some(client);
            a.proc_ = proc_;
            w.hot[a.idx].store(true, Relaxed);
            w.stats.count("activated");
            w.trace(a.uid, format!("activated, pending {}", a.client.as_ref().map_or(0, |c| c.pending())));
            true
        }
        Err(error) => {
            w.stats.violation(format!("{}: the state on disk does not open: {error}", a.name));
            false
        }
    }
}

/// The system stops the app: what it saved goes to disk, the rest is gone (photos not yet uploaded).
fn evict(w: &World, a: &mut Acct) {
    let Some(client) = a.client.take() else { return };
    harvest_drops(w, Some(&client));
    drop(client);
    if a.alive {
        if let Err(error) = std::fs::write(state_path(w, a.idx), a.proc_.saved()) {
            w.stats.violation(format!("{}: state to disk: {error}", a.name));
        }
    }
    a.uploads.clear();
    a.group_uploads.clear();
    a.proc_.disk_full.store(false, Relaxed);
    a.disk_full = 0;
    w.hot[a.idx].store(false, Relaxed);
    w.trace(a.uid, "evicted".into());
}

fn harvest_drops(w: &World, client: Option<&Client>) {
    for reason in client.map(|c| c.drop_reasons()).unwrap_or_default() {
        let kind: String = reason.chars().take(60).collect();
        w.stats.count(format!("dropped: {kind}"));
    }
}

/// The app killed and started again from what it had saved.
fn restart(w: &World, a: &mut Acct, rng: &mut Rng) {
    harvest_drops(w, a.client.as_ref());
    let saved = a.proc_.saved();
    let proc_ = new_proc(w, a, rng.next_u64(), saved.clone());
    match Client::restore(proc_.clone(), &saved, &STATE_KEY) {
        Ok(mut client) => {
            client.set_clock_skew(a.skew);
            let _ = client.persist_to(proc_.clone(), STATE_KEY);
            a.client = Some(client);
            a.proc_ = proc_;
            a.disk_full = 0;
            w.stats.count("restarted");
        }
        Err(error) => {
            a.client = None;
            w.stats.violation(format!("{}: the saved state does not open: {error}", a.name));
        }
    }
    w.trace(a.uid, format!("restarted, pending {}", a.client.as_ref().map_or(0, |c| c.pending())));
}

// ---- receiving ----

fn short(error: &Error) -> String {
    match error {
        Error::Network(_) => "network".into(),
        Error::Save(_) => "save".into(),
        other => other.to_string(),
    }
}

/// An error no fault explains is a finding.
fn note_error(w: &World, a: &Acct, what: &str, error: &Error) {
    match error {
        Error::Network(_) => w.stats.count(format!("{what}: network")),
        Error::Refused(s) if *s == 429 || *s == 507 || *s >= 500 => w.stats.count(format!("{what}: {s}")),
        Error::Refused(401) if a.wrong_clock > 0 => w.stats.count(format!("{what}: 401, clock wrong")),
        Error::Save(_) if a.disk_full > 0 => w.stats.count(format!("{what}: disk full")),
        _ if a.proc_.crashed.load(Relaxed) => w.stats.count(format!("{what}: during a crash")),
        Error::Blocked => w.stats.count(format!("{what}: blocked")),
        _ => {
            w.stats.violation(format!("{} ({}) {what}: {error}", a.name, a.user));
            w.dump(a.uid);
        }
    }
}

/// A photo's blob, fetched as the app does, with the fetch faults: cut off (the count comes back), two at once (one
/// gets it), and again after (gone: view once).
fn fetch_photo(w: &World, a: &Acct, m: &yo_core::Media, info: Option<MediaInfo>, rng: &mut Rng) {
    let clean = Proc::new(&w.net, &w.stats, &a.ip, 0, rng.next_u64(), Vec::new());
    let path = media::blob_path(&m.id);
    let roll = rng.below(100);
    if roll < 10 {
        w.net.cut_fetch(&path, &a.ip);
        w.stats.count("fetch cut off");
    }
    let mut got = None;
    if (10..16).contains(&roll) {
        let (one, two) = std::thread::scope(|s| {
            let first = s.spawn(|| definite(|| media::fetch_open(clean.as_ref(), m)));
            let second = definite(|| media::fetch_open(clean.as_ref(), m));
            (first.join().unwrap(), second)
        });
        match (one, two) {
            (Ok(Some(_)), Ok(Some(_))) => w.stats.violation(format!("blob of {} fetched twice at once", a.name)),
            (Ok(Some(p)), _) | (_, Ok(Some(p))) => got = Some(Ok(Some(p))),
            _ => {}
        }
        w.stats.count("fetch race");
    }
    let got = got.unwrap_or_else(|| definite(|| media::fetch_open(clean.as_ref(), m)));
    let uploaded = info.is_some_and(|i| i.uploaded);
    let sender_gone = info.is_some_and(|i| !w.alive(&w.user_of(i.from)));
    match got {
        Ok(Some(plain)) => {
            if !uploaded {
                w.stats.violation(format!("{}: a blob never uploaded was fetched", a.name));
            } else if info.map(|i| i.size) != Some(plain.len()) {
                w.stats.violation(format!(
                    "{}: photo of {} bytes, sent {:?}",
                    a.name,
                    plain.len(),
                    info.map(|i| i.size)
                ));
            }
            w.stats.count("photo opened");
            if let Ok(Some(_)) = media::fetch(clean.as_ref(), m) {
                w.stats.violation(format!("{}: a photo fetched twice", a.name));
            }
        }
        Ok(None) if !uploaded => w.stats.count("photo never uploaded: not there"),
        Ok(None) if sender_gone => w.stats.count("photo gone with its sender"),
        // Cut off after the hub handed on its last byte (into the socket's buffers): counted (tests/CHAOS.md).
        Ok(None) if roll < 10 => w.stats.count("photo gone: the cut fetch had it all handed on"),
        Ok(None) => w.stats.violation(format!(
            "{}: photo gone before its fetch ({} bytes, roll {roll}, {} kills so far)",
            a.name,
            m.size,
            lock(&w.hub).kills
        )),
        Err(error) => w.stats.violation(format!("{}: photo: {error}", a.name)),
    }
}

/// Takes in what a call returned: every message checked against the ledger, bots answer.
fn take_events(w: &World, a: &mut Acct, events: Vec<Event>, rng: &mut Rng) {
    if a.proc_.crashed.load(Relaxed) {
        return;
    }
    let mut answer = Vec::new();
    for event in events {
        let (from, tag, photo) = match event {
            Event::Message { user, content: Content::Text(tag), .. } => (user, Some(tag), None),
            Event::Message { user, content: Content::Photo(m) | Content::Video { media: m, .. }, .. } => {
                (user, None, Some(m))
            }
            Event::Message { .. } => {
                w.stats.count("plain Yo received");
                continue;
            }
            Event::IdentityChanged { .. } => {
                w.stats.count("identity changed");
                continue;
            }
            Event::GroupMessage { group, user, sent_at_ms, content } => {
                group_received(w, a, &group, &user, sent_at_ms, content, rng);
                continue;
            }
            _ => continue,
        };
        if a.blocked.contains(&from) {
            w.stats.violation(format!("{} was shown {tag:?} from {from}, whom it blocks", a.user));
        }
        let info = photo.as_ref().and_then(|m| w.ledger().media.get(&m.id).copied());
        let key = match (&tag, &info) {
            (Some(tag), _) => parse_tag(&w.run, tag),
            (None, Some(i)) => Some(i.key),
            _ => None,
        };
        let Some(key) = key else {
            w.stats.violation(format!("{} received {tag:?}, which nobody sent", a.user));
            continue;
        };
        if let Some(m) = &photo {
            fetch_photo(w, a, m, info, rng);
        }
        let from_uid = w.uid(&from);
        let now = w.now_ms();
        let mut ledger = w.ledger();
        let Some(msg) = ledger.msgs.get_mut(&key).copied() else {
            drop(ledger);
            w.stats.violation(format!("{} received {tag:?}, which the ledger does not have", a.user));
            continue;
        };
        let a_name = w.name_id(&a.name);
        if msg.to != a.uid && msg.from == from_uid && msg.name == a_name && msg.got == 0 {
            let m = ledger.msgs.get_mut(&key).unwrap();
            m.must = false;
            m.got = 1;
            ledger.arrived(key);
            drop(ledger);
            w.stats.count("received by the name's new owner");
            continue;
        }
        if msg.to != a.uid || msg.from != from_uid {
            drop(ledger);
            w.stats.violation(format!("{tag:?}: from {} to {}, received by {} from {from}", msg.from, msg.to, a.user));
            continue;
        }
        if msg.got > 0 && msg.got_by != a.gen as u16 {
            ledger.msgs.get_mut(&key).unwrap().got_by = a.gen as u16;
            drop(ledger);
            w.stats.count("received again on a new phone");
            continue;
        }
        {
            let m = ledger.msgs.get_mut(&key).unwrap();
            m.got = m.got.saturating_add(1);
            m.got_by = a.gen as u16;
        }
        ledger.arrived(key);
        if msg.got >= 1 {
            drop(ledger);
            w.stats.violation(format!("{} received {tag:?} {} times", a.user, msg.got + 1));
            w.dump(a.uid);
            continue;
        }
        let latency = (now.saturating_sub(msg.sent_ms)) as u64 * 1000;
        let last = ledger.last.entry((a.uid, from_uid)).or_default();
        if msg.seq <= *last {
            let was = *last;
            drop(ledger);
            w.stats.violation(format!("{} received {tag:?} (#{}) after #{was} from {from}", a.user, msg.seq));
            w.dump(a.uid);
            w.dump(from_uid);
        } else {
            *last = msg.seq;
            drop(ledger);
            w.received.fetch_add(1, Relaxed);
            w.stats.count("received");
            w.delivery.add(latency);
            if msg.hot {
                w.delivery_hot.add(latency);
            }
        }
        w.trace(
            a.uid,
            format!("got {}:{}:{} from {from} #{}", key >> 40, (key >> 28) & 0xFFF, key & 0x0FFF_FFFF, msg.seq),
        );
        if a.bot && rng.below(2) == 0 {
            answer.push(from);
        }
    }
    if let Some(client) = a.client.as_mut() {
        client.acknowledge_events();
    }
    for user in answer {
        for _ in 0..1 + rng.below(3) {
            send_text(w, a, &user, None, rng);
        }
    }
}

/// Repeats `f` while the answer is "not now".
fn definite<T>(mut f: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
    for _ in 0..60 {
        match f() {
            Err(Error::Network(_)) | Err(Error::Refused(429)) => std::thread::sleep(Duration::from_millis(300)),
            Err(Error::Refused(status)) if status >= 500 && status != 507 => {
                std::thread::sleep(Duration::from_millis(300))
            }
            other => return other,
        }
    }
    f()
}

// ---- sending ----

fn new_msg(w: &World, a: &mut Acct, to: &str, by_name: Option<&str>) -> (u64, String, bool) {
    let to_uid = w.uid(to);
    let seq = a.seq.entry(to_uid).or_default();
    *seq += 1;
    let seq = *seq;
    a.sent += 1;
    let key = msg_key(a.idx, a.gen, a.sent);
    let tag = format!("{}:{}:{}:{}", w.run, a.idx, a.gen, a.sent);
    let alive = w.alive(to);
    let hot = w.idx_of(to).is_some_and(|i| w.hot[i].load(Relaxed));
    let name = by_name.map_or(0, |n| w.name_id(n));
    let mut ledger = w.ledger();
    let blocked = ledger.blocks.contains(&(to_uid, a.uid));
    let must = alive && !blocked;
    let sent_ms = w.now_ms();
    ledger.insert(key, Msg { from: a.uid, to: to_uid, seq, sent_ms, name, got_by: 0, got: 0, must, hot });
    drop(ledger);
    w.sent.fetch_add(1, Relaxed);
    (key, tag, alive)
}

fn send_text(w: &World, a: &mut Acct, to: &str, by_name: Option<&str>, rng: &mut Rng) {
    if a.client.is_none() {
        return;
    }
    let (key, tag, alive) = new_msg(w, a, to, by_name);
    let target = by_name.unwrap_or(to).to_owned();
    let t = Instant::now();
    let result = a.client.as_mut().unwrap().send(&target, Content::Text(tag.clone()));
    w.send_latency.add(t.elapsed().as_micros() as u64);
    w.trace(a.uid, format!("send {tag} to {to}: {:?}", result.as_ref().map(|e| e.len())));
    after_send(w, a, key, to, result, alive, rng);
}

fn after_send(
    w: &World,
    a: &mut Acct,
    key: u64,
    to: &str,
    result: Result<Vec<Event>, Error>,
    alive: bool,
    rng: &mut Rng,
) {
    w.stats.count("sent");
    match result {
        Ok(events) if !a.proc_.crashed.load(Relaxed) => take_events(w, a, events, rng),
        Ok(_) => {
            if let Some(m) = w.ledger().msgs.get_mut(&key) {
                m.must = false;
            }
        }
        Err(error) => {
            // Not sent, or not for sure: it may still go from the outbox, or never.
            if let Some(m) = w.ledger().msgs.get_mut(&key) {
                m.must = false;
            }
            let blocked = a.blocked.contains(to)
                || a.client.as_ref().is_some_and(|c| c.contacts().iter().any(|k| k.user == to && k.blocked));
            let expected = matches!(error, Error::NotOnYo) && (!alive || !w.alive(to))
                || matches!(error, Error::Blocked) && blocked
                || matches!(error, Error::Network(_))
                || a.proc_.crashed.load(Relaxed);
            if expected {
                w.stats.count(format!("send: {}", short(&error)));
            } else {
                note_error(w, a, "send", &error);
            }
        }
    }
}

/// A friend mostly, anyone alive sometimes.
fn pick_peer(w: &World, a: &Acct, rng: &mut Rng) -> Option<Entry> {
    let n = w.args.accounts as u64;
    let j = if rng.below(20) != 0 && !a.friends.is_empty() {
        a.friends[rng.below(a.friends.len() as u64) as usize]
    } else {
        rng.below(n) as usize
    };
    let e = w.entry(j);
    (j != a.idx && e.user.len() == 32).then_some(e)
}

/// Who a typed name is, as the app asks before the first Yo.
fn whois(w: &World, a: &mut Acct, peer: &Entry) -> bool {
    let Some(client) = a.client.as_mut() else { return false };
    match client.whois_begin(&peer.name).and_then(|whois| whois.send()) {
        Ok((user, _)) if user == peer.user => true,
        Ok((user, _)) => {
            if w.idx_of(&user).is_none() {
                w.stats.violation(format!("whois {} answered {user}, not {}", peer.name, peer.user));
            }
            false
        }
        Err(Error::NotOnYo) if !w.alive(&peer.user) => false,
        Err(error) => {
            note_error(w, a, "whois", &error);
            false
        }
    }
}

fn knows(a: &Acct, user: &str) -> bool {
    a.client.as_ref().is_some_and(|c| c.contacts().iter().any(|k| k.user == user))
}

fn op_send(w: &World, a: &mut Acct, rng: &mut Rng) {
    let Some(peer) = pick_peer(w, a, rng) else { return };
    let by_name = (!peer.reused && rng.below(8) == 0).then_some(peer.name.clone());
    if !knows(a, &peer.user) && by_name.is_none() && !whois(w, a, &peer) {
        return;
    }
    // Bursts: a few, now and then dozens.
    let burst = match rng.below(20) {
        0 => 10 + rng.below(40),
        1..=4 => 2 + rng.below(5),
        _ => 1,
    };
    for _ in 0..burst {
        send_text(w, a, &peer.user, by_name.as_deref(), rng);
        if a.proc_.crashed.load(Relaxed) || a.client.is_none() {
            break;
        }
    }
}

fn op_fan_out(w: &World, a: &mut Acct, rng: &mut Rng) {
    for _ in 0..rng.below(10) + 3 {
        let Some(peer) = pick_peer(w, a, rng) else { continue };
        if !knows(a, &peer.user) && !whois(w, a, &peer) {
            continue;
        }
        send_text(w, a, &peer.user, None, rng);
        if a.proc_.crashed.load(Relaxed) {
            break;
        }
    }
}

fn op_sync(w: &World, a: &mut Acct, rng: &mut Rng) {
    let Some(client) = a.client.as_mut() else { return };
    let result = client.sync(if rng.below(100) == 0 { 1 } else { 0 });
    w.trace(a.uid, format!("sync: {:?}", result.as_ref().map(|e| e.len())));
    match result {
        Ok(events) if !a.proc_.crashed.load(Relaxed) => take_events(w, a, events, rng),
        Ok(_) => {}
        Err(error) => note_error(w, a, "sync", &error),
    }
    if rng.below(2) == 0 && !a.proc_.crashed.load(Relaxed) {
        if let Some(client) = a.client.as_mut() {
            if let Err(error) = client.after_sync() {
                note_error(w, a, "after_sync", &error);
            }
        }
    }
}

/// A photo or a video of a real size; now and then one whose blob never comes.
fn op_photo(w: &World, a: &mut Acct, rng: &mut Rng) {
    let Some(peer) = pick_peer(w, a, rng) else { return };
    let Some(to) = w.idx_of(&peer.user) else { return };
    if a.uploads.len() >= 2 {
        return op_uploads(w, a, rng);
    }
    let video = rng.below(4) == 0;
    let size = if video {
        1_000_000 + rng.below(media::MAX_MEDIA_LEN as u64 - 1_000_000) as usize
    } else {
        200_000 + rng.below(5_800_000) as usize
    };
    let mut plain = vec![0u8; size];
    for (i, chunk) in plain.chunks_mut(8).enumerate() {
        let x = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ a.sent as u64;
        chunk.copy_from_slice(&x.to_le_bytes()[..chunk.len()]);
    }
    let Ok((m, sealed)) = media::seal(&plain, 1080, 1920) else { return };
    drop(plain);
    a.sent += 1;
    let key = msg_key(a.idx, a.gen, a.sent);
    let tag = format!("{}:{}:{}:{}", w.run, a.idx, a.gen, a.sent);
    let never = rng.below(100) == 0;
    w.ledger().media.insert(m.id, MediaInfo { key, size, uploaded: false, from: a.uid });
    w.stats.count(if video { "video sealed" } else { "photo sealed" });
    let pending = Pending { to, media: m, sealed, key, tag, video };
    if never {
        // The Yo goes, its blob never: the receiver finds nothing, and goes on.
        w.stats.count("photo without its blob");
        send_media(w, a, pending, rng);
        return;
    }
    a.uploads.push(pending);
    op_uploads(w, a, rng);
}

fn send_media(w: &World, a: &mut Acct, p: Pending, rng: &mut Rng) {
    let peer = w.entry(p.to);
    let to_uid = w.uid(&peer.user);
    let seq = a.seq.entry(to_uid).or_default();
    *seq += 1;
    let seq = *seq;
    {
        let hot = w.hot[p.to].load(Relaxed);
        let mut ledger = w.ledger();
        let must = peer.alive && !ledger.blocks.contains(&(to_uid, a.uid));
        let sent_ms = w.now_ms();
        ledger.insert(p.key, Msg { from: a.uid, to: to_uid, seq, sent_ms, name: 0, got_by: 0, got: 0, must, hot });
    }
    w.sent.fetch_add(1, Relaxed);
    let content = if p.video { Content::Video { media: p.media, duration_ms: 9_000 } } else { Content::Photo(p.media) };
    let t = Instant::now();
    let result = a.client.as_mut().unwrap().send(&peer.user, content);
    w.send_latency.add(t.elapsed().as_micros() as u64);
    w.stats.count("photo sent");
    w.trace(a.uid, format!("media {} to {}: {:?}", p.tag, peer.user, result.as_ref().map(|e| e.len())));
    after_send(w, a, p.key, &peer.user, result, peer.alive, rng);
}

/// Uploads what waits, then sends its Yo; "later" keeps it for the next try.
fn op_uploads(w: &World, a: &mut Acct, rng: &mut Rng) {
    let mut keep = Vec::new();
    for p in std::mem::take(&mut a.uploads) {
        let peer = w.entry(p.to);
        if !peer.alive || a.client.is_none() || a.blocked.contains(&peer.user) || a.proc_.crashed.load(Relaxed) {
            continue;
        }
        let client = a.client.as_mut().unwrap();
        let fetches = match client.media_fetches(&peer.user) {
            Some(n) => n,
            None => {
                if !knows(a, &peer.user) && !whois(w, a, &peer) {
                    continue;
                }
                match a.client.as_mut().unwrap().open_chat(&peer.user) {
                    Ok((n, events)) => {
                        take_events(w, a, events, rng);
                        n
                    }
                    Err(error) => {
                        note_error(w, a, "open chat", &error);
                        continue;
                    }
                }
            }
        };
        if fetches != 1 {
            w.stats.violation(format!("{}: a photo for {fetches} fetches, one device each side", a.name));
        }
        let Some(client) = a.client.as_mut() else { continue };
        let sent = client.upload(&p.media, fetches).and_then(|upload| upload.send(&p.sealed));
        match sent {
            Ok(()) => {}
            Err(error) if media::retry_later(&error) => {
                w.stats.count(format!("upload later: {}", short(&error)));
                keep.push(p);
                continue;
            }
            Err(error) => {
                note_error(w, a, "upload", &error);
                continue;
            }
        }
        if let Some(info) = w.ledger().media.get_mut(&p.media.id) {
            info.uploaded = true;
        }
        w.stats.count("uploaded");
        send_media(w, a, p, rng);
    }
    a.uploads.extend(keep);
}

fn op_block(w: &World, a: &mut Acct, rng: &mut Rng) {
    let Some(client) = a.client.as_mut() else { return };
    let contacts = client.contacts();
    if contacts.is_empty() {
        return;
    }
    let who = contacts[rng.below(contacts.len() as u64) as usize].user.clone();
    let who_uid = w.uid(&who);
    let block = !a.blocked.contains(&who);
    if block {
        let mut ledger = w.ledger();
        ledger.blocks.insert((a.uid, who_uid));
        ledger.relax_pair(a.uid, who_uid);
        lock(&w.groups).blocked_ever.insert((a.uid, who_uid));
    }
    match a.client.as_mut().unwrap().set_blocked(&who, block) {
        Ok(()) => {
            w.stats.count(if block { "blocked" } else { "unblocked" });
            w.trace(a.uid, format!("{} {who}", if block { "blocked" } else { "unblocked" }));
            if block {
                a.blocked.insert(who.clone());
            } else {
                a.blocked.remove(&who);
                w.ledger().blocks.remove(&(a.uid, who_uid));
            }
        }
        Err(error) => {
            // Not saved, not made: no block (what was relaxed meanwhile stays may-be-lost).
            if block {
                w.ledger().blocks.remove(&(a.uid, who_uid));
            }
            note_error(w, a, "block", &error);
            return;
        }
    }
    let Some(client) = a.client.as_mut().filter(|_| block) else { return };
    match client.send(&who, Content::Text("never".into())) {
        Err(Error::Blocked) => w.stats.count("send to blocked refused"),
        Err(Error::NotOnYo) => {}
        Err(error) if a.disk_full > 0 || a.wrong_clock > 0 || a.proc_.crashed.load(Relaxed) => {
            note_error(w, a, "send blocked", &error)
        }
        other => w.stats.violation(format!("send to a blocked contact: {:?}", other.map(|_| ()))),
    }
}

fn op_misc(w: &World, a: &mut Acct, rng: &mut Rng) {
    let Some(client) = a.client.as_mut() else { return };
    match rng.below(4) {
        0 => match client.maintenance() {
            Ok(events) if !a.proc_.crashed.load(Relaxed) => take_events(w, a, events, rng),
            Ok(_) => {}
            Err(error) => note_error(w, a, "maintenance", &error),
        },
        1 if a.backed_up == Some(false) => {
            let mut locator = [0u8; 16];
            locator[..8].copy_from_slice(&rng.next_u64().to_le_bytes());
            locator[8..].copy_from_slice(&rng.next_u64().to_le_bytes());
            let auth = yo_core::backup::Door::Passkey(&[9; 32]).auth_key().verifying_key().to_bytes();
            let backup = Backup::new(vec![1; 60], vec![2; 60], vec![3; 300], locator.to_vec(), None, auth);
            match client.put_backup(&backup) {
                Ok(()) => a.backed_up = Some(true),
                Err(error) => {
                    a.backed_up = None;
                    note_error(w, a, "backup", &error)
                }
            }
        }
        2 => {
            // A wrong clock for a while: hours off, refused by the hub until it is set right.
            a.wrong_clock = 3 + rng.below(10) as u32;
            let off = (360_000 + rng.below(3 * 3600 * 1000)) as i64;
            let off = if rng.below(2) == 0 { off } else { -off };
            client.set_clock_skew(off);
            w.stats.count("clock wrong for a while");
            w.trace(a.uid, format!("clock {off} ms off"));
        }
        _ => {
            a.disk_full = 2 + rng.below(8) as u32;
            a.proc_.disk_full.store(true, Relaxed);
            w.stats.count("disk full for a while");
            w.trace(a.uid, "disk full".into());
        }
    }
}

/// A new phone with the same account key: the old device answers 410 from then on.
fn op_new_phone(w: &World, a: &mut Acct, rng: &mut Rng) {
    let proc_ = new_proc(w, a, rng.next_u64(), Vec::new());
    let mut client = match register(w, &a.name, Some(a.secret), &proc_) {
        Ok(client) => client,
        Err(error) => {
            w.stats.violation(format!("{}: a new phone with the account key: {error}", a.name));
            return;
        }
    };
    if client.user_id() != a.user {
        w.stats.violation(format!("{}: a new phone got another user id", a.name));
    }
    client.set_clock_skew(a.skew);
    let _ = client.persist_to(proc_.clone(), STATE_KEY);
    harvest_drops(w, a.client.as_ref());
    let mut old = a.client.replace(client).unwrap();
    let old_proc = a.proc_.clone();
    {
        // What the old phone had not received is gone with it, and so is what it sent that was not received yet: a
        // message from a replaced device is not opened (spec 4.6), though the hub took it.
        let mut ledger = w.ledger();
        ledger.relax_to(a.uid);
        ledger.relax_from(a.uid);
        let me = a.uid;
        ledger.last.retain(|(to, _), _| *to != me);
        drop(ledger);
        let mut groups = lock(&w.groups);
        groups.uncertain(me, true);
        groups.uncertain(me, false);
    }
    a.uploads.clear();
    a.group_uploads.clear();
    a.proc_ = proc_;
    a.gen += 1;
    a.disk_full = 0;
    w.stats.count("new phone");
    w.trace(a.uid, "new phone".into());
    old.stop_persisting();
    let answer = (0..10).map(|_| definite(|| old.sync(0))).find(|r| r.is_err()).unwrap_or(Ok(Vec::new()));
    match answer {
        Err(Error::Replaced) => w.stats.count("old phone: 410"),
        Err(Error::Network(_)) if old_proc.crashed.load(Relaxed) => {}
        Err(Error::Refused(401)) if a.wrong_clock > 0 => {}
        other => w.stats.violation(format!("{}: the replaced device: {:?}", a.name, other.map(|_| ()))),
    }
}

/// Deletes the account; now and then someone takes the name again with a new key.
fn op_delete(w: &World, a: &mut Acct, rng: &mut Rng) {
    if a.proc_.crashed.load(Relaxed) {
        restart(w, a, rng);
    }
    let Some(_) = a.client.as_ref() else { return };
    a.proc_.crash_in.store(0, Relaxed);
    a.proc_.disk_full.store(false, Relaxed);
    a.disk_full = 0;
    if a.wrong_clock > 0 {
        a.wrong_clock = 0;
        a.client.as_mut().unwrap().set_clock_skew(a.skew);
    }
    let me = a.uid;
    w.dir.write().unwrap_or_else(PoisonError::into_inner)[a.idx].alive = false;
    {
        let mut ledger = w.ledger();
        ledger.relax_to(me);
        ledger.relax_from(me);
    }
    let client = a.client.as_mut().unwrap();
    match definite(|| client.delete_account()) {
        Ok(()) | Err(Error::Replaced) => w.stats.count("deleted"),
        Err(error) => w.stats.violation(format!("{}: delete: {error}", a.name)),
    }
    let mut old = a.client.take().unwrap();
    old.stop_persisting();
    let answer = (0..10).map(|_| definite(|| old.sync(0))).find(|r| r.is_err()).unwrap_or(Ok(Vec::new()));
    if !matches!(answer, Err(Error::Replaced)) {
        w.stats.violation(format!("{}: a deleted account's device: {:?}", a.name, answer.map(|_| ())));
    }
    {
        let mut ledger = w.ledger();
        for who in std::mem::take(&mut a.blocked) {
            let who = w.uid(&who);
            ledger.blocks.remove(&(me, who));
        }
    }
    a.uploads.clear();
    a.group_uploads.clear();
    w.hot[a.idx].store(false, Relaxed);
    if rng.below(2) == 0 {
        a.alive = false;
        return;
    }
    let proc_ = new_proc(w, a, rng.next_u64(), Vec::new());
    match register(w, &a.name, None, &proc_) {
        Ok(mut client) => {
            client.set_clock_skew(a.skew);
            let _ = client.persist_to(proc_.clone(), STATE_KEY);
            if client.user_id() == a.user {
                w.stats.violation(format!("{}: the freed name kept the deleted user id", a.name));
            }
            a.user = client.user_id().to_owned();
            a.uid = w.uid(&a.user);
            a.secret = client.account_secret();
            a.client = Some(client);
            a.proc_ = proc_;
            a.gen += 1;
            a.seq.clear();
            a.backed_up = Some(false);
            w.by_user.write().unwrap_or_else(PoisonError::into_inner).insert(a.user.clone(), a.idx);
            w.dir.write().unwrap_or_else(PoisonError::into_inner)[a.idx] =
                Entry { name: a.name.clone(), user: a.user.clone(), alive: true, reused: true };
            w.hot[a.idx].store(true, Relaxed);
            w.stats.count("name taken again");
        }
        Err(error) => {
            a.alive = false;
            w.stats.violation(format!("{}: the freed name: {error}", a.name));
        }
    }
}

fn sync_blocks(w: &World, a: &mut Acct) {
    let Some(client) = a.client.as_ref() else { return };
    // What the client dropped or noted since, into the trace (`--trace-file`).
    if w.trace_file.is_some() {
        let reasons = client.drop_reasons();
        if reasons.len() < a.drops_traced {
            a.drops_traced = 0;
        }
        for reason in &reasons[a.drops_traced..] {
            w.trace(a.uid, format!("noted: {reason}"));
        }
        a.drops_traced = reasons.len();
    }
    let now: BTreeSet<String> = client.contacts().into_iter().filter(|c| c.blocked).map(|c| c.user).collect();
    if now == a.blocked {
        return;
    }
    let mut ledger = w.ledger();
    for who in a.blocked.difference(&now) {
        ledger.blocks.remove(&(a.uid, w.uid(who)));
    }
    for who in now.difference(&a.blocked) {
        let who = w.uid(who);
        ledger.blocks.insert((a.uid, who));
        ledger.relax_pair(a.uid, who);
        lock(&w.groups).blocked_ever.insert((a.uid, who));
    }
    drop(ledger);
    a.blocked = now;
}

fn step(w: &World, a: &mut Acct, rng: &mut Rng) {
    if a.client.is_none() {
        return;
    }
    if rng.below(1000) < w.args.crashes {
        a.proc_.crash_in.store(1 + rng.below(8), Relaxed);
    }
    let roll = rng.below(1000);
    if a.bot {
        op_sync(w, a, rng);
    } else if roll < w.args.media {
        op_photo(w, a, rng);
    } else if roll < 450 {
        op_send(w, a, rng);
    } else if roll < 750 {
        op_sync(w, a, rng);
    } else if roll < 780 {
        op_fan_out(w, a, rng);
    } else if roll < 820 {
        op_group(w, a, rng);
    } else if roll < 835 {
        op_block(w, a, rng);
    } else if roll < 850 {
        op_misc(w, a, rng);
    } else if roll < 865 {
        restart(w, a, rng);
    } else if roll < if a.phones { 870 } else { 866 } {
        op_new_phone(w, a, rng);
    } else if roll == 870 && rng.below(2) == 0 {
        op_delete(w, a, rng);
    } else {
        op_uploads(w, a, rng);
        op_group_uploads(w, a, rng);
        op_sync(w, a, rng);
    }
    if a.client.is_none() {
        return;
    }
    sync_blocks(w, a);
    // Episodes end: the clock set right, room on the disk again.
    if a.wrong_clock > 0 {
        a.wrong_clock -= 1;
        if a.wrong_clock == 0 {
            a.client.as_mut().unwrap().set_clock_skew(a.skew);
        }
    }
    if a.disk_full > 0 {
        a.disk_full -= 1;
        if a.disk_full == 0 {
            a.proc_.disk_full.store(false, Relaxed);
        }
    }
    if a.proc_.crashed.load(Relaxed) {
        a.client = None;
        restart(w, a, rng);
    }
}

// ---- the run ----

fn worker(w: Arc<World>, t: usize, mut accts: Vec<Acct>) -> Vec<Acct> {
    let mut rng = Rng(w.args.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (t as u64 + 1));
    let mut hot: Vec<usize> = Vec::new();
    let mut tick = 0u64;
    while !w.done() {
        tick += 1;
        let sent = w.sent.load(Relaxed);
        let i = if hot.len() >= w.args.hot.min(accts.len()) && rng.below(3) != 0 {
            hot[rng.below(hot.len() as u64) as usize]
        } else {
            let i = rng.below(accts.len() as u64) as usize;
            let a = &accts[i];
            // Asleep, gone, or one everybody knows and who is rarely there.
            if !a.alive || a.sleeps_until > sent || (a.celeb && rng.below(8) != 0) {
                continue;
            }
            i
        };
        if !activate(&w, &mut accts[i], &mut rng) {
            hot.retain(|&h| h != i);
            continue;
        }
        if !hot.contains(&i) {
            hot.push(i);
        }
        accts[i].last_used = tick;
        // A session: a few steps while the app is in front.
        for _ in 0..1 + rng.below(4) {
            step(&w, &mut accts[i], &mut rng);
        }
        if accts[i].client.is_none() {
            hot.retain(|&h| h != i);
        }
        while hot.len() > w.args.hot {
            let (k, _) = hot.iter().enumerate().min_by_key(|(_, &h)| accts[h].last_used).unwrap();
            let h = hot.remove(k);
            evict(&w, &mut accts[h]);
        }
        w.progress[t].store(yo_hub::now_ms(), Relaxed);
    }
    for h in hot {
        evict(&w, &mut accts[h]);
    }
    accts
}

/// Everyone (asleep or not) runs until nothing moves: each catches up, sends what waits.
fn quiesce(w: &Arc<World>, all: Vec<Vec<Acct>>, what: &str) -> Vec<Vec<Acct>> {
    let mut all = all;
    let mut waited = false;
    for round in 0..60 {
        let before = w.received.load(Relaxed);
        let waiting = Arc::new(AtomicUsize::new(0));
        all = all
            .into_iter()
            .enumerate()
            .map(|(t, mut accts)| {
                let (w, waiting) = (w.clone(), waiting.clone());
                std::thread::spawn(move || {
                    let mut rng = Rng(w.args.seed ^ 0xABCD ^ ((round as u64) << 20) ^ t as u64);
                    for a in accts.iter_mut().filter(|a| a.alive) {
                        if !activate(&w, a, &mut rng) {
                            continue;
                        }
                        a.proc_.crash_in.store(0, Relaxed);
                        if a.wrong_clock > 0 {
                            a.wrong_clock = 0;
                            a.client.as_mut().unwrap().set_clock_skew(a.skew);
                        }
                        a.disk_full = 0;
                        a.proc_.disk_full.store(false, Relaxed);
                        for _ in 0..3 {
                            op_uploads(&w, a, &mut rng);
                            op_group_uploads(&w, a, &mut rng);
                            op_sync(&w, a, &mut rng);
                            if let Some(c) = a.client.as_mut() {
                                if let Err(error) = c.after_sync() {
                                    note_error(&w, a, "after_sync", &error);
                                }
                            }
                            sync_blocks(&w, a);
                            if a.proc_.crashed.load(Relaxed) {
                                a.client = None;
                                restart(&w, a, &mut rng);
                            }
                            if a.client.as_ref().is_none_or(|c| c.pending() == 0)
                                && a.uploads.is_empty()
                                && a.group_uploads.is_empty()
                            {
                                break;
                            }
                        }
                        let left =
                            a.client.as_ref().map_or(0, |c| c.pending()) + a.uploads.len() + a.group_uploads.len();
                        waiting.fetch_add(left, Relaxed);
                        // Uploads cannot wait on disk: what is left goes now or not at all.
                        if a.uploads.is_empty() && a.group_uploads.is_empty() {
                            evict(&w, a);
                        }
                    }
                    accts
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect();
        let moved = w.received.load(Relaxed) - before;
        let waiting = waiting.load(Relaxed);
        println!("{what} round {round}: {moved} received, {waiting} waiting, {}", watch_hub(w, &None));
        if moved == 0 && waiting == 0 && round >= 1 {
            // A payload from a device under another's number whose roster was asked a moment ago is received again
            // a minute later (spec 4.6): once more after that, then done.
            if waited {
                break;
            }
            waited = true;
            std::thread::sleep(Duration::from_secs(65));
            continue;
        }
        waited = false;
        if waiting > 0 && moved == 0 {
            std::thread::sleep(Duration::from_secs(5));
        }
    }
    for a in all.iter_mut().flatten() {
        evict(w, a);
    }
    all
}

/// Between friends, a message each way: no session may stay wedged after what happened to it.
fn probe(w: &Arc<World>, all: Vec<Vec<Acct>>) -> Vec<Vec<Acct>> {
    all.into_iter()
        .enumerate()
        .map(|(t, mut accts)| {
            let w = w.clone();
            std::thread::spawn(move || {
                let mut rng = Rng(w.args.seed ^ 0x9909 ^ t as u64);
                for a in accts.iter_mut().filter(|a| a.alive) {
                    if !activate(&w, a, &mut rng) {
                        continue;
                    }
                    let contacts = a.client.as_ref().unwrap().contacts();
                    let mut friends: Vec<_> =
                        contacts.into_iter().filter(|c| !c.blocked && !c.gone && w.alive(&c.user)).collect();
                    friends.truncate(4);
                    for c in friends {
                        let blocked = w.ledger().blocks.contains(&(w.uid(&c.user), a.uid));
                        if blocked {
                            continue;
                        }
                        let (key, tag, alive) = new_msg(&w, a, &c.user, None);
                        let result = definite(|| a.client.as_mut().unwrap().send(&c.user, Content::Text(tag.clone())));
                        // The probe must arrive whatever the first try said: it waits in the outbox otherwise.
                        if let Some(m) = w.ledger().msgs.get_mut(&key) {
                            m.must = alive;
                        }
                        match result {
                            Ok(events) => take_events(&w, a, events, &mut rng),
                            Err(error) => note_error(&w, a, "probe", &error),
                        }
                        w.stats.count("probe sent");
                    }
                    evict(&w, a);
                }
                accts
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect()
}

fn check_ledger(w: &World, all: &[Vec<Acct>]) -> serde_json::Value {
    let mut rng = Rng(w.args.seed ^ 0x77);
    let mut outbox = 0;
    let mut opened = 0;
    for a in all.iter().flatten().filter(|a| a.alive) {
        let Ok(blob) = std::fs::read(state_path(w, a.idx)) else {
            w.stats.violation(format!("{}: no state on disk at the end", a.name));
            continue;
        };
        let proc_ = Proc::new(&w.net, &w.stats, &a.ip, 0, rng.next_u64(), Vec::new());
        match Client::restore(proc_, &blob, &STATE_KEY) {
            Ok(c) => {
                opened += 1;
                if c.pending() > 0 {
                    outbox += 1;
                    if outbox <= 20 {
                        w.stats.violation(format!("{} still has {} envelopes in its outbox", a.name, c.pending()));
                        w.dump(a.uid);
                    }
                }
            }
            Err(error) => w.stats.violation(format!("{}: the saved state does not open: {error}", a.name)),
        }
    }
    let ledger = w.ledger();
    let (mut must, mut lost, mut may, mut may_got, mut dup) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for (key, m) in &ledger.msgs {
        if m.got > 1 {
            dup += 1;
        }
        if m.must {
            must += 1;
            if m.got == 0 {
                lost += 1;
                if lost <= 30 {
                    println!(
                        "VIOLATION: {}:{}:{}:{} from {} to {} never arrived (sent at {} ms)",
                        w.run,
                        key >> 40,
                        (key >> 28) & 0xFFF,
                        key & 0x0FFF_FFFF,
                        w.user_of(m.from),
                        w.user_of(m.to),
                        m.sent_ms
                    );
                }
                if lost <= 5 {
                    w.dump(m.from);
                    w.dump(m.to);
                }
            }
        } else {
            may += 1;
            if m.got > 0 {
                may_got += 1;
            }
        }
    }
    drop(ledger);
    println!(
        "ledger: {must} must arrive ({lost} lost), {may} may be lost ({may_got} arrived anyway), {dup} shown twice"
    );
    if lost > 0 {
        w.stats.violations.fetch_add(lost as usize, Relaxed);
    }
    serde_json::json!({
        "must_arrive": must, "lost": lost, "may_be_lost": may, "arrived_anyway": may_got, "shown_twice": dup,
        "states_opened": opened, "outboxes_not_empty": outbox,
    })
}

/// The hub opened in this process on its database after the last kill: its audit, then a month later every blob
/// that waited is gone, with its file.
fn audit(w: &World) -> serde_json::Value {
    kill_hub(w, false);
    let addr = lock(&w.hub).addr.clone();
    let config = hub_config(&w.args);
    let hub = TestHub::start_with(config, &addr);
    let mut failed: Vec<String> = Vec::new();
    match hub.hub.audit() {
        Ok(f) => failed.extend(f.into_iter().map(|(what, n)| format!("{n} {what}"))),
        Err(error) => failed.push(format!("audit: {error}")),
    }
    let blobs = std::path::PathBuf::from(format!("{}/blobs", w.args.dir));
    let before = dir_bytes(&blobs);
    let db = rusqlite::Connection::open(w.args.db()).unwrap();
    let rows = |sql: &str| db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(-1);
    let waiting_blobs = rows("SELECT count(*) FROM blobs");
    let waiting_envelopes = rows("SELECT count(*) FROM messages");
    let drained = rows(
        "SELECT count(*) FROM devices d WHERE NOT EXISTS (SELECT 1 FROM prekeys p WHERE p.device_id = d.id AND p.kind = 3)",
    );
    // A blob uploaded now and never fetched: a month later it is gone.
    let probe = blob_probe(w);
    hub.hub.prune(yo_hub::now_ms() + 31 * 24 * 3600 * 1000).unwrap();
    if let Some(id) = probe {
        let status = reqwest::blocking::Client::new()
            .get(format!("http://{addr}{}", media::blob_path(&id)))
            .header("cf-connecting-ip", "10.200.0.1")
            .send()
            .map(|r| r.status().as_u16());
        if status.as_ref().ok() != Some(&404) {
            failed.push(format!("a blob past its time answers {status:?}"));
        }
    }
    // Uploads a kill cut off stay in `tmp/` for an hour of real time (then the hourly sweep takes them): not counted.
    let tmp = dir_bytes(&blobs.join("tmp"));
    let after = dir_bytes(&blobs);
    let after = (after.0 - tmp.0, after.1 - tmp.1);
    let blobs_left = rows("SELECT count(*) FROM blobs");
    if blobs_left != 0 {
        failed.push(format!("{blobs_left} blobs past their time"));
    }
    // Upload directories may stay; files may not.
    if after.1 > 0 {
        failed.push(format!("{} blob files left after the month ({} bytes)", after.1, after.0));
    }
    if let Ok(f) = hub.hub.audit() {
        failed.extend(f.into_iter().map(|(what, n)| format!("after a month: {n} {what}")));
    }
    for f in &failed {
        w.stats.violation(format!("audit: {f}"));
    }
    if failed.is_empty() {
        println!("audit: the hub's database is consistent, a month later no blob is left");
    }
    hub.stop();
    serde_json::json!({
        "failed": failed,
        "blobs_waiting_at_end": waiting_blobs,
        "blob_bytes_before_expiry": before.0,
        "blob_files_before_expiry": before.1,
        "cut_uploads_in_tmp": tmp.1,
        "envelopes_waiting_at_end": waiting_envelopes,
        "devices_without_one_time_kyber_at_end": drained,
    })
}

/// A new account uploads a blob nobody fetches: for the expiry.
fn blob_probe(w: &World) -> Option<[u8; 16]> {
    let proc_ = Proc::new(&w.net, &w.stats, "10.200.0.1", 0, 5, Vec::new());
    let mut a = register(w, &format!("Expiry{}", w.run), None, &proc_).ok()?;
    let (m, sealed) = media::seal(&[5; 300_000], 10, 10).ok()?;
    a.upload(&m, 1).ok()?.send(&sealed).ok()?;
    Some(m.id)
}

/// Friends: a small world. Neighbours on a ring mostly, some far, and a few everybody knows.
fn graph(n: usize, rng: &mut Rng) -> Vec<Vec<usize>> {
    let mut friends: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    let celebs: Vec<usize> = (0..n).filter(|i| i % 300 == 11).collect();
    for i in 0..n {
        let u = (rng.below(1000) as f64) / 1000.0;
        let degree = 5 + (45.0 * u * u) as usize;
        let mut k = 1;
        while friends[i].len() < degree && k < 4 * degree + 8 {
            let j = if rng.below(100) < 15 {
                if !celebs.is_empty() && rng.below(4) == 0 {
                    celebs[rng.below(celebs.len() as u64) as usize]
                } else {
                    rng.below(n as u64) as usize
                }
            } else {
                (i + k.div_ceil(2) * if k % 2 == 0 { n - 1 } else { 1 }) % n
            };
            k += 1;
            if j != i && (friends[j].len() < 60 || celebs.contains(&j)) {
                friends[i].insert(j);
                friends[j].insert(i);
            }
        }
    }
    friends.into_iter().map(|f| f.into_iter().collect()).collect()
}

fn main() {
    // Photos and videos are buffers of megabytes on sixteen threads: given back to the system when freed (mmap), not
    // kept in per-thread arenas that only grow.
    // SAFETY: mallopt only sets the allocator's parameters, before any thread is started.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 512 * 1024);
        libc::mallopt(libc::M_ARENA_MAX, 4);
    }
    let args = args();
    if let Some(addr) = args.serve.clone() {
        serve(&args, &addr);
    }
    let started = Instant::now();
    let run: String = {
        let mut r = Rng(args.seed + 1);
        (0..3).map(|_| (b'a' + r.below(26) as u8) as char).collect()
    };
    let _ = std::fs::remove_dir_all(&args.dir);
    if let Some(db) = &args.db_dir {
        let _ = std::fs::remove_dir_all(db);
        std::fs::create_dir_all(db).unwrap();
    }
    let states = std::path::PathBuf::from(format!("{}/states", args.dir));
    std::fs::create_dir_all(&states).unwrap();
    println!(
        "yo-scale: seed {} (run {run}), {} accounts, {} messages, {} threads, {} hot each, faults {}‰, crashes {}‰, \
         {} kills, media {}‰, inbox {}, in {}",
        args.seed,
        args.accounts,
        args.messages,
        args.threads,
        args.hot,
        args.faults,
        args.crashes,
        args.kills,
        args.media,
        args.inbox,
        args.dir
    );
    let addr = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().to_string()
    };
    let child = spawn_hub(&args, &addr);
    println!("hub: pid {} on {addr}", child.id());
    let net = Arc::new(Net {
        http: reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .pool_max_idle_per_host(512)
            .user_agent("yo-scale")
            .build()
            .unwrap(),
        base: format!("http://{addr}"),
        addr: addr.clone(),
    });
    let threads = args.threads;
    let n = args.accounts;
    let w = Arc::new(World {
        net,
        stats: Arc::new(Stats::default()),
        ledger: Mutex::default(),
        groups: Mutex::default(),
        users: RwLock::default(),
        names: RwLock::default(),
        dir: RwLock::new(Vec::new()),
        by_user: RwLock::default(),
        hot: (0..n).map(|_| AtomicBool::new(false)).collect(),
        run,
        started,
        sent: AtomicU64::new(0),
        received: AtomicU64::new(0),
        stop: AtomicBool::new(false),
        low_memory: AtomicBool::new(false),
        send_latency: Hist::new(),
        delivery: Hist::new(),
        delivery_hot: Hist::new(),
        traces: Mutex::default(),
        trace_file: args.trace_file.as_ref().map(|path| {
            Arc::new(Mutex::new(std::io::BufWriter::new(std::fs::File::create(path).expect("trace file"))))
        }),
        progress: (0..threads).map(|_| AtomicU64::new(yo_hub::now_ms())).collect(),
        states,
        hub: Mutex::new(HubProc {
            child: Some(child),
            addr,
            kills: 0,
            rss_max: 0,
            wal_max: 0,
            db_max: 0,
            blobs_max: 0,
            pending_max: 0,
            drained_max: 0,
        }),
        args,
    });

    // Registration, everyone at once.
    let t0 = Instant::now();
    let mut all: Vec<Vec<Acct>> = (0..threads)
        .map(|t| {
            let w = w.clone();
            std::thread::spawn(move || {
                let mut rng = Rng(w.args.seed ^ (0x5151 + t as u64));
                (t..n).step_by(threads).filter_map(|i| new_acct(&w, i, &mut rng)).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    let registered: usize = all.iter().map(Vec::len).sum();
    let registration_s = t0.elapsed().as_secs_f64();
    println!("registered {registered} in {registration_s:.1} s ({:.0}/s)", registered as f64 / registration_s);
    {
        let mut dir: Vec<Entry> =
            (0..n).map(|_| Entry { name: "Nobody".into(), user: "00".into(), alive: false, reused: false }).collect();
        let mut by_user = HashMap::new();
        for a in all.iter().flatten() {
            dir[a.idx] = Entry { name: a.name.clone(), user: a.user.clone(), alive: true, reused: false };
            by_user.insert(a.user.clone(), a.idx);
        }
        *w.dir.write().unwrap() = dir;
        *w.by_user.write().unwrap() = by_user;
        let mut rng = Rng(w.args.seed ^ 0x6A6A);
        let friends = graph(n, &mut rng);
        let degrees: Vec<usize> = friends.iter().map(Vec::len).collect();
        println!(
            "friends: {} edges, degree {}–{}, celebrities {}",
            degrees.iter().sum::<usize>() / 2,
            degrees.iter().min().unwrap_or(&0),
            degrees.iter().max().unwrap_or(&0),
            (0..n).filter(|i| i % 300 == 11).count()
        );
        for a in all.iter_mut().flatten() {
            a.friends = friends[a.idx].clone();
        }
    }

    // Watchdog and progress.
    {
        let w = w.clone();
        std::thread::spawn(move || {
            let db =
                rusqlite::Connection::open_with_flags(w.args.db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok();
            let mut last = (Instant::now(), 0u64, 0u64);
            loop {
                std::thread::sleep(Duration::from_secs(20));
                let (sent, received) = (w.sent.load(Relaxed), w.received.load(Relaxed));
                let dt = last.0.elapsed().as_secs_f64();
                println!(
                    "t+{} s: sent {sent} ({:.0}/s), received {received} ({:.0}/s), violations {}, send p50 {:.1} \
                     p99 {:.1} ms, delivery (running) p50 {:.0} p99 {:.0} ms, rss {} MB, free {} MB; {}",
                    w.started.elapsed().as_secs(),
                    (sent - last.1) as f64 / dt,
                    (received - last.2) as f64 / dt,
                    w.stats.violations.load(Relaxed),
                    w.send_latency.percentile_ms(0.5),
                    w.send_latency.percentile_ms(0.99),
                    w.delivery_hot.percentile_ms(0.5),
                    w.delivery_hot.percentile_ms(0.99),
                    rss_mb(None),
                    free_mb(),
                    watch_hub(&w, &db)
                );
                if free_mb() < w.args.min_free_mb && !w.low_memory.swap(true, Relaxed) {
                    println!("LOW MEMORY: {} MB free, the chaos ends at {sent} sent", free_mb());
                }
                last = (Instant::now(), sent, received);
                let now = yo_hub::now_ms();
                for (t, p) in w.progress.iter().enumerate() {
                    let idle = now.saturating_sub(p.load(Relaxed));
                    if idle > 600_000 && !w.done() {
                        println!("VIOLATION: worker {t} has not moved for {} s: a hang", idle / 1000);
                    }
                }
            }
        });
    }

    // The chaos, with the hub killed meanwhile.
    let t1 = Instant::now();
    let killer = {
        let w = w.clone();
        std::thread::spawn(move || {
            let kills = w.args.kills as u64;
            for k in 0..kills {
                let at = w.args.messages * (k + 1) / (kills + 1);
                while w.sent.load(Relaxed) < at && !w.done() {
                    std::thread::sleep(Duration::from_millis(100));
                }
                if w.done() {
                    break;
                }
                kill_hub(&w, true);
                println!("hub killed and started again at {} sent", w.sent.load(Relaxed));
            }
        })
    };
    all = all
        .into_iter()
        .enumerate()
        .map(|(t, accts)| {
            let w = w.clone();
            std::thread::spawn(move || worker(w, t, accts))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    w.stop.store(true, Relaxed);
    killer.join().unwrap();
    let chaos_s = t1.elapsed().as_secs_f64();
    let (sent, received) = (w.sent.load(Relaxed), w.received.load(Relaxed));
    println!(
        "chaos: {chaos_s:.0} s, {sent} sent ({:.0}/s), {received} received ({:.0}/s)",
        sent as f64 / chaos_s,
        received as f64 / chaos_s
    );
    for p in &w.progress {
        p.store(u64::MAX / 2, Relaxed);
    }
    // Once more, right as everyone comes back.
    kill_hub(&w, true);

    let t2 = Instant::now();
    all = quiesce(&w, all, "catching up");
    all = probe(&w, all);
    all = quiesce(&w, all, "after the probes");
    let quiet_s = t2.elapsed().as_secs_f64();
    if let Some(file) = &w.trace_file {
        let _ = lock(file).flush();
    }
    let ledger = check_ledger(&w, &all);
    let groups = check_groups(&w);
    let audit = audit(&w);

    let hub = lock(&w.hub);
    let bounded = hub.rss_max < 4096 && hub.wal_max < 2048;
    if !bounded {
        w.stats.violation(format!("the hub grew: rss {} MB, wal {} MB at most", hub.rss_max, hub.wal_max));
    }
    let counts = lock(&w.stats.counts).clone();
    let mut sorted: Vec<_> = counts.iter().collect();
    sorted.sort();
    for (what, n) in &sorted {
        println!("  {what}: {n}");
    }
    let violations = w.stats.violations.load(Relaxed);
    let result = serde_json::json!({
        "seed": w.args.seed,
        "accounts": registered,
        "threads": threads,
        "registration": { "seconds": registration_s, "per_s": registered as f64 / registration_s },
        "chaos": { "seconds": chaos_s, "sent": sent, "received": received, "sent_per_s": sent as f64 / chaos_s },
        "catch_up_seconds": quiet_s,
        "chaos_ended_for_low_memory": w.low_memory.load(Relaxed),
        "send_call": w.send_latency.json(),
        "delivery_to_running": w.delivery_hot.json(),
        "delivery_all": w.delivery.json(),
        "hub": { "kills": hub.kills, "rss_max_mb": hub.rss_max, "wal_max_mb": hub.wal_max, "db_max_mb": hub.db_max,
                 "blobs_max_mb": hub.blobs_max, "envelopes_waiting_max": hub.pending_max,
                 "devices_without_one_time_kyber_max": hub.drained_max },
        "ledger": ledger,
        "groups": groups,
        "audit": audit,
        "violations": violations,
        "counts": counts,
        "rss_mb": rss_mb(None),
        "seconds": started.elapsed().as_secs_f64(),
    });
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
    if let Some(out) = &w.args.out {
        std::fs::write(out, serde_json::to_string_pretty(&result).unwrap() + "\n").unwrap();
    }
    println!("done in {:.0} s: {violations} violations, seed {}", started.elapsed().as_secs_f64(), w.args.seed);
    std::process::exit(if violations > 0 { 1 } else { 0 });
}

// ---- groups ----

/// What one member sent to one other in one group, and what arrived: group Yos carry no text, so they are counted.
#[derive(Default, Clone, Copy)]
struct GroupCount {
    /// Sent for sure (the send answered, the sender's view had the receiver in it).
    sure: u32,
    /// May have gone (the send failed after it may have been queued; or before a new phone).
    maybe: u32,
    got: u32,
}

#[derive(Default)]
struct GroupLedger {
    ids: Interner,
    counts: HashMap<(u32, u32, u32), GroupCount>,
    by_to: HashMap<u32, Vec<(u32, u32, u32)>>,
    by_from: HashMap<u32, Vec<(u32, u32, u32)>>,
    /// Members added or removed (or who left) at some point: what concerns them is counted at most, not at least,
    /// as views of the group differ while a change travels and of two changes at once one wins.
    touched: HashMap<u32, HashSet<u32>>,
    /// The last `sent_at` shown, per (group, sender, receiver).
    last_at: HashMap<(u32, u32, u32), u64>,
    blocked_ever: HashSet<(u32, u32)>,
    /// Users who had a new phone: Yos of the old one may still be shown after the new one's (by members who never had
    /// the sender's devices), and order holds per device only.
    new_phones: HashSet<u32>,
}

impl GroupLedger {
    fn count(&mut self, key: (u32, u32, u32)) -> &mut GroupCount {
        if !self.counts.contains_key(&key) {
            self.by_from.entry(key.1).or_default().push(key);
            self.by_to.entry(key.2).or_default().push(key);
        }
        self.counts.entry(key).or_default()
    }

    /// What `user` was sent (`to`) or sent until now becomes "may have".
    fn uncertain(&mut self, user: u32, to: bool) {
        let keys = if to { self.by_to.get(&user) } else { self.by_from.get(&user) }.cloned().unwrap_or_default();
        for key in keys {
            if let Some(c) = self.counts.get_mut(&key) {
                // A new phone may show again what the old one showed.
                c.maybe += c.sure + if to { c.got } else { 0 };
                c.sure = 0;
            }
        }
        if !to {
            self.new_phones.insert(user);
        }
        // A new phone's clock and messages start anew.
        self.last_at.retain(|(_, f, r), _| if to { *r != user } else { *f != user });
    }
}

fn group_view(a: &Acct, id: &str) -> Option<yo_core::groups::GroupView> {
    a.client.as_ref()?.groups().into_iter().find(|g| g.id == id)
}

/// A group Yo arrived: from a member, to a member as the sender had the group, once, in order.
fn group_received(w: &World, a: &mut Acct, group: &str, from: &str, sent_at: u64, content: Content, rng: &mut Rng) {
    if a.blocked.contains(from) {
        w.stats.violation(format!("{} was shown a group Yo from {from}, whom it blocks", a.user));
    }
    let photo = match &content {
        Content::Photo(m) | Content::Video { media: m, .. } => Some(m.clone()),
        _ => None,
    };
    let (from_uid, me) = (w.uid(from), a.uid);
    let mut groups = lock(&w.groups);
    let g = groups.ids.id(group);
    let key = (g, from_uid, me);
    let Some(c) = groups.counts.get(&key).copied() else {
        // Added by a change that came in with the very send: recorded once the send returns.
        let changed = groups.touched.get(&g).is_some_and(|t| t.contains(&me));
        drop(groups);
        if changed {
            w.stats.count("group Yo to a member added meanwhile");
        } else {
            w.stats.violation(format!("{} was shown a group Yo from {from} it was never sent", a.user));
        }
        return;
    };
    if c.got + 1 > c.sure + c.maybe {
        drop(groups);
        w.stats.violation(format!("{} was shown more group Yos from {from} than were sent ({})", a.user, c.got + 1));
        return;
    }
    groups.counts.get_mut(&key).unwrap().got += 1;
    let per_device = groups.new_phones.contains(&from_uid);
    let last = groups.last_at.entry(key).or_default();
    if sent_at <= *last && !per_device {
        let was = *last;
        drop(groups);
        w.stats.violation(format!("{}: a group Yo from {from} of {sent_at} after one of {was}", a.user));
    } else {
        *last = sent_at;
        drop(groups);
    }
    w.stats.count("group Yo received");
    w.trace(a.uid, format!("group Yo in {group} from {from}"));
    if let Some(m) = photo {
        let info = w.ledger().media.get(&m.id).copied();
        group_fetch(w, a, &m, info, rng);
    }
}

/// A group photo: fetched once by each member device, cut off now and then (the count comes back).
fn group_fetch(w: &World, a: &Acct, m: &yo_core::Media, info: Option<MediaInfo>, rng: &mut Rng) {
    let clean = Proc::new(&w.net, &w.stats, &a.ip, 0, rng.next_u64(), Vec::new());
    let cut = rng.below(10) == 0;
    if cut {
        w.net.cut_fetch(&media::blob_path(&m.id), &a.ip);
    }
    match definite(|| media::fetch_open(clean.as_ref(), m)) {
        Ok(Some(plain)) if info.map(|i| i.size) == Some(plain.len()) => w.stats.count("group photo opened"),
        Ok(Some(plain)) => w.stats.violation(format!("{}: group photo of {} bytes", a.name, plain.len())),
        Ok(None) if cut => w.stats.count("group photo gone: the cut fetch had it all handed on"),
        Ok(None) if info.is_some_and(|i| !w.alive(&w.user_of(i.from))) => {
            w.stats.count("group photo gone with its sender")
        }
        // Its fetches are the member devices the sender knew; a member's new phone, or one the sender did not know
        // yet, finds them spent.
        Ok(None) => w.stats.count("group photo: its fetches spent"),
        Err(error) => w.stats.violation(format!("{}: group photo: {error}", a.name)),
    }
}

/// Before a send to a group: each member as the sender has it now may get one more (the receivers may show it
/// before the send returns). Returns the members so reserved.
fn group_reserve(w: &World, a: &Acct, id: &str) -> Vec<u32> {
    let Some(view) = group_view(a, id) else { return Vec::new() };
    let mut groups = lock(&w.groups);
    let g = groups.ids.id(id);
    let members: Vec<u32> = view.members.iter().filter(|m| **m != a.user).map(|m| w.uid(m)).collect();
    for &to in &members {
        groups.count((g, a.uid, to)).maybe += 1;
    }
    members
}

/// After it: the members as the sender had the group for the send (a change may have come in with it) got it for
/// sure if the send answered.
fn group_sent(w: &World, a: &Acct, id: &str, reserved: &[u32], sure: bool) {
    let after: Vec<u32> = group_view(a, id)
        .map(|v| v.members.iter().filter(|m| **m != a.user).map(|m| w.uid(m)).collect())
        .unwrap_or_default();
    let mut groups = lock(&w.groups);
    let g = groups.ids.id(id);
    for to in after.iter().chain(reserved).copied().collect::<BTreeSet<u32>>() {
        let (was, now) = (reserved.contains(&to), after.contains(&to));
        let c = groups.count((g, a.uid, to));
        match (was, now) {
            (true, true) if sure => {
                c.maybe -= 1;
                c.sure += 1;
            }
            (false, true) => {
                if sure {
                    c.sure += 1;
                } else {
                    c.maybe += 1;
                }
            }
            _ => {}
        }
    }
}

fn op_group(w: &World, a: &mut Acct, rng: &mut Rng) {
    let Some(client) = a.client.as_ref() else { return };
    let mine: Vec<_> = client.groups().into_iter().filter(|g| !g.left && g.revision > 0).collect();
    if mine.is_empty() || (mine.len() < 3 && rng.below(10) == 0) {
        return group_create(w, a, rng);
    }
    let g = mine[rng.below(mine.len() as u64) as usize].clone();
    let me = a.uid;
    match rng.below(100) {
        0..=69 => {
            for _ in 0..1 + rng.below(4) {
                group_yo(w, a, &g.id, rng);
                if a.client.is_none() || a.proc_.crashed.load(Relaxed) {
                    break;
                }
            }
        }
        70..=74 => group_photo(w, a, &g.id, rng),
        75..=82 if g.members.len() < 20 => {
            let Some(peer) = pick_peer(w, a, rng) else { return };
            if g.members.contains(&peer.user) || !w.alive(&peer.user) {
                return;
            }
            touch(w, &g.id, &[w.uid(&peer.user)]);
            let result = a.client.as_mut().unwrap().add_to_group(&g.id, std::slice::from_ref(&peer.user));
            group_result(w, a, "add to group", result, rng);
        }
        83..=90 if g.members.len() > 2 => {
            let others: Vec<_> = g.members.iter().filter(|m| **m != a.user).cloned().collect();
            let who = others[rng.below(others.len() as u64) as usize].clone();
            touch(w, &g.id, &[w.uid(&who)]);
            let result = a.client.as_mut().unwrap().remove_from_group(&g.id, std::slice::from_ref(&who));
            group_result(w, a, "remove from group", result, rng);
        }
        91..=93 => {
            touch(w, &g.id, &[me]);
            let result = a.client.as_mut().unwrap().leave_group(&g.id);
            group_result(w, a, "leave group", result, rng);
        }
        _ => {
            let result = a.client.as_mut().unwrap().rename_group(&g.id, &format!("G{}", rng.below(1000)));
            group_result(w, a, "rename group", result, rng);
        }
    }
}

fn touch(w: &World, id: &str, users: &[u32]) {
    let mut groups = lock(&w.groups);
    let g = groups.ids.id(id);
    groups.touched.entry(g).or_default().extend(users);
}

fn group_result(w: &World, a: &mut Acct, what: &str, result: Result<Vec<Event>, Error>, rng: &mut Rng) {
    match result {
        Ok(events) if !a.proc_.crashed.load(Relaxed) => {
            w.stats.count(what);
            take_events(w, a, events, rng);
        }
        Ok(_) => {}
        Err(Error::Protocol("not in this group")) => w.stats.count(format!("{what}: not in it any more")),
        Err(error) => note_error(w, a, what, &error),
    }
}

fn group_create(w: &World, a: &mut Acct, rng: &mut Rng) {
    let size = 1 + rng.below(19) as usize;
    let mut members = Vec::new();
    for _ in 0..size * 2 {
        if members.len() >= size {
            break;
        }
        let Some(peer) = pick_peer(w, a, rng) else { continue };
        if w.alive(&peer.user) && !members.contains(&peer.user) && !a.blocked.contains(&peer.user) {
            members.push(peer.user);
        }
    }
    if members.is_empty() {
        return;
    }
    let result = a.client.as_mut().unwrap().create_group(&format!("Chaos {}", rng.below(1000)), &members);
    match result {
        Ok((id, events)) if !a.proc_.crashed.load(Relaxed) => {
            w.stats.count("group created");
            w.trace(a.uid, format!("created group {id} of {}", members.len() + 1));
            lock(&w.groups).ids.id(&id);
            take_events(w, a, events, rng);
        }
        Ok(_) => {}
        Err(error) => note_error(w, a, "create group", &error),
    }
}

fn group_yo(w: &World, a: &mut Acct, id: &str, rng: &mut Rng) {
    let reserved = group_reserve(w, a, id);
    let t = Instant::now();
    let result = a.client.as_mut().unwrap().send_group(id, Content::Yo);
    w.send_latency.add(t.elapsed().as_micros() as u64);
    w.stats.count("group Yo sent");
    let ok = result.is_ok() && !a.proc_.crashed.load(Relaxed);
    w.trace(a.uid, format!("group Yo to {id}: {:?}", result.as_ref().map(|e| e.len())));
    group_sent(w, a, id, &reserved, ok);
    match result {
        Ok(events) if ok => take_events(w, a, events, rng),
        Ok(_) => {}
        // Left meanwhile (a change taken in by the send itself).
        Err(Error::Protocol("not in this group")) => w.stats.count("group Yo: not in it any more"),
        Err(error) => note_error(w, a, "group Yo", &error),
    }
}

fn group_photo(w: &World, a: &mut Acct, id: &str, rng: &mut Rng) {
    if !a.group_uploads.is_empty() {
        return op_group_uploads(w, a, rng);
    }
    let video = rng.below(4) == 0;
    let size = if video { 1_000_000 + rng.below(7_000_000) } else { 200_000 + rng.below(5_800_000) } as usize;
    let plain: Vec<u8> = (0..size).map(|i| (i as u64 ^ a.sent as u64) as u8).collect();
    let Ok((m, sealed)) = media::seal(&plain, 1080, 1920) else { return };
    w.ledger().media.insert(m.id, MediaInfo { key: 1 << 63, size, uploaded: false, from: a.uid });
    w.stats.count("group photo sealed");
    a.group_uploads.push((id.to_owned(), m, sealed, video));
    op_group_uploads(w, a, rng);
}

fn op_group_uploads(w: &World, a: &mut Acct, rng: &mut Rng) {
    let mut keep = Vec::new();
    for (id, m, sealed, video) in std::mem::take(&mut a.group_uploads) {
        if a.client.is_none() || a.proc_.crashed.load(Relaxed) || group_view(a, &id).is_none_or(|g| g.left) {
            continue;
        }
        let fetches = match a.client.as_mut().unwrap().open_group(&id) {
            Ok((n, events)) => {
                take_events(w, a, events, rng);
                n
            }
            Err(error) => {
                note_error(w, a, "open group", &error);
                keep.push((id, m, sealed, video));
                continue;
            }
        };
        let Some(client) = a.client.as_mut() else { continue };
        match client.upload(&m, fetches).and_then(|upload| upload.send(&sealed)) {
            Ok(()) => {}
            Err(error) if media::retry_later(&error) => {
                w.stats.count(format!("group upload later: {}", short(&error)));
                keep.push((id, m, sealed, video));
                continue;
            }
            Err(error) => {
                note_error(w, a, "group upload", &error);
                continue;
            }
        }
        if let Some(info) = w.ledger().media.get_mut(&m.id) {
            info.uploaded = true;
        }
        w.stats.count(format!("group photo uploaded for {} fetches", if fetches > 4 { "5+" } else { "1-4" }));
        let content = if video { Content::Video { media: m, duration_ms: 9_000 } } else { Content::Photo(m) };
        let reserved = group_reserve(w, a, &id);
        let result = a.client.as_mut().unwrap().send_group(&id, content);
        let ok = result.is_ok() && !a.proc_.crashed.load(Relaxed);
        group_sent(w, a, &id, &reserved, ok);
        match result {
            Ok(events) if ok => take_events(w, a, events, rng),
            Ok(_) => {}
            Err(Error::Protocol("not in this group")) => {}
            Err(error) => note_error(w, a, "group photo", &error),
        }
    }
    a.group_uploads.extend(keep);
}

/// Every group Yo to a member as its sender had the group arrived, unless a change, a block, a new phone or a
/// deletion concerned them.
fn check_groups(w: &World) -> serde_json::Value {
    let groups = lock(&w.groups);
    let (mut checked, mut lost, mut loose, mut sent, mut got) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for (&(g, from, to), c) in &groups.counts {
        sent += (c.sure + c.maybe) as u64;
        got += c.got as u64;
        let touched = groups.touched.get(&g).is_some_and(|t| t.contains(&from) || t.contains(&to));
        let alive = w.alive(&w.user_of(from)) && w.alive(&w.user_of(to));
        let blocked = groups.blocked_ever.contains(&(to, from)) || groups.blocked_ever.contains(&(from, to));
        // A member's new phone is a device the others may not know: a member who never wrote to them asks the hub at
        // most 10 times a day (recovery lookups) and keeps the rest for later (tests/CHAOS.md).
        let new_phone = groups.new_phones.contains(&from);
        if touched || !alive || blocked || new_phone {
            loose += 1;
            continue;
        }
        checked += 1;
        if c.got < c.sure {
            lost += (c.sure - c.got) as u64;
            if lost <= 20 {
                println!(
                    "VIOLATION: group {}: {} of {} group Yos from {} to {} never arrived",
                    groups.ids.all[g as usize - 1],
                    c.sure - c.got,
                    c.sure,
                    w.user_of(from),
                    w.user_of(to)
                );
            }
        }
    }
    println!(
        "groups: {} groups, {sent} group Yos to members ({got} shown), {checked} member pairs checked ({lost} \
         lost), {loose} with a change, block or deletion",
        groups.ids.all.len()
    );
    w.stats.violations.fetch_add(lost as usize, Relaxed);
    serde_json::json!({ "groups": groups.ids.all.len(), "yos_to_members": sent, "shown": got,
                        "pairs_checked": checked, "lost": lost, "pairs_loose": loose })
}
