//! YoGPT: an account that answers every Yo, after a moment, with a Yo or a few. One process serves the account on
//! several hubs, each on its own.
//!
//!   yo-gpt --account NAME=HUB,DIR[,USERNAME] [--account …]
//!   yo-gpt --account dev=https://dev.justyo.app,state/dev,YoGPT
//!
//! The username defaults to YOGPT; once registered, the account keeps the name it has, whatever is given later.
//! Each account has its own client, state directory, threads and limits. Its first run registers the username with a
//! new account key; the directory then holds the account secret (`account`, 32 raw bytes), the state key (`key`) and
//! the sealed state (`state`), all 0600, and later runs go on from there. An account secret without a state registers
//! the name again with it (the same account, a new device). An account stops when the hub no longer knows its device
//! (replaced by another, the name released, or the hub reset) or cannot be opened; the others go on. The process
//! exits 1 when none is left.
//!
//! Anyone can write to it, so whatever comes is treated as hostile: it acts on a Yo and nothing else, never reads,
//! logs or runs what a message says (a sender shows in the log as the start of its user id), and holds a bounded
//! amount of work per account: every Yo answered in turn, up to `MAX_OWED` owed to one sender, `MAX_ANSWERING`
//! senders at once, `MAX_QUEUED` events waiting; what
//! does not fit is dropped and counted. A panic anywhere ends the process (systemd starts it again): the state on disk
//! is only ever replaced whole, after an operation completed.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use yo_core::{Client, Content, Error, Event, Push, Response, Store, Transport};

/// How many Yos an answer has, in percent: one mostly, more ever rarer (a half-normal tail), five at most.
const YOS: [u32; 5] = [70, 17, 8, 4, 1];
/// Before an answer, in milliseconds, uniform: it thinks.
const THINK_MS: (u64, u64) = (1000, 3000);
/// Between the Yos of one answer, in milliseconds, uniform.
const GAP_MS: (u64, u64) = (300, 1000);
/// Senders being answered at once, per account (a thread each).
const MAX_ANSWERING: usize = 32;
/// Answers owed to one sender at most (a burst of Yos is answered one after the other, up to this many).
const MAX_OWED: u32 = 200;
/// Events waiting for an account's dispatcher.
const MAX_QUEUED: usize = 1024;
/// An answer from the hub, at most: a full inbox fetch (100 envelopes of at most 65 KiB) fits.
const MAX_RESPONSE: u64 = 16 << 20;
/// Chats are kept fresh (spec 4) as the app does when it comes to the foreground.
const MAINTENANCE: Duration = Duration::from_secs(3600);
/// At most this many accounts on the command line.
const MAX_ACCOUNTS: usize = 8;

struct Http {
    client: reqwest::blocking::Client,
    base: String,
}

impl Transport for Http {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        let url = format!("{}{}", self.base, path);
        let mut request = self.client.request(method.parse().map_err(|_| "method")?, url).body(body.to_vec());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let mut body = Vec::new();
        response.take(MAX_RESPONSE + 1).read_to_end(&mut body).map_err(|e| e.to_string())?;
        if body.len() as u64 > MAX_RESPONSE {
            return Err("answer too large".into());
        }
        Ok(Response { status, body })
    }
}

/// The sealed state, replaced whole after every operation.
struct File(PathBuf);

impl Store for File {
    fn save(&self, blob: Vec<u8>) {
        if let Err(error) = write_private(&self.0, &blob) {
            eprintln!("yo-gpt: saving {}: {error}", self.0.display());
        }
    }
}

/// Writes beside the file, then renames: a crash leaves the old one or the new one.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(tmp, path)
}

/// 32 bytes kept in `path`, made at random the first time.
fn secret(path: &Path) -> std::io::Result<[u8; 32]> {
    match std::fs::read(path) {
        Ok(bytes) => bytes.try_into().map_err(|_| std::io::Error::other(format!("{}: not 32 bytes", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut bytes = [0u8; 32];
            getrandom::getrandom(&mut bytes).map_err(std::io::Error::other)?;
            write_private(path, &bytes)?;
            Ok(bytes)
        }
        Err(error) => Err(error),
    }
}

/// Uniform in `low..=high`, from the system's random source.
fn uniform(low: u64, high: u64) -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).expect("random");
    low + u64::from_le_bytes(bytes) % (high - low + 1)
}

/// How many Yos for a percentile `p` in 0..100, by `YOS`.
fn yos(p: u32) -> usize {
    let mut sum = 0;
    YOS.iter()
        .position(|w| {
            sum += w;
            p < sum
        })
        .unwrap_or(YOS.len() - 1)
        + 1
}

fn ms((low, high): (u64, u64)) -> Duration {
    Duration::from_millis(uniform(low, high))
}

struct Account {
    name: String,
    hub: String,
    state: PathBuf,
    username: String,
}

