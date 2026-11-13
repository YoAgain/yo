//! YoGPT: an account that answers every Yo, after a moment, with a Yo or a few. One process serves the account on
//! several hubs, each on its own.
//!
//!   yo-gpt [--replies DIR] --account NAME=HUB,DIR[,USERNAME] [--account …]
//!   yo-gpt --replies app/yogpt/replies --account dev=https://dev.justyo.app,state/dev,YoGPT
//!
//! Pictures: with `--replies`, a photo or video sent to it is answered with one picture from DIR (files named like
//! `007.jpg`, read as they are there at the time, at most 512 KB each), and one Yo in five it answers carries one
//! (`--picture-every N`: one in N; 1 always, 0 never). Without it, Yos only.
//!
//! The username defaults to YOGPT; once registered, the account keeps the name it has, whatever is given later.
//! Each account has its own client, state directory, threads and limits. Its first run registers the username with a
//! new account key; the directory then holds the account secret (`account`, 32 raw bytes), the state key (`key`) and
//! the sealed state (`state`), all 0600, and later runs go on from there. An account secret without a state registers
//! the name again with it (the same account, a new device). An account stops when the hub no longer knows its device
//! (replaced by another, the name released, or the hub reset) or cannot be opened; the others go on. The process
//! exits 1 when none is left.
//!
//! Anyone can write to it, so whatever comes is treated as hostile: it acts on a Yo, a photo or a video and nothing
//! else, never reads, logs or runs what a message says (a sender shows in the log as the start of its user id), and
//! holds a bounded amount of work per account: every Yo answered in turn, up to `MAX_OWED` owed to one sender,
//! `MAX_ANSWERING` senders at once, `MAX_QUEUED` events waiting; what does not fit is dropped and counted. A photo or
//! video sent to it is never opened, decoded, stored or logged: its encrypted blob is fetched into nothing (so the hub
//! can delete it at once), one at a time, at most `MAX_DRAINING` waiting, its key forgotten with the event. A panic
//! anywhere ends the process (systemd starts it again): the state on disk
//! is only ever replaced whole, after an operation completed.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use yo_core::media::{self, Media};
use yo_core::{Client, Content, Error, Event, Push, Response, Store, Transport};

/// How many Yos an answer has, in percent: one mostly, more ever rarer (a half-normal tail), five at most.
const YOS: [u32; 5] = [70, 17, 8, 4, 1];
/// Before the first answer to a sender, in milliseconds, uniform: it thinks. The answers after keep the rhythm the
/// Yos came in (as their senders sent them), each gap at most `MAX_GAP_MS`.
const THINK_MS: (u64, u64) = (1000, 3000);
const MAX_GAP_MS: u64 = 5000;
/// Senders being answered at once, per account (a thread each).
const MAX_ANSWERING: usize = 32;
/// Answers owed to one sender at most (a burst of Yos is answered one after the other, up to this many).
const MAX_OWED: usize = 200;
/// Events waiting for an account's dispatcher.
const MAX_QUEUED: usize = 1024;
/// An answer from the hub, at most: a full inbox fetch (100 envelopes of at most 65 KiB) fits.
const MAX_RESPONSE: u64 = 16 << 20;
/// Chats are kept fresh (spec 4) as the app does when it comes to the foreground.
const MAINTENANCE: Duration = Duration::from_secs(3600);
/// At most this many accounts on the command line.
const MAX_ACCOUNTS: usize = 8;
/// One Yo in this many carries a picture, by default.
const PICTURE_EVERY: u64 = 5;
/// The largest reply picture read; a larger file is skipped.
const MAX_PICTURE: u64 = 512 * 1024;
/// Files looked at in the replies directory, at most.
const MAX_PICTURES_LISTED: usize = 1000;
/// Blobs of photos and videos sent to it, waiting to be fetched into nothing.
const MAX_DRAINING: usize = 64;

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

/// What the whole process answers with.
#[derive(Clone)]
struct Options {
    /// Reply pictures, if any.
    replies: Option<PathBuf>,
    /// One Yo in this many carries a picture; 0 never.
    picture_every: u64,
}

/// Whether a roll (uniform in `0..every`) gives a picture: one in `every`, none for 0.
fn picture_due(roll: u64, every: u64) -> bool {
    every > 0 && roll == 0
}

/// A reply picture's file name: digits, `.jpg`.
fn picture_name(name: &str) -> bool {
    name.strip_suffix(".jpg").is_some_and(|stem| !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit()))
}

