//! The Yo client core (spec/v1.md): sessions, delivery and the outbox for iOS and Android; the platform supplies
//! HTTP and opens the local database.

mod aead;
pub mod backup;
mod contacts;
pub mod error;
pub mod groups;
pub mod help;
mod hub;
pub mod identity;
pub mod keychain;
pub mod legacy;
pub mod media;
pub mod net;
mod outbox;
mod prekeys;
mod probe;
mod receive;
pub mod sealed;
pub mod security;
pub mod shared;
pub mod signal;
pub mod state;
pub mod statedb;
pub mod storage;
#[cfg(any(test, feature = "test-support"))]
mod testing;
#[cfg(any(test, feature = "test-support"))]
pub use testing::{Memory, Saved};

pub use error::{Error, Result};
pub use net::{Response, Transport};
pub use yo_wire::api::anonymous as anonymous_request;
pub use yo_wire::api::{Backup, Push, SupportThread, BACKUP_VERSION, REPORT_ABUSE, REPORT_OTHER, REPORT_SPAM};
pub use yo_wire::content::{Content, Media};
/// Usernames as the hub takes and compares them (spec 3.3), for the apps.
pub use yo_wire::handle;

use identity::Key;
use libsignal_protocol::{
    extract_decryption_error_message_from_serialized_content, message_decrypt, message_encrypt, process_prekey_bundle,
    CiphertextMessage, CiphertextMessageType, DecryptionErrorMessage, DeviceId, IdentityKeyPair, PlaintextContent,
    PreKeyBundle, PreKeySignalMessage, ProtocolAddress, SignalMessage, SignalProtocolError, Timestamp,
};
use signal::Records;
use state::{hex, unhex, Contact, DecryptionError, Outgoing, Registering, Slot, State};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use yo_wire::api::{self, status};
use yo_wire::content::Message;
use yo_wire::envelope::{ForDevice, Payload, Plain, KIND_MESSAGE, KIND_PLAINTEXT, KIND_PREKEY};
use yo_wire::handle::{same_name, Handle, KIND_USER, KIND_USERNAME};
use yo_wire::hubsig;
use yo_wire::limits::{MAX_INBOX_FETCH, PREKEY_REFILL_BELOW, PREKEY_TARGET};
use yo_wire::request::{HEADER_DEVICE, HEADER_SIGNATURE, HEADER_TIME};
use yo_wire::{decode, encode, sha256, DeviceId as HubDeviceId, UserId};

const OUTBOX_KEPT_MS: u64 = 30 * 24 * 3600 * 1000;
const OUTBOX_PER_RECIPIENT: usize = 1_000;
/// The signed prekey and the last resort Kyber prekey are replaced this often (spec 4.1)...
const ROTATE_EVERY_MS: u64 = 7 * 24 * 3600 * 1000;
/// ... and the ones replaced kept this long (wall clock): a session started with one may still wait at the hub.
const RETIRED_KEPT_MS: u64 = 31 * 24 * 3600 * 1000;
/// One-time prekeys of each kind kept: lookups that never lead to a message use them up at the hub (spec 4.6).
const ONE_TIME_KEPT: usize = 4 * PREKEY_TARGET as usize;
/// A saved signing time at most this far ahead of the clock is kept as the floor of the next (spec 6.2).
const SIGN_AHEAD_MS: u64 = 60 * 1000;
/// Session message hashes remembered against redelivery.
const SEEN: usize = 2000;
/// Messages kept per contact to resend when a decryption error names one, and decryption errors due at most (4.6).
const KEPT_FOR_RESEND: usize = 64;
/// A sender certificate holds a day at the hub (spec 6.1); used for an hour less by this device's own clock.
const CERTIFICATE_USED_MS: u64 = 23 * 3600 * 1000;

/// A new profile key (Signal's, zkgroup): 32 bytes, sent to contacts (spec 4.10).
fn new_profile_key() -> [u8; 32] {
    zkgroup::profiles::ProfileKey::generate(identity::random()).get_bytes()
}

/// The access key a profile key gives (Signal's unidentified access key): what a delivery shows in `Yo-Access`.
fn access_key(profile_key: &[u8; 32]) -> [u8; api::ACCESS_KEY_LEN] {
    zkgroup::profiles::ProfileKey::create(*profile_key).derive_access_key()
}

/// The trust roots a client starts with: none (an app pins its hub's, `set_trust_roots`); under tests, the test hubs'.
fn test_trust_roots() -> Vec<Key> {
    #[cfg(feature = "test-support")]
    {
        let private = libsignal_protocol::PrivateKey::deserialize(&yo_wire::envelope::TEST_TRUST_ROOT).expect("a key");
        let public = private.public_key().expect("a key").public_key_bytes().try_into().expect("32 bytes");
        vec![public]
    }
    #[cfg(not(feature = "test-support"))]
    Vec::new()
}
/// How often a payload may make this device ask the hub for a user's device (spec 4.4).
const ROSTER_ASK_EVERY_MS: u64 = 60 * 1000;
/// Lookups a day that unopened payloads may cause, by default: the rest of the hub's allowance (6.6) stays for the
/// app. A bot the hub does not limit raises it (`set_recovery_lookups_per_day`).
pub const RECOVERY_LOOKUPS_PER_DAY: u32 = 10;
/// Payloads parked for a roster that could not be asked yet (`State::deferred`); past it the oldest is dropped.
const DEFERRED: usize = 256;
/// Names a contact is remembered by at most: the newest.
const HANDLES_KEPT: usize = 4;
/// Events a guest (the notification service) keeps for the app at most: bounds its state while the app is closed.
pub const GUEST_EVENTS: usize = 256;
/// How often a registration whose answer did not come is tried again, with the same account key.
const REGISTER_TRIES: usize = 2;
/// How often a send updates its view of a user's device and tries again before it leaves the rest to the next call.
const DEVICE_ROUNDS: usize = 3;
/// A contact whose app is too old for sessions is asked about again at most this often (spec 4.2).
const OUTDATED_RETRY_MS: u64 = 3600 * 1000;

/// Errors that say "not now", not "never" (spec 6.6): no network, `429`, `426`, `507`, `5xx`.
pub fn retryable(error: &Error) -> bool {
    match error {
        Error::Network(_) => true,
        Error::Refused(status) => {
            matches!(*status, status::RATE_LIMITED | status::UPGRADE_REQUIRED | status::INSUFFICIENT_STORAGE | 500..)
        }
        _ => false,
    }
}

/// For the apps: keep what they were doing and try again later (`retryable`, or a state that did not save).
pub fn retry_later(error: &Error) -> bool {
    retryable(error) || matches!(error, Error::Save(_))
}

/// What happened, for the app to show. `user` is the contact's user id (hex).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Event {
    /// Someone wrote to this device for the first time.
    NewChat { user: String },
    /// A message: a Yo, with optional media.
    Message { user: String, sent_at_ms: u64, content: Content },
    /// A contact's name.
    Profile { user: String, name: String },
    /// A contact's account key changed: a verification is reset and must be confirmed before sending (spec 3.4).
    IdentityChanged { user: String },
    /// A Yo (with a photo or video or not) from `user` to a group (id, hex): `groups()` describes it.
    GroupMessage { group: String, user: String, sent_at_ms: u64, content: Content },
    /// A group came, changed (name, members) or was left: `groups()` has it as it is now.
    GroupChanged { group: String },
}

/// What waits in the outbox for one recipient in one conversation (`Client::outbox_view`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    /// The recipient's user id (hex).
    pub user: String,
    /// The group (id, hex) its messages are of; none: 1:1 (and keys, receipts, names).
    pub group: Option<String>,
    pub count: usize,
    pub reason: OutboxReason,
    /// Until when (this clock, ms) the hub holds it back (`RateLimited`), if it said.
    pub retry_at_ms: Option<u64>,
    /// It goes sealed (their key and a sender certificate here); else identified.
    pub sealed: bool,
    /// It goes as a request: their profile key is not here (spec 4.10).
    pub request: bool,
}

/// Why something waits in the outbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxReason {
    /// On its way to the hub now.
    Sending,
    /// It goes with the next delivery.
    Next,
    /// A request past its limit (`429`): until `retry_at_ms`, or until their key comes.
    RateLimited,
    /// Refused with their old key (`403`): after the next inbox read, with the new one.
    TheirKey,
    /// Their device is being asked of the hub (a new phone).
    Roster,
    /// Their inbox is full (`507`).
    InboxFull,
    /// No network, or the hub did not answer.
    Offline,
    /// Blocked here: it goes after an unblock.
    Blocked,
    /// Their key changed after a verification: it goes once confirmed.
    Unconfirmed,
    /// Their app is too old for sessions: it goes once updated.
    Outdated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactInfo {
    /// The user id (hex).
    pub user: String,
    /// How this device knows the contact: "CHRIS".
    pub handles: Vec<String>,
    pub name: Option<String>,
    pub verified: bool,
    /// The key changed after verification: `confirm_identity` before sending.
    pub unconfirmed: bool,
    pub blocked: bool,
    /// The account is gone (deleted, or its name released): sending to it is `NotOnYo`.
    pub gone: bool,
    /// Its app is too old for sessions: what is sent to it waits until it is updated (`Error::Outdated`).
    pub outdated: bool,
    /// A contact said yes to, or one this account added or wrote to; else a request (spec 4.10).
    pub accepted: bool,
}

/// A session with one device of a contact, for the app to show (spec 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// The device's key (hex).
    pub device: String,
    /// A message can go to it now: a session made with PQXDH whose sending chain is good.
    pub usable: bool,
    /// Its message did not open here last: a decryption error is on its way (4.6).
    pub retrying: bool,
}

/// Where the state goes after every operation. A failed save stops the operation before anything leaves (4.7).
pub trait Store: Send + Sync {
    fn save_state(&self, state: &State, records: signal::Entries) -> std::result::Result<(), String>;
}

pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The signed inbox request of a `sync`, sent without the client: see `Client::sync_begin`.
#[derive(Clone)]
pub struct InboxWait {
    transport: Arc<dyn Transport>,
    path: String,
    headers: Vec<(String, String)>,
    time: u64,
}

impl InboxWait {
    /// The long poll: blocks up to the wait the request asks for.
    pub fn send(&self) -> std::result::Result<Response, String> {
        self.transport.request("GET", &self.path, &self.headers, &[])
    }
}

/// A probe's inbox wait runs beside its sealed delivery, outside the caller's state lock.
pub struct Probe {
    pub began: std::time::Instant,
    user: String,
    ticket: [u8; 32],
    wait: InboxWait,
    response: std::sync::mpsc::Receiver<Answer>,
}

impl Probe {
    pub fn receive(self, timeout_ms: u64) -> Option<ProbeReply> {
        let remaining = std::time::Duration::from_millis(timeout_ms).saturating_sub(self.began.elapsed());
        let response = self.response.recv_timeout(remaining).ok()?;
        Some(ProbeReply { began: self.began, user: self.user, ticket: self.ticket, wait: self.wait, response })
    }
}

pub struct ProbeReply {
    pub began: std::time::Instant,
    user: String,
    ticket: [u8; 32],
    wait: InboxWait,
    response: Answer,
}

/// The signed lookup of `Client::whois_begin`, sent without the client.
pub struct Whois {
    transport: Arc<dyn Transport>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    me: String,
    hub_keys: Vec<Key>,
    nonce: Option<[u8; hubsig::NONCE_LEN]>,
}