/// `NAME=HUB,DIR[,USERNAME]`.
fn account(value: &str) -> Result<Account, String> {
    let bad = || format!("--account {value}: NAME=HUB,DIR[,USERNAME]");
    let (name, rest) = value.split_once('=').ok_or_else(bad)?;
    let mut parts = rest.splitn(3, ',');
    let (hub, state) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    let username = parts.next().unwrap_or("YOGPT");
    if [name, hub, state, username].iter().any(|part| part.is_empty()) {
        return Err(bad());
    }
    let hub = hub.trim_end_matches('/').into();
    Ok(Account { name: name.into(), hub, state: state.into(), username: username.into() })
}

fn args() -> Result<Vec<Account>, String> {
    let mut accounts = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().ok_or(format!("{flag}: a value"))?;
        match flag.as_str() {
            "--account" => accounts.push(account(&value)?),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    if accounts.is_empty() || accounts.len() > MAX_ACCOUNTS {
        return Err(format!("1 to {MAX_ACCOUNTS} accounts: --account NAME=HUB,DIR[,USERNAME]"));
    }
    Ok(accounts)
}

/// One account on one hub: its client, and what bounds its work.
struct Bot {
    name: String,
    client: Mutex<Client>,
    /// The hub no longer knows the device: everything of this account stops.
    stopped: AtomicBool,
    /// Senders being answered, with the answers still owed to each.
    pending: Mutex<HashMap<String, u32>>,
    dropped: AtomicU64,
    dropped_logged_s: AtomicU64,
}

impl Bot {
    fn log(&self, what: impl std::fmt::Display) {
        eprintln!("yo-gpt [{}]: {what}", self.name);
    }

    fn client(&self) -> std::sync::MutexGuard<'_, Client> {
        self.client.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn stop(&self, why: impl std::fmt::Display) {
        if !self.stopped.swap(true, Ordering::Relaxed) {
            self.log(format!("stopped: {why}"));
        }
    }

    fn running(&self) -> bool {
        !self.stopped.load(Ordering::Relaxed)
    }

    /// Something dropped for want of room; logged at most once a minute.
    fn dropped(&self) {
        let n = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let last = self.dropped_logged_s.load(Ordering::Relaxed);
        if now >= last + 60
            && self.dropped_logged_s.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
        {
            self.log(format!("{n} dropped so far (too much at once)"));
        }
    }

    /// Queues an event for the dispatcher, or drops it when the queue is full; never blocks. Only Yos are worth it.
    fn queue(&self, events: &SyncSender<Event>, event: Event) {
        if !matches!(event, Event::Message { content: Content::Yo, .. }) {
            return;
        }
        if let Err(TrySendError::Full(_)) = events.try_send(event) {
            self.dropped();
        }
    }
}

/// Registers or restores, trying again while the hub cannot be reached; `None` (logged) when it cannot be opened.
fn open(account: &Account) -> Option<Client> {
    let username = account.username.as_str();
    let log = |what: String| eprintln!("yo-gpt [{}]: {what}", account.name);
    let prepared = (|| {
        std::fs::create_dir_all(&account.state)?;
        std::fs::set_permissions(&account.state, std::fs::Permissions::from_mode(0o700))?;
        Ok::<_, std::io::Error>((secret(&account.state.join("account"))?, secret(&account.state.join("key"))?))
    })();
    let (secret, key) = prepared.map_err(|e| log(format!("stopped: {}: {e}", account.state.display()))).ok()?;
    let path = account.state.join("state");
    let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(60)).build().ok()?;
    let transport = Arc::new(Http { client: http, base: account.hub.clone() });
    let mut backoff = Backoff::default();
    loop {
        let opened = match std::fs::read(&path) {
            Ok(blob) => Client::restore(transport.clone(), &blob, &key),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Client::register_username(transport.clone(), username, Some(secret), None, Push::none())
                    .inspect(|c| log(format!("registered {} on {}", c.username().unwrap_or("?"), account.hub)))
            }
            Err(error) => {
                log(format!("stopped: {}: {error}", path.display()));
                return None;
            }
        };
        match opened {
            Ok(mut client) => {
                client.persist_to(Arc::new(File(path)), key);
                return Some(client);
            }
            Err(error) if retryable(&error) => backoff.wait(&error, &account.name),
            Err(error) => {
                log(format!("stopped: {username} on {}: {}", account.hub, reason(&error)));
                return None;
            }
        }
    }
}

/// Network trouble, or the hub saying "not now".
fn retryable(error: &Error) -> bool {
    matches!(error, Error::Network(_) | Error::Refused(429 | 500..))
}

const GONE: &str = "the hub no longer knows this device (replaced by another, the name released, or the hub reset)";

/// `410`: replaced, or the name released. `401`: unknown, as after a reset hub (or a clock far off).
fn gone(error: &Error) -> bool {
    matches!(error, Error::Replaced | Error::Refused(401))
}

/// An error for the log, without text that came from outside (OpenMLS's reasons).
fn reason(error: &Error) -> String {
    match error {
        Error::Mls(_) => "mls".into(),
        other => other.to_string(),
    }
}

/// How the log names a sender: the start of its user id (hex), nothing it chose itself.
fn who(user: &str) -> String {
    user.chars().take(8).filter(char::is_ascii_hexdigit).collect()
}

