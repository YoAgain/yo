//! yo-chaos: thousands of accounts through the real core, with faults, crashes and hub restarts; every message
//! accounted for.
//!
//!   yo-chaos [--seed n] [--accounts 2000] [--threads 16] [--seconds 120] [--faults 20] [--crashes 20]
//!            [--restarts 3] [--url https://…] [--db path]
//!
//! Without `--url` the hub runs in-process on a SQLite file, so it can be restarted on the same database and audited
//! at the end, and time can be jumped (the 365-day release). With `--url` it runs against that hub as it is (its
//! limits, its clock): no restarts, no audit, no time travel.
//!
//! Every account is owned by one worker thread, which runs random operations on it (seeded: `--seed` replays the
//! choices; threads interleave as they will). Messages are texts that name themselves, so the ledger knows every
//! message sent and checks every one received: from whom, to whom, once, in order. What a fault may legitimately
//! lose (a recipient replaced or deleted, a block, a send that crashed) is marked as such when it happens; the rest
//! must arrive. Failures are printed as `VIOLATION` and make the exit status 1.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};
use yo_core::{media, Backup, Client, Content, Error, Event, Push, Response, Store, Transport};
use yo_hub::config::{Config, Limits};
use yo_tests::{Rng, TestHub};

// ---- arguments ----

struct Args {
    seed: u64,
    accounts: usize,
    threads: usize,
    seconds: u64,
    /// Faults per thousand requests.
    faults: u64,
    /// Crashes per thousand operations.
    crashes: u64,
    restarts: usize,
    url: Option<String>,
    db: Option<String>,
    limits: bool,
}

fn args() -> Args {
    let mut args = Args {
        seed: yo_tests::random_u64() % 1_000_000,
        accounts: 2000,
        threads: 16,
        seconds: 120,
        faults: 20,
        crashes: 20,
        restarts: 3,
        url: None,
        db: None,
        limits: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        if flag == "--limits" {
            args.limits = true;
            continue;
        }
        let value = it.next().unwrap_or_default();
        let number = || value.parse::<u64>().unwrap_or_else(|_| panic!("{flag}: a number"));
        match flag.as_str() {
            "--seed" => args.seed = number(),
            "--accounts" => args.accounts = number() as usize,
            "--threads" => args.threads = number().max(1) as usize,
            "--seconds" => args.seconds = number(),
            "--faults" => args.faults = number(),
            "--crashes" => args.crashes = number(),
            "--restarts" => args.restarts = number() as usize,
            "--url" => args.url = Some(value),
            "--db" => args.db = Some(value),
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
        *self.counts.lock().unwrap_or_else(PoisonError::into_inner).entry(what.into()).or_default() += 1;
    }

    fn violation(&self, what: String) {
        // Counted by kind too (words with digits blanked), so what was not printed still shows in the summary.
        let kind: Vec<&str> = what
            .split_whitespace()
            .take(10)
            .map(|w| if w.chars().any(|c| c.is_ascii_digit()) { "#" } else { w })
            .collect();
        self.count(format!("VIOLATION {}", kind.join(" ")));
        if self.violations.fetch_add(1, Relaxed) < 200 {
            println!("VIOLATION: {what}");
        }
    }

    fn get(&self, what: &str) -> u64 {
        self.counts.lock().unwrap_or_else(PoisonError::into_inner).get(what).copied().unwrap_or(0)
    }
}

fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).map(str::to_owned))
        .and_then(|l| l.split_whitespace().nth(1).and_then(|kb| kb.parse::<u64>().ok()))
        .map_or(0, |kb| kb / 1024)
}

// ---- the network, with faults ----

struct Net {
    http: reqwest::blocking::Client,
    base: String,
}

impl Net {
    fn send(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        let mut request = self
            .http
            .request(method.parse().map_err(|_| "method")?, format!("{}{path}", self.base))
            .body(body.to_vec());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        Ok(Response { status, body: response.bytes().map_err(|e| e.to_string())?.to_vec() })
    }
}

/// One process of one device: its network (with faults) and its saved state. A crash freezes the saved state at
/// the moment of the crash and fails every request after it; the worker then starts the process again from it.
struct Proc {
    net: Arc<Net>,
    stats: Arc<Stats>,
    faults: u64,
    rng: Mutex<Rng>,
    crash_in: AtomicU64,
    crashed: AtomicBool,
    saved: Mutex<Vec<u8>>,
}

impl Proc {
    fn new(net: &Arc<Net>, stats: &Arc<Stats>, faults: u64, seed: u64) -> Arc<Self> {
        Arc::new(Proc {
            net: net.clone(),
            stats: stats.clone(),
            faults,
            rng: Mutex::new(Rng(seed.max(1))),
            crash_in: AtomicU64::new(0),
            crashed: AtomicBool::new(false),
            saved: Mutex::default(),
        })
    }

    fn roll(&self, n: u64) -> u64 {
        self.rng.lock().unwrap_or_else(PoisonError::into_inner).below(n)
    }

