//! YoGPT: an account that answers every Yo, after a moment, with a Yo or a few. One process serves the account on
//! several hubs, each on its own.
//!
//!   yo-gpt [--replies DIR] --account NAME=HUB,DIR[,USERNAME] [--hub-key NAME=KEY] [--trust-root NAME=KEY]
//!          [--zk-params NAME=PARAMS] [--account …] [--echo NAME] [--quiet-for USERNAME[,USERNAME…]]
//!          [--make-group "GROUP NAME=USERNAME,USERNAME…" …]
//!   yo-gpt --replies app/yogpt/replies --account dev=https://dev.justyo.app,state/dev,YoGPT
//!
//! `--echo NAME` reserves that account for immediate echoes with aggregate-only logs: what it gets goes back once,
//! as it came (a Yo as a Yo, a photo or video as the same one, sealed anew), to the sender or into the group; see
//! README.md for echo deployment and limits. Normal accounts keep the behavior below.
//!
//! `--quiet-for USERNAME[,USERNAME…]` (given once or more, any case): Yos, photos and videos from these users are never
//! answered, one to one or in a group (by the group message's sender), so bot accounts in one group never answer each
//! other. Each account looks the names up once at start (a name not found yet is tried again every minute) and
//! compares user ids; its own name is left out. Only how many were passed over is logged. Independently, an account
//! answers into one group at most once per `GROUP_COOLDOWN` (30 s): group Yos within that window are dropped, counted
//! in the log only.
//!
//! Pictures: with `--replies`, a photo or video sent to it is answered with one picture from DIR (files named like
//! `007.jpg`, read as they are there at the time, at most 512 KB each), always; a plain Yo gets Yos only, unless
//! `--picture-every N` (one in N; 1 always, 0 never, the default) adds one now and then. Without `--replies`, Yos only. Each picture is drawn uniformly at random,
//! leaving out the last `RECENT_PICTURES` that sender got (remembered in memory only, for at most `MAX_REMEMBERED`
//! senders) while others are left.
//!
//! `--hub-key NAME=KEY` pins a hub key of that account's hub (base64url of its 32-byte public key, spec 6.10), given
//! once per key (the current one and a spare): answers that hand out keys are then taken only signed by one of them.
//! `--trust-root NAME=KEY` pins a sender-certificate trust root of that account's hub (base64url of 32 bytes, spec
//! 6.1, as `yo-hub trust-root new` printed it), once per key (the current one and a spare): without one no sealed Yo
//! opens and its own go identified. `--zk-params NAME=PARAMS` pins the hub's zkgroup public params (base64url, as
//! `yo-hub zk new` printed them, spec 4.9): without them it answers in no group and leaves none.
//!
//! `--make-group "GROUP NAME=USERNAME,…"` (repeatable, needs the account's `--zk-params`): the FIRST account makes
//! each group with those members (looked up by username), prints each group's id and exits (1 if one failed); run
//! with the service stopped, like `--yo`. A name is 1 to 32 bytes (UTF-8, emoji count 4 each), no control characters.
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
//! video sent to it (other than to `--echo`, which sends it back as it came) is never opened, decoded, stored or logged: its encrypted blob is fetched into nothing (so the hub
//! can delete it at once), one at a time, at most `MAX_DRAINING` waiting, its key forgotten with the event. Account
//! workers log unexpected exits and panics and restart; the state on disk is only ever replaced whole, after an
//! operation completed.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use yo_core::media::{self, Media};
use yo_core::{retryable, Client, Content, Error, Event, Push, Response, Store, Transport};

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
/// What waits is answered and sent (spec 4) as the app does when it comes to the foreground.
const MAINTENANCE: Duration = Duration::from_secs(3600);
/// At most this many accounts on the command line.
const MAX_ACCOUNTS: usize = 8;
/// One Yo in this many carries a picture, by default: none (a picture answers a picture only).
const PICTURE_EVERY: u64 = 0;
/// The largest reply picture read; a larger file is skipped.
const MAX_PICTURE: u64 = 512 * 1024;
/// Files looked at in the replies directory, at most.
const MAX_PICTURES_LISTED: usize = 1000;
/// Pictures sent to one sender that are not sent to them again while others are left.
const RECENT_PICTURES: usize = 50;
/// Senders whose recent pictures are remembered, at most; the one remembered longest is forgotten first.
const MAX_REMEMBERED: usize = 1000;
/// Blobs of photos and videos sent to it, waiting to be fetched into nothing.
const MAX_DRAINING: usize = 64;
/// One answer into a group at most this often, per account: what comes within is dropped (no loop between bots).
const GROUP_COOLDOWN: Duration = Duration::from_secs(30);
/// Names of `--quiet-for` not found on the hub are looked up again this often.
const QUIET_RETRY: Duration = Duration::from_secs(60);

/// Its `Yo-App`: "yogpt/" and the build's UTC minute (YYYYMMDDHHMM, set by yo-hub's deploy as `YO_BUILD`).
const BUILD: &str = match option_env!("YO_BUILD") {
    Some(build) => build,
    None => "0",
};

/// Signed requests and anonymous ones (sealed deliveries, blob fetches) on separate clients: never one connection for
/// both (spec 1).
struct Http {
    client: reqwest::blocking::Client,
    anonymous: reqwest::blocking::Client,
    base: String,
}

impl Transport for Http {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        let url = format!("{}{}", self.base, path);
        // Only what names nobody goes anonymously; a registration or a backup's fetch names the account (spec 1).
        let signed = headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("yo-device"));
        let anonymous = !signed && yo_wire::api::anonymous(method, path);
        let client = if anonymous { &self.anonymous } else { &self.client };
        let mut request = client.request(method.parse().map_err(|_| "method")?, url).body(body.to_vec());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        // Its build, as an app names its own (spec 6.3), on what names the account anyway: never on an anonymous one.
        if signed {
            request = request.header(yo_wire::api::HEADER_APP, format!("yogpt/{BUILD}"));
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
    fn save(&self, blob: Vec<u8>) -> Result<(), String> {
        write_private(&self.0, &blob).map_err(|error| format!("saving {}: {error}", self.0.display()))
    }
}

/// Writes beside the file, then renames, then syncs the directory: a crash or a power cut leaves the old one or the
/// new one, and once this returns, the new one.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(tmp, path)?;
    std::fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()
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

/// Uniform in `low..=high`, from the system's random source (without modulo bias: draws past the last whole multiple
/// of the span are drawn again).
fn uniform(low: u64, high: u64) -> u64 {
    let span = high - low + 1;
    let zone = u64::MAX - u64::MAX % span;
    loop {
        let mut bytes = [0u8; 8];
        getrandom::getrandom(&mut bytes).expect("random");
        let draw = u64::from_le_bytes(bytes);
        if draw < zone {
            return low + draw % span;
        }
    }
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

/// `THINK_MS`, or `YO_GPT_THINK_MS=low,high` (tests: a think far longer than any slow runner's echo, so timing tells
/// the paths apart).
fn think_ms() -> (u64, u64) {
    std::env::var("YO_GPT_THINK_MS")
        .ok()
        .and_then(|v| v.split_once(',').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))))
        .unwrap_or(THINK_MS)
}