/// 1, 2, 4 … 60 seconds; back to 1 after a success.
#[derive(Default)]
struct Backoff(u32);

impl Backoff {
    fn wait(&mut self, error: &Error, name: &str) {
        let s = (1u64 << self.0.min(6)).min(60);
        eprintln!("yo-gpt [{name}]: {}, again in {s} s", reason(error));
        std::thread::sleep(Duration::from_secs(s));
        self.0 += 1;
    }
}

fn main() {
    let accounts = args().unwrap_or_else(|error| {
        eprintln!("yo-gpt: {error}");
        std::process::exit(2);
    });
    // A panic in any thread ends the process, after the usual message; systemd starts it again.
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        report(info);
        std::process::exit(101);
    }));
    let serving: Vec<_> = accounts.into_iter().map(|account| std::thread::spawn(move || serve(&account))).collect();
    for thread in serving {
        let _ = thread.join();
    }
    eprintln!("yo-gpt: no account left");
    std::process::exit(1);
}

/// Serves one account until it stops.
fn serve(account: &Account) {
    let Some(client) = open(account) else { return };
    let bot = Arc::new(Bot {
        name: account.name.clone(),
        client: Mutex::new(client),
        stopped: AtomicBool::new(false),
        pending: Mutex::default(),
        dropped: AtomicU64::new(0),
        dropped_logged_s: AtomicU64::new(0),
    });
    let started = {
        let client = bot.client();
        format!("{} on {}, user {}", client.username().unwrap_or("?"), account.hub, client.user_id())
    };
    bot.log(started);
    let (events, received) = sync_channel(MAX_QUEUED);
    let (receiving, to_dispatch) = (bot.clone(), events.clone());
    std::thread::spawn(move || receive(&receiving, &to_dispatch));
    // Every Yo gets its answer: one sender's in turn, on one thread; senders side by side.
    while bot.running() {
        let event = match received.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let Event::Message { user, content: Content::Yo, .. } = event else { continue };
        {
            let mut pending = bot.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(owed) = pending.get_mut(&user) {
                if *owed < MAX_OWED {
                    *owed += 1;
                } else {
                    bot.dropped();
                }
                continue;
            }
            if pending.len() >= MAX_ANSWERING {
                bot.dropped();
                continue;
            }
            pending.insert(user.clone(), 1);
        }
        let (answering, events, sender) = (bot.clone(), events.clone(), user.clone());
        let spawned = std::thread::Builder::new().spawn(move || loop {
            answer(&answering, &sender, &events);
            let mut pending = answering.pending.lock().unwrap_or_else(PoisonError::into_inner);
            match pending.get_mut(&sender) {
                Some(owed) if *owed > 1 => *owed -= 1,
                _ => {
                    pending.remove(&sender);
                    break;
                }
            }
        });
        if spawned.is_err() {
            bot.pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&user);
            bot.dropped();
        }
    }
}

/// The long poll, without holding the client while it waits; every Yo to the dispatcher.
fn receive(bot: &Bot, events: &SyncSender<Event>) {
    let mut backoff = Backoff::default();
    let mut maintained = Instant::now();
    while bot.running() {
        let result = if maintained.elapsed() >= MAINTENANCE {
            maintained = Instant::now();
            bot.client().maintenance()
        } else {
            let wait = bot.client().sync_begin(25);
            wait.and_then(|wait| {
                let response = wait.send();
                bot.client().sync_end(&wait, response)
            })
        };
        match result {
            Ok(received) => {
                backoff = Backoff::default();
                received.into_iter().for_each(|event| bot.queue(events, event));
                bot.client().acknowledge_events();
            }
            Err(error) if gone(&error) => bot.stop(GONE),
            Err(error) => backoff.wait(&error, &bot.name),
        }
    }
}

/// Thinks, then sends a Yo or a few. What sending receives goes to the dispatcher.
fn answer(bot: &Bot, user: &str, events: &SyncSender<Event>) {
    std::thread::sleep(ms(THINK_MS));
    let n = yos(uniform(0, 99) as u32);
    for i in 0..n {
        if i > 0 {
            std::thread::sleep(ms(GAP_MS));
        }
        if !bot.running() {
            return;
        }
        let sent = bot.client().send_yo(user);
        match sent {
            Ok(received) => received.into_iter().for_each(|event| bot.queue(events, event)),
            Err(error) if gone(&error) => return bot.stop(GONE),
            Err(error) => return bot.log(format!("Yo {} of {n} to {}: {}", i + 1, who(user), reason(&error))),
        }
    }
    bot.log(format!("{n} Yo back to {}", who(user)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yos_follow_the_table() {
        assert_eq!(YOS.iter().sum::<u32>(), 100);
        let mut counts = [0u32; 5];
        for p in 0..100 {
            counts[yos(p) - 1] += 1;
        }
        assert_eq!(counts, YOS);
    }

    #[test]
    fn uniform_stays_in_its_bounds() {
        for _ in 0..1000 {
            assert!((300..=1000).contains(&uniform(300, 1000)));
        }
        assert!((0..1000).map(|_| uniform(0, 99)).any(|p| p == 0));
    }
}