/// A JPEG's width and height from its frame header, if it has one where it should.
fn jpeg_size(bytes: &[u8]) -> Option<(u16, u16)> {
    let be16 = |i: usize| Some(u16::from_be_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]));
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while bytes.get(i) == Some(&0xFF) {
        let marker = *bytes.get(i + 1)?;
        // A start of frame (not DHT, JPG, DAC): height, then width.
        if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
            return Some((be16(i + 7)?, be16(i + 5)?));
        }
        i += 2 + be16(i + 2)? as usize;
    }
    None
}

/// One reply picture at random from the directory as it is now: its bytes, width and height.
fn pick_picture(dir: &Path) -> Option<(Vec<u8>, u16, u16)> {
    let names: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .take(MAX_PICTURES_LISTED)
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_str().is_some_and(picture_name))
        .map(|entry| entry.path())
        .collect();
    if names.is_empty() {
        return None;
    }
    let path = &names[uniform(0, names.len() as u64 - 1) as usize];
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(MAX_PICTURE + 1).read_to_end(&mut bytes).ok()?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_PICTURE {
        return None;
    }
    let (width, height) = jpeg_size(&bytes).unwrap_or((720, 1280));
    Some((bytes, width, height))
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

fn args() -> Result<(Vec<Account>, Options), String> {
    let mut accounts = Vec::new();
    let mut options = Options { replies: None, picture_every: PICTURE_EVERY };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().ok_or(format!("{flag}: a value"))?;
        match flag.as_str() {
            "--account" => accounts.push(account(&value)?),
            "--replies" => options.replies = Some(value.into()),
            "--picture-every" => options.picture_every = value.parse().map_err(|_| "--picture-every: a number")?,
            other => return Err(format!("unknown flag {other}")),
        }
    }
    if accounts.is_empty() || accounts.len() > MAX_ACCOUNTS {
        return Err(format!("1 to {MAX_ACCOUNTS} accounts: --account NAME=HUB,DIR[,USERNAME]"));
    }
    Ok((accounts, options))
}

/// One account on one hub: its client, and what bounds its work.
struct Bot {
    name: String,
    options: Options,
    /// Blob ids of photos and videos sent to it, for the drainer.
    draining: SyncSender<[u8; 16]>,
    client: Mutex<Client>,
    /// The hub no longer knows the device: everything of this account stops.
    stopped: AtomicBool,
    /// Senders being answered, with the answers still owed to each, the one under way first: true for an answer to
    /// a photo or video.
    pending: Mutex<HashMap<String, VecDeque<(bool, u64)>>>,
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

    /// Queues an event for the dispatcher, or drops it when the queue is full; never blocks. Only Yos, photos and
    /// videos are worth it; of a photo or video only that it came (its blob goes to the drainer, its key nowhere).
    fn queue(&self, events: &SyncSender<Event>, event: Event) {
        let event = match event {
            Event::Message { user, sent_at_ms, content: Content::Photo(media) | Content::Video { media, .. } } => {
                self.drain(&media);
                Event::Message { user, sent_at_ms, content: Content::Photo(unopened()) }
            }
            event @ Event::Message { content: Content::Yo, .. } => event,
            _ => return,
        };
        if let Err(TrySendError::Full(_)) = events.try_send(event) {
            self.dropped();
        }
    }

    /// Hands a blob's id to the drainer, or drops it when too many wait (the hub deletes it after its time).
    fn drain(&self, media: &Media) {
        if let Err(TrySendError::Full(_)) = self.draining.try_send(media.id) {
            self.dropped();
        }
    }
}

/// What the dispatcher is told of a photo or video: that one came, nothing of it.
fn unopened() -> Media {
    Media { id: [0; 16], key: [0; 32], sha256: [0; 32], size: 0, width: 0, height: 0 }
}

/// Fetches the blobs of photos and videos sent to the account into nothing, one at a time: each fetch counts at the
/// hub, which deletes a blob after its last. Nothing of them is kept, opened or logged.
fn drainer(hub: String, ids: std::sync::mpsc::Receiver<[u8; 16]>) {
    let Ok(http) = reqwest::blocking::Client::builder().timeout(Duration::from_secs(120)).build() else { return };
    while let Ok(id) = ids.recv() {
        let url = format!("{hub}{}", yo_core::media::blob_path(&id));
        if let Ok(response) = http.get(url).send() {
            let _ = std::io::copy(&mut response.take(media::MAX_MEDIA_LEN as u64 + 64), &mut std::io::sink());
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
                // It never opens a photo or video: their keys are not even kept with the events.
                client.forget_media_keys();
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
    let (accounts, options) = args().unwrap_or_else(|error| {
        eprintln!("yo-gpt: {error}");
        std::process::exit(2);
    });
    // A panic in any thread ends the process, after the usual message; systemd starts it again.
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        report(info);
        std::process::exit(101);
    }));
    let serving: Vec<_> = accounts
        .into_iter()
        .map(|account| {
            let options = options.clone();
            std::thread::spawn(move || serve(&account, options))
        })
        .collect();
    for thread in serving {
        let _ = thread.join();
    }
    eprintln!("yo-gpt: no account left");
    std::process::exit(1);
}