/// `GROUP_COOLDOWN`, or `YO_GPT_GROUP_COOLDOWN_MS` (tests).
fn group_cooldown() -> Duration {
    std::env::var("YO_GPT_GROUP_COOLDOWN_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(GROUP_COOLDOWN, Duration::from_millis)
}

fn ms((low, high): (u64, u64)) -> Duration {
    Duration::from_millis(uniform(low, high))
}

struct Account {
    name: String,
    hub: String,
    state: PathBuf,
    username: String,
    echo: bool,
    /// The hub's public keys, pinned (`--hub-key`, as often as there are: the current one and a spare).
    hub_keys: Vec<[u8; 32]>,
    /// The hub's sender-certificate trust roots (`--trust-root`, the current one and a spare): without one, no sealed
    /// Yo opens (spec 6.1).
    trust_roots: Vec<[u8; 32]>,
    /// The hub's zkgroup public params (`--zk-params`): without them it makes, changes and fetches no group (spec 4.9).
    zk_params: Option<Vec<u8>>,
}

/// What the whole process answers with.
#[derive(Clone)]
struct Options {
    /// Reply pictures, if any.
    replies: Option<PathBuf>,
    /// One Yo in this many carries a picture; 0 never.
    picture_every: u64,
    /// `--yo NAME=COUNT`: send that many Yos to NAME from each account, then exit (the service stopped first: one
    /// process per state).
    yo: Option<(String, u32)>,
    /// With `yo`: each Yo carries this JPEG (at most 1600 px a side), as a reply picture.
    photo: Option<PathBuf>,
    /// `--quiet-for`: usernames whose Yos are never answered (upper case).
    quiet_for: Vec<String>,
    /// `--make-group "NAME=USER,USER…"`: the first account makes these groups (name, member usernames), then exits.
    make_groups: Vec<(String, Vec<String>)>,
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

/// A JPEG as a photo goes (spec 5): its metadata segments (APP1 to APP15, comments: EXIF, GPS, XMP) taken out, and
/// its width and height, at most 1600 px; `None` for no JPEG or a larger one.
fn jpeg_clean(bytes: &[u8]) -> Option<(Vec<u8>, u16, u16)> {
    let (width, height) = jpeg_size(bytes)?;
    if width.max(height) > 1600 {
        return None;
    }
    let be16 = |i: usize| Some(u16::from_be_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]));
    let mut out = bytes[..2].to_vec();
    let mut i = 2;
    loop {
        if *bytes.get(i)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(i + 1)?;
        if marker == 0xD9 {
            // The end of the image: nothing after it goes along.
            out.extend_from_slice(&[0xFF, 0xD9]);
            return Some((out, width, height));
        }
        let end = i + 2 + be16(i + 2)? as usize;
        let segment = bytes.get(i..end)?;
        if !((0xE1..=0xEF).contains(&marker) || marker == 0xFE) {
            out.extend_from_slice(segment);
        }
        i = end;
        if marker == 0xDA {
            // A scan's data, as it is, up to the next marker (0xFF then neither 0 nor a restart): progressive JPEGs
            // have several scans, with segments between them.
            let start = i;
            while i + 1 < bytes.len()
                && !(bytes[i] == 0xFF && bytes[i + 1] != 0 && !(0xD0..=0xD7).contains(&bytes[i + 1]))
            {
                i += 1;
            }
            out.extend_from_slice(bytes.get(start..i)?);
            if i + 1 >= bytes.len() {
                return None; // no end of image
            }
        }
    }
}

/// The reply pictures each sender got last (file names), bounded: `RECENT_PICTURES` per sender, `MAX_REMEMBERED`
/// senders.
#[derive(Default)]
struct Recent {
    by_sender: HashMap<String, VecDeque<String>>,
    /// Senders in the order they were first remembered.
    order: VecDeque<String>,
}

impl Recent {
    fn of(&self, user: &str) -> Option<&VecDeque<String>> {
        self.by_sender.get(user)
    }

    fn sent(&mut self, user: &str, name: String) {
        if !self.by_sender.contains_key(user) {
            if self.order.len() >= MAX_REMEMBERED {
                if let Some(oldest) = self.order.pop_front() {
                    self.by_sender.remove(&oldest);
                }
            }
            self.order.push_back(user.to_owned());
        }
        let list = self.by_sender.entry(user.to_owned()).or_default();
        list.push_back(name);
        while list.len() > RECENT_PICTURES {
            list.pop_front();
        }
    }
}

/// Which of `names` to send, uniform among those not in `recent`; when every one is (fewer pictures than
/// `RECENT_PICTURES`), among all but the last one sent.
fn choose(names: &[String], recent: Option<&VecDeque<String>>) -> Option<usize> {
    let seen = |name: &String| recent.is_some_and(|r| r.contains(name));
    let mut pool: Vec<usize> = (0..names.len()).filter(|&i| !seen(&names[i])).collect();
    if pool.is_empty() {
        let last = recent.and_then(|r| r.back());
        pool = (0..names.len()).filter(|&i| Some(&names[i]) != last).collect();
    }
    if pool.is_empty() {
        pool = (0..names.len()).collect();
    }
    let last = pool.len().checked_sub(1)? as u64;
    Some(pool[uniform(0, last) as usize])
}

/// One reply picture at random from the directory as it is now, not one of `recent` while others are left: its file
/// name, bytes, width and height.
fn pick_picture(dir: &Path, recent: Option<&VecDeque<String>>) -> Option<(String, Vec<u8>, u16, u16)> {
    let names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .take(MAX_PICTURES_LISTED)
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| picture_name(name))
        .collect();
    let name = names.get(choose(&names, recent)?)?;
    let mut bytes = Vec::new();
    std::fs::File::open(dir.join(name)).ok()?.take(MAX_PICTURE + 1).read_to_end(&mut bytes).ok()?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_PICTURE {
        return None;
    }
    // As a photo goes: no metadata, at most 1600 px; another file is no reply picture.
    let (bytes, width, height) = jpeg_clean(&bytes)?;
    Some((name.clone(), bytes, width, height))
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
    Ok(Account {
        name: name.into(),
        hub,
        state: state.into(),
        username: username.into(),
        echo: false,
        hub_keys: Vec::new(),
        trust_roots: Vec::new(),
        zk_params: None,
    })
}

/// `NAME=KEY`: a key of an account's hub (`flag`), base64url of 32 bytes.
fn named_key(flag: &str, text: &str) -> Result<(String, [u8; 32]), String> {
    let (name, key) = text.split_once('=').ok_or(format!("{flag} NAME=KEY"))?;
    let key = yo_wire::unb64(key).ok().and_then(|k| k.try_into().ok()).ok_or(format!("{flag}: 32 bytes, base64url"))?;
    Ok((name.into(), key))
}

/// `NAME=USER,USER…`: a group's name (1 to 32 bytes, no control characters, as the core takes it; it may hold `=`,
/// the last one splits) and its members' usernames (one at least, at most 19: with the maker, 20).
fn make_group(value: &str) -> Result<(String, Vec<String>), String> {
    let bad = |why: &str| format!("--make-group {value}: NAME=USER,USER… ({why})");
    let (name, members) = value.rsplit_once('=').ok_or_else(|| bad("no ="))?;
    if name.is_empty() || name.len() > yo_wire::content::MAX_NAME_LEN || name.chars().any(char::is_control) {
        return Err(bad("a name of 1 to 32 bytes, no control characters"));
    }
    let members: Vec<String> =
        members.split(',').map(|m| m.trim().trim_start_matches('@').to_owned()).filter(|m| !m.is_empty()).collect();
    if members.is_empty() || members.len() + 1 > yo_wire::content::MAX_GROUP {
        return Err(bad("1 to 19 members"));
    }
    Ok((name.to_owned(), members))
}