    fn saved(&self) -> Vec<u8> {
        self.saved.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Transport for Proc {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        if self.crashed.load(Relaxed) {
            return Err("process gone".into());
        }
        if self.crash_in.load(Relaxed) > 0 && self.crash_in.fetch_sub(1, Relaxed) == 1 {
            // The process dies with this request on its way: it may or may not have reached the hub.
            self.crashed.store(true, Relaxed);
            self.stats.count("fault: crash");
            if self.roll(2) == 0 {
                let _ = self.net.send(method, path, headers, body);
            }
            return Err("process gone".into());
        }
        // The long poll is left alone mostly: a fault there only costs time.
        if self.roll(1000) >= self.faults {
            return self.net.send(method, path, headers, body);
        }
        let kind = self.roll(6);
        self.stats
            .count(format!("fault: {}", ["drop", "lose answer", "duplicate", "delay", "503", "429"][kind as usize]));
        match kind {
            0 => Err("dropped (injected)".into()),
            1 => {
                let _ = self.net.send(method, path, headers, body);
                Err("answer lost (injected)".into())
            }
            2 => {
                // The request arrives twice (a retransmission); the client sees one answer.
                let first = self.net.send(method, path, headers, body);
                let _ = self.net.send(method, path, headers, body);
                first
            }
            3 => {
                std::thread::sleep(Duration::from_millis(self.roll(400)));
                self.net.send(method, path, headers, body)
            }
            4 => Ok(Response { status: 503, body: Vec::new() }),
            _ => Ok(Response { status: 429, body: Vec::new() }),
        }
    }
}

impl Store for Proc {
    fn save(&self, blob: Vec<u8>) {
        if !self.crashed.load(Relaxed) {
            *self.saved.lock().unwrap_or_else(PoisonError::into_inner) = blob;
        }
    }
}

// ---- the ledger ----

struct Msg {
    from: String,
    to: String,
    seq: u64,
    must: bool,
    got: u32,
    /// The receiver's device that got it (its generation): another device of the user may show it again.
    got_by: u32,
    /// Sent by this name: whoever has it when the hub is asked is the one it goes to.
    name: Option<String>,
}

#[derive(Default)]
struct Ledger {
    msgs: HashMap<String, Msg>,
    /// The last sequence number received, per (receiver, sender).
    last: HashMap<(String, String), u64>,
    /// Photos and videos by blob id: the tag of their message, and the plaintext's length.
    media: HashMap<[u8; 16], (String, usize)>,
    /// Pairs (blocker, blocked) as the blockers' workers set them.
    blocks: BTreeSet<(String, String)>,
}

impl Ledger {
    /// What has not arrived and matches may now be lost.
    fn relax(&mut self, matches: impl Fn(&Msg) -> bool) {
        for msg in self.msgs.values_mut().filter(|m| m.got == 0 && matches(m)) {
            msg.must = false;
        }
    }
}

// ---- the world ----

#[derive(Clone)]
struct Entry {
    name: String,
    user: String,
    alive: bool,
    /// The name changed hands (deleted, taken again): sent by name it may still find its old owner's contact.
    reused: bool,
}

struct World {
    args: Args,
    net: Arc<Net>,
    stats: Arc<Stats>,
    ledger: Mutex<Ledger>,
    dir: RwLock<Vec<Entry>>,
    hub: Mutex<Option<TestHub>>,
    config: Option<Config>,
    addr: Option<String>,
    stop: AtomicBool,
    /// Logical time: a tick per send, so a send can tell whether it started after a block or a replacement.
    progress: Vec<AtomicU64>,
    run: String,
    /// The last things each user's devices did, by user id: printed beside a violation.
    traces: Mutex<HashMap<String, std::collections::VecDeque<String>>>,
}

impl World {
    fn trace(&self, user: &str, what: String) {
        let mut traces = self.traces.lock().unwrap_or_else(PoisonError::into_inner);
        let trace = traces.entry(user.to_owned()).or_default();
        trace.push_back(format!("{} {what}", yo_hub::now_ms() % 1_000_000));
        if trace.len() > 60 {
            trace.pop_front();
        }
    }

    fn dump(&self, user: &str) {
        let traces = self.traces.lock().unwrap_or_else(PoisonError::into_inner);
        println!("  trace of {user}:");
        for line in traces.get(user).into_iter().flatten() {
            println!("    {line}");
        }
    }