impl Whois {
    /// The user id and the username as its owner writes it; `NotOnYo`: nobody by that name.
    pub fn send(&self) -> Result<(String, Option<String>)> {
        let response =
            self.transport.request("POST", api::LOOKUP, &self.headers, &self.body).map_err(Error::Network)?;
        match response.status {
            status::OK => {}
            status::NOT_FOUND => return Err(Error::NotOnYo),
            _ => return Err(refused(&response)),
        }
        let answer = hub_signed(&self.hub_keys, self.nonce.as_ref(), api::LOOKUP, &self.body, &response.body)?;
        let named: api::LookupNamed = decode(answer)?;
        let user = hex(&named.found.user_id);
        if user == self.me {
            return Err(Error::Protocol("oneself"));
        }
        Ok((user, spelled(named.username)))
    }
}

/// A username the hub tells, if it is one.
fn spelled(username: Option<yo_wire::VLBytes>) -> Option<String> {
    username.and_then(|n| String::from_utf8(n.into()).ok()).filter(|n| yo_wire::handle::check_username_form(n).is_ok())
}

/// A request signed under the client and sent without it, so the client's lock is not held for the round trip.
pub struct Request {
    transport: Arc<dyn Transport>,
    method: &'static str,
    path: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    /// The hub's status, whatever it is.
    pub fn status(&self) -> Result<u16> {
        Ok(self.response()?.status)
    }

    fn response(&self) -> Result<Response> {
        self.transport.request(self.method, self.path, &self.headers, &self.body).map_err(Error::Network)
    }
}

/// What `Client::send_end` leaves to do.
pub enum Sent {
    /// More of the outbox to send without the client, then to end again.
    More(Deliveries),
    /// Done: the events, as `send` returns them.
    Done(Result<Vec<Event>>),
}

/// An answer from the hub, or why there was none.
pub type Answer = std::result::Result<Response, String>;

/// A delivery the hub refused for good (4.4): the envelope is dropped, the user's next ones go on.
fn gone_for_good(status: u16) -> bool {
    matches!(status, status::NOT_FOUND | status::BAD_REQUEST | status::TOO_LARGE)
}

/// A delivery's answer that ends it: taken, refused for good, or a request the recipient refuses (spec 4.10).
fn settled(status: u16, item: &Delivery) -> bool {
    status == status::ACCEPTED
        || (status == status::FORBIDDEN && item.access.is_none() && !item.endorsed)
        || gone_for_good(status)
}

/// Envelopes on their way to the hub without the client: each must be ended with `Client::send_end`, even unsent.
pub struct Deliveries {
    transport: Arc<dyn Transport>,
    items: Vec<Delivery>,
    /// The user `send_begin` sent a message to.
    to: Option<String>,
}

struct Delivery {
    /// The recipient's profile key whose access key it shows; none: a request (spec 4.10).
    access: Option<[u8; 32]>,
    /// Without the profile key, a group send token for the recipient instead (spec 4.9): a contact's as well.
    endorsed: bool,
    /// Sealed with this device's sender certificate; else identified (none to hand), signed (spec 6.1).
    sealed: bool,
    seq: u64,
    user: String,
    path: String,
    headers: Vec<(String, String)>,
    envelope: Vec<u8>,
    /// The nonce a `409` must be signed for (spec 6.10), in `headers` too.
    nonce: Option<[u8; hubsig::NONCE_LEN]>,
}

/// Whether a carried key of `generation` replaces the one held: only a later generation does (spec 4.10).
fn carried_key_taken(contact: &Contact, key: &[u8; 32], generation: u32) -> bool {
    match contact.profile_key {
        Some(held) if held == *key => true,
        Some(_) => generation > contact.key_gen,
        None => generation >= contact.key_gen,
    }
}

/// Whether a queued message is this account's profile key (type 10).
fn is_key(item: &state::Outgoing) -> bool {
    decode::<yo_wire::envelope::Plain>(&item.plain)
        .ok()
        .and_then(|plain| Message::decode(plain.content.as_slice()).ok())
        .is_some_and(|m| matches!(m.content, Content::ProfileKey(_)))
}

/// Sync copies and protocol traffic must never trigger a visible push.
fn urgent(plain: &[u8], control_or_own: bool) -> bool {
    !control_or_own
        && decode::<yo_wire::envelope::Plain>(plain)
            .ok()
            .filter(|plain| plain.to.is_none())
            .and_then(|plain| Message::decode(plain.content.as_slice()).ok())
            .is_some_and(|m| shown(&m.content) && !matches!(m.content, Content::Group(_)))
}

/// What a `429` holds back (spec 6.6), by the limit the hub names in its body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Limit {
    /// A request's bucket (4.10): this recipient only, for the ms named.
    Recipient(u64),
    /// The sender's address (or bytes): every delivery.
    Address,
    /// This device's signed requests: what goes signed.
    Device,
    /// Not named (an older hub): everything, this round.
    All,
}

fn limit_of(response: &Response) -> Option<Limit> {
    if response.status != status::RATE_LIMITED {
        return None;
    }
    let body = std::str::from_utf8(&response.body).unwrap_or("");
    let mut words = body.split_whitespace();
    let seconds = |w: Option<&str>| w.and_then(|s| s.parse::<u64>().ok()).unwrap_or(60).clamp(1, 24 * 3600) * 1000;
    Some(match words.next() {
        Some("pair" | "new" | "recipient") => Limit::Recipient(seconds(words.next())),
        Some("ip" | "bytes") => Limit::Address,
        Some("device") => Limit::Device,
        _ => Limit::All,
    })
}

/// The limits a `429` of a round named so far, shared by its users' chains (`Deliveries::send`): a bit per limit.
#[derive(Default)]
struct Limits(std::sync::atomic::AtomicU8);

impl Limits {
    const ALL: u8 = 1;
    const ADDRESS: u8 = 2;
    const DEVICE: u8 = 4;