/// Serves one account until it stops.
/// The operator's mailbox: `NAME.anything.jpg` dropped into the account's `outbox` (root only, on the server) goes
/// to NAME as a photo with its Yo, then the file is removed; one that could not go is renamed `.failed`.
fn mailbox(bot: &Bot, dir: &Path, events: &SyncSender<Event>) {
    let _ = std::fs::create_dir_all(dir);
    while bot.running() {
        std::thread::sleep(Duration::from_secs(5));
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for path in entries.flatten().map(|entry| entry.path()).take(20) {
            let Some(name) = path.file_name().and_then(|n| n.to_str()).filter(|n| n.ends_with(".jpg")) else {
                continue;
            };
            let to = name.split('.').next().unwrap_or_default().to_owned();
            let mut bytes = Vec::new();
            let read = std::fs::File::open(&path).and_then(|f| f.take(MAX_PICTURE + 1).read_to_end(&mut bytes));
            if to.is_empty() || read.is_err() || bytes.is_empty() || bytes.len() as u64 > MAX_PICTURE {
                let _ = std::fs::rename(&path, path.with_extension("failed"));
                continue;
            }
            let (width, height) = jpeg_size(&bytes).unwrap_or((720, 1280));
            match send_this_picture(bot, &to, &bytes, width, height) {
                Ok((received, sent)) => {
                    received.into_iter().for_each(|event| bot.queue(events, event));
                    let _ = std::fs::remove_file(&path);
                    bot.log(format!("mail to {to}{}", if sent { "" } else { " (as a plain Yo)" }));
                }
                Err(error) if gone(&error) => return bot.stop(GONE),
                Err(error) => {
                    bot.log(format!("mail to {to}: {}", reason(&error)));
                    let _ = std::fs::rename(&path, path.with_extension("failed"));
                }
            }
        }
    }
}