fn args() -> Result<(Vec<Account>, Options), String> {
    args_from(std::env::args().skip(1))
}

fn args_from(mut it: impl Iterator<Item = String>) -> Result<(Vec<Account>, Options), String> {
    let mut accounts: Vec<Account> = Vec::new();
    let mut echoes = Vec::new();
    let (mut hub_keys, mut trust_roots, mut zk_params) = (Vec::new(), Vec::new(), Vec::new());
    let mut options = Options {
        replies: None,
        picture_every: PICTURE_EVERY,
        yo: None,
        photo: None,
        quiet_for: Vec::new(),
        make_groups: Vec::new(),
    };
    while let Some(flag) = it.next() {
        let value = it.next().ok_or(format!("{flag}: a value"))?;
        match flag.as_str() {
            "--echo" => echoes.push(value),
            "--account" => accounts.push(account(&value)?),
            "--replies" => options.replies = Some(value.into()),
            "--picture-every" => options.picture_every = value.parse().map_err(|_| "--picture-every: a number")?,
            "--hub-key" => hub_keys.push(named_key("--hub-key", &value)?),
            "--trust-root" => trust_roots.push(named_key("--trust-root", &value)?),
            "--zk-params" => {
                let (name, params) = value.split_once('=').ok_or("--zk-params NAME=PARAMS")?;
                let params = yo_wire::unb64(params).map_err(|_| "--zk-params: base64url")?;
                zk_params.push((name.to_owned(), params));
            }
            "--photo" => options.photo = Some(value.into()),
            "--quiet-for" => {
                let names = value.split(',').map(str::trim).filter(|n| !n.is_empty());
                options.quiet_for.extend(names.map(|n| n.trim_start_matches('@').to_uppercase()));
            }
            "--yo" => {
                let (to, n) = value.split_once('=').ok_or("--yo NAME=COUNT")?;
                options.yo = Some((to.into(), n.parse().map_err(|_| "--yo NAME=COUNT: a number")?));
            }
            "--make-group" => options.make_groups.push(make_group(&value)?),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    for name in echoes {
        accounts.iter_mut().find(|a| a.name == name).ok_or(format!("--echo: no account {name}"))?.echo = true;
    }
    if accounts.iter().any(|a| a.echo) && options.yo.is_some() {
        return Err("--echo cannot be combined with --yo".into());
    }
    if !options.make_groups.is_empty() && options.yo.is_some() {
        return Err("--make-group cannot be combined with --yo".into());
    }
    for (name, key) in hub_keys {
        let account = accounts.iter_mut().find(|a| a.name == name).ok_or(format!("--hub-key: no account {name}"))?;
        account.hub_keys.push(key);
    }
    for (name, params) in zk_params {
        let account = accounts.iter_mut().find(|a| a.name == name).ok_or(format!("--zk-params: no account {name}"))?;
        account.zk_params = Some(params);
    }
    for (name, key) in trust_roots {
        let account = accounts.iter_mut().find(|a| a.name == name).ok_or(format!("--trust-root: no account {name}"))?;
        account.trust_roots.push(key);
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
    echo: bool,
    /// Blob ids of photos and videos sent to it, for the drainer.
    draining: SyncSender<[u8; 16]>,
    /// `--echo`: what goes back (to a user or `GROUP` and an id) other than a Yo to a user, for the echo worker.
    echoing: SyncSender<(String, Content)>,
    /// `--echo`: answers sent, for the log (a count only).
    echoed: AtomicU64,
    client: Mutex<Client>,
    /// The hub no longer knows the device: everything of this account stops.
    stopped: AtomicBool,
    /// Senders being answered, with the answers still owed to each, the one under way first: true for an answer to
    /// a photo or video.
    pending: Mutex<HashMap<String, VecDeque<(bool, u64)>>>,
    /// The pictures each sender got last, not sent to them again soon.
    recent: Mutex<Recent>,
    /// `--quiet-for` users found (user ids), and the names not found yet.
    quiet: Mutex<std::collections::HashSet<String>>,
    quiet_unresolved: Mutex<Vec<String>>,
    dropped: Tally,
    /// Yos from `--quiet-for` users passed over.
    quieted: Tally,
    /// Group Yos dropped within `GROUP_COOLDOWN`.
    cooled: Tally,
}

/// A count logged at most once a minute.
#[derive(Default)]
struct Tally {
    n: AtomicU64,
    logged_s: AtomicU64,
}

impl Tally {
    /// Counts one; the count so far when it is time to log it.
    fn add(&self) -> Option<u64> {
        let n = self.n.fetch_add(1, Ordering::Relaxed) + 1;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let last = self.logged_s.load(Ordering::Relaxed);
        (now >= last + 60 && self.logged_s.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok())
            .then_some(n)
    }
}

impl Bot {
    fn log(&self, what: impl std::fmt::Display) {
        eprintln!("yo-gpt [{}]: {what}", self.name);
    }

    fn client(&self) -> std::sync::MutexGuard<'_, Client> {
        self.client.lock().expect("client operation panicked; account must be restored")
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
        if let Some(n) = self.dropped.add() {
            self.log(format!("{n} dropped so far (too much at once)"));
        }
    }

    /// Whether `user` is one of `--quiet-for`.
    fn is_quiet(&self, user: &str) -> bool {
        self.quiet.lock().unwrap_or_else(PoisonError::into_inner).contains(user)
    }

    /// A Yo from a `--quiet-for` user passed over; logged as a count only.
    fn quieted(&self) {
        if let Some(n) = self.quieted.add() {
            self.log(format!("{n} from --quiet-for users not answered so far"));
        }
    }

    /// A group Yo within the group's cooldown dropped; logged as a count only.
    fn cooled(&self) {
        if let Some(n) = self.cooled.add() {
            self.log(format!(
                "{n} group Yos dropped so far (one answer per group per {} s)",
                group_cooldown().as_secs()
            ));
        }
    }

    /// Looks up the `--quiet-for` names not found yet (its own name is left out); those the hub does not know or
    /// cannot tell now are tried again later.
    fn resolve_quiet(&self) {
        let names = std::mem::take(&mut *self.quiet_unresolved.lock().unwrap_or_else(PoisonError::into_inner));
        let mut left = Vec::new();
        for name in names {
            // Signed under the client, asked without it.
            let begun = self.client().whois_begin(&name);
            match begun.and_then(|whois| whois.send()) {
                Ok((user, _)) => {
                    self.quiet.lock().unwrap_or_else(PoisonError::into_inner).insert(user);
                }
                Err(Error::Protocol("oneself")) => {}
                Err(error) if gone(&error) => return self.stop(GONE),
                Err(error) if retryable(&error) || matches!(error, Error::NotOnYo) => {
                    self.log(format!("--quiet-for {name}: {}, again later", reason(&error)));
                    left.push(name);
                }
                Err(error) => self.log(format!("--quiet-for {name}: {}", reason(&error))),
            }
        }
        *self.quiet_unresolved.lock().unwrap_or_else(PoisonError::into_inner) = left;
    }

    /// Queues an event for the dispatcher, or drops it when the queue is full; never blocks. Only Yos, photos and
    /// videos are worth it; of a photo or video only that it came (its blob goes to the drainer, its key nowhere).
    fn queue(&self, events: &SyncSender<Event>, event: Event) {
        // A group's Yo is answered into the group: it goes on as a message from the group ("group:" and its id), so
        // the group has its own turn and limits, as a sender has. Its own (from another device) and changes are not.
        // Nothing from a `--quiet-for` user, one to one or in a group (its sender); a photo's blob is still drained.
        if let Event::Message { user, content, .. } | Event::GroupMessage { user, content, .. } = &event {
            if matches!(content, Content::Yo | Content::Photo(_) | Content::Video { .. }) && self.is_quiet(user) {
                if let (Content::Photo(media) | Content::Video { media, .. }, false) = (content, self.echo) {
                    self.drain(media);
                }
                return self.quieted();
            }
        }
        let event = match event {
            Event::GroupMessage { group, sent_at_ms, content, .. } => {
                Event::Message { user: format!("{GROUP}{group}"), sent_at_ms, content }
            }
            event => event,
        };
        let event = match event {
            Event::Message { user, sent_at_ms, content: Content::Photo(media) | Content::Video { media, .. } } => {
                if self.echo {
                    return;
                }
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

    /// An echo sent, or not: counted (logged as a count only, never whom), dropped, or the account stops.
    fn echo_sent<T>(&self, sent: Result<T, Error>) {
        match sent {
            Ok(_) => {
                let answered = self.echoed.fetch_add(1, Ordering::Relaxed) + 1;
                self.log(format!("echo answered={answered}"));
            }
            Err(error) if gone(&error) => self.stop(GONE),
            Err(_) => self.dropped(),
        }
    }
}

/// What the dispatcher is told of a photo or video: that one came, nothing of it.
fn unopened() -> Media {
    Media { id: [0; 16], key: [0; 32], sha256: [0; 32], size: 0, width: 0, height: 0 }
}

/// Fetches the blobs of photos and videos sent to the account into nothing, one at a time: each fetch counts at the
/// hub, which deletes a blob after its last. Nothing of them is kept, opened or logged.
fn drainer(bot: &Bot, hub: &str, ids: &std::sync::mpsc::Receiver<[u8; 16]>) {
    let Ok(http) = reqwest::blocking::Client::builder().timeout(Duration::from_secs(120)).build() else {
        panic!("building blob HTTP client")
    };
    while bot.running() {
        let id = match ids.recv_timeout(Duration::from_secs(1)) {
            Ok(id) => id,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let url = format!("{hub}{}", yo_core::media::blob_path(&id));
        if let Ok(response) = http.get(url).send() {
            let _ = std::io::copy(&mut response.take(media::MAX_MEDIA_LEN as u64 + 64), &mut std::io::sink());
        }
    }
}

/// Registers or restores, trying again while the hub cannot be reached; `None` (logged) when it cannot be opened.
fn open(account: &Account) -> Option<(Client, std::fs::File)> {
    let username = account.username.as_str();
    let log = |what: String| eprintln!("yo-gpt [{}]: {what}", account.name);
    let prepared = (|| {
        std::fs::create_dir_all(&account.state)?;
        std::fs::set_permissions(&account.state, std::fs::Permissions::from_mode(0o700))?;
        // One process, one account, per directory, before its secrets are read or made: a second (another entry
        // naming it, or another YoGPT) would advance the same state and roll the other's back. Held until every
        // worker of this account has joined, including before a restart.
        let lock = std::fs::File::create(account.state.join("lock"))?;
        lock.try_lock().map_err(|e| std::io::Error::other(format!("another one has it ({e})")))?;
        Ok::<_, std::io::Error>((secret(&account.state.join("account"))?, secret(&account.state.join("key"))?, lock))
    })();
    let (secret, key, lock) = prepared.map_err(|e| log(format!("stopped: {}: {e}", account.state.display()))).ok()?;
    let path = account.state.join("state");
    let client = || reqwest::blocking::Client::builder().timeout(Duration::from_secs(60)).build().ok();
    let transport = Arc::new(Http { client: client()?, anonymous: client()?, base: account.hub.clone() });
    let mut backoff = Backoff::default();
    loop {
        let opened = match std::fs::read(&path) {
            // A registration under way when it stopped is finished first (sent again only if it did not arrive).
            Ok(blob) => Client::restore(transport.clone(), &blob, &key).and_then(|mut client| {
                client.persist_to(Arc::new(File(path.clone())), key)?;
                client.finish_registration()?;
                Ok(client)
            }),
            // Saved before it is sent (spec 6.4): a lost answer or a stop halfway loses neither key nor prekeys.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Client::register_username_saved(
                transport.clone(),
                Arc::new(File(path.clone())),
                key,
                username,
                Some(secret),
                None,
                None,
                Push::none(),
            )
            .inspect(|c| log(format!("registered {} on {}", c.username().unwrap_or("?"), account.hub))),
            Err(error) => {
                log(format!("stopped: {}: {error}", path.display()));
                return None;
            }
        };
        match opened {
            Ok(mut client) => {
                if !account.hub_keys.is_empty() {
                    client.set_hub_keys(account.hub_keys.clone());
                }
                client.set_trust_roots(account.trust_roots.clone());
                if let Some(params) = &account.zk_params {
                    if let Err(error) = client.set_zk_params(params) {
                        eprintln!("yo-gpt: {}: --zk-params: {error}", account.name);
                    }
                }
                // It never opens a photo or video: their keys are not even kept with the events. An echo account
                // opens them only to send them back as they came (`echo_worker`), never keeping or logging them.
                if !account.echo {
                    client.forget_media_keys();
                }
                if let Err(error) = client.persist_to(Arc::new(File(path)), key) {
                    log(format!("stopped: {username}: {error}"));
                    return None;
                }
                return Some((client, lock));
            }
            Err(error) if retryable(&error) => backoff.wait(&error, &account.name),
            // A state it cannot open (one from before the sessions, or damaged) is set aside as `state.old`; the account
            // secret registers the account again as a new device, as the app does from the keychain.
            Err(Error::State) if path.exists() => {
                let old = account.state.join("state.old");
                if let Err(error) = std::fs::rename(&path, &old) {
                    log(format!("stopped: {}: {error}", path.display()));
                    return None;
                }
                log(format!("{username}: state set aside as {}, registering again", old.display()));
            }
            Err(error) => {
                log(format!("stopped: {username} on {}: {}", account.hub, reason(&error)));
                return None;
            }
        }
    }
}

const GONE: &str = "the hub no longer knows this device (replaced by another, the name released, or the hub reset)";

/// `410`: replaced, or the name released. `401`: unknown, as after a reset hub (or a clock far off).
fn gone(error: &Error) -> bool {
    matches!(error, Error::Replaced | Error::Refused(401))
}

/// An error for the log, without text that came from outside (libsignal's reasons).
fn reason(error: &Error) -> String {
    match error {
        Error::Session(_) => "session".into(),
        other => other.to_string(),
    }
}

/// How the log names a sender: the start of its user id (hex), nothing it chose itself.
fn who(user: &str) -> String {
    match user.strip_prefix(GROUP) {
        Some(group) => format!("group {}", group.chars().take(8).filter(char::is_ascii_hexdigit).collect::<String>()),
        None => user.chars().take(8).filter(char::is_ascii_hexdigit).collect(),
    }
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
    if !options.make_groups.is_empty() {
        std::process::exit(make_groups(&accounts[0], &options.make_groups));
    }
    if let Some((to, n)) = options.yo.clone() {
        for account in &accounts {
            let Some((mut client, _lock)) = open(account) else { std::process::exit(1) };
            let photo = options.photo.as_deref().map(|path| {
                let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("--photo {}: {e}", path.display()));
                jpeg_clean(&bytes).unwrap_or_else(|| panic!("--photo: a JPEG of at most 1600 px a side"))
            });
            for i in 0..n {
                let sent = match &photo {
                    None => client.send_yo(&to).map(drop),
                    Some((bytes, width, height)) => media::seal(bytes, *width, *height).and_then(|(media, sealed)| {
                        let fetches = client.media_fetches(&to).unwrap_or(1);
                        client.upload(&media, fetches)?.send(&sealed)?;
                        client.send(&to, Content::Photo(media)).map(drop)
                    }),
                };
                match sent {
                    Ok(_) => eprintln!("yo-gpt [{}]: Yo {} of {n} to {to}", account.name, i + 1),
                    Err(error) => {
                        eprintln!("yo-gpt [{}]: Yo {} of {n} to {to}: {}", account.name, i + 1, reason(&error))
                    }
                }
                std::thread::sleep(Duration::from_millis(400));
            }
        }
        std::process::exit(0);
    }
    let serving: Vec<_> = accounts
        .into_iter()
        .map(|account| {
            let options = options.clone();
            std::thread::spawn(move || serve(&account, options))
        })
        .collect();
    for thread in serving {
        if thread.join().is_err() {
            eprintln!("yo-gpt: ACCOUNT THREAD PANICKED during initialization");
        }
    }
    eprintln!("yo-gpt: no account left");
    std::process::exit(1);
}

/// `--make-group`: the account makes each group of the members it finds (by username), printing each group's id;
/// exits 1 when one could not be made.
fn make_groups(account: &Account, groups: &[(String, Vec<String>)]) -> i32 {
    if account.zk_params.is_none() {
        eprintln!("yo-gpt [{}]: --make-group needs --zk-params {}=PARAMS", account.name, account.name);
        return 2;
    }
    let Some((mut client, _lock)) = open(account) else { return 1 };
    let mut failed = false;
    for (name, usernames) in groups {
        let mut members = Vec::new();
        for username in usernames {
            match client.whois_begin(username).and_then(|whois| whois.send()) {
                Ok((user, _)) => members.push(user),
                Err(error) => {
                    eprintln!("yo-gpt [{}]: group {name:?}: member {username}: {}", account.name, reason(&error));
                    failed = true;
                }
            }
        }
        if members.is_empty() {
            failed = true;
            continue;
        }
        match client.create_group(name, &members) {
            Ok((id, _)) => {
                println!("yo-gpt [{}]: group {name:?} made: {id} ({} members)", account.name, members.len() + 1)
            }
            Err(error) => {
                eprintln!("yo-gpt [{}]: group {name:?}: {}", account.name, reason(&error));
                failed = true;
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    i32::from(failed)
}

/// `stats.json` beside the outbox: `{"contacts":N}`, written whole.
fn write_stats(bot: &Bot, outbox: &Path) {
    let Some(dir) = outbox.parent() else { return };
    let contacts = bot.client().contacts().len();
    let tmp = dir.join("stats.json.new");
    if std::fs::write(&tmp, format!("{{\"contacts\":{contacts}}}")).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join("stats.json"));
    }
}

/// The operator's mailbox: `NAME.anything.jpg` dropped into the account's `outbox` (root only, on the server) goes
/// to NAME as a photo with its Yo, then the file is removed; one that could not go is renamed `.failed`.
fn mailbox(bot: &Bot, dir: &Path, events: &SyncSender<Event>) {
    let _ = std::fs::create_dir_all(dir);
    let mut ticks = 0u32;
    while bot.running() {
        std::thread::sleep(Duration::from_secs(5));
        // For the admin page, once a minute: how many people YoGPT knows (it is a client: it has their chats).
        if ticks.is_multiple_of(12) {
            write_stats(bot, dir);
        }
        ticks = ticks.wrapping_add(1);
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for path in entries.flatten().map(|entry| entry.path()).take(20) {
            if !bot.running() {
                return;
            }
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
            let Some((bytes, width, height)) = jpeg_clean(&bytes) else {
                let _ = std::fs::rename(&path, path.with_extension("failed"));
                continue;
            };
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

/// Volatile, bounded token buckets; idle senders expire after three seconds. No eviction
/// of active buckets (a flood of fresh senders cannot reset another sender's limit).
#[derive(Default)]
struct EchoLimits(HashMap<String, (Instant, f64)>);
impl EchoLimits {
    fn allow(&mut self, user: &str, now: Instant) -> bool {
        self.0.retain(|_, (last, _)| now.duration_since(*last) < Duration::from_secs(3));
        if !self.0.contains_key(user) && self.0.len() >= MAX_REMEMBERED {
            return false;
        }
        let (last, tokens) = self.0.entry(user.to_owned()).or_insert((now, 3.0));
        *tokens = (*tokens + now.duration_since(*last).as_secs_f64()).min(3.0);
        *last = now;
        if *tokens < 1.0 {
            return false;
        }
        *tokens -= 1.0;
        true
    }
}

/// Serves one account until it stops.
fn serve(account: &Account, options: Options) {
    loop {
        // A failed worker stops and joins this account's other workers before reopening the saved state.
        // No old worker may write over the restarted account's state or advance its sessions.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| serve_once(account, &options))) {
            Ok(false) => return,
            Ok(true) => {}
            Err(_) => eprintln!("yo-gpt [{}]: ACCOUNT PANICKED during initialization; restarting", account.name),
        }
        eprintln!("yo-gpt [{}]: RESTARTING account from saved state in 1 s", account.name);
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn serve_once(account: &Account, options: &Options) -> bool {
    let Some((client, _lock)) = open(account) else { return false };
    let (draining, to_drain) = sync_channel(MAX_DRAINING);
    let (echoing, to_echo) = sync_channel(MAX_DRAINING);
    let hub = account.hub.clone();
    let bot = Arc::new(Bot {
        name: account.name.clone(),
        options: options.clone(),
        echo: account.echo,
        draining,
        echoing,
        echoed: AtomicU64::new(0),
        client: Mutex::new(client),
        stopped: AtomicBool::new(false),
        pending: Mutex::default(),
        recent: Mutex::default(),
        quiet: Mutex::default(),
        quiet_unresolved: Mutex::new(options.quiet_for.clone()),
        dropped: Tally::default(),
        quieted: Tally::default(),
        cooled: Tally::default(),
    });
    let started = if account.echo {
        "echo ready".to_owned()
    } else {
        let client = bot.client();
        format!("{} on {}, user {}", client.username().unwrap_or("?"), account.hub, client.user_id())
    };
    bot.log(started);
    let (events, received) = sync_channel(MAX_QUEUED);
    let restart = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        worker(&bot, "dispatcher", &restart, false, || {
            let (receiving, to_dispatch, restart_rx) = (bot.clone(), events.clone(), restart.clone());
            scope.spawn(move || {
                worker(&receiving, "receiver", &restart_rx, false, || receive(&receiving, &to_dispatch))
            });
            let (draining_bot, restart_drain) = (bot.clone(), restart.clone());
            scope.spawn(move || {
                worker(&draining_bot, "blob drainer", &restart_drain, false, || drainer(&draining_bot, &hub, &to_drain))
            });
            if account.echo {
                let (echoing_bot, restart_echo) = (bot.clone(), restart.clone());
                scope.spawn(move || {
                    worker(&echoing_bot, "echo worker", &restart_echo, false, || echo_worker(&echoing_bot, &to_echo))
                });
            } else {
                let (mailing, to_queue, outbox, restart_mail) =
                    (bot.clone(), events.clone(), account.state.join("outbox"), restart.clone());
                scope.spawn(move || {
                    worker(&mailing, "mailbox", &restart_mail, false, || mailbox(&mailing, &outbox, &to_queue))
                });
            }
            dispatch(scope, &bot, &events, &received, &restart);
        });
    });
    restart.load(Ordering::Relaxed)
}

/// Unexpected exits and panics stop the whole account, so it can restart from durable state rather than reuse
/// a client whose operation unwound halfway through. Intentional device removal remains terminal.
fn worker(bot: &Bot, name: &str, restart: &AtomicBool, may_finish: bool, work: impl FnOnce()) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
    if result.is_err() || (!may_finish && bot.running()) {
        let failure = if result.is_err() { "PANICKED" } else { "EXITED unexpectedly" };
        bot.log(format!("{name} {failure}; restarting account after workers stop"));
        restart.store(true, Ordering::Relaxed);
        bot.stop("worker failed");
    }
}

fn dispatch<'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    bot: &Arc<Bot>,
    events: &SyncSender<Event>,
    received: &std::sync::mpsc::Receiver<Event>,
    restart: &Arc<AtomicBool>,
) {
    // Every Yo gets its answer: one sender's in turn, on one thread; senders side by side. A group, one at most per
    // cooldown.
    let mut cooldown = GroupCooldown::default();
    let window = group_cooldown();
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
        if user.starts_with(GROUP) && !cooldown.allow(&user, Instant::now(), window) {
            bot.cooled();
            continue;
        }
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
        let (answering, events, sender, restart) = (Arc::clone(bot), events.clone(), user.clone(), restart.clone());
        let spawned = std::thread::Builder::new().spawn_scoped(scope, move || {
            worker(&answering, "answer worker", &restart, true, || {
                std::thread::sleep(ms(think_ms()));
                answer_pending(&answering, &sender, &events);
            });
        });
        if let Err(error) = spawned {
            bot.pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&user);
            bot.log(format!("cannot start answer worker: {error}"));
            bot.dropped();
        }
    }
}

/// When each group was last answered, for the groups answered within the window.
#[derive(Default)]
struct GroupCooldown(HashMap<String, Instant>);

impl GroupCooldown {
    /// Whether `group` may be answered now: not answered within `window`. Counts as answered when it may.
    fn allow(&mut self, group: &str, now: Instant, window: Duration) -> bool {
        self.0.retain(|_, at| now.duration_since(*at) < window);
        if self.0.contains_key(group) {
            return false;
        }
        self.0.insert(group.to_owned(), now);
        true
    }
}

/// Keep the current answer in pending until it completes; one sender is answered in order.
fn answer_pending(bot: &Bot, sender: &str, events: &SyncSender<Event>) {
    while bot.running() {
        let next = bot
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(sender)
            .and_then(|owed| owed.front())
            .copied();
        let Some((picture, last)) = next else { return };
        let began = Instant::now();
        answer(bot, sender, events, picture);
        let next = {
            let mut pending = bot.pending.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(owed) = pending.get_mut(sender) else { return };
            owed.pop_front();
            match owed.front() {
                Some(&next) => next,
                None => {
                    pending.remove(sender);
                    return;
                }
            }
        };
        let gap = Duration::from_millis(next.1.saturating_sub(last).min(MAX_GAP_MS));
        std::thread::sleep(gap.saturating_sub(began.elapsed()));
    }
}

/// The long poll, without holding the client while it waits; every Yo to the dispatcher.
fn receive(bot: &Bot, events: &SyncSender<Event>) {
    if !bot.echo {
        leave_empty(bot, events);
    } // the groups left alone while it was away
    let mut backoff = Backoff::default();
    let mut maintained = Instant::now();
    let mut limits = EchoLimits::default();
    let mut cooldown = GroupCooldown::default();
    let mut quiet_tried: Option<Instant> = None;
    while bot.running() {
        let unresolved = !bot.quiet_unresolved.lock().unwrap_or_else(PoisonError::into_inner).is_empty();
        if unresolved && quiet_tried.is_none_or(|at| at.elapsed() >= QUIET_RETRY) {
            quiet_tried = Some(Instant::now());
            bot.resolve_quiet();
        }
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
                let changed = received.iter().any(|event| matches!(event, Event::GroupChanged { .. }));
                if bot.echo {
                    let probes = bot.client().take_probe_requests();
                    for (user, token) in probes {
                        if limits.allow(&user, Instant::now()) {
                            let sent = bot.client().echo_probe(&user, token);
                            bot.echo_sent(sent);
                        } else {
                            bot.dropped();
                        }
                    }
                    for event in received {
                        echo(bot, event, &mut limits, &mut cooldown);
                    }
                } else {
                    received.into_iter().for_each(|event| bot.queue(events, event));
                }
                bot.client().acknowledge_events();
                if changed && !bot.echo {
                    leave_empty(bot, events);
                }
            }
            Err(error) if gone(&error) => bot.stop(GONE),
            Err(error) => backoff.wait(&error, &bot.name),
        }
    }
}

/// `--echo`: what came goes back as it came, once: a Yo as a Yo, a photo or video as the same photo or video, to its
/// sender or into its group. Nothing from a `--quiet-for` user; a group once per cooldown; each sender by its token
/// bucket. A Yo to a user goes at once; the rest waits for the echo worker (`MAX_DRAINING` at most, more dropped).
fn echo(bot: &Bot, event: Event, limits: &mut EchoLimits, cooldown: &mut GroupCooldown) {
    let (to, user, content) = match event {
        Event::Message { user, content, .. } => (user.clone(), user, content),
        Event::GroupMessage { group, user, content, .. } => (format!("{GROUP}{group}"), user, content),
        _ => return,
    };
    if !matches!(content, Content::Yo | Content::Photo(_) | Content::Video { .. }) {
        return;
    }
    if bot.is_quiet(&user) {
        return bot.quieted();
    }
    if to.starts_with(GROUP) && !cooldown.allow(&to, Instant::now(), group_cooldown()) {
        return bot.cooled();
    }
    if !limits.allow(&user, Instant::now()) {
        return bot.dropped();
    }
    if content == Content::Yo && !to.starts_with(GROUP) {
        let sent = bot.client().echo_yo(&user);
        return bot.echo_sent(sent);
    }
    if let Err(TrySendError::Full(_)) = bot.echoing.try_send((to, content)) {
        bot.dropped();
    }
}

/// `--echo`: sends back, one at a time, what `echo` handed over: a group's Yo, and photos and videos, each fetched,
/// opened, sealed anew under a key of its own and uploaded for its receivers. Its bytes are held in memory only while
/// this is under way, never stored, decoded or logged.
fn echo_worker(bot: &Bot, jobs: &std::sync::mpsc::Receiver<(String, Content)>) {
    let transport = bot.client().transport();
    while bot.running() {
        let (to, content) = match jobs.recv_timeout(Duration::from_secs(1)) {
            Ok(job) => job,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let content = match content {
            Content::Photo(media) => reseal(bot, transport.as_ref(), &to, &media).map(|m| m.map(Content::Photo)),
            Content::Video { media, duration_ms } => reseal(bot, transport.as_ref(), &to, &media)
                .map(|m| m.map(|media| Content::Video { media, duration_ms })),
            content => Ok(Some(content)),
        };
        match content {
            Ok(Some(content)) => {
                let sent = send_to(bot, &to, content);
                bot.echo_sent(sent);
            }
            Ok(None) => bot.dropped(),
            Err(error) => bot.echo_sent::<()>(Err(error)),
        }
    }
}

/// A photo or video sent to it, fetched and opened, then sealed anew and uploaded for `to`: what goes back. `None`
/// when its blob is no more (fetched by all, expired, never uploaded) or larger than a blob may be.
fn reseal(bot: &Bot, transport: &dyn Transport, to: &str, media: &Media) -> Result<Option<Media>, Error> {
    if media.size as usize > yo_wire::limits::MAX_BLOB_LEN {
        return Ok(None);
    }
    let Some(plain) = media::fetch_open(transport, media)? else { return Ok(None) };
    // Its fetch done: the hub may let it go once its other receivers have theirs.
    let _ = media::ack(transport, &media.id);
    let (fresh, sealed) = media::seal(&plain, media.width, media.height)?;
    drop(plain);
    // Signed under the client, sent without it.
    let upload = {
        let mut client = bot.client();
        let fetches = match to.strip_prefix(GROUP) {
            Some(group) => client.group_media_fetches(group),
            None => client.media_fetches(to),
        };
        client.upload(&fresh, fetches.unwrap_or(1))?
    };
    upload.send(&sealed)?;
    Ok(Some(fresh))
}

/// Leaves every group YoGPT is alone in (the others left or removed one another): nobody to answer there.
fn leave_empty(bot: &Bot, events: &SyncSender<Event>) {
    let me = bot.client().user_id().to_owned();
    let alone: Vec<String> = bot
        .client()
        .groups()
        .into_iter()
        .filter(|g| !g.left && g.members.contains(&me) && g.members.iter().all(|m| *m == me))
        .map(|g| g.id)
        .collect();
    for id in alone {
        match bot.client().leave_group(&id) {
            Ok(received) => {
                received.into_iter().for_each(|event| bot.queue(events, event));
                bot.log(format!("left group {} (alone in it)", &id[..8.min(id.len())]));
            }
            Err(error) => bot.log(format!("leaving group {}: {}", &id[..8.min(id.len())], reason(&error))),
        }
    }
}

/// What answers go to: "group:" and a group's id for a group, else a user.
const GROUP: &str = "group:";

/// A Yo to a user or into a group (`GROUP`).
fn send_to(bot: &Bot, to: &str, content: Content) -> Result<Vec<Event>, Error> {
    match to.strip_prefix(GROUP) {
        Some(group) => bot.client().send_group(group, content),
        None => {
            let deliveries = bot.client().send_begin(to, content.clone());
            let Some(mut deliveries) = deliveries else { return bot.client().send(to, content) };
            loop {
                let answers = deliveries.send();
                match bot.client().send_end(&deliveries, answers) {
                    yo_core::Sent::More(more) => deliveries = more,
                    yo_core::Sent::Done(result) => return result,
                }
            }
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
            false => send_to(bot, user, Content::Yo),
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
    let recent = bot.recent.lock().unwrap_or_else(PoisonError::into_inner).of(user).cloned();
    let picked = bot.options.replies.as_deref().and_then(|dir| pick_picture(dir, recent.as_ref()));
    let Some((name, bytes, width, height)) = picked else { return send_to(bot, user, Content::Yo).map(|e| (e, false)) };
    let sent = send_this_picture(bot, user, &bytes, width, height);
    // Only a picture that went counts against a repeat: a plain Yo instead keeps the sender's last 50 as they were.
    if matches!(sent, Ok((_, true))) {
        bot.recent.lock().unwrap_or_else(PoisonError::into_inner).sent(user, name);
    }
    sent
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
        let fetches = match user.strip_prefix(GROUP) {
            Some(group) => client.group_media_fetches(group),
            None => client.media_fetches(user),
        };
        let fetches = fetches.unwrap_or(1);
        client.upload(&media, fetches)?
    };
    match upload.send(&sealed) {
        Ok(()) => send_to(bot, user, Content::Photo(media)).map(|e| (e, true)),
        Err(error) if gone(&error) => Err(error),
        Err(error) => {
            bot.log(format!("picture to {}: {}", who(user), reason(&error)));
            send_to(bot, user, Content::Yo).map(|e| (e, false))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_workers_request_an_account_restart_from_durable_state() {
        let hub = yo_tests::TestHub::start();
        for panics in [false, true] {
            let account = Account {
                name: "test".into(),
                hub: hub.url(),
                state: std::env::temp_dir().join(format!("yogpt-supervise-{}", yo_tests::random_u64())),
                username: yo_tests::username(yo_tests::random_u64() % 1_000_000_000),
                echo: false,
                hub_keys: vec![],
                trust_roots: vec![],
                zk_params: None,
            };
            let (mut client, lock) = open(&account).unwrap();
            client.maintenance().unwrap();
            let user = client.user_id().to_owned();
            let account_key = client.account_secret();
            let access_key = client.access_key_for_tests().expect("profile key initialized");
            let (draining, _drain) = sync_channel(MAX_DRAINING);
            let (echoing, _echo) = sync_channel(MAX_DRAINING);
            let bot = Bot {
                name: account.name.clone(),
                options: Options {
                    replies: None,
                    picture_every: 0,
                    yo: None,
                    photo: None,
                    quiet_for: vec![],
                    make_groups: vec![],
                },
                echo: false,
                draining,
                echoing,
                echoed: AtomicU64::new(0),
                client: Mutex::new(client),
                stopped: AtomicBool::new(false),
                pending: Mutex::default(),
                recent: Mutex::default(),
                quiet: Mutex::default(),
                quiet_unresolved: Mutex::default(),
                dropped: Tally::default(),
                quieted: Tally::default(),
                cooled: Tally::default(),
            };
            let restart = AtomicBool::new(false);
            worker(&bot, "answer", &restart, true, || {});
            assert!(!restart.load(Ordering::Relaxed));
            assert!(bot.running());
            worker(&bot, "receiver", &restart, false, || {
                if panics {
                    let _client = bot.client();
                    panic!("injected failure while holding the client");
                }
            });
            assert!(restart.load(Ordering::Relaxed));
            assert!(!bot.running());
            assert_eq!(bot.client.is_poisoned(), panics);
            drop(bot);
            drop(lock);
            // Reopening reacquires the state lock and discards the poisoned in-memory client.
            let (mut restored, lock) = open(&account).unwrap();
            assert_eq!(restored.user_id(), user);
            assert_eq!(restored.account_secret(), account_key);
            assert_eq!(restored.access_key_for_tests().as_ref(), Some(&access_key));
            restored.sync(0).unwrap();
            drop(restored);
            drop(lock);
            std::fs::remove_dir_all(account.state).unwrap();
        }
    }

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
        // A frame header after an APP0 segment: 720 wide, 1280 high; then the scan.
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 17, 8, 0x05, 0x00, 0x02, 0xD0, 3, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0xFF, 0xDA, 0, 2, 1, 2, 3, 0xFF, 0xD9,
        ];
        assert_eq!(jpeg_size(&jpeg), Some((720, 1280)));
        assert_eq!(jpeg_size(&jpeg[..12]), None);
        assert_eq!(jpeg_size(b"not a jpeg"), None);
        // Metadata out (an APP1 "Exif" segment), the rest as it was; too large: none.
        let mut with_exif = jpeg[..2].to_vec();
        with_exif.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x08, b'E', b'x', b'i', b'f', 0, 0]);
        with_exif.extend_from_slice(&jpeg[2..]);
        let (clean, w, h) = jpeg_clean(&with_exif).unwrap();
        assert_eq!((clean.as_slice(), w, h), (&jpeg[..], 720, 1280));
        // A comment after the first scan, a second scan, and bytes after the end: the comment and the tail go.
        let end = jpeg.len() - 2;
        let mut progressive = jpeg[..end].to_vec();
        progressive.extend_from_slice(&[0xFF, 0xFE, 0, 4, b'h', b'i', 0xFF, 0xDA, 0, 2, 9, 0xFF, 0x00, 9, 0xFF, 0xD9]);
        progressive.extend_from_slice(b"trailing");
        let mut expected = jpeg[..end].to_vec();
        expected.extend_from_slice(&[0xFF, 0xDA, 0, 2, 9, 0xFF, 0x00, 9, 0xFF, 0xD9]);
        assert_eq!(jpeg_clean(&progressive).unwrap().0, expected);
        let dir = std::env::temp_dir().join(format!("yogpt-pick-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(pick_picture(&dir, None).is_none(), "nothing there");
        std::fs::write(dir.join("001.jpg"), vec![0u8; MAX_PICTURE as usize + 1]).unwrap();
        assert!(pick_picture(&dir, None).is_none(), "too large");
        std::fs::write(dir.join("001.jpg"), jpeg).unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let picked = pick_picture(&dir, None).map(|(n, b, w, h)| (n, b.len(), w, h));
        assert_eq!(picked, Some(("001.jpg".to_owned(), jpeg.len(), 720, 1280)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn uniform_stays_in_its_bounds() {
        for _ in 0..1000 {
            assert!((300..=1000).contains(&uniform(300, 1000)));
        }
        assert!((0..1000).map(|_| uniform(0, 99)).any(|p| p == 0));
    }

    #[test]
    fn every_picture_comes_up_about_equally_often() {
        let names: Vec<String> = (1..=20).map(|i| format!("{i:03}.jpg")).collect();
        let mut counts = [0u32; 20];
        for _ in 0..20_000 {
            counts[choose(&names, None).unwrap()] += 1;
        }
        // 1000 expected each, a standard deviation of about 31.
        assert!(counts.iter().all(|&c| (850..=1150).contains(&c)), "{counts:?}");
        assert_eq!(choose(&[], None), None);
    }

    #[test]
    fn no_picture_again_to_the_same_sender_within_the_last_fifty() {
        let names: Vec<String> = (1..=200).map(|i| format!("{i:03}.jpg")).collect();
        let mut recent = Recent::default();
        let mut sent: Vec<usize> = Vec::new();
        let mut counts = [0u32; 200];
        for _ in 0..20_000 {
            let i = choose(&names, recent.of("a")).unwrap();
            assert!(!sent.iter().rev().take(RECENT_PICTURES).any(|&j| j == i), "{} again within 50", names[i]);
            recent.sent("a", names[i].clone());
            sent.push(i);
            counts[i] += 1;
        }
        // Still uniform overall: 100 expected each.
        assert!(counts.iter().all(|&c| (60..=140).contains(&c)), "{counts:?}");
        // Another sender is not held back by what "a" got.
        assert!(recent.of("b").is_none());

        // Fewer pictures than the window: never the same twice in a row, and every one comes up.
        let few = &names[..5];
        let mut recent = Recent::default();
        let (mut last, mut seen) = (None, [false; 5]);
        for _ in 0..1000 {
            let i = choose(few, recent.of("a")).unwrap();
            assert_ne!(Some(i), last);
            recent.sent("a", few[i].clone());
            (last, seen[i]) = (Some(i), true);
        }
        assert!(seen.iter().all(|&s| s));
        let one = &names[..1];
        assert_eq!(choose(one, recent.of("a")), Some(0), "a single picture is still sent");
    }

    #[test]
    fn recent_pictures_stay_bounded() {
        let mut recent = Recent::default();
        for i in 0..MAX_REMEMBERED + 10 {
            for j in 0..RECENT_PICTURES + 5 {
                recent.sent(&format!("user{i}"), format!("{j:03}.jpg"));
            }
        }
        assert_eq!(recent.by_sender.len(), MAX_REMEMBERED);
        assert_eq!(recent.order.len(), MAX_REMEMBERED);
        assert!(recent.of("user0").is_none(), "the oldest sender forgotten");
        let last = recent.of(&format!("user{}", MAX_REMEMBERED + 9)).unwrap();
        assert_eq!(last.len(), RECENT_PICTURES);
        assert_eq!(last.front().map(String::as_str), Some("005.jpg"));
    }
}

#[cfg(test)]
mod cooldown_tests {
    use super::*;
    #[test]
    fn a_group_is_answered_once_per_window() {
        let mut cooldown = GroupCooldown::default();
        let (now, window) = (Instant::now(), Duration::from_secs(30));
        assert!(cooldown.allow("group:a", now, window));
        assert!(!cooldown.allow("group:a", now + Duration::from_secs(29), window));
        assert!(cooldown.allow("group:b", now + Duration::from_secs(1), window), "another group is not held back");
        assert!(cooldown.allow("group:a", now + window, window));
        assert!(!cooldown.allow("group:a", now + window + Duration::from_secs(1), window));
        assert_eq!(cooldown.0.len(), 1, "only groups within the window are kept");
    }
}

#[cfg(test)]
mod echo_tests {
    use super::*;
    #[test]
    fn sender_buckets_refill_expire_and_stay_bounded() {
        let mut limits = EchoLimits::default();
        let now = Instant::now();
        for _ in 0..3 {
            assert!(limits.allow("sender", now));
        }
        assert!(!limits.allow("sender", now));
        assert!(!limits.allow("sender", now + Duration::from_millis(999)));
        assert!(limits.allow("sender", now + Duration::from_secs(1)));
        assert!(!limits.allow("sender", now + Duration::from_secs(1)));
        for n in 0..MAX_REMEMBERED - 1 {
            assert!(limits.allow(&n.to_string(), now + Duration::from_secs(1)));
        }
        assert!(!limits.allow("overflow", now + Duration::from_secs(1)));
        assert!(!limits.allow("sender", now + Duration::from_secs(1)));
        assert!(limits.allow("fresh", now + Duration::from_secs(5)));
        assert_eq!(limits.0.len(), 1);
    }

    #[test]
    fn make_group_parses_names_with_signs_and_rejects_bad_ones() {
        let args = |list: &[&str]| args_from(list.iter().map(|s| s.to_string()));
        let account = "--account";
        let leo = "leo=https://hub.example,state/leo,LEO";
        let (_, options) =
            args(&[account, leo, "--make-group", "👥 Hey=mia, @Noah", "--make-group", "a=b=emma"]).unwrap();
        assert_eq!(
            options.make_groups,
            vec![("👥 Hey".into(), vec!["mia".into(), "Noah".into()]), ("a=b".into(), vec!["emma".into()])]
        );
        for bad in ["Hey", "=mia", "Hey=", "Hey= , ", &format!("{}=mia", "x".repeat(33)), "a\nb=mia"] {
            assert!(make_group(bad).is_err(), "{bad:?}");
        }
        assert!(make_group(&format!("{}=mia", "x".repeat(32))).is_ok());
        assert!(make_group(&format!("g={}", vec!["u"; 20].join(","))).is_err(), "20 others and the maker: 21");
        assert!(args(&[account, leo, "--make-group", "g=mia", "--yo", "MIA=1"]).is_err());
    }
}