    fn hit(&self, limit: Limit) {
        let bit = match limit {
            Limit::All => Self::ALL,
            Limit::Address => Self::ADDRESS,
            Limit::Device => Self::DEVICE,
            Limit::Recipient(_) => return,
        };
        self.0.fetch_or(bit, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether a delivery (`signed` or sealed) waits for the next round.
    fn holds(&self, signed: bool) -> bool {
        let held = Self::ALL | Self::ADDRESS | if signed { Self::DEVICE } else { 0 };
        self.0.load(std::sync::atomic::Ordering::Relaxed) & held != 0
    }
}

impl Deliveries {
    /// Posts each user's envelopes in order, users in parallel. After one the hub did not take, that user's next are
    /// held (`None`) to keep their order; after a `429`, what shares its limit is held (6.6).
    pub fn send(&self) -> Vec<Option<Answer>> {
        let mut users: Vec<&str> = Vec::new();
        for item in &self.items {
            if !users.contains(&item.user.as_str()) {
                users.push(&item.user);
            }
        }
        let limits = Limits::default();
        let chain = |user: &str| -> Vec<(usize, Option<Answer>)> {
            let mut held = false;
            let mut answers = Vec::new();
            for (i, item) in self.items.iter().enumerate().filter(|(_, item)| item.user == user) {
                if held || limits.holds(!item.sealed) {
                    answers.push((i, None));
                    continue;
                }
                let answer = self.transport.request("POST", &item.path, &item.headers, &item.envelope);
                let status = answer.as_ref().map_or(0, |r| r.status);
                // A `429` holds back only what shares its limit (spec 6.6): a request's this recipient alone.
                if let Some(limit) = answer.as_ref().ok().and_then(limit_of) {
                    limits.hit(limit);
                }
                held = !settled(status, item);
                answers.push((i, Some(answer)));
            }
            answers
        };
        let mut answers: Vec<Option<Answer>> = (0..self.items.len()).map(|_| None).collect();
        let chains: Vec<Vec<(usize, Option<Answer>)>> = match users.as_slice() {
            [] => Vec::new(),
            [user] => vec![chain(user)],
            _ => std::thread::scope(|scope| {
                let running: Vec<_> = users.iter().map(|user| scope.spawn(|| chain(user))).collect();
                running.into_iter().map(|t| t.join().unwrap_or_default()).collect()
            }),
        };
        for (i, answer) in chains.into_iter().flatten() {
            answers[i] = answer;
        }
        answers
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

pub struct Client {
    transport: Arc<dyn Transport>,
    store: Option<(Arc<dyn Store>, [u8; 32])>,
    records: Records,
    state: State,
    clock: Clock,
    last_request_ms: u64,
    /// The signing time of the answer that set `prekeys.left` last: an older answer does not overwrite it.
    prekeys_known_ms: u64,
    dropped: u64,
    drop_reasons: Vec<String>,
    /// Users whose envelopes are on their way to the hub (`deliveries`): nothing else to them goes meanwhile.
    delivering: BTreeSet<String>,
    /// Users refused as a request though their key was shown (4.10): held until the inbox, where a new key may wait.
    refused_until_read: BTreeSet<String>,
    /// The hub's last answer to a delivery to each user (0: none came, no network): why theirs wait (`outbox_view`).
    last_answer: BTreeMap<String, u16>,
    /// Users whose requests the hub limits (`429`, 4.10), until when, and whether their key was refused too. Not saved.
    request_wait: BTreeMap<String, (u64, bool)>,
    /// An envelope was made (and its ratchet moved) since the state was last saved: saved before anything leaves.
    envelopes_unsaved: std::cell::Cell<bool>,
    /// Keys of photos and videos received are forgotten before anything is kept (a bot that never opens them).
    media_keys_forgotten: bool,
    /// A guest: the events it receives stay for the app; how many it has returned already.
    guest_events: Option<usize>,
    /// Inbox ids saved and handed out, not yet acknowledged at the hub. Lost with the process, they come again.
    unacked: Vec<u64>,
    /// Users this process asked the hub the name of (a contact known without one: a group's member), once each.
    names_asked: BTreeSet<String>,
    /// Inbox ids received and not yet handed out: acknowledged only once their events are saved and handed over.
    received_ids: Vec<u64>,
    /// `sync_end` received: the work after it waits for `after_sync` or the next `sync_begin`.
    after_due: bool,
    /// Once a signed inbox response says this device is revoked, no further sends leave.
    replaced: std::cell::Cell<bool>,
    probe_requests: Vec<(String, [u8; 32])>,
    /// The hub's public keys the app ships (spec 6.10): answers that hand out keys must be signed by one of them.
    hub_keys: Vec<Key>,
    /// Sender-certificate trust roots (spec 6.1): none, nothing sealed opens or is sent.
    trust_roots: Vec<Key>,
    /// Lookups a day that unopened payloads may cause (`RECOVERY_LOOKUPS_PER_DAY`).
    recovery_lookups_per_day: u32,
    /// When (this clock) a sender certificate was last asked for: at most once a minute.
    certificate_asked_at: u64,
    /// How far this device's clock is off (tests: a phone set to the wrong time); the signing time is wall time plus
    /// it.
    skew_ms: i64,
    /// The hub's zkgroup public params the app pins (spec 4.9, `set_zk_params`): none, no groups.
    zk: Option<zkgroup::ServerPublicParams>,
    /// Today's group auth credential, for the day it is (seconds), fetched once a day.
    group_auth: Option<(u64, zkgroup::auth::AuthCredentialWithPniZkc)>,
    /// Deliveries the hub took on a group send endorsement alone (`endorsed_deliveries`).
    endorsed_sent: u64,
}

/// Request headers with the nonce a signed answer is asked for (spec 6.10), if any.
fn with_nonce(mut headers: Vec<(String, String)>, nonce: Option<[u8; hubsig::NONCE_LEN]>) -> Vec<(String, String)> {
    if let Some(nonce) = nonce {
        headers.push((hubsig::NONCE_HEADER.to_owned(), yo_wire::b64(&nonce)));
    }
    headers
}

/// An answer that hands out keys, checked against the pinned hub keys and its signature cut off (spec 6.10).
/// One device per account (spec 4.4): a roster naming more is refused before anything is pinned or changed.
fn one_device(devices: &[api::Device]) -> Result<()> {
    match devices.len() {
        0 | 1 => Ok(()),
        _ => Err(Error::Protocol("more than one device")),
    }
}

fn hub_signed<'a>(
    keys: &[Key],
    nonce: Option<&[u8; hubsig::NONCE_LEN]>,
    path: &str,
    request: &[u8],
    body: &'a [u8],
) -> Result<&'a [u8]> {
    let Some(nonce) = nonce else { return Ok(body) };
    keys.iter()
        .find_map(|key| hubsig::open(key, nonce, path, request, body).ok())
        .ok_or(Error::Protocol("answer not signed by the hub"))
}

const HOUR_MS: u64 = 3_600_000;

fn wall_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// The time and signature headers of a signed request (spec 6.2), signed by `sign` (XEdDSA).
fn signature_headers(
    sign: impl FnOnce(&[u8]) -> [u8; yo_wire::SIGNATURE_LEN],
    method: &str,
    path: &str,
    time: u64,
    body_hash: &[u8; 32],
) -> Vec<(String, String)> {
    let signature = sign(&yo_wire::request::to_be_signed_hash(method, path, time, body_hash));
    vec![(HEADER_TIME.to_owned(), time.to_string()), (HEADER_SIGNATURE.to_owned(), yo_wire::b64(&signature))]
}

fn user_bytes(user: &str) -> Result<UserId> {
    unhex(user).and_then(|u| u.try_into().ok()).ok_or(Error::Protocol("user id"))
}

const PROBE_HANDLE: &str = "PINGTEST";

/// Remembers how a contact is known: a username in another spelling replaces the one before.
fn note_handle(contact: &mut Contact, shown: String) {
    match contact.handles.iter().position(|h| same_name(h, &shown)) {
        Some(index) => contact.handles[index] = shown,
        None => contact.handles.push(shown),
    }
    // Names a contact had before (renamed again and again): the newest few, not every one for good.
    if contact.handles.len() > HANDLES_KEPT {
        let extra = contact.handles.len() - HANDLES_KEPT;
        contact.handles.drain(..extra);
    }
}

/// The answer if the hub said `ok`; else why not (`410`: this device was replaced).
fn expect(response: Response, ok: u16) -> Result<Response> {
    if response.status == ok {
        return Ok(response);
    }
    Err(refused(&response))
}

fn refused(response: &Response) -> Error {
    match response.status {
        status::GONE => Error::Replaced,
        other => Error::Refused(other),
    }
}

/// A sender found by its account key at the hub, not a contact yet: pinned only once its message opened.
struct Unpinned {
    account_key: Key,
    username: Option<String>,
}

/// Content worth keeping to send again, and a receipt for: what a person sent, not the protocol's own.
fn shown(content: &Content) -> bool {
    !matches!(
        content,
        Content::Profile { .. }
            | Content::Receipt
            | Content::ProfileKey(_)
            | Content::Probe(_)
            | Content::ProbeReply(_)
            | Content::Unknown(_)
    )
}

impl Client {
    // ---- registration and state ----

    /// Registers this device with a username (spec 6.4): a new account, or with `account_secret` from the backup
    /// (spec 7) the same user again, replacing its old device. `409`: the name is taken by another key.
    pub fn register_username(
        transport: Arc<dyn Transport>,
        username: &str,
        account_secret: Option<[u8; 32]>,
        name: Option<String>,
        push: Push,
    ) -> Result<Client> {
        let mut client = Client::registering(transport, username, account_secret, None, name, push)?;
        client.finish_registration()?;
        Ok(client)
    }

    /// `register_username`, saved to `store` before the request leaves, so a crash loses neither account nor device.
    /// `legacy_storage_key`: the key an old install's storage is sealed under (`legacy::Identity`), so it opens.
    #[allow(clippy::too_many_arguments)]
    pub fn register_username_saved(
        transport: Arc<dyn Transport>,
        store: Arc<dyn Store>,
        key: [u8; 32],
        username: &str,
        account_secret: Option<[u8; 32]>,
        legacy_storage_key: Option<[u8; 32]>,
        name: Option<String>,
        push: Push,
    ) -> Result<Client> {
        let mut client = Client::registering(transport, username, account_secret, legacy_storage_key, name, push)?;
        client.persist_to(store, key)?;
        client.finish_registration()?;
        Ok(client)
    }

    /// A device with its keys and its registration (spec 6.4) made, not yet sent.
    fn registering(
        transport: Arc<dyn Transport>,
        username: &str,
        account_secret: Option<[u8; 32]>,
        legacy_storage_key: Option<[u8; 32]>,
        name: Option<String>,
        push: Push,
    ) -> Result<Client> {
        let handle = Handle::parse(username)?;
        if handle.kind != KIND_USERNAME {
            return Err(Error::Protocol("not a username"));
        }
        let account = identity::account_pair(&account_secret.unwrap_or_else(identity::new_account_secret))?;
        let (mut client, device) = Client::new_device(transport, &handle, account, name, push)?;
        let request = api::Register { username: handle.value.clone(), device };
        client.state.registering = Some(Registering { request: encode(&request), sent: false });
        client.state.username = Some(String::from_utf8_lossy(handle.value.as_slice()).into_owned());
        // A restored account fetches its storage once registered, so its blocks hold before the first message.
        client.state.storage_fetched = account_secret.is_none();
        client.state.legacy_storage_key = legacy_storage_key;
        Ok(client)
    }

    /// Whether the registration is unanswered: `finish_registration` before anything else.
    pub fn is_registering(&self) -> bool {
        self.state.registering.is_some()
    }

    /// Sends the registration again after a lost answer or a restart; a try that arrived is found by a signed lookup
    /// first. Never a new key: that would find the name taken by itself.
    pub fn finish_registration(&mut self) -> Result<()> {
        if !self.is_registering() {
            return Ok(());
        }
        let mut tries = 0;
        let mut answered = true;
        while let Some(registering) = self.state.registering.clone() {
            if registering.sent {
                let mine = Handle::account_key(&self.my_account_key());
                match self.lookup(&mine, 0, Vec::new()) {
                    Ok((found, _)) if found.devices.iter().any(|d| d.key == self.state.device_key) => {
                        self.state.user_id = hex(&found.user_id);
                        self.state.device_number =
                            found.devices.iter().find(|d| d.key == self.state.device_key).map_or(0, |me| me.id);
                        self.state.registering = None;
                        answered = false;
                        break;
                    }
                    Ok(_) | Err(Error::Refused(status::UNAUTHORIZED)) | Err(Error::NotOnYo) => {}
                    Err(error) => return Err(error),
                }
            } else if let Some(pending) = self.state.registering.as_mut() {
                pending.sent = true;
                self.persist()?;
            }
            match self.registered(api::REGISTER, &registering.request) {
                Ok(()) => self.state.registering = None,
                // The answer may be lost after the hub took it: asked first, next round.
                Err(Error::Network(_)) if tries < REGISTER_TRIES => tries += 1,
                Err(error) => return Err(error),
            }
        }
        let restored = !self.state.storage_fetched;
        if restored {
            let _ = self.sync_storage_quietly();
            // Registered without the hub's answer (it was lost): the name the user has now is asked for.
            if !answered {
                let me = Handle::user(&user_bytes(&self.state.user_id)?);
                if let Ok((_, spelled)) = self.lookup(&me, 0, Vec::new()) {
                    self.state.username = spelled;
                }
            }
        }
        // A sender certificate at once, so the first Yo goes sealed (without one it would go identified).
        if self.refresh_certificate().is_err() {
            self.note("sender certificate: not now".into());
        }
        self.persist()
    }

    /// The account key's secret: what the backup seals (spec 7), so a new phone registers with it.
    pub fn account_secret(&self) -> [u8; 32] {
        self.state.account_secret
    }

    /// Stores the backup record at the hub, replacing the one before (spec 7). `409`: another account's locator.
    pub fn put_backup(&mut self, backup: &api::Backup) -> Result<()> {
        backup.check()?;
        let response = self.signed("PUT", api::BACKUP, &encode(backup))?;
        self.persist()?; // its signing time kept, as every signed request's
        expect(response, status::NO_CONTENT).map(drop)
    }

    /// Whether a username (NFC) is free, also in other spellings and look-alikes (spec 6.3). `Refused(400)`: no
    /// username; `Refused(429)`: asked too often.
    pub fn username_free(transport: &dyn Transport, username: &str) -> Result<bool> {
        let response = transport.request("GET", &api::username_path(username), &[], &[]).map_err(Error::Network)?;
        match response.status {
            status::NO_CONTENT => Ok(true),
            status::CONFLICT => Ok(false),
            _ => Err(refused(&response)),
        }
    }

    /// A device's keys, certificate, first prekeys, and the proof of possession over its handle (spec 6.4).
    fn new_device(
        transport: Arc<dyn Transport>,
        handle: &Handle,
        account: IdentityKeyPair,
        name: Option<String>,
        push: Push,
    ) -> Result<(Client, api::NewDevice)> {
        let device_key: Key = identity::random();
        let device_id: HubDeviceId = identity::random();
        let auth_secret = identity::new_account_secret();
        // A registration id as Signal's: 14 bits, never 0.
        let registration_id = u32::from(u16::from_be_bytes(identity::random()) & 0x3fff).max(1);
        // Field by field: `State` zeroizes on drop, so it takes no `..Default::default()`.
        let mut state = State::default();
        state.account_secret =
            account.private_key().serialize().try_into().map_err(|_| Error::Internal("account key"))?;
        (state.auth_secret, state.device_id, state.device_key) = (auth_secret, device_id, device_key);
        state.certificate = identity::certify(&account, &device_key).to_bytes();
        (state.registration_id, state.name, state.requests_open) = (registration_id, name, true);
        let records = Records::new(&Default::default(), &state.account_secret, registration_id)?;
        let mut client = Client::with(transport, records, state);
        let prekeys = client.make_prekeys(PREKEY_TARGET, true)?;
        client.state.prekeys.left = PREKEY_TARGET;
        let auth_key = identity::public_of(&auth_secret);
        let proof =
            identity::sign(&account, &api::registration_to_be_signed(handle, &device_id, &device_key, &auth_key));
        let device = api::NewDevice {
            device_id,
            device_key,
            certificate: client.state.certificate.clone().into(),
            auth_key,
            push,
            registration_id,
            prekeys,
            proof,
        };
        Ok((client, device))
    }

    fn registered(&mut self, path: &str, body: &[u8]) -> Result<()> {
        let response = expect(self.transport.request("POST", path, &[], body).map_err(Error::Network)?, status::OK)?;
        let registered: api::Registered = decode(&response.body)?;
        self.state.user_id = hex(&registered.user_id);
        self.state.device_number = registered.device_id;
        // A restored account keeps the name it has at the hub now, whatever the backup had (spec 6.4).
        self.state.username = registered.username.map(|name| String::from_utf8_lossy(name.as_slice()).into_owned());
        Ok(())
    }

    /// Opens a decoded database snapshot. A guest leaves events handed to the app where they are.
    pub fn from_snapshot(
        transport: Arc<dyn Transport>,
        mut state: State,
        entries: signal::Entries,
        guest: bool,
    ) -> Result<(Client, bool)> {
        if guest {
            let records = Records::new(&entries, &state.account_secret, state.registration_id)?;
            let mut client = Client::with(transport, records, state);
            client.guest_events = Some(client.state.events.len());
            return Ok((client, false));
        }
        // What the last call handed out may not have reached the app before the restart: it comes again.
        let handed = std::mem::take(&mut state.handed);
        let changed = !handed.is_empty();
        state.events.splice(0..0, handed);
        let records = Records::new(&entries, &state.account_secret, state.registration_id)?;
        Ok((Client::with(transport, records, state), changed))
    }

    fn with(transport: Arc<dyn Transport>, records: Records, state: State) -> Client {
        Client {
            transport,
            store: None,
            records,
            state,
            clock: Arc::new(wall_ms),
            last_request_ms: 0,
            prekeys_known_ms: 0,
            dropped: 0,
            drop_reasons: Vec::new(),
            delivering: BTreeSet::new(),
            refused_until_read: BTreeSet::new(),
            last_answer: BTreeMap::new(),
            request_wait: BTreeMap::new(),
            envelopes_unsaved: std::cell::Cell::new(false),
            media_keys_forgotten: false,
            guest_events: None,
            unacked: Vec::new(),
            names_asked: BTreeSet::new(),
            received_ids: Vec::new(),
            after_due: false,
            replaced: std::cell::Cell::new(false),
            probe_requests: Vec::new(),
            hub_keys: Vec::new(),
            trust_roots: test_trust_roots(),
            recovery_lookups_per_day: RECOVERY_LOOKUPS_PER_DAY,
            certificate_asked_at: 0,
            skew_ms: 0,
            zk: groups::test_zk(),
            group_auth: None,
            endorsed_sent: 0,
        }
    }

    /// Lookups a day that unopened payloads may cause (default `RECOVERY_LOOKUPS_PER_DAY`): a bot the hub does not
    /// limit, with many users replacing phones, raises it. Not saved.
    pub fn set_recovery_lookups_per_day(&mut self, lookups: u32) {
        self.recovery_lookups_per_day = lookups;
    }

    /// Pins the hub's sender-certificate trust roots (spec 6.1), 32 bytes each (Curve25519).
    pub fn set_trust_roots(&mut self, keys: Vec<Key>) {
        self.trust_roots = keys;
    }

    /// This device's sender certificate while in date by this clock and the trust roots are pinned.
    fn certificate(&mut self) -> Option<libsignal_protocol::SenderCertificate> {
        let now = (self.clock)();
        self.state.certificate_clock = self.state.certificate_clock.max(now);
        let fresh = self.certificate_left(now).is_some_and(|left| left > 0);
        let (bytes, _) = self.state.sender_certificate.as_ref()?;
        (fresh && !self.trust_roots.is_empty())
            .then(|| libsignal_protocol::SenderCertificate::deserialize(bytes).ok())
            .flatten()
    }

    /// How long the sender certificate is still used by this clock: none if absent or the clock went back.
    fn certificate_left(&self, now: u64) -> Option<u64> {
        let (_, at) = self.state.sender_certificate.as_ref()?;
        let back = now < *at || now + 60_000 < self.state.certificate_clock;
        (!back).then(|| CERTIFICATE_USED_MS.saturating_sub(now - at))
    }

    /// Fetches a sender certificate when none is in date or it is due, at most once a minute. Quietly: without one,
    /// deliveries go identified.
    fn refresh_certificate(&mut self) -> Result<()> {
        let now = (self.clock)();
        self.state.certificate_clock = self.state.certificate_clock.max(now);
        let due = self.certificate_left(now).is_none_or(|left| left <= sealed::REFRESH_MS);
        if !due || self.trust_roots.is_empty() || self.state.user_id.is_empty() {
            return Ok(());
        }
        if self.certificate_asked_at != 0
            && now >= self.certificate_asked_at
            && now - self.certificate_asked_at < 60_000
        {
            return Ok(());
        }
        self.certificate_asked_at = now;
        let response = match self.signed("GET", api::CERTIFICATE, &[]) {
            Ok(response) => response,
            Err(error) if retryable(&error) => return Ok(()),
            Err(error) => return Err(error),
        };
        if response.status == status::GONE {
            return Err(Error::Replaced);
        }
        if response.status != status::OK {
            self.note(format!("sender certificate: {}", response.status));
            return Ok(());
        }
        let roots = sealed::roots(&self.trust_roots);
        let own = identity::account_pair(&self.state.account_secret)?;
        let user = user_bytes(&self.state.user_id)?;
        match sealed::check_own(&response.body, &roots, &user, self.state.device_number, own.identity_key()) {
            Ok(_) => {
                self.state.sender_certificate = Some((response.body, now));
                self.state.certificate_clock = now;
                self.persist()
            }
            Err(error) => {
                self.note(format!("sender certificate: {error}"));
                Ok(())
            }
        }
    }

    /// Pins the hub's public keys (spec 6.10); none: answers are not checked.
    pub fn set_hub_keys(&mut self, keys: Vec<Key>) {
        self.hub_keys = keys;
    }

    pub fn hub_key_pinned(&self) -> bool {
        !self.hub_keys.is_empty()
    }

    /// A fresh nonce for an answer that hands out keys, when keys are pinned (spec 6.10).
    fn hub_nonce(&self) -> Option<[u8; hubsig::NONCE_LEN]> {
        self.hub_key_pinned().then(identity::random::<{ hubsig::NONCE_LEN }>)
    }

    /// Reloads rows read in a single database snapshot.
    pub fn reload_snapshot(&mut self, state: State, entries: signal::Entries) -> Result<()> {
        // Another account's or device's (signed out and in again meanwhile, under the same key): not this client's.
        if state.user_id != self.state.user_id || state.device_id != self.state.device_id {
            return Err(Error::State);
        }
        self.records = Records::new(&entries, &state.account_secret, state.registration_id)?;
        self.state = state;
        // Left to the hub's repeat: the state taken in says whether they are seen.
        self.unacked.clear();
        self.received_ids.clear();
        if self.guest_events.is_some() {
            self.guest_events = Some(self.state.events.len());
        }
        Ok(())
    }

    /// Takes in a newer database snapshot, if another process changed the state.
    pub fn load_db(&self, store: &statedb::DbStore) -> Result<statedb::Loaded> {
        let Some((_, key)) = &self.store else { return Err(Error::State) };
        store.load(key)
    }

    /// Stops saving: the state is gone or someone else's now.
    pub fn stop_persisting(&mut self) {
        self.store = None;
    }

    /// Saves the sealed state to `store` after every operation from now on, and now.
    pub fn persist_to(&mut self, store: Arc<dyn Store>, key: [u8; 32]) -> Result<()> {
        self.store = Some((store, key));
        self.persist()
    }

    /// Saves to `store` after every operation from now on, not now (opened from it, unchanged).
    pub fn keep_in(&mut self, store: Arc<dyn Store>, key: [u8; 32]) {
        self.store = Some((store, key));
    }

    /// The own user id (hex): stable across new phones.
    pub fn user_id(&self) -> &str {
        &self.state.user_id
    }

    pub fn username(&self) -> Option<&str> {
        self.state.username.as_deref()
    }

    pub fn device_id(&self) -> HubDeviceId {
        self.state.device_id
    }

    /// Envelopes that could not be processed and were dropped: zero unless someone sends garbage.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Why the last envelopes were dropped: for diagnostics.
    pub fn drop_reasons(&self) -> &[String] {
        &self.drop_reasons
    }

    fn drop_one(&mut self, reason: String) {
        self.dropped += 1;
        self.note(reason);
    }

    /// Something odd that dropped nothing, for diagnostics with the drops.
    fn note(&mut self, reason: String) {
        if self.drop_reasons.len() < 20 {
            self.drop_reasons.push(reason);
        }
    }

    /// What waits in the outbox, per recipient and conversation, and why (Settings › Encryption).
    pub fn outbox_view(&self) -> Vec<OutboxEntry> {
        let now = (self.clock)();
        let mut entries: Vec<OutboxEntry> = Vec::new();
        for item in &self.state.outbox {
            let group = decode::<yo_wire::envelope::Plain>(&item.plain)
                .ok()
                .and_then(|plain| Message::decode(plain.content.as_slice()).ok())
                .and_then(|m| m.group)
                .map(|tag| groups::group_id_of(&tag.key));
            match entries.iter_mut().find(|e| e.user == item.user && e.group == group) {
                Some(entry) => entry.count += 1,
                None => {
                    let (reason, retry_at_ms) = self.waits_for(&item.user, now);
                    let sealed = self.state.contacts.get(&item.user).is_some_and(|c| c.profile_key.is_some());
                    entries.push(OutboxEntry {
                        user: item.user.clone(),
                        group,
                        count: 1,
                        reason,
                        retry_at_ms,
                        sealed: sealed && self.state.sender_certificate.is_some(),
                        request: !sealed,
                    });
                }
            }
        }
        entries
    }

    /// Why what waits for `user` waits now (`outbox_view`), and until when if the hub said.
    fn waits_for(&self, user: &str, now: u64) -> (OutboxReason, Option<u64>) {
        let wait = self.request_wait.get(user).map(|(until, _)| *until).filter(|until| *until > now);
        let contact = |flag: fn(&Contact) -> bool| self.state.contacts.get(user).is_some_and(flag);
        let reason = match self.last_answer.get(user).copied() {
            _ if self.delivering.contains(user) => OutboxReason::Sending,
            _ if wait.is_some() => return (OutboxReason::RateLimited, wait),
            _ if contact(|c| c.blocked) => OutboxReason::Blocked,
            _ if contact(|c| c.unconfirmed) => OutboxReason::Unconfirmed,
            _ if contact(|c| c.outdated_at > 0) => OutboxReason::Outdated,
            _ if self.refused_until_read.contains(user) => OutboxReason::TheirKey,
            _ if self.roster_unknown(user) || self.no_session(user) => OutboxReason::Roster,
            Some(status::INSUFFICIENT_STORAGE) => OutboxReason::InboxFull,
            Some(status::RATE_LIMITED) => OutboxReason::RateLimited,
            Some(status) if status == 0 || status >= 500 => OutboxReason::Offline,
            _ => OutboxReason::Next,
        };
        (reason, None)
    }

    /// One-time prekeys this device still holds the private keys of: of the scarcer kind.
    pub fn own_prekeys(&self) -> usize {
        self.records.prekeys.count().min(self.records.kyber.one_time())
    }

    /// Payloads kept for a roster that could not be asked yet (`State::deferred`): received again by a later sync.
    pub fn deferred(&self) -> usize {
        self.state.deferred.len()
    }

    /// Envelopes waiting to be taken by the hub; not those held for a blocked user (spec 4.8).
    pub fn waiting(&self) -> usize {
        self.state.outbox.iter().filter(|item| !self.blocked(&item.user)).count()
    }

    fn blocked(&self, user: &str) -> bool {
        self.state.contacts.get(user).is_some_and(|c| c.blocked)
    }

    /// This device's wall clock: the system's, off by the skew a test set.
    pub(crate) fn wall(&self) -> u64 {
        wall_ms().saturating_add_signed(self.skew_ms)
    }

    fn persist(&self) -> Result<()> {
        if let Some((store, _)) = &self.store {
            store.save_state(&self.state, self.records.export()).map_err(Error::Save)?;
        }
        self.envelopes_unsaved.set(false);
        Ok(())
    }

    fn push_event(&mut self, event: Event) {
        let user = match &event {
            Event::Message { user, .. }
            | Event::NewChat { user }
            | Event::Profile { user, .. }
            | Event::IdentityChanged { user } => Some(user),
            _ => None,
        };
        if user.is_some_and(|u| self.probe_service(u)) {
            return;
        }
        self.state.events.push(event);
    }

    /// Runs `f` as one transaction: on an error, state and sessions are as before. Nothing the hub remembers may run
    /// inside: what it accepted cannot be rolled back.
    fn atomic<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let state = self.state.clone();
        // The records keep aside only what the operation changes (they nest as these transactions do).
        self.records.begin();
        let result = f(self);
        if result.is_ok() {
            self.records.commit();
        } else {
            self.records.rollback();
            // The recovery lookup limits hold though the payload that used them failed (spec 4.6).
            let (asked, lookups) = (std::mem::take(&mut self.state.roster_asked), self.state.recovery_lookups);
            self.state = state;
            self.state.roster_asked = asked;
            self.state.recovery_lookups = lookups;
        }
        result
    }

    /// The app has shown or stored the handed-out events `acked` names: they will not come again after a crash.
    pub fn acknowledge_only(&mut self, mut acked: impl FnMut(&Event) -> bool) {
        if self.guest_events.is_some() {
            return;
        }
        let before = self.state.handed.len();
        self.state.handed.retain(|event| !acked(event));
        if self.state.handed.len() != before {
            // Not saved: they come again after a crash, as without the acknowledgement.
            let _ = self.persist();
        }
    }

    /// Events a step handed out whose caller failed after it (an upload that did not go): they wait for the next call
    /// again (`saved_for_app`), as if never handed out.
    pub fn hand_back(&mut self, events: &[Event]) {
        if self.guest_events.is_some() {
            return;
        }
        let back: Vec<Event> = events
            .iter()
            .filter_map(|event| {
                self.state.handed.iter().position(|e| e == event).map(|at| self.state.handed.remove(at))
            })
            .collect();
        if !back.is_empty() {
            self.state.events.splice(0..0, back);
            // Not saved: they are handed out again after a restart anyway.
            let _ = self.persist();
        }
    }

    /// Ends a public operation: saves and hands out the events. After an error they wait for the next call.
    fn finish<T>(&mut self, result: Result<T>) -> Result<Vec<Event>> {
        if let Some(returned) = self.guest_events {
            // A guest: the events stay for the app; only those new since the last call are returned.
            let total = self.state.events.len();
            let result = result.map(|_| self.state.events[returned.min(total)..].to_vec());
            let saved = self.persist();
            if saved.is_err() {
                // Not saved, whether the call went or not: the hub sends them again, nothing of it is acknowledged.
                self.received_ids.clear();
                return result.and(saved.map(|()| Vec::new()));
            }
            self.guest_events = Some(total);
            let received = std::mem::take(&mut self.received_ids);
            self.unacked.extend(received);
            return result;
        }
        let previously_handed = self.state.handed.len();
        let result = result.map(|_| {
            let new = std::mem::take(&mut self.state.events);
            let returned = new.to_vec();
            self.state.handed.extend(new);
            returned
        });
        match self.persist() {
            // Not saved: nothing is handed out, it waits for the next call.
            Err(error) if result.is_ok() => {
                let handed = self.state.handed.split_off(previously_handed);
                self.state.events.splice(0..0, handed);
                self.received_ids.clear(); // not saved: the hub sends them again
                Err(error)
            }
            Ok(()) => {
                let received = std::mem::take(&mut self.received_ids);
                self.unacked.extend(received);
                result
            }
            _ => result,
        }
    }

    /// `finish` with what the operation made.
    fn finish_with<T: Clone + Default>(&mut self, result: Result<T>) -> Result<(T, Vec<Event>)> {
        let made = result.as_ref().ok().cloned().unwrap_or_default();
        Ok((made, self.finish(result)?))
    }

    /// Like `finish`, for receiving: what arrived before a passing trouble ("not now") is handed out now.
    fn finish_receiving<T>(&mut self, result: Result<T>) -> Result<Vec<Event>> {
        match result {
            Err(error) if retryable(&error) && !self.state.events.is_empty() => self.finish(Ok(())),
            other => self.finish(other),
        }
    }

    // ---- sessions ----

    fn my_account_key(&self) -> Key {
        identity::account_pair(&self.state.account_secret).map(|pair| identity::account_key(&pair)).unwrap_or_default()
    }

    fn device_of(&self, user: &str) -> Option<Slot> {
        self.state.contacts.get(user).and_then(|c| c.device)
    }

    /// Whether this device knows `key` as the user's device at number `slot`.
    fn knows_device(&self, user: &str, key: &Key, slot: u8) -> bool {
        self.device_of(user).is_some_and(|d| d.key == *key && d.slot == slot)
    }

    fn address(&self, user: &str, slot: u8) -> Result<ProtocolAddress> {
        signal::address(&user_bytes(user)?, slot)
    }

    /// This device's address: its account's ACI and its number. Both ends bind their addresses into each message.
    fn local_address(&self) -> Result<ProtocolAddress> {
        self.address(&self.state.user_id, self.state.device_number)
    }

    fn forget_device(&mut self, user: &str) {
        let Some(device) = self.state.contacts.get_mut(user).and_then(|c| c.device.take()) else { return };
        if let Ok(address) = self.address(user, device.slot) {
            self.records.delete_session(&address);
        }
    }

    /// Notes a user's device with the number the hub gave it. A device under another number moves: its session at the
    /// old address goes.
    fn note_device(&mut self, user: &str, key: Key, slot: u8) -> Result<()> {
        DeviceId::new(slot).map_err(|_| Error::Protocol("device number"))?;
        let contact = self.state.contacts.get_mut(user).ok_or(Error::Protocol("unknown user"))?;
        let old = contact.device.replace(Slot { key, slot });
        if let Some(old) = old.filter(|old| old.slot != slot) {
            let address = self.address(user, old.slot)?;
            self.records.delete_session(&address);
        }
        Ok(())
    }

    /// Takes in a user's device as the hub names it (a lookup, or a `409`, spec 4.4; `one_device` checked it); another
    /// than known replaces it and its session. A new account key is an identity change (3.2).
    fn take_devices(
        &mut self,
        user: &str,
        account_key: Key,
        roster: &[api::Device],
        username: Option<String>,
    ) -> Result<()> {
        self.pin(user, account_key, username);
        let device = roster.first();
        if self.device_of(user).is_some_and(|known| device.is_none_or(|d| d.key != known.key)) {
            self.forget_device(user);
        }
        if let Some(device) = device {
            self.note_device(user, device.key, device.id)?;
        }
        // What waits for the user is encrypted for its device as it is now.
        for item in self.state.outbox.iter_mut().filter(|item| item.user == user) {
            item.payload = None;
        }
        Ok(())
    }

    /// Reads the inbox before a new phone's device replaces the known one: what the old one sent opens only in its
    /// session, which the replacement deletes. "Not now": the device waits too.
    fn read_before_roster(&mut self, user: &str, roster: &[api::Device]) -> Result<()> {
        match self.device_of(user) {
            Some(known) if !roster.iter().any(|r| r.key == known.key) => self.receive_all(0),
            _ => Ok(()),
        }
    }

    /// Starts a session with the user's device from its bundle: PQXDH, against the account key's identity (spec 4.1).
    fn take_bundles(&mut self, user: &str, account_key: Key, bundles: &[api::Bundle]) -> Result<()> {
        let identity = signal::identity_of(&account_key)?;
        let Some(device) = self.device_of(user) else { return Ok(()) };
        // Only for the device taken in with it.
        for bundle in bundles.iter().filter(|b| b.device_key == device.key) {
            let slot = device.slot;
            let address = self.address(user, slot)?;
            let key = |bytes: &[u8]| libsignal_protocol::PublicKey::deserialize(bytes);
            let parsed = (|| {
                PreKeyBundle::new(
                    bundle.registration_id,
                    DeviceId::new(slot).map_err(|_| SignalProtocolError::InvalidArgument("slot".into()))?,
                    match &bundle.one_time {
                        Some(one) => Some((one.id.into(), key(one.key.as_slice())?)),
                        None => None,
                    },
                    bundle.signed.id.into(),
                    key(bundle.signed.key.as_slice())?,
                    bundle.signed.signature.as_slice().to_vec(),
                    bundle.kyber.id.into(),
                    libsignal_protocol::kem::PublicKey::deserialize(bundle.kyber.key.as_slice())?,
                    bundle.kyber.signature.as_slice().to_vec(),
                    identity,
                )
            })();
            let started = parsed.and_then(|parsed| {
                self.records.trust(&address, identity);
                let local = self.local_address().map_err(|_| SignalProtocolError::InvalidArgument("me".into()))?;
                let records = &mut self.records;
                signal::now(process_prekey_bundle(
                    &address,
                    &local,
                    &mut records.sessions,
                    &mut records.identities,
                    &parsed,
                    SystemTime::now(),
                    &mut rand::rng(),
                ))
            });
            if let Err(error) = started {
                self.drop_one(format!("bundle: {error}"));
            }
        }
        Ok(())
    }

    /// Whether the user's known device has no session a message can go on now: none yet, or one gone stale.
    fn no_session(&self, user: &str) -> bool {
        self.device_of(user).is_some_and(|d| self.address(user, d.slot).map_or(true, |a| !self.records.usable(&a)))
    }

    /// Gets the session a user's waiting messages need (spec 4.1). "Not now" leaves it for the next delivery.
    fn ensure_sessions(&mut self, user: &str) -> Result<()> {
        if self.roster_unknown(user) || self.no_session(user) {
            return self.fetch_bundles(user);
        }
        Ok(())
    }

    /// A recipient whose device is unknown (a group member never written to): looked up before encrypting.
    fn roster_unknown(&self, user: &str) -> bool {
        user != self.state.user_id && self.device_of(user).is_none()
    }

    /// A new session with the user's device from a bundle the hub hands out (a usable session is named as had, so no
    /// prekey is spent), its device taken in on the way.
    fn fetch_bundles(&mut self, user: &str) -> Result<()> {
        let have: Vec<Key> =
            self.device_of(user).filter(|_| !self.no_session(user)).map(|d| d.key).into_iter().collect();
        let (found, username) = self.lookup(&Handle::user(&user_bytes(user)?), api::LOOKUP_PREKEYS, have)?;
        if hex(&found.user_id) != user {
            return Err(Error::Protocol("the hub answered for another user"));
        }
        self.read_before_roster(user, &found.devices)?;
        self.take_devices(user, found.account_key, &found.devices, username)?;
        if self.state.contacts.get(user).is_some_and(|c| c.unconfirmed) {
            return Err(Error::Unconfirmed);
        }
        self.take_bundles(user, found.account_key, &found.bundles)?;
        self.note_outdated(user);
        Ok(())
    }

    /// Notes whether the hub named a device of a contact it has no prekeys for: an app too old for sessions.
    fn note_outdated(&mut self, user: &str) {
        let outdated = self.no_session(user);
        let now = (self.clock)();
        if let Some(contact) = self.state.contacts.get_mut(user) {
            contact.outdated_at = if outdated { now.max(1) } else { 0 };
        }
    }

    /// A contact noted as too old whose device has a session now (after its update) is not any more.
    fn note_updated(&mut self, user: &str) {
        if !self.no_session(user) {
            if let Some(contact) = self.state.contacts.get_mut(user).filter(|c| c.outdated_at > 0) {
                contact.outdated_at = 0;
            }
        }
    }

    /// Encrypts a message for the user's device (spec 4.4): one payload with one copy. With `plaintext`, `plain` is a
    /// `PlaintextContent` that goes as it is (4.6). Returns the payload and the device it was made for.
    fn encrypt_for(&mut self, user: &str, plain: &[u8], plaintext: bool) -> Result<(Vec<u8>, Key)> {
        let contact = self.state.contacts.get(user).ok_or(Error::Protocol("user"))?;
        let (account_key, device) = (contact.account_key, contact.device.ok_or(Error::Protocol("no devices"))?);
        let copy = if plaintext {
            ForDevice { device_key: device.key, kind: KIND_PLAINTEXT, message: plain.to_vec().into() }
        } else {
            let address = self.address(user, device.slot)?;
            self.records.trust(&address, signal::identity_of(&account_key)?);
            let local = self.local_address()?;
            let records = &mut self.records;
            let message = signal::now(message_encrypt(
                plain,
                &address,
                &local,
                &mut records.sessions,
                &mut records.identities,
                SystemTime::now(),
                &mut rand::rng(),
            ))
            .map_err(signal::signal)?;
            ForDevice {
                device_key: device.key,
                kind: if message.message_type() == CiphertextMessageType::PreKey { KIND_PREKEY } else { KIND_MESSAGE },
                message: message.serialize().to_vec().into(),
            }
        };
        // The content's time, as Signal's envelope shows it: what a decryption error names (4.6).
        let sent_at = match plaintext {
            true => self.wall(),
            false => decode::<Plain>(plain)
                .ok()
                .and_then(|plain| Message::decode(plain.content.as_slice()).ok())
                .map_or(0, |m| m.sent_at_ms),
        };
        let payload = Payload {
            account_key: self.my_account_key(),
            device_key: self.state.device_key,
            device_id: self.state.device_number,
            sent_at,
            copies: vec![copy],
        };
        Ok((encode(&payload), device.key))
    }

    /// Pins a contact's account key; another key is an identity change: its device and session go.
    fn pin(&mut self, user: &str, account_key: Key, handle: Option<String>) {
        let contact =
            self.state.contacts.entry(user.to_owned()).or_insert_with(|| Contact { account_key, ..Contact::default() });
        if contact.account_key != account_key {
            let known = contact.account_key != [0; 32];
            contact.account_key = account_key;
            // A verified contact's new key is held until confirmed: the warning before the message (spec 3.4).
            contact.unconfirmed |= contact.verified;
            contact.verified = false;
            self.forget_device(user);
            if known {
                self.push_event(Event::IdentityChanged { user: user.to_owned() });
            }
        }
        if let Some(handle) = handle {
            self.take_handle(user, handle);
        }
    }

    pub(crate) fn probe_service(&self, user: &str) -> bool {
        self.state.contacts.get(user).is_some_and(|c| c.probe_service && !c.chat)
    }

    /// A handle the hub says is this user's now: no other contact is known by it any more (it was theirs once).
    fn take_handle(&mut self, user: &str, shown: String) {
        if same_name(&shown, PROBE_HANDLE) {
            let added = self.state.storage.friends.get(user).is_some_and(|f| !f.removed.value);
            if let Some(contact) = self.state.contacts.get_mut(user) {
                contact.probe_service = true;
                contact.chat |= added;
                if !contact.chat {
                    contact.profile_due = false;
                    contact.key_due = false;
                }
            }
        }
        for (other, contact) in self.state.contacts.iter_mut() {
            if other != user {
                contact.handles.retain(|h| !same_name(h, &shown));
            }
        }
        if let Some(contact) = self.state.contacts.get_mut(user) {
            note_handle(contact, shown);
        }
    }

    /// The user a handle names, if known: by id, or by a handle in any spelling ("yogpt" finds "YoGPT").
    fn known_user(&self, handle: &Handle) -> Option<String> {
        if handle.kind == KIND_USER {
            return Some(hex(handle.value.as_slice()));
        }
        let shown = handle.display();
        self.state
            .contacts
            .iter()
            .find(|(_, c)| c.handles.iter().any(|h| same_name(h, &shown)))
            .map(|(user, _)| user.clone())
    }

    /// The own handles: sending to them is sending to oneself.
    fn is_me(&self, handle: &Handle) -> bool {
        match handle.kind {
            KIND_USER => hex(handle.value.as_slice()) == self.state.user_id,
            KIND_USERNAME => self.state.username.as_deref().is_some_and(|mine| same_name(mine, &handle.display())),
            _ => false,
        }
    }

    fn has_chat(&self, user: &str) -> bool {
        self.device_of(user).is_some()
    }

    /// Starts writing to whoever `handle` names (spec 4.2), with a bundle; returns the user id.
    fn start_chat(&mut self, handle: &Handle) -> Result<String> {
        let (found, username) = self.lookup(handle, api::LOOKUP_PREKEYS, Vec::new())?;
        let peer = hex(&found.user_id);
        if handle.kind == KIND_USER && found.user_id.as_slice() != handle.value.as_slice() {
            return Err(Error::Protocol("the hub answered for another user"));
        }
        if peer == self.state.user_id {
            return Err(Error::Protocol("sending to oneself"));
        }
        if found.devices.is_empty() {
            return Err(Error::Protocol("no devices"));
        }
        // The name as its owner writes it; from a hub that does not tell, as it was typed.
        let shown = username.or_else(|| (handle.kind != KIND_USER).then(|| handle.display()));
        self.pin(&peer, found.account_key, shown);
        if self.state.contacts.get(&peer).is_some_and(|c| c.unconfirmed) {
            return Err(Error::Unconfirmed);
        }
        self.atomic(|c| {
            c.take_devices(&peer, found.account_key, &found.devices, None)?;
            c.take_bundles(&peer, found.account_key, &found.bundles)?;
            c.note_outdated(&peer);
            let contact = c.state.contacts.get_mut(&peer).ok_or(Error::Internal("contact"))?;
            contact.chat |= !contact.probe_service;
            contact.profile_due = contact.chat && c.state.name.is_some();
            // Added from here: accepted, and told this account's profile key (spec 4.10).
            if !contact.accepted {
                contact.accepted = true;
                contact.key_due = true;
            }
            c.persist()
        })?;
        Ok(peer)
    }

    /// Queues a message for a user: one path with a group's messages (`queue`).
    fn send_message(&mut self, user: &str, content: Content) -> Result<()> {
        if self.state.contacts.get(user).is_some_and(|c| c.unconfirmed) {
            return Err(Error::Unconfirmed);
        }
        let shown = shown(&content) && !self.probe_service(user);
        self.queue(&[user.to_owned()], None, content)?;
        if shown {
            self.accept_writing(user);
            if let Some(contact) = self.state.contacts.get_mut(user) {
                contact.chat = true;
            }
            self.keep_friend(user);
        }
        Ok(())
    }

    /// Writing to someone accepts them (spec 4.10): they get this account's profile key and its name.
    fn accept_writing(&mut self, user: &str) {
        let name = self.state.name.is_some();
        if let Some(contact) = self.state.contacts.get_mut(user).filter(|c| !c.accepted && (!c.probe_service || c.chat))
        {
            contact.accepted = true;
            contact.key_due = true;
            contact.profile_due = name;
        }
    }

    // ---- what the app calls ----

    pub fn send_yo(&mut self, to: &str) -> Result<Vec<Event>> {
        self.send(to, Content::Yo)
    }

    /// Explicitly adding or writing to the probe service makes it an ordinary contact.
    pub(crate) fn promote_contact(&mut self, user: &str) {
        if let Some(contact) = self.state.contacts.get_mut(user).filter(|c| c.probe_service && !c.chat) {
            contact.chat = true;
            contact.accepted = true;
            contact.key_due = true;
            contact.profile_due = self.state.name.is_some();
            let key = contact.profile_key;
            if key.is_some() {
                self.store_key(user, key);
            }
        }
    }

    /// Sends a message: starts writing to the user if this device has not yet (spec 4.2).
    pub fn send(&mut self, to: &str, content: Content) -> Result<Vec<Event>> {
        let result = self.send_inner(to, content);
        self.finish_sent(result)
    }

    /// `finish` for a send: queued and saved before it went (4.7), so a later failed save does not make it unsent
    /// (`Save` would make the app send a second message).
    pub(crate) fn finish_sent(&mut self, result: Result<()>) -> Result<Vec<Event>> {
        let queued = result.is_ok();
        match self.finish(result) {
            Err(Error::Save(_)) if queued => Ok(Vec::new()),
            other => other,
        }
    }

    /// Answers a Yo just received by sync, before its deferred maintenance. No inbox fetch.
    pub fn echo_yo(&mut self, user: &str) -> Result<Vec<Event>> {
        if Handle::parse(user)?.kind != KIND_USER {
            return Err(Error::Protocol("echo requires a user id"));
        }
        let result = self.send_inner(user, Content::Yo);
        self.finish_sent(result)
    }

    fn send_inner(&mut self, to: &str, content: Content) -> Result<()> {
        if self.replaced.get() {
            return Err(Error::Replaced);
        }
        let handle = Handle::parse(to)?;
        if self.is_me(&handle) {
            return Err(Error::Protocol("sending to oneself"));
        }
        if !shown(&content) {
            return Err(Error::Protocol("not content to send"));
        }
        let user = self.send_target(&handle)?;
        self.check_sendable(user.as_deref())?;
        let peer = match user.filter(|u| self.has_chat(u)) {
            Some(user) => user,
            None => self.start_chat(&handle)?,
        };
        // Writing back accepts a request: its name goes first, as one not yet told does.
        self.promote_contact(&peer);
        self.accept_writing(&peer);
        self.send_profiles();
        // Saved with the message and the due profile key ahead of it, or not made at all (spec 4.7); its envelope is
        // made in the same step, so one save precedes its leaving.
        self.atomic(|c| {
            c.queue_key(&peer)?;
            c.send_message(&peer, content)?;
            c.encrypt_waiting(&peer);
            c.persist()
        })?;
        self.deliver_now()?;
        // The hub knows the user no more: the message went nowhere. Its app too old: the message waits for it.
        self.check_sendable(Some(&peer))?;
        match self.state.contacts.get(&peer) {
            Some(contact) if contact.outdated_at > 0 => Err(Error::Outdated),
            _ => Ok(()),
        }
    }

    /// The user a send goes to: a username is asked of the hub now (without spending a prekey), since a name may have
    /// changed hands (3.3). `None`: no contacts yet, the first lookup asks.
    pub(crate) fn send_target(&mut self, handle: &Handle) -> Result<Option<String>> {
        if handle.kind == KIND_USER {
            return Ok(Some(hex(handle.value.as_slice())));
        }
        if self.state.contacts.is_empty() {
            return Ok(None);
        }
        let (found, username) = self.lookup(handle, 0, Vec::new())?;
        let id = hex(&found.user_id);
        if id == self.state.user_id {
            return Err(Error::Protocol("sending to oneself"));
        }
        if self.state.contacts.contains_key(&id) {
            self.take_handle(&id, username.unwrap_or_else(|| handle.display()));
        }
        Ok(Some(id))
    }

    /// Whether a message may go to a contact: not blocked, not waiting for a confirmation, not gone.
    fn check_sendable(&self, user: Option<&str>) -> Result<()> {
        match user.and_then(|u| self.state.contacts.get(u)) {
            Some(contact) if contact.gone => Err(Error::NotOnYo),
            Some(contact) if contact.blocked => Err(Error::Blocked),
            Some(contact) if contact.unconfirmed => Err(Error::Unconfirmed),
            _ => Ok(()),
        }
    }

    /// `send` in three steps, so a caller sharing the client behind a lock need not hold it while the message travels:
    /// `send_begin` (locked) encrypts and takes what can go, `Deliveries::send` (unlocked) posts, `send_end` (locked)
    /// takes the answers. Only to a sendable contact with a session; `None`, with nothing changed: `send` instead.
    pub fn send_begin(&mut self, to: &str, content: Content) -> Option<Deliveries> {
        if self.replaced.get() {
            return None;
        }
        let handle = Handle::parse(to).ok()?;
        // A typed name is discovery; bindings resolve it through whois.
        if handle.kind != KIND_USER || self.is_me(&handle) || !shown(&content) {
            return None;
        }
        let user = self.known_user(&handle)?;
        let contact = self.state.contacts.get(&user)?;
        if contact.blocked || contact.unconfirmed || contact.gone || contact.device.is_none() || self.no_session(&user)
        {
            return None;
        }
        // As in `send`: the name first, saved with the message or not made at all.
        self.atomic(|c| {
            c.promote_contact(&user);
            c.accept_writing(&user);
            c.send_profiles();
            c.queue_key(&user)?;
            c.send_message(&user, content)?;
            c.encrypt_waiting(&user); // its envelope in the same save
            c.persist()
        })
        .ok()?;
        let mut deliveries = self.deliveries().unwrap_or_else(|_| self.no_deliveries());
        deliveries.to = Some(user);
        Some(deliveries)
    }

    fn no_deliveries(&self) -> Deliveries {
        Deliveries { transport: self.transport.clone(), items: Vec::new(), to: None }
    }

    /// The outbox without the client, like `send_begin` without a message. Ended with `send_end`.
    pub fn deliver_begin(&mut self) -> Result<Deliveries> {
        if self.replaced.get() {
            return Err(Error::Replaced);
        }
        self.prepare()?;
        self.deliveries()
    }

    /// Deliveries whose answers were never taken (`shared::Locked::forget`): their users are no longer on their way.
    pub fn forget_deliveries(&mut self, deliveries: &Deliveries) {
        for item in &deliveries.items {
            self.delivering.remove(&item.user);
        }
    }

    /// Takes the answers to deliveries: `More` when the hub took them all and more can go now; else what `send`
    /// returns. What the hub did not take waits in the outbox.
    pub fn send_end(&mut self, deliveries: &Deliveries, answers: Vec<Option<Answer>>) -> Sent {
        let all_taken = answers.len() == deliveries.items.len()
            && answers
                .iter()
                .zip(&deliveries.items)
                .all(|(answer, item)| matches!(answer, Some(Ok(r)) if settled(r.status, item)));
        let result = match self.delivered(deliveries, answers) {
            Ok(true) => self.deliver(),
            Ok(false) if all_taken && self.check_sendable(deliveries.to.as_deref()).is_ok() => {
                match self.deliveries() {
                    Ok(mut more) if !more.is_empty() => {
                        more.to = deliveries.to.clone();
                        return Sent::More(more);
                    }
                    Ok(_) => Ok(()),
                    Err(error) => Err(error),
                }
            }
            Ok(false) => Ok(()),
            Err(error) => Err(error),
        };
        // As `send`: offline, it waits; a failed save after queuing is no reason to call it unsent.
        let result = match result {
            Ok(()) | Err(Error::Network(_) | Error::Save(_)) => self.check_sendable(deliveries.to.as_deref()),
            Err(error) => Err(error),
        };
        Sent::Done(self.finish_sent(result))
    }

    /// What another process (the notification service) received and saved for the app, handed out now.
    pub fn saved_for_app(&mut self) -> Result<Vec<Event>> {
        if self.state.events.is_empty() {
            return Ok(Vec::new());
        }
        self.finish(Ok(()))
    }

    /// Waits up to `wait_s` seconds for mail and processes it. Returns once it is saved; the hub's ack and the work
    /// after it wait for `after_sync` or the next call.
    pub fn sync(&mut self, wait_s: u64) -> Result<Vec<Event>> {
        let wait = self.sync_begin(wait_s)?;
        let response = wait.send();
        self.sync_end(&wait, response)
    }

    /// `sync` in three steps, so the client's lock is not held through the long poll: `sync_begin` (locked) delivers
    /// and signs, `InboxWait::send` (unlocked) waits, `sync_end` (locked) processes the answer against the state as it
    /// is then. The hub checks the signature's time before the wait, so signing early is fine.
    pub fn sync_begin(&mut self, wait_s: u64) -> Result<InboxWait> {
        Ok(self.sync_begin_with(wait_s, true)?.0)
    }

    /// `sync_begin` for a client shared behind a lock (the bindings' turn): the outbox is only taken, as
    /// `deliver_begin`; its way to the hub is the caller's, without the lock (`Deliveries::send`, then `send_end`). The
    /// turn is then not held while envelopes travel, so the other processes on the state (the notification service) do
    /// not wait for them. `None`: nothing to take (a guest leaves the outbox to the app).
    pub fn sync_begin_shared(&mut self, wait_s: u64) -> Result<(InboxWait, Option<Deliveries>)> {
        self.sync_begin_with(wait_s, false)
    }

    fn sync_begin_with(&mut self, wait_s: u64, deliver: bool) -> Result<(InboxWait, Option<Deliveries>)> {
        // What the last sync left (`after_sync`), then the outbox; a guest leaves the outbox to the app.
        let before = self.after_sync_inner(deliver).and_then(|()| match self.guest_events {
            Some(_) => Ok(()),
            None if deliver => self.deliver(),
            None => Ok(()),
        });
        if let Err(error) = before {
            if !retryable(&error) {
                self.persist()?;
                return Err(error);
            }
        }
        // The inbox's answer is processed in `sync_end`, without `receive_all`: the storage first.
        self.storage_fetched()?;
        let path = api::inbox_path(wait_s);
        let (headers, time) = self.sign("GET", &path, &[]);
        // Idempotent GET: a signature collision after a crash is retried by sync_end.
        let wait = InboxWait { transport: self.transport.clone(), path, headers, time };
        // Taken last: nothing after it may fail and leave its users marked as on their way.
        let deliveries = match self.guest_events {
            Some(_) => None,
            None if deliver => None,
            None => match self.deliver_begin() {
                Ok(deliveries) => Some(deliveries).filter(|d| !d.is_empty()),
                Err(error) if retryable(&error) => None,
                Err(error) => {
                    self.persist()?;
                    return Err(error);
                }
            },
        };
        Ok((wait, deliveries))
    }

    /// Processes what `InboxWait::send` brought and fetches the rest; saves and hands out the events. The hub is
    /// acknowledged only what was saved, and the rest of the work waits for `after_sync` or the next `sync_begin`.
    pub fn sync_end(
        &mut self,
        wait: &InboxWait,
        response: std::result::Result<Response, String>,
    ) -> Result<Vec<Event>> {
        let mut saved_later = Vec::new();
        let received = response
            .map_err(Error::Network)
            .and_then(|response| match response.status {
                // As `signed`: the same signature seen before (another process of this device): asked anew.
                status::UNAUTHORIZED => self.receive_all(0).map(|()| false),
                _ => self.inbox(response, wait.time, true).map(|(more, ids)| {
                    saved_later = ids;
                    more
                }),
            })
            .and_then(|more| if more { self.receive_all(0) } else { Ok(()) });
        if matches!(received, Err(Error::Replaced)) {
            self.replaced.set(true);
        }
        if received.is_ok() {
            self.refresh_groups_quietly();
        }
        // A guest does none of the work after (the app does): it only acknowledges.
        if received.is_ok() && self.guest_events.is_none() {
            self.after_due = true;
        }
        if received.is_ok()
            && saved_later.is_empty()
            && self.received_ids.is_empty()
            && self.state.events.is_empty()
            && self.state.deferred.is_empty()
        {
            // Only GET signing and maintenance hints changed.
            return Ok(Vec::new());
        }
        let events = self.finish_receiving(received);
        // Handed out only once saved: from now on the hub may forget them.
        if events.is_ok() {
            self.unacked.extend(saved_later);
        }
        events
    }

    /// The rest of a `sync`: acknowledges at the hub what was saved and, for the app, sends receipts and decryption
    /// errors, refills prekeys, delivers, writes the storage.
    pub fn after_sync(&mut self) -> Result<()> {
        let result = self.after_sync_inner(true);
        let saved = self.persist();
        result.and(saved)
    }

    /// `deliver` false: the outbox is left to the caller (`sync_begin_shared`).
    fn after_sync_inner(&mut self, deliver: bool) -> Result<()> {
        self.acknowledge_inbox()?;
        if !self.after_due {
            return Ok(());
        }
        // Due until all of it is done: a step that fails leaves it for the next call, not only the next receive.
        self.answer_due()?;
        self.refresh_groups_quietly();
        self.refill()?;
        self.profile_keys_with(deliver)?;
        if deliver {
            self.deliver()?;
        }
        // After what waits went (identified, if the certificate is out of date): never a send held for it.
        self.refresh_certificate()?;
        self.sync_storage_quietly()?;
        self.after_due = false;
        Ok(())
    }

    /// Writes a storage change (friends, blocks, the icon) to the hub now. Quietly: "not now" leaves it for later.
    pub fn write_storage(&mut self) -> Result<()> {
        let result = self.sync_storage_quietly();
        let saved = self.persist();
        result.and(saved)
    }

    /// What the app runs when it comes to the foreground: receive, answer, refill prekeys, deliver.
    pub fn maintenance(&mut self) -> Result<Vec<Event>> {
        // All of the work after a sync is done here.
        self.after_due = false;
        let result = (|| {
            self.deliver()?;
            self.receive_all(0)?;
            self.answer_due()?;
            self.refresh_groups_quietly();
            self.refresh_certificate()?;
            self.refill()?;
            self.send_profiles();
            self.profile_keys()?;
            self.deliver()?;
            self.sync_storage_quietly()
        })();
        self.finish_receiving(result)
    }

    // ---- contacts and requests (spec 4.10) ----

    /// This account's profile key: made once, its check told to the hub, sent to every contact due it.
    fn profile_keys(&mut self) -> Result<()> {
        self.profile_keys_with(true)
    }

    /// `deliver` false: the keys queued go with the caller's outbox, and the hub is told in a later round, after them.
    fn profile_keys_with(&mut self, deliver: bool) -> Result<()> {
        if self.state.profile_key.is_none() {
            // A new phone goes on with the key in the storage (spec 6.9): fetched first. Not now: later.
            match self.storage_fetched() {
                Err(error) if retryable(&error) => return Ok(()),
                other => other?,
            }
        }
        if self.state.profile_key.is_none() {
            self.state.profile_key = Some(new_profile_key());
            self.state.profile_key_gen += 1;
            self.store_own_key();
            self.key_due_to_all();
            self.persist()?;
        }
        let me = self.state.user_id.clone();
        let due: Vec<String> = self
            .state
            .contacts
            .iter()
            .filter(|(user, c)| c.key_due && !c.blocked && !c.gone && **user != me)
            .map(|(user, _)| user.clone())
            .collect();
        for user in &due {
            self.queue_key(user)?;
        }
        if !self.state.access_key_told {
            // The new key reaches contacts before the hub takes it, so their next refused delivery finds it waiting.
            if !due.is_empty() {
                if !deliver {
                    return Ok(());
                }
                match self.deliver() {
                    Err(error) if retryable(&error) => return Ok(()),
                    other => other?,
                }
            }
            let key = self.state.profile_key.expect("made above");
            let me = libsignal_protocol::Aci::from_uuid_bytes(user_bytes(&self.state.user_id)?);
            let commitment = zkgroup::profiles::ProfileKey::create(key).get_commitment(me);
            let body = encode(&api::AccessKey {
                check: api::access_check(&access_key(&key)),
                requests_open: self.state.requests_open as u8,
                commitment: zkgroup::serialize(&commitment).into(),
            });
            match self.signed("PUT", api::ACCESS_KEY, &body) {
                Ok(response) if response.status == status::NO_CONTENT => {
                    self.state.access_key_told = true;
                    self.persist()?;
                }
                Ok(response) => self.note(format!("profile key: {}", response.status)),
                Err(error) if retryable(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Queues the profile key for `user` if due, ahead of what waits for them, so their answer comes as a contact's,
    /// not a request (spec 4.10). Without a key yet, `profile_keys` sends it later.
    fn queue_key(&mut self, user: &str) -> Result<()> {
        let Some(key) = self.state.profile_key else { return Ok(()) };
        if user == self.state.user_id
            || !self.state.contacts.get(user).is_some_and(|c| c.key_due && !c.blocked && !c.gone)
        {
            return Ok(());
        }
        self.queue(&[user.to_owned()], None, Content::ProfileKey(key))?;
        // Ahead of what waits for them: a key never waits behind a request's limit.
        if let Some(first) = self.state.outbox.iter().position(|item| item.user == user) {
            if let Some(item) = self.state.outbox.pop_back() {
                self.state.outbox.insert(first, item);
            }
        }
        if let Some(contact) = self.state.contacts.get_mut(user) {
            (contact.key_due, contact.key_given) = (false, true);
        }
        Ok(())
    }

    /// A member of a group this account is in and said yes to: no request, though perhaps no contact.
    pub(crate) fn in_joined_group(&self, user: &str) -> bool {
        self.state.groups.values().any(|g| g.accepted && !g.left && g.members.iter().any(|m| m == user))
    }

    /// The current key is to go to every accepted contact written with and every member of a group joined.
    fn key_due_to_all(&mut self) {
        for (user, contact) in self.state.contacts.iter_mut() {
            let friend = self.state.storage.friends.get(user).is_some_and(|f| !f.removed.value);
            contact.key_given = false;
            // Once an hour per key: a new key goes back at once to a contact that writes identified.
            contact.key_resent_at = 0;
            contact.key_due =
                (contact.key_due || contact.accepted && (contact.chat || friend)) && !contact.blocked && !contact.gone;
        }
        let members: Vec<String> = self
            .state
            .groups
            .values()
            .filter(|g| g.accepted && !g.left)
            .flat_map(|g| g.members.iter().cloned())
            .collect();
        for member in members {
            if let Some(contact) = self.state.contacts.get_mut(&member).filter(|c| !c.blocked && !c.gone) {
                contact.key_due = true;
            }
        }
    }
}

#[cfg(test)]
mod handle_tests {
    use super::*;

    #[test]
    fn urgency_is_only_visible_content_and_never_sync_or_control() {
        use yo_wire::content::{GroupTag, Media};
        let tag = GroupTag { key: [1; 32], revision: 1 };
        let media = Media { id: [1; 16], key: [2; 32], sha256: [3; 32], size: 32, width: 1, height: 1 };
        let cases = vec![
            (Content::Yo, true),
            (Content::Photo(media.clone()), true),
            (Content::Video { media, duration_ms: 100 }, true),
            (Content::Receipt, false),
            (Content::ProfileKey([4; 32]), false),
            (Content::Profile { name: "TEST".into() }, false),
            (Content::Group(tag), false),
            (Content::Unknown(999), false),
        ];
        for (content, expected) in cases {
            for group in [None, Some(tag)] {
                // Only Yo/media can carry a group tag on the wire.
                if group.is_some() && !matches!(content, Content::Yo | Content::Photo(_) | Content::Video { .. }) {
                    continue;
                }
                let message = if let Content::Unknown(kind) = content {
                    let mut bytes = Message { sent_at_ms: 1, group: None, content: Content::Yo }.encode().unwrap();
                    bytes[10..12].copy_from_slice(&kind.to_be_bytes());
                    bytes
                } else {
                    Message { sent_at_ms: 1, group, content: content.clone() }.encode().unwrap()
                };
                for to in [None, Some([5; 16])] {
                    let plain = encode(&yo_wire::envelope::Plain { to, content: message.clone().into() });
                    assert_eq!(urgent(&plain, false), expected && to.is_none(), "{content:?}, {to:?}");
                    assert!(!urgent(&plain, true), "control reply or own-account copy, including group sync");
                }
            }
        }
        assert!(!urgent(&[255], false));
    }

    #[test]
    fn decryption_error_resends_remain_silent_after_reload() {
        let mut client = Client::registering(Arc::new(Offline), "RESEND", None, None, None, Push::none()).unwrap();
        let message = Message { sent_at_ms: 1, group: None, content: Content::Yo }.encode().unwrap();
        let plain = encode(&yo_wire::envelope::Plain { to: None, content: message.into() });
        client.enqueue_with_urgency("other", plain, true);
        let item = client.state.outbox.back().unwrap();
        let value = serde_json::to_value(item).unwrap();
        let restored: Outgoing = serde_json::from_value(value.clone()).unwrap();
        assert!(restored.non_urgent);
        assert!(!urgent(&restored.plain, restored.non_urgent));
        let mut old = value;
        old.as_object_mut().unwrap().remove("non_urgent");
        let restored: Outgoing = serde_json::from_value(old).unwrap();
        assert!(!restored.non_urgent, "older outboxes still deserialize");
        assert!(urgent(&restored.plain, restored.non_urgent));
    }

    #[test]
    fn a_failed_member_name_lookup_is_retried_for_the_next_sender() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct FailedLookup(AtomicUsize);
        impl Transport for FailedLookup {
            fn request(
                &self,
                _: &str,
                path: &str,
                _: &[(String, String)],
                _: &[u8],
            ) -> std::result::Result<Response, String> {
                assert_eq!(path, api::LOOKUP);
                self.0.fetch_add(1, Ordering::SeqCst);
                Err("offline".into())
            }
        }
        let transport = Arc::new(FailedLookup(AtomicUsize::new(0)));
        let mut client = Client::registering(transport.clone(), "RETRY", None, None, None, Push::none()).unwrap();
        let user = hex(&[7; 16]);
        let key = [8; 32];
        client.state.contacts.insert(user.clone(), Contact { account_key: key, ..Contact::default() });
        // Each incoming Yo resolves its sender through this path; a failed name lookup must not suppress the next.
        for expected in 1..=2 {
            assert_eq!(client.sender(key).unwrap().unwrap().0, user);
            assert_eq!(transport.0.load(Ordering::SeqCst), expected);
        }
    }

    #[test]
    fn a_long_poll_cut_off_is_asked_again_at_once_signed_anew() {
        // Back in front the app cuts off the wait from before the suspension and asks again with no wait: the core
        // keeps no retry state of its own, so the next sync goes out at once, with a signature the hub has not seen.
        type Seen = Vec<(String, Vec<(String, String)>)>;
        struct CutOff(std::sync::Mutex<Seen>);
        impl Transport for CutOff {
            fn request(
                &self,
                _: &str,
                path: &str,
                headers: &[(String, String)],
                _: &[u8],
            ) -> std::result::Result<Response, String> {
                self.0.lock().unwrap().push((path.to_owned(), headers.to_vec()));
                Err("cancelled".into())
            }
        }
        let transport = Arc::new(CutOff(std::sync::Mutex::new(Vec::new())));
        let mut client = Client::registering(transport.clone(), "CUTOFF", None, None, None, Push::none()).unwrap();
        client.state.storage_fetched = true;
        client.set_clock(Arc::new(|| 1_000));
        let error = client.sync(25).unwrap_err();
        assert!(matches!(error, Error::Network(_)) && retryable(&error), "{error:?}");
        assert!(client.sync(0).is_err());
        let polls: Vec<_> =
            transport.0.lock().unwrap().iter().filter(|(path, _)| path.starts_with(api::INBOX)).cloned().collect();
        assert_eq!(
            polls.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
            [api::inbox_path(25), api::inbox_path(0)]
        );
        assert_ne!(polls[0].1, polls[1].1, "signed anew, never the cut-off request's signature again");
    }

    struct Offline;
    impl Transport for Offline {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> std::result::Result<Response, String> {
            Err("offline".into())
        }
    }

    #[test]
    fn outbox_retention_survives_restore_and_keeps_recipients_separate() {
        let mut client = Client::registering(Arc::new(Offline), "RETENTION", None, None, None, Push::none()).unwrap();
        client.set_clock(Arc::new(|| 100));
        client.enqueue("other", vec![2]);
        for n in 0..OUTBOX_PER_RECIPIENT + 3 {
            client.enqueue("recipient", n.to_be_bytes().to_vec());
        }
        assert_eq!(client.state.outbox.len(), OUTBOX_PER_RECIPIENT + 1);
        assert_eq!(client.state.outbox[1].plain, 3usize.to_be_bytes());
        assert_eq!(client.dropped(), 3);
        let mut client = Client::restore(Arc::new(Offline), &client.saved()).unwrap();
        client.set_clock(Arc::new(|| 100 + OUTBOX_KEPT_MS));
        assert!(!client.prune_outbox(), "exactly 30 days is kept");
        client.enqueue("fresh", vec![3]);
        client.set_clock(Arc::new(|| 101 + OUTBOX_KEPT_MS));
        client.prune_outbox();
        assert_eq!(client.state.outbox.len(), 1);
        assert_eq!(client.state.outbox[0].user, "fresh");
    }

    #[test]
    fn permanent_delivery_rejections_leave_the_outbox_but_transient_ones_wait() {
        let mut client = Client::registering(Arc::new(Offline), "REFUSALS", None, None, None, Push::none()).unwrap();
        for status in [202, 400, 403, 404, 413, 429, 500, 503, 507] {
            client.state.outbox.clear();
            client.enqueue("recipient", vec![]);
            let deliveries = Deliveries {
                transport: Arc::new(Offline),
                to: None,
                items: vec![Delivery {
                    access: None,
                    endorsed: false,
                    sealed: false,
                    seq: client.state.next_seq,
                    user: "recipient".into(),
                    path: String::new(),
                    headers: vec![],
                    envelope: vec![],
                    nonce: None,
                }],
            };
            client.delivered(&deliveries, vec![Some(Ok(Response { status, body: vec![] }))]).unwrap();
            assert_eq!(client.state.outbox.is_empty(), matches!(status, 202 | 400 | 403 | 404 | 413), "{status}");
        }
    }

    /// A contact renamed again and again is remembered by its newest few names, the latest last, not by every one.
    #[test]
    fn a_contact_keeps_only_its_newest_names() {
        let mut contact = Contact::default();
        for i in 0..20 {
            note_handle(&mut contact, format!("NAME{i}"));
        }
        assert_eq!(contact.handles, ["NAME16", "NAME17", "NAME18", "NAME19"]);
        note_handle(&mut contact, "name19".into());
        assert_eq!(contact.handles.last().map(String::as_str), Some("name19"), "a spelling replaces its name");
    }

    /// Keys are ordered by their generation, never by a message's time (spec 4.10).
    #[test]
    fn a_carried_key_replaces_only_an_older_one() {
        let mut contact = Contact { profile_key: Some([2; 32]), key_gen: 2, ..Contact::default() };
        assert!(!carried_key_taken(&contact, &[1; 32], 1), "a Yo sent again (4.6), with the key before");
        assert!(carried_key_taken(&contact, &[3; 32], 3), "the next key");
        assert!(carried_key_taken(&contact, &[2; 32], 2), "the same");
        contact.profile_key = Some([3; 32]);
        assert!(!carried_key_taken(&contact, &[2; 32], 2), "a type 10's key since: the one before is its bound");
        contact.profile_key = None;
        assert!(!carried_key_taken(&contact, &[1; 32], 1), "forgotten (a 403): still not an older one");
        assert!(carried_key_taken(&contact, &[4; 32], 2), "a key of its generation or later");
        let unknown = Contact { profile_key: None, ..Contact::default() };
        assert!(carried_key_taken(&unknown, &[5; 32], 1));
    }
}