fn serve(account: &Account, options: Options) {
    let Some(client) = open(account) else { return };
    let (draining, to_drain) = sync_channel(MAX_DRAINING);
    let hub = account.hub.clone();
    std::thread::spawn(move || drainer(hub, to_drain));
    let bot = Arc::new(Bot {
        name: account.name.clone(),
        options,
        draining,
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
    let (mailing, to_queue, outbox) = (bot.clone(), events.clone(), account.state.join("outbox"));
    std::thread::spawn(move || mailbox(&mailing, &outbox, &to_queue));
    // Every Yo gets its answer: one sender's in turn, on one thread; senders side by side.
    while bot.running() {
        let event = match received.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let (user, picture, sent_at) = match event {
            Event::Message { user, sent_at_ms, content: Content::Yo } => (user, false, sent_at_ms),
            Event::Message { user, sent_at_ms, content: Content::Photo(_) } => (user, true, sent_at_ms),
            _ => continue,
        };
        {
            let mut pending = bot.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(owed) = pending.get_mut(&user) {
                if owed.len() < MAX_OWED {
                    owed.push_back((picture, sent_at));
                } else {
                    bot.dropped();
                }
                continue;
            }
            if pending.len() >= MAX_ANSWERING {
                bot.dropped();
                continue;
            }
            pending.insert(user.clone(), VecDeque::from([(picture, sent_at)]));
        }
        let (answering, events, sender) = (bot.clone(), events.clone(), user.clone());
        let spawned = std::thread::Builder::new().spawn(move || {
            // It thinks before its first answer only.
            std::thread::sleep(ms(THINK_MS));
            let (mut picture, mut last) = (picture, sent_at);
            loop {
                // The rhythm counts from when an answer began: its own time (a picture sealed, uploaded and sent, a
                // few Yos) is part of the gap, never added to it.
                let began = Instant::now();
                answer(&answering, &sender, &events, picture);
                let next = {
                    let mut pending = answering.pending.lock().unwrap_or_else(PoisonError::into_inner);
                    let Some(owed) = pending.get_mut(&sender) else { break };
                    owed.pop_front();
                    match owed.front() {
                        Some(&next) => next,
                        None => {
                            pending.remove(&sender);
                            break;
                        }
                    }
                };
                // As far apart as the sender's Yos were (their clocks' difference, never more than MAX_GAP_MS).
                let gap = Duration::from_millis(next.1.saturating_sub(last).min(MAX_GAP_MS));
                std::thread::sleep(gap.saturating_sub(began.elapsed()));
                (picture, last) = next;
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

/// Sends a Yo or a few, one in `picture_every` of them with a picture; to a photo or video, one Yo with a picture.
/// What sending receives goes to the dispatcher.
fn answer(bot: &Bot, user: &str, events: &SyncSender<Event>, to_picture: bool) {
    let n = if to_picture { 1 } else { yos(uniform(0, 99) as u32) };
    let every = bot.options.picture_every;
    let with_picture = match to_picture {
        true => Some(0),
        false => picture_due(uniform(0, every.max(1) - 1), every).then(|| uniform(0, n as u64 - 1) as usize),
    };
    let mut pictures = 0;
    for i in 0..n {
        if !bot.running() {
            return;
        }
        let sent = match with_picture == Some(i) {
            true => send_picture(bot, user).inspect(|(_, sent)| pictures += *sent as usize).map(|(events, _)| events),
            false => bot.client().send_yo(user),
        };
        match sent {
            Ok(received) => received.into_iter().for_each(|event| bot.queue(events, event)),
            Err(error) if gone(&error) => return bot.stop(GONE),
            Err(error) => return bot.log(format!("Yo {} of {n} to {}: {}", i + 1, who(user), reason(&error))),
        }
    }
    let with = if pictures > 0 { " with a picture" } else { "" };
    bot.log(format!("{n} Yo back to {}{with}", who(user)));
}

/// A Yo with a reply picture; a plain one when there is none or the hub takes no picture now. True when it had one.
fn send_picture(bot: &Bot, user: &str) -> Result<(Vec<Event>, bool), Error> {
    let picked = bot.options.replies.as_deref().and_then(pick_picture);
    let Some((bytes, width, height)) = picked else { return bot.client().send_yo(user).map(|e| (e, false)) };
    send_this_picture(bot, user, &bytes, width, height)
}

/// A Yo with this picture; a plain one when the hub takes no picture now. True when it had the picture.
fn send_this_picture(
    bot: &Bot,
    user: &str,
    bytes: &[u8],
    width: u16,
    height: u16,
) -> Result<(Vec<Event>, bool), Error> {
    let (media, sealed) = media::seal(bytes, width, height)?;
    // Signed under the client, sent without it: other answers go on meanwhile.
    let upload = {
        let mut client = bot.client();
        let fetches = client.media_fetches(user).unwrap_or(1);
        client.upload(&media, fetches)
    };
    match upload.send(&sealed) {
        Ok(()) => bot.client().send(user, Content::Photo(media)).map(|e| (e, true)),
        Err(error) if gone(&error) => Err(error),
        Err(error) => {
            bot.log(format!("picture to {}: {}", who(user), reason(&error)));
            bot.client().send_yo(user).map(|e| (e, false))
        }
    }
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
    fn one_yo_in_five_carries_a_picture() {
        let due = |every: u64| (0..100).filter(|roll| picture_due(roll % every.max(1), every)).count();
        assert_eq!(due(5), 20);
        assert_eq!(due(1), 100);
        assert_eq!(due(0), 0);
    }

    #[test]
    fn reply_pictures_are_named_by_number_and_read_bounded() {
        assert!(picture_name("007.jpg") && picture_name("200.jpg"));
        for name in ["", ".jpg", "a.jpg", "007.jpeg", "007.jpg.sh", "../1.jpg", "1.JPG"] {
            assert!(!picture_name(name), "{name}");
        }
        // A frame header after an APP0 segment: 720 wide, 1280 high.
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 17, 8, 0x05, 0x00, 0x02, 0xD0, 3, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(jpeg_size(&jpeg), Some((720, 1280)));
        assert_eq!(jpeg_size(&jpeg[..12]), None);
        assert_eq!(jpeg_size(b"not a jpeg"), None);
        let dir = std::env::temp_dir().join(format!("yogpt-pick-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(pick_picture(&dir).is_none(), "nothing there");
        std::fs::write(dir.join("001.jpg"), vec![0u8; MAX_PICTURE as usize + 1]).unwrap();
        assert!(pick_picture(&dir).is_none(), "too large");
        std::fs::write(dir.join("001.jpg"), jpeg).unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        assert_eq!(pick_picture(&dir).map(|(b, w, h)| (b.len(), w, h)), Some((jpeg.len(), 720, 1280)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn uniform_stays_in_its_bounds() {
        for _ in 0..1000 {
            assert!((300..=1000).contains(&uniform(300, 1000)));
        }
        assert!((0..1000).map(|_| uniform(0, 99)).any(|p| p == 0));
    }
}