    fn ledger(&self) -> std::sync::MutexGuard<'_, Ledger> {
        self.ledger.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct Acct {
    idx: usize,
    name: String,
    secret: [u8; 32],
    user: String,
    client: Option<Client>,
    proc_: Arc<Proc>,
    gen: u32,
    blocked: BTreeSet<String>,
    seq: HashMap<String, u64>,
    sent: u64,
    friends: Vec<usize>,
    bot: bool,
    feedback: u32,
    /// Whether the hub has a backup record of the account: `None` when that is not known (a lost answer).
    backed_up: Option<bool>,
    /// Photos sealed and waiting for their upload: (to, media, ciphertext, tag).
    uploads: Vec<(usize, yo_core::Media, Vec<u8>, String)>,
}

const STATE_KEY: [u8; 32] = [7; 32];

// ---- names ----

/// A username for account `i` in one of several spellings (spec 3.3): accents, emoji, mixed case; unique by skeleton
/// within the run (`run` is a few letters that differ between runs on a persistent hub).
fn name_for(run: &str, i: usize, style: u64) -> String {
    match style % 6 {
        0 => format!("Leo{run}{i}"),
        1 => format!("Zoë{run}{i}"),
        2 => format!("José_{run}{i}"),
        3 => format!("🔥{run}{i}"),
        4 => format!("🧙‍♂️{run}{i}"),
        _ => format!("yOgPt{run}{i}"),
    }
}

/// Spellings that look like `name` and so are the same name: every one must be refused.
fn lookalikes(name: &str) -> Vec<String> {
    let mut all = vec![name.to_uppercase(), name.to_lowercase()];
    all.push(name.replace(['o', 'O'], "0"));
    all.push(name.replace('e', "é").replace('E', "É"));
    all.push(name.replace("🔥", "🔥\u{FE0F}"));
    all.retain(|n| n != name && yo_wire::handle::check_username_form(n).is_ok());
    all.dedup();
    all
}

const RESERVED: &[&str] = &["YoGPT", "Support", "Admin", "Justyo"];

// ---- operations ----

/// Registers like the app: a new account key unless one is given; `429` waits and tries again.
fn register(w: &World, name: &str, secret: Option<[u8; 32]>, proc_: &Arc<Proc>) -> Result<Client, Error> {
    for attempt in 0.. {
        match Client::register_username(proc_.clone(), name, secret, None, Push::none()) {
            Err(Error::Refused(429)) if attempt < 200 && !w.stop.load(Relaxed) => {
                w.stats.count("register: 429, waiting");
                std::thread::sleep(Duration::from_secs(if w.args.url.is_some() { 60 } else { 1 }));
            }
            Err(Error::Network(_)) | Err(Error::Refused(503)) if attempt < 20 => {
                std::thread::sleep(Duration::from_millis(200));
            }
            other => return other,
        }
    }
    unreachable!()
}

fn new_acct(w: &World, idx: usize, rng: &mut Rng) -> Option<Acct> {
    let name = name_for(&w.run, idx, rng.below(6));
    let proc_ = Proc::new(&w.net, &w.stats, w.args.faults, rng.next_u64());
    let mut client = match register(w, &name, None, &proc_) {
        Ok(client) => client,
        Err(error) => {
            w.stats.violation(format!("registering {name}: {error}"));
            return None;
        }
    };
    client.persist_to(proc_.clone(), STATE_KEY);
    w.stats.count("registered");
    let user = client.user_id().to_owned();
    // The same name in another look is taken (only now and then: each try spends the address's registrations).
    // Not on a shared hub: each try spends registrations other testers need (10 per address per hour).
    if w.args.url.is_none() {
        for other in lookalikes(&name) {
            let probe = Proc::new(&w.net, &w.stats, 0, rng.next_u64());
            match Client::register_username(probe, &other, None, None, Push::none()) {
                Err(Error::Refused(409)) => w.stats.count("lookalike refused"),
                Err(Error::Refused(429)) => {}
                Ok(_) => w.stats.violation(format!("{other} registered beside {name}")),
                Err(error) => w.stats.count(format!("lookalike: {error}")),
            }
        }
    }
    Some(Acct {
        idx,
        name,
        secret: client.account_secret(),
        user,
        client: Some(client),
        proc_,
        gen: 0,
        blocked: BTreeSet::new(),
        seq: HashMap::new(),
        sent: 0,
        friends: Vec::new(),
        bot: idx % 50 == 7,
        feedback: 0,
        backed_up: Some(false),
        uploads: Vec::new(),
    })
}

/// Takes in what a call returned: every message checked against the ledger, bots answer.
fn take_events(w: &World, a: &mut Acct, events: Vec<Event>, rng: &mut Rng) {
    // A process that died during the call never showed what it returned: it comes again after the restart.
    if a.proc_.crashed.load(Relaxed) {
        return;
    }
    let mut answer = Vec::new();
    for event in events {
        let (from, tag, photo) = match event {
            Event::Message { user, content: Content::Text(tag), .. } => (user, tag, None),
            Event::Message { user, content: Content::Photo(m), .. } => {
                let tag = w.ledger().media.get(&m.id).map(|(t, _)| t.clone()).unwrap_or_default();
                (user, tag, Some(m))
            }
            Event::Message { user, content: Content::Video { media: m, .. }, .. } => {
                let tag = w.ledger().media.get(&m.id).map(|(t, _)| t.clone()).unwrap_or_default();
                (user, tag, Some(m))
            }
            Event::Message { user, .. } => {
                w.stats.count("plain Yo received");
                (user, String::new(), None)
            }
            Event::IdentityChanged { .. } => {
                w.stats.count("identity changed");
                continue;
            }
            _ => continue,
        };
        if a.blocked.contains(&from) {
            w.stats.violation(format!("{} was shown {tag} from {from}, whom it blocks", a.user));
            w.dump(&a.user);
        }
        if let Some(m) = &photo {
            // A fetch without faults: the photo's arrival is what is checked here, not the network's.
            let transport = Proc::new(&w.net, &w.stats, 0, 1);
            match definite(|| media::fetch_open(transport.as_ref(), m)) {
                Ok(Some(plain)) => {
                    let want = w.ledger().media.get(&m.id).map(|(_, n)| *n);
                    if want != Some(plain.len()) {
                        w.stats.violation(format!("photo {tag}: {} bytes, sent {want:?}", plain.len()));
                    }
                    w.stats.count("photo opened");
                    // View once: one device, one fetch; the next finds nothing.
                    if let Ok(Some(_)) = media::fetch(transport.as_ref(), m) {
                        w.stats.violation(format!("photo {tag} fetched twice"));
                    }
                }
                Ok(None) => w.stats.count("photo gone before its fetch"),
                Err(error) => w.stats.violation(format!("photo {tag}: {error}")),
            }
        }
        let mut ledger = w.ledger();
        let Some(msg) = ledger.msgs.get_mut(&tag) else {
            if !tag.is_empty() {
                w.stats.violation(format!("{} received {tag:?}, which nobody sent", a.user));
            }
            continue;
        };
        // Sent by a name that changed hands in between (deleted, taken again): the new owner has it.
        if msg.to != a.user && msg.from == from && msg.name.as_deref() == Some(a.name.as_str()) && msg.got == 0 {
            msg.must = false;
            msg.got = 1;
            drop(ledger);
            w.stats.count("received by the name's new owner");
            continue;
        }
        if msg.to != a.user || msg.from != from {
            let (to, sender) = (msg.to.clone(), msg.from.clone());
            drop(ledger);
            w.stats.violation(format!("{tag}: from {sender} to {to}, received by {} from {from}", a.user));
            continue;
        }
        if msg.got > 0 && msg.got_by != a.gen {
            // Shown on the old phone, its answer lost to the sender, sent again to the new one: once per device.
            msg.got_by = a.gen;
            drop(ledger);
            w.stats.count("received again on a new phone");
            continue;
        }
        msg.got += 1;
        msg.got_by = a.gen;
        let (got, seq) = (msg.got, msg.seq);
        w.trace(&a.user, format!("got {tag} from {from} (#{seq})"));
        if got > 1 {
            drop(ledger);
            w.stats.violation(format!("{} received {tag} {got} times", a.user));
            w.dump(&a.user);
            continue;
        }
        let last = ledger.last.entry((a.user.clone(), from.clone())).or_default();
        if seq <= *last {
            let was = *last;
            drop(ledger);
            w.stats.violation(format!("{} received {tag} (#{seq}) after #{was} from {from}", a.user));
            w.dump(&a.user);
            w.dump(&from);
        } else {
            *last = seq;
            drop(ledger);
            w.stats.count("received");
        }
        if a.bot && rng.below(2) == 0 {
            answer.push(from);
        }
    }
    if let Some(client) = a.client.as_mut() {
        client.acknowledge_events();
    }
    // A bot answers a burst with a burst.
    for user in answer {
        for _ in 0..1 + rng.below(3) {
            send_text(w, a, &user, None, rng);
        }
    }
}

/// The index of the account a user id is.
fn idx_of(w: &World, user: &str) -> Option<usize> {
    w.dir.read().unwrap_or_else(PoisonError::into_inner).iter().position(|e| e.user == user)
}

/// Sends a text that names itself to `to` (a user id), by user id or by name; records it in the ledger first.
fn send_text(w: &World, a: &mut Acct, to: &str, by_name: Option<&str>, _rng: &mut Rng) {
    if a.client.is_none() {
        return;
    }
    let seq = a.seq.entry(to.to_owned()).or_default();
    *seq += 1;
    let seq = *seq;
    a.sent += 1;
    let tag = format!("{}:{}:{}:{}", w.run, a.idx, a.gen, a.sent);
    let alive = idx_of(w, to).is_some_and(|i| w.dir.read().unwrap_or_else(PoisonError::into_inner)[i].alive);
    {
        let mut ledger = w.ledger();
        let blocked = ledger.blocks.contains(&(to.to_owned(), a.user.clone()));
        let must = alive && !blocked;
        ledger.msgs.insert(
            tag.clone(),
            Msg {
                from: a.user.clone(),
                to: to.to_owned(),
                seq,
                must,
                got: 0,
                got_by: 0,
                name: by_name.map(str::to_owned),
            },
        );
    }
    let target = by_name.unwrap_or(to).to_owned();
    let result = a.client.as_mut().unwrap().send(&target, Content::Text(tag.clone()));
    w.trace(
        &a.user,
        format!(
            "send {tag} to {to} via {target}: {:?} pending {}",
            result.as_ref().map(|e| e.len()),
            a.client.as_ref().map_or(0, |c| c.pending())
        ),
    );
    w.trace(to, format!("({tag} sent to me by {})", a.user));
    after_send(w, a, &tag, result, alive, _rng);
}

fn after_send(w: &World, a: &mut Acct, tag: &str, result: Result<Vec<Event>, Error>, alive: bool, rng: &mut Rng) {
    w.stats.count("sent");
    match result {
        Ok(events) if !a.proc_.crashed.load(Relaxed) => take_events(w, a, events, rng),
        Ok(_) => {
            // The process died during the send: the app never learnt that it went.
            if let Some(msg) = w.ledger().msgs.get_mut(tag) {
                msg.must = false;
            }
        }
        Err(error) => {
            // Not sent, or not for sure: it may still go (it is in the outbox), or never.
            if let Some(msg) = w.ledger().msgs.get_mut(tag) {
                msg.must = false;
            }
            let to = w.ledger().msgs.get(tag).map(|m| m.to.clone());
            // Blocked here, or by the storage just taken in (a new phone): the device's word counts.
            let blocked = to.as_ref().is_some_and(|to| {
                a.blocked.contains(to)
                    || a.client.as_ref().is_some_and(|c| c.contacts().iter().any(|k| k.user == *to && k.blocked))
            });
            // Deleted meanwhile, by another thread: gone by now.
            let gone_now = to.is_some_and(|to| {
                idx_of(w, &to).is_none_or(|i| !w.dir.read().unwrap_or_else(PoisonError::into_inner)[i].alive)
            });
            let expected = matches!(error, Error::NotOnYo) && (!alive || gone_now)
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

fn short(error: &Error) -> String {
    match error {
        Error::Network(_) => "network".into(),
        other => other.to_string(),
    }
}

/// An error no fault explains is a finding; a replaced device is one when nobody replaced it.
fn note_error(w: &World, a: &Acct, what: &str, error: &Error) {
    match error {
        Error::Network(_) => w.stats.count(format!("{what}: network")),
        Error::Refused(status) if *status == 429 || *status >= 500 => w.stats.count(format!("{what}: {status}")),
        _ if a.proc_.crashed.load(Relaxed) => w.stats.count(format!("{what}: during a crash")),
        // A block the device took in from the storage during the call.
        Error::Blocked => w.stats.count(format!("{what}: blocked")),
        _ => {
            w.stats.violation(format!("{} ({}) {what}: {error}", a.name, a.user));
            w.dump(&a.user);
        }
    }
}

/// A recipient: a friend mostly (chats are reused), anyone alive sometimes.
fn pick_peer(w: &World, a: &mut Acct, rng: &mut Rng) -> Option<(usize, Entry)> {
    let dir = w.dir.read().unwrap_or_else(PoisonError::into_inner);
    if a.friends.is_empty() {
        for _ in 0..6 {
            let j = rng.below(dir.len() as u64) as usize;
            if j != a.idx {
                a.friends.push(j);
            }
        }
    }
    let j = if rng.below(10) < 8 && !a.friends.is_empty() {
        a.friends[rng.below(a.friends.len() as u64) as usize]
    } else {
        rng.below(dir.len() as u64) as usize
    };
    // Not oneself, not an account that never registered.
    (j != a.idx && dir[j].user.len() == 32).then(|| (j, dir[j].clone()))
}

fn op_send(w: &World, a: &mut Acct, rng: &mut Rng) {
    let Some((_, peer)) = pick_peer(w, a, rng) else { return };
    let known = a.client.as_ref().is_some_and(|c| c.contacts().iter().any(|k| k.user == peer.user));
    // As the app: a typed name is looked up first (whois), then the user id is what it sends to. Now and then by
    // name, as older apps did, while the name has had one owner.
    let by_name = (!peer.reused && rng.below(4) == 0).then_some(peer.name.as_str());
    if !known && by_name.is_none() && !whois(w, a, &peer) {
        return;
    }
    let burst = if rng.below(10) == 0 { 2 + rng.below(20) } else { 1 };
    for _ in 0..burst {
        send_text(w, a, &peer.user, by_name, rng);
        if a.proc_.crashed.load(Relaxed) {
            break;
        }
    }
}

/// Who a typed name is, as the app asks before the first Yo: false when that cannot be told now.
fn whois(w: &World, a: &mut Acct, peer: &Entry) -> bool {
    let Some(client) = a.client.as_mut() else { return false };
    let asked = client.whois_begin(&peer.name).and_then(|whois| whois.send());
    match asked {
        Ok((user, _)) if user == peer.user => true,
        Ok((user, _)) => {
            // Taken again meanwhile (the directory lags a moment behind a re-registration): not a finding.
            let now = idx_of(w, &user).is_some();
            if !now {
                w.stats.violation(format!("whois {} answered {user}, not {}", peer.name, peer.user));
            }
            false
        }
        Err(Error::NotOnYo) if !peer.alive => false,
        Err(error) => {
            note_error(w, a, "whois", &error);
            false
        }
    }
}

/// A Yo to many friends at once.
fn op_fan_out(w: &World, a: &mut Acct, rng: &mut Rng) {
    for _ in 0..rng.below(8) + 2 {
        let Some((_, peer)) = pick_peer(w, a, rng) else { continue };
        let known = a.client.as_ref().is_some_and(|c| c.contacts().iter().any(|k| k.user == peer.user));
        if !known && !whois(w, a, &peer) {
            continue;
        }
        send_text(w, a, &peer.user, None, rng);
        if a.proc_.crashed.load(Relaxed) {
            break;
        }
    }
}

fn op_sync(w: &World, a: &mut Acct, rng: &mut Rng) {
    let wait = if rng.below(10) == 0 { 1 } else { 0 };
    let result = a.client.as_mut().unwrap().sync(wait);
    w.trace(&a.user, format!("sync: {:?}", result.as_ref().map(|e| e.len())));
    match result {
        Ok(events) if !a.proc_.crashed.load(Relaxed) => take_events(w, a, events, rng),
        Ok(_) => {}
        Err(error) => note_error(w, a, "sync", &error),
    }
}

fn op_photo(w: &World, a: &mut Acct, rng: &mut Rng) {
    let Some((j, peer)) = pick_peer(w, a, rng) else { return };
    let most = if rng.below(10) == 0 { 2_000_000 } else { 50_000 };
    let size = 1 + rng.below(most) as usize;
    let plain = vec![(a.sent % 251) as u8; size];
    let Ok((m, sealed)) = media::seal(&plain, 1080, 1920) else { return };
    a.sent += 1;
    let tag = format!("{}:{}:{}:{}", w.run, a.idx, a.gen, a.sent);
    w.ledger().media.insert(m.id, (tag.clone(), size));
    a.uploads.push((j, m, sealed, tag));
    let _ = peer;
    op_uploads(w, a, rng);
}

/// Uploads what waits, then sends its Yo; "later" keeps it for the next try.
fn op_uploads(w: &World, a: &mut Acct, rng: &mut Rng) {
    let mut keep = Vec::new();
    for (j, m, sealed, tag) in std::mem::take(&mut a.uploads) {
        let peer = w.dir.read().unwrap_or_else(PoisonError::into_inner)[j].clone();
        if !peer.alive || a.client.is_none() || a.blocked.contains(&peer.user) {
            continue;
        }
        let client = a.client.as_mut().unwrap();
        let fetches = match client.media_fetches(&peer.user).or_else(|| client.media_fetches(&peer.name)) {
            Some(n) => n,
            None => match client.open_chat(&peer.user) {
                Ok((n, events)) => {
                    take_events(w, a, events, rng);
                    n
                }
                Err(error) => {
                    note_error(w, a, "open chat", &error);
                    continue;
                }
            },
        };
        let Some(client) = a.client.as_mut() else { continue };
        match client.upload(&m, fetches).send(&sealed) {
            Ok(()) => {}
            Err(error) if media::retry_later(&error) => {
                w.stats.count(format!("upload later: {}", short(&error)));
                keep.push((j, m, sealed, tag));
                continue;
            }
            Err(error) => {
                note_error(w, a, "upload", &error);
                continue;
            }
        }
        let seq = a.seq.entry(peer.user.clone()).or_default();
        *seq += 1;
        let seq = *seq;
        {
            let mut ledger = w.ledger();
            let must = !ledger.blocks.contains(&(peer.user.clone(), a.user.clone()));
            ledger.msgs.insert(
                tag.clone(),
                Msg { from: a.user.clone(), to: peer.user.clone(), seq, must, got: 0, got_by: 0, name: None },
            );
        }
        let content =
            if rng.below(3) == 0 { Content::Video { media: m, duration_ms: 9_000 } } else { Content::Photo(m) };
        let result = a.client.as_mut().unwrap().send(&peer.user, content);
        w.stats.count("photo sent");
        after_send(w, a, &tag, result, true, rng);
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
    let block = !a.blocked.contains(&who);
    if block {
        // Known to the ledger first: whatever from them is still on its way may be dropped from now on.
        let mut ledger = w.ledger();
        ledger.blocks.insert((a.user.clone(), who.clone()));
        let me = a.user.clone();
        ledger.relax(|m| m.to == me && m.from == who);
    }
    match client.set_blocked(&who, block) {
        Ok(()) => {
            w.stats.count(if block { "blocked" } else { "unblocked" });
            w.trace(&a.user, format!("{} {who}", if block { "blocked" } else { "unblocked" }));
            if block {
                a.blocked.insert(who.clone());
            } else {
                a.blocked.remove(&who);
                w.ledger().blocks.remove(&(a.user.clone(), who.clone()));
            }
        }
        Err(error) => note_error(w, a, "block", &error),
    }
    // Sending to someone one blocked is refused.
    if block {
        match a.client.as_mut().unwrap().send(&who, Content::Text("never".into())) {
            Err(Error::Blocked) => w.stats.count("send to blocked refused"),
            Err(Error::NotOnYo) => w.stats.count("send to a blocked contact: gone"),
            other => w.stats.violation(format!("send to a blocked contact: {:?}", other.map(|_| ()))),
        }
    }
}

fn op_misc(w: &World, a: &mut Acct, rng: &mut Rng) {
    let client = a.client.as_mut().unwrap();
    match rng.below(5) {
        0 => {
            // To oneself, by name and by id.
            let me = if rng.below(2) == 0 { a.name.clone() } else { a.user.clone() };
            match client.send(&me, Content::Text("me".into())) {
                Err(Error::Protocol(_)) => w.stats.count("send to oneself refused"),
                other => w.stats.violation(format!("send to oneself: {:?}", other.map(|_| ()))),
            }
        }
        1 => {
            let Some((_, peer)) = pick_peer(w, a, rng) else { return };
            match a.client.as_mut().unwrap().report(&peer.user, 1 + rng.below(3) as u8) {
                Ok(()) => w.stats.count("reported"),
                Err(Error::NotOnYo) if !peer.alive => w.stats.count("report: not on Yo"),
                Err(error) => note_error(w, a, "report", &error),
            }
        }
        2 => {
            a.feedback += 1;
            match client.send_feedback(&format!("chaos feedback {} from {}", a.feedback, a.idx), "chaos") {
                Ok(()) => w.stats.count("feedback"),
                Err(Error::Refused(429)) => w.stats.count("feedback: 429"),
                Err(error) => note_error(w, a, "feedback", &error),
            }
        }
        3 => {
            if a.backed_up == Some(false) {
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
        }
        _ => match client.maintenance() {
            Ok(events) if !a.proc_.crashed.load(Relaxed) => take_events(w, a, events, rng),
            Ok(_) => {}
            Err(error) => note_error(w, a, "maintenance", &error),
        },
    }
}

/// Repeats `f` while the answer is "not now" (a fault, a hub restarting): the definite answer.
fn definite<T>(mut f: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
    for _ in 0..30 {
        match f() {
            Err(Error::Network(_)) | Err(Error::Refused(429)) => std::thread::sleep(Duration::from_millis(300)),
            Err(Error::Refused(status)) if status >= 500 => std::thread::sleep(Duration::from_millis(300)),
            other => return other,
        }
    }
    f()
}

/// What a client dropped (its count lives in memory only): counted before it goes.
fn harvest_drops(w: &World, client: Option<&Client>) {
    for reason in client.map(|c| c.drop_reasons()).unwrap_or_default() {
        let kind: String = reason.chars().take(60).collect();
        w.stats.count(format!("dropped: {kind}"));
    }
}

/// The app killed and started again from what it saved.
fn restart(w: &World, a: &mut Acct, rng: &mut Rng) {
    harvest_drops(w, a.client.as_ref());
    let saved = a.proc_.saved();
    let proc_ = Proc::new(&w.net, &w.stats, w.args.faults, rng.next_u64());
    match Client::restore(proc_.clone(), &saved, &STATE_KEY) {
        Ok(mut client) => {
            client.persist_to(proc_.clone(), STATE_KEY);
            a.client = Some(client);
            a.proc_ = proc_;
            w.stats.count("restarted");
        }
        Err(error) => w.stats.violation(format!("{}: the saved state does not open: {error}", a.name)),
    }
    w.trace(&a.user, format!("restarted, pending {}", a.client.as_ref().map_or(0, |c| c.pending())));
}

/// A new phone with the same account key (restored from the backup): the old device answers 410 from then on.
fn op_new_phone(w: &World, a: &mut Acct, rng: &mut Rng) {
    let proc_ = Proc::new(&w.net, &w.stats, w.args.faults, rng.next_u64());
    // On a hub with its limits, one try: the address's registrations are few, the phone waits for another day.
    let tried = if w.args.url.is_some() {
        Client::register_username(proc_.clone(), &a.name, Some(a.secret), None, Push::none())
    } else {
        register(w, &a.name, Some(a.secret), &proc_)
    };
    let mut client = match tried {
        Ok(client) => client,
        Err(Error::Refused(429)) => {
            w.stats.count("new phone: 429, not now");
            return;
        }
        Err(error) => {
            w.stats.violation(format!("{}: a new phone with the account key: {error}", a.name));
            return;
        }
    };
    if client.user_id() != a.user {
        w.stats.violation(format!("{}: a new phone got another user id", a.name));
    }
    client.persist_to(proc_.clone(), STATE_KEY);
    // What the old device had not received, or not sent, is gone with it.
    harvest_drops(w, a.client.as_ref());
    let mut old = a.client.replace(client).unwrap();
    let old_proc = a.proc_.clone();
    {
        let me = a.user.clone();
        let pending = old.pending() > 0 || !a.uploads.is_empty();
        let gen = format!("{}:{}:{}:", w.run, a.idx, a.gen);
        w.ledger().relax(|m| m.to == me || (pending && m.from == me));
        let _ = gen;
    }
    a.uploads.clear();
    a.proc_ = proc_;
    a.gen += 1;
    w.stats.count("new phone");
    // The new phone's order starts anew: what was sent again to it may be older than what the old one saw.
    w.ledger().last.retain(|(to, _), _| *to != a.user);
    w.trace(&a.user, "new phone".into());
    old.stop_persisting();
    let answer = (0..10).map(|_| definite(|| old.sync(0))).find(|r| r.is_err()).unwrap_or(Ok(Vec::new()));
    match answer {
        Err(Error::Replaced) => w.stats.count("old phone: 410"),
        // Its process died meanwhile: it asks nothing any more.
        Err(Error::Network(_)) if old_proc.crashed.load(Relaxed) => {}
        other => w.stats.violation(format!("{}: the replaced device: {:?}", a.name, other.map(|_| ()))),
    }
}

/// Deletes the account, and now and then someone takes the name again with a new key: a new user id.
fn op_delete(w: &World, a: &mut Acct, rng: &mut Rng) {
    let me = a.user.clone();
    if a.proc_.crashed.load(Relaxed) {
        restart(w, a, rng);
    }
    a.proc_.crash_in.store(0, Relaxed);
    {
        let mut dir = w.dir.write().unwrap_or_else(PoisonError::into_inner);
        dir[a.idx].alive = false;
    }
    w.ledger().relax(|m| m.to == me || m.from == me);
    let client = a.client.as_mut().unwrap();
    // Until the hub says it is done; a repeat whose first try arrived finds the device gone already.
    match definite(|| client.delete_account()) {
        Ok(()) | Err(Error::Replaced) => {
            w.stats.count("deleted");
            w.trace(&me, "deleted".into());
        }
        Err(error) => w.stats.violation(format!("{}: delete: {error}", a.name)),
    }
    let mut old = a.client.take().unwrap();
    old.stop_persisting();
    // Until the hub answers: a sync hands out what waited even when its request found no hub (finish_receiving).
    let answer = (0..10).map(|_| definite(|| old.sync(0))).find(|r| r.is_err()).unwrap_or(Ok(Vec::new()));
    match answer {
        Err(Error::Replaced) => {}
        other => w.stats.violation(format!("{}: a deleted account's device: {:?}", a.name, other.map(|_| ()))),
    }
    for who in std::mem::take(&mut a.blocked) {
        w.ledger().blocks.remove(&(me.clone(), who));
    }
    if rng.below(2) == 0 {
        return;
    }
    // The name is free: a new person takes it, with a new key.
    let proc_ = Proc::new(&w.net, &w.stats, w.args.faults, rng.next_u64());
    match register(w, &a.name, None, &proc_) {
        Ok(mut client) => {
            client.persist_to(proc_.clone(), STATE_KEY);
            if client.user_id() == me {
                w.stats.violation(format!("{}: the freed name kept the deleted user id", a.name));
            }
            a.user = client.user_id().to_owned();
            a.secret = client.account_secret();
            a.client = Some(client);
            a.proc_ = proc_;
            a.gen += 1;
            a.seq.clear();
            a.backed_up = Some(false);
            a.uploads.clear();
            let mut dir = w.dir.write().unwrap_or_else(PoisonError::into_inner);
            dir[a.idx] = Entry { name: a.name.clone(), user: a.user.clone(), alive: true, reused: true };
            w.stats.count("name taken again");
        }
        Err(error) => w.stats.violation(format!("{}: the freed name: {error}", a.name)),
    }
}

fn step(w: &World, a: &mut Acct, rng: &mut Rng) {
    if a.client.is_none() {
        return;
    }
    if rng.below(1000) < w.args.crashes {
        a.proc_.crash_in.store(1 + rng.below(8), Relaxed);
    }
    let roll = rng.below(1000);
    w.trace(&a.user, format!("step {roll} (gen {}, crash in {})", a.gen, a.proc_.crash_in.load(Relaxed)));
    if a.bot {
        op_sync(w, a, rng);
    } else if roll < 380 {
        op_send(w, a, rng);
    } else if roll < 700 {
        op_sync(w, a, rng);
    } else if roll < 760 {
        op_fan_out(w, a, rng);
    } else if roll < 800 {
        op_photo(w, a, rng);
    } else if roll < 830 {
        op_block(w, a, rng);
    } else if roll < 900 {
        op_misc(w, a, rng);
    } else if roll < 930 {
        restart(w, a, rng);
    } else if roll < 940 {
        op_new_phone(w, a, rng);
    } else if roll < 944 && w.args.url.is_none() {
        op_delete(w, a, rng);
    } else {
        op_uploads(w, a, rng);
        op_sync(w, a, rng);
    }
    sync_blocks(w, a);
    if a.proc_.crashed.load(Relaxed) && a.client.is_some() {
        w.trace(&a.user, "crashed".into());
        // The process died somewhere in that step: started again from what it had saved.
        a.client = None;
        restart(w, a, rng);
    }
}

/// The blocks as the device has them: a new phone gets them back from the storage (spec 6.9), now or at its first
/// receive. What comes from someone newly blocked may be dropped from then on.
fn sync_blocks(w: &World, a: &mut Acct) {
    let Some(client) = a.client.as_ref() else { return };
    let now: BTreeSet<String> = client.contacts().into_iter().filter(|c| c.blocked).map(|c| c.user).collect();
    if now == a.blocked {
        return;
    }
    let mut ledger = w.ledger();
    for who in a.blocked.difference(&now) {
        ledger.blocks.remove(&(a.user.clone(), who.clone()));
    }
    for who in now.difference(&a.blocked) {
        ledger.blocks.insert((a.user.clone(), who.clone()));
        let me = a.user.clone();
        ledger.relax(|m| m.to == me && m.from == *who);
    }
    drop(ledger);
    w.trace(&a.user, format!("blocks now {now:?}"));
    a.blocked = now;
}

// ---- the hub ----

fn hub_config(args: &Args, db: &str) -> Config {
    let mut config = Config::for_tests();
    config.db_path = db.to_owned();
    if args.limits {
        config.limits = Limits { register_per_ip_hour: u32::MAX, backup_per_ip_hour: u32::MAX, ..Limits::default() };
    } else {
        config.limits.feedback_per_user_day = 10;
    }
    config.reserved = yo_hub::config::parse_reserved(&RESERVED.join("\n"));
    config
}

fn restart_hub(w: &World) {
    let (Some(config), Some(addr)) = (&w.config, &w.addr) else { return };
    let mut hub = w.hub.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(old) = hub.take() {
        old.stop();
    }
    std::thread::sleep(Duration::from_millis(500));
    for _ in 0..50 {
        let started = std::panic::catch_unwind(|| TestHub::start_with(config.clone(), addr));
        if let Ok(started) = started {
            *hub = Some(started);
            w.stats.count("hub restarted");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("the hub does not start again on {addr}");
}

// ---- the run ----

fn worker(w: Arc<World>, t: usize, mut accts: Vec<Acct>, deadline: Instant) -> Vec<Acct> {
    let mut rng = Rng(w.args.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (t as u64 + 1));
    while Instant::now() < deadline && !w.stop.load(Relaxed) {
        let i = rng.below(accts.len() as u64) as usize;
        step(&w, &mut accts[i], &mut rng);
        w.progress[t].store(yo_hub::now_ms(), Relaxed);
    }
    accts
}

/// Everyone receives and sends what waits, until nothing moves.
fn quiesce(w: &World, all: &mut [Vec<Acct>]) {
    let mut rng = Rng(w.args.seed ^ 0xABCD);
    for round in 0..120 {
        let before = w.stats.get("received");
        let mut waiting = 0;
        for accts in all.iter_mut() {
            for a in accts.iter_mut().filter(|a| a.client.is_some()) {
                op_uploads(w, a, &mut rng);
                op_sync(w, a, &mut rng);
                sync_blocks(w, a);
                if a.proc_.crashed.load(Relaxed) {
                    a.client = None;
                    restart(w, a, &mut rng);
                }
                waiting += a.client.as_ref().map_or(0, |c| c.pending()) + a.uploads.len();
            }
        }
        let moved = w.stats.get("received") - before;
        println!("quiet round {round}: {moved} received, {waiting} waiting");
        if moved == 0 && waiting == 0 && round >= 2 {
            return;
        }
        // A hub with its limits takes the backlog at its pace (deliveries per address per minute).
        if (w.args.url.is_some() || w.args.limits) && waiting > 0 {
            std::thread::sleep(Duration::from_secs(20));
        }
    }
}

fn main() {
    let args = args();
    let started = Instant::now();
    let run: String = {
        let mut r = Rng(args.seed + 1);
        (0..3).map(|_| (b'a' + r.below(26) as u8) as char).collect()
    };
    println!(
        "yo-chaos: seed {} (run {run}), {} accounts, {} threads, {} s, faults {}‰, crashes {}‰, {}",
        args.seed,
        args.accounts,
        args.threads,
        args.seconds,
        args.faults,
        args.crashes,
        args.url.as_deref().unwrap_or("in-process hub")
    );
    // Faults stay off the hub's own clients: these are the phones' networks.
    let (hub, config, addr, base) = match &args.url {
        Some(url) => (None, None, None, url.clone()),
        None => {
            let db = args.db.clone().unwrap_or_else(|| {
                std::env::temp_dir().join(format!("yo-chaos-{}.sqlite", args.seed)).display().to_string()
            });
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{db}{suffix}"));
            }
            let config = hub_config(&args, &db);
            let hub = TestHub::start_with(config.clone(), "127.0.0.1:0");
            let addr = hub.addr.to_string();
            (Some(hub), Some(config), Some(addr.clone()), format!("http://{addr}"))
        }
    };
    let net = Arc::new(Net {
        http: reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(256)
            .user_agent("yo-chaos")
            .build()
            .unwrap(),
        base,
    });
    let threads = args.threads;
    let w = Arc::new(World {
        net,
        stats: Arc::new(Stats::default()),
        ledger: Mutex::default(),
        dir: RwLock::new(Vec::new()),
        hub: Mutex::new(hub),
        config,
        addr,
        stop: AtomicBool::new(false),
        progress: (0..threads).map(|_| AtomicU64::new(yo_hub::now_ms())).collect(),
        run,
        args,
        traces: Mutex::default(),
    });

    // Reserved names are taken in every spelling.
    if w.args.url.is_none() {
        for name in RESERVED.iter().flat_map(|n| [n.to_string(), n.to_lowercase(), n.replace('o', "0")]) {
            let probe = Proc::new(&w.net, &w.stats, 0, 1);
            match Client::register_username(probe, &name, None, None, Push::none()) {
                Err(Error::Refused(409)) => w.stats.count("reserved refused"),
                other => w.stats.violation(format!("reserved {name}: {:?}", other.map(|c| c.user_id().to_owned()))),
            }
        }
    }

    // Phase 1: everyone registers at once.
    let t0 = Instant::now();
    let n = w.args.accounts;
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
    println!(
        "registered {registered} in {:.1} s ({:.0}/s)",
        t0.elapsed().as_secs_f64(),
        registered as f64 / t0.elapsed().as_secs_f64()
    );
    {
        let mut entries: Vec<(usize, Entry)> = all
            .iter()
            .flatten()
            .map(|a| (a.idx, Entry { name: a.name.clone(), user: a.user.clone(), alive: true, reused: false }))
            .collect();
        entries.sort_by_key(|(i, _)| *i);
        let mut dir = w.dir.write().unwrap();
        // Accounts that failed to register stay as dead entries, so indices are positions.
        *dir =
            (0..n).map(|_| Entry { name: "Nobody".into(), user: "00".into(), alive: false, reused: false }).collect();
        for (i, e) in entries {
            dir[i] = e;
        }
    }

    // A watchdog: no worker may stand still for long.
    {
        let w = w.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(10));
            let now = yo_hub::now_ms();
            for (t, p) in w.progress.iter().enumerate() {
                let idle = now.saturating_sub(p.load(Relaxed));
                if idle > 300_000 && !w.stop.load(Relaxed) {
                    println!("VIOLATION: worker {t} has not moved for {} s: a hang", idle / 1000);
                    std::process::exit(3);
                }
            }
        });
    }

    // Phase 2: chaos, with hub restarts meanwhile.
    let t1 = Instant::now();
    let received_before = w.stats.get("received");
    let deadline = Instant::now() + Duration::from_secs(w.args.seconds);
    let restarter = {
        let w = w.clone();
        std::thread::spawn(move || {
            let restarts = if w.args.url.is_some() { 0 } else { w.args.restarts };
            for k in 0..restarts {
                let at = w.args.seconds * (k as u64 + 1) / (restarts as u64 + 1);
                while t1.elapsed() < Duration::from_secs(at) {
                    std::thread::sleep(Duration::from_millis(200));
                }
                restart_hub(&w);
                println!("hub restarted at {} s, rss {} MB", t1.elapsed().as_secs(), rss_mb());
            }
        })
    };
    let reporter = {
        let w = w.clone();
        std::thread::spawn(move || {
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_secs(15));
                println!(
                    "t+{} s: sent {}, received {}, violations {}, rss {} MB",
                    t1.elapsed().as_secs(),
                    w.stats.get("sent"),
                    w.stats.get("received"),
                    w.stats.violations.load(Relaxed),
                    rss_mb()
                );
            }
        })
    };
    all = all
        .into_iter()
        .enumerate()
        .map(|(t, accts)| {
            let w = w.clone();
            std::thread::spawn(move || worker(w, t, accts, deadline))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    restarter.join().unwrap();
    reporter.join().unwrap();
    let chaos_s = t1.elapsed().as_secs_f64();
    let received_chaos = w.stats.get("received") - received_before;
    println!("chaos: {:.0} s, {received_chaos} received ({:.0}/s)", chaos_s, received_chaos as f64 / chaos_s);
    for p in &w.progress {
        p.store(u64::MAX / 2, Relaxed);
    }

    // Phase 3: quiet, then every message must be accounted for.
    quiesce(&w, &mut all);
    check_ledger(&w, &all);

    // Phase 4 (in-process): a year later, the names not secured are released; the secured stay.
    if w.args.url.is_none() {
        release(&w, &mut all);
        audit(&w);
    } else {
        // On a shared hub: leave nothing behind.
        for a in all.iter_mut().flatten() {
            if let Some(c) = a.client.as_mut() {
                let _ = c.delete_account();
            }
        }
    }

    let counts = w.stats.counts.lock().unwrap();
    let mut sorted: Vec<_> = counts.iter().collect();
    sorted.sort();
    for (what, n) in sorted {
        println!("  {what}: {n}");
    }
    let violations = w.stats.violations.load(Relaxed);
    println!(
        "done in {:.0} s: {} violations, rss {} MB, seed {}",
        started.elapsed().as_secs_f64(),
        violations,
        rss_mb(),
        w.args.seed
    );
    std::process::exit(if violations > 0 { 1 } else { 0 });
}

fn check_ledger(w: &World, all: &[Vec<Acct>]) {
    for a in all.iter().flatten() {
        if let Some(c) = &a.client {
            if c.pending() > 0 {
                w.stats.violation(format!("{} still has {} envelopes in its outbox", a.name, c.pending()));
            }
            harvest_drops(w, Some(c));
            // The saved state opens.
            if Client::restore(a.proc_.clone(), &a.proc_.saved(), &STATE_KEY).is_err() {
                w.stats.violation(format!("{}: the saved state does not open", a.name));
            }
        }
    }
    let ledger = w.ledger();
    let (mut must, mut lost) = (0, 0);
    for (tag, m) in &ledger.msgs {
        if m.must {
            must += 1;
            if m.got == 0 {
                lost += 1;
                if lost <= 30 {
                    println!("VIOLATION: {tag} from {} to {} never arrived", m.from, m.to);
                }
                if lost <= 3 {
                    w.dump(&m.from);
                    w.dump(&m.to);
                }
            }
        }
    }
    let may = ledger.msgs.values().filter(|m| !m.must).count();
    let may_got = ledger.msgs.values().filter(|m| !m.must && m.got > 0).count();
    drop(ledger);
    println!("ledger: {must} must arrive ({lost} lost), {may} may be lost ({may_got} arrived anyway)");
    if lost > 0 {
        w.stats.violations.fetch_add(lost, Relaxed);
    }
}

fn release(w: &World, all: &mut [Vec<Acct>]) {
    let hub = w.hub.lock().unwrap();
    let Some(hub) = hub.as_ref() else { return };
    let year = 366 * 24 * 3600 * 1000;
    hub.hub.prune(yo_hub::now_ms() + year).unwrap();
    let mut rng = Rng(w.args.seed ^ 0x365);
    let (mut released, mut kept) = (0, 0);
    for a in all.iter_mut().flatten() {
        if a.client.is_none() {
            continue;
        }
        if a.proc_.crashed.load(Relaxed) {
            restart(w, a, &mut rng);
        }
        a.proc_.crash_in.store(0, Relaxed);
        let Some(client) = a.client.as_mut() else { continue };
        let result = definite(|| client.sync(0));
        match (a.backed_up, result) {
            (Some(false), Err(Error::Replaced)) => released += 1,
            (Some(true), Ok(_)) => kept += 1,
            (None, _) => {}
            (secured, other) => {
                // A secured name whose device was replaced meanwhile has a new device that never backed up.
                w.stats.violation(format!("{}: secured {secured:?}, a year later: {:?}", a.name, other.map(|_| ())))
            }
        }
        if a.backed_up == Some(false) {
            // Free again: anyone may take it.
            let probe = Proc::new(&w.net, &w.stats, 0, rng.next_u64());
            match Client::username_free(probe.as_ref(), &a.name) {
                Ok(true) => {}
                other => w.stats.violation(format!("{}: released but not free: {other:?}", a.name)),
            }
            a.client = None;
        }
    }
    println!("a year later: {released} names released, {kept} secured kept");
}

fn audit(w: &World) {
    let hub = w.hub.lock().unwrap();
    let Some(hub) = hub.as_ref() else { return };
    // Strays from deleted accounts go with the sweep; then nothing may be left over.
    hub.hub.prune(yo_hub::now_ms()).unwrap();
    match hub.hub.audit() {
        Ok(failed) if failed.is_empty() => println!("audit: the hub's database is consistent"),
        Ok(failed) => {
            for (what, n) in failed {
                w.stats.violation(format!("audit: {n} {what}"));
            }
        }
        Err(error) => w.stats.violation(format!("audit: {error}")),
    }
}
