//! The Yo client core (spec/v1.md): one implementation of YoIP for iOS and Android.
//!
//! The platform supplies HTTP (`Transport`) and a place to keep one sealed blob (`Store`); everything else, from the
//! sessions to the outbox, is here. Every operation either completes or leaves the state as it was, except what the
//! hub has already accepted, which is kept.

pub mod backup;
pub mod error;
pub mod groups;
pub mod identity;
pub mod media;
pub mod net;
pub mod seal;
pub mod signal;
pub mod state;
pub mod storage;
pub mod tokens;

pub use error::{Error, Result};
pub use net::{Response, Transport};
pub use yo_wire::api::{Backup, Push, BACKUP_VERSION, REPORT_ABUSE, REPORT_OTHER, REPORT_SPAM};
pub use yo_wire::content::{Content, Media};
/// Usernames as the hub takes and compares them (spec 3.3), for the apps.
pub use yo_wire::handle;

use ed25519_dalek::{Signer as _, SigningKey};
use identity::Key;
use libsignal_protocol::{
    message_decrypt, message_encrypt, process_prekey_bundle, CiphertextMessage, CiphertextMessageType, DeviceId,
    PreKeyBundle, PreKeySignalMessage, ProtocolAddress, SignalMessage, SignalProtocolError,
};
use signal::Records;
use state::{hex, unhex, Contact, Outgoing, Registering, Slot, State, StoredContent, StoredEvent};
use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use yo_wire::api::{self, status};
use yo_wire::content::Message;
use yo_wire::envelope::{ForDevice, Payload, Plain};
use yo_wire::handle::{Handle, KIND_USER, KIND_USERNAME};
use yo_wire::limits::{
    MAX_DEVICES, MAX_INBOX_FETCH, MAX_TOKENS_PER_REQUEST, PREKEY_REFILL_BELOW, PREKEY_TARGET, TOKEN_REFILL_BELOW,
    TOKEN_TARGET,
};
use yo_wire::request::{HEADER_DEVICE, HEADER_SIGNATURE, HEADER_TIME};
use yo_wire::token;
use yo_wire::{decode, encode, sha256, DeviceId as HubDeviceId, UserId};

/// A send goes without receiving first (`send_begin`) only while the inbox is up to date: waited on, or processed
/// this recently.
const INBOX_FRESH_MS: u64 = 5_000;
/// The signed prekey and the last resort Kyber prekey are replaced this often (spec 4.1)...
const ROTATE_EVERY_MS: u64 = 7 * 24 * 3600 * 1000;
/// ... and the ones replaced kept this long (wall clock): a session started with one may still wait at the hub.
const RETIRED_KEPT_MS: u64 = 31 * 24 * 3600 * 1000;
/// One-time prekeys of each kind whose private keys are kept: lookups that never lead to a message use them up at
/// the hub, and the oldest beyond this go (a message made with one later is answered by a retry, spec 4.6).
const ONE_TIME_KEPT: usize = 4 * PREKEY_TARGET as usize;
/// A contact's retry is answered at most this often (spec 4.6).
const RESEND_EVERY_MS: u64 = 60 * 1000;
/// Session message hashes remembered against redelivery.
const SEEN: usize = 2000;
/// Messages sent to a contact kept to send again when it asks (spec 4.6).
const KEPT_FOR_RETRY: usize = 64;
/// A retry is asked of one contact at most this often.
const RETRY_EVERY_MS: u64 = 60 * 1000;
/// How often a payload may make this device ask the hub for a user's devices (spec 4.4).
const ROSTER_ASK_EVERY_MS: u64 = 60 * 1000;
/// Lookups a day that payloads which did not open may make (roster checks and retries' bundles together): what is left
/// of the hub's allowance (6.6) stays for sessions the app starts, however many forgeries come.
const RECOVERY_LOOKUPS_PER_DAY: u32 = 10;
/// Payloads kept for a roster that could not be asked yet (`State::deferred`).
const DEFERRED: usize = 64;
/// How often a registration whose answer did not come is tried again, with the same account key.
const REGISTER_TRIES: usize = 2;
/// How often a send updates its view of a user's devices and tries again before it leaves the rest to the next call.
const DEVICE_ROUNDS: usize = 3;
/// After an issuance of tokens failed, the next is asked for this much later (spec 6.10).
const TOKENS_RETRY_MS: u64 = 3600 * 1000;
/// A contact whose app is too old for sessions is asked about again at most this often (spec 4.2).
const OUTDATED_RETRY_MS: u64 = 3600 * 1000;

/// Errors that say "not now", not "never" (spec 6.6), the one rule of the core and the apps: the envelope or
/// operation is tried again later. No network, `429`, `507` (no room now), `5xx`; a `426` (this app too old, spec
/// 6.3) too: nothing goes until the app is updated, and the update sends what waits.
pub fn retryable(error: &Error) -> bool {
    match error {
        Error::Network(_) => true,
        Error::Refused(status) => {
            matches!(*status, status::RATE_LIMITED | status::UPGRADE_REQUIRED | status::INSUFFICIENT_STORAGE | 500..)
        }
        _ => false,
    }
}

/// For the apps: whether to keep what they were doing (a photo or video to upload and send, an account to open) and
/// try again later: what `retryable` says, and a state that did not save (a full disk).
pub fn retry_later(error: &Error) -> bool {
    retryable(error) || matches!(error, Error::Save(_))
}

/// What happened, for the app to show. `user` is the contact's user id (hex): what `contacts()` describes and what
/// `send` takes to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Someone wrote to this device for the first time.
    NewChat { user: String },
    /// A message: a Yo, or later text, a link, a location.
    Message { user: String, sent_at_ms: u64, content: Content },
    /// One's own other device sent this to `user`.
    SentElsewhere { user: String, sent_at_ms: u64, content: Content },
    /// A contact's name.
    Profile { user: String, name: String },
    /// A contact's account key changed (a new phone without passkey, or something worse). A verification is reset; a
    /// verified contact must be confirmed before anything more is sent to it.
    IdentityChanged { user: String },
    /// A Yo (with a photo or video or not) from `user` to a group (id, hex): `groups()` describes it.
    GroupMessage { group: String, user: String, sent_at_ms: u64, content: Content },
    /// One's own other device sent this to the group.
    GroupSentElsewhere { group: String, sent_at_ms: u64, content: Content },
    /// A group came, changed (name, members) or was left: `groups()` has it as it is now.
    GroupChanged { group: String },
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
}

/// A session with one device of a contact, for the app to show (spec 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// The device's key (hex).
    pub device: String,
    /// A message can go to it now: a session made with PQXDH whose sending chain is good.
    pub usable: bool,
    /// Its message did not open here last: a new session and a retry are on their way (4.6).
    pub retrying: bool,
}

/// Where the sealed state goes after every operation. A save that fails stops the operation before anything that
/// needed it leaves (spec 4.7): `Error::Save`.
pub trait Store: Send + Sync {
    fn save(&self, blob: Vec<u8>) -> std::result::Result<(), String>;
}

pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The signed inbox request of a `sync`, sent without the client: see `Client::sync_begin`.
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

/// The signed lookup of `Client::whois_begin`, sent without the client.
pub struct Whois {
    transport: Arc<dyn Transport>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    me: String,
}

impl Whois {
    /// The user id and the username as its owner writes it. `NotOnYo`: nobody by that name. Blocks for the round
    /// trip.
    pub fn send(&self) -> Result<(String, Option<String>)> {
        let response =
            self.transport.request("POST", api::LOOKUP, &self.headers, &self.body).map_err(Error::Network)?;
        match response.status {
            status::OK => {}
            status::NOT_FOUND => return Err(Error::NotOnYo),
            _ => return Err(refused(&response)),
        }
        let named: api::LookupNamed = decode(&response.body)?;
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

/// The signed feedback of `Client::feedback_begin`, sent without the client.
pub struct FeedbackRequest {
    transport: Arc<dyn Transport>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl FeedbackRequest {
    /// Blocks for the round trip. `Refused(429)`: the day's feedback is used up.
    pub fn send(&self) -> Result<()> {
        let response =
            self.transport.request("POST", api::FEEDBACK, &self.headers, &self.body).map_err(Error::Network)?;
        match response.status {
            status::NO_CONTENT => Ok(()),
            _ => Err(refused(&response)),
        }
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

/// Envelopes of the outbox on their way to the hub, sent without the client: see `Client::send_begin`. Each must
/// be ended with `Client::send_end`, even unsent: until then its users' next envelopes wait.
pub struct Deliveries {
    transport: Arc<dyn Transport>,
    items: Vec<Delivery>,
    /// The user `send_begin` sent a message to.
    to: Option<String>,
    /// The hub refused a token of these (`401`): the delivery went again without it.
    token_refused: AtomicBool,
}

struct Delivery {
    seq: u64,
    user: String,
    path: String,
    headers: Vec<(String, String)>,
    envelope: Vec<u8>,
    /// The token it carries (`tokens::kept`), in `headers` as `Authorization`: back to the stash if it never left.
    token: Option<String>,
}

impl Deliveries {
    /// Posts each envelope, in order. After one to a user that the hub did not take, that user's next ones are held
    /// (`None`), so they keep their order; after a `429` (the hub's limit per sender, 6.6) all the rest are. Blocks
    /// for the round trips.
    pub fn send(&self) -> Vec<Option<Answer>> {
        let mut held = BTreeSet::new();
        let mut limited = false;
        let mut answers = Vec::new();
        for item in &self.items {
            if limited || held.contains(&item.user) {
                answers.push(None);
                continue;
            }
            let mut answer = self.transport.request("POST", &item.path, &item.headers, &item.envelope);
            // A token refused (spent, or of a key the hub no longer has): once more without it, as any delivery
            // without one. Never anything that would name the sender (spec 6.10).
            if item.token.is_some() && matches!(&answer, Ok(r) if r.status == status::UNAUTHORIZED) {
                self.token_refused.store(true, Ordering::Relaxed);
                let headers: Vec<_> =
                    item.headers.iter().filter(|(name, _)| name != "authorization").cloned().collect();
                answer = self.transport.request("POST", &item.path, &headers, &item.envelope);
            }
            let status = answer.as_ref().map_or(0, |r| r.status);
            limited = status == status::RATE_LIMITED;
            if status != status::ACCEPTED && !gone_for_good(status) {
                held.insert(item.user.clone());
            }
            answers.push(Some(answer));
        }
        answers
    }

    /// Nothing to send.
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
    /// The signing time of the request whose answer set `prekeys.left` last: an older answer, waited for outside the
    /// app's lock while another call refilled, does not overwrite it.
    prekeys_known_ms: u64,
    dropped: u64,
    drop_reasons: Vec<String>,
    /// Users whose envelopes are on their way to the hub (`deliveries`): nothing else to them goes meanwhile.
    delivering: BTreeSet<String>,
    /// An envelope was made (and its ratchet moved) since the state was last saved: saved before anything leaves.
    envelopes_unsaved: std::cell::Cell<bool>,
    /// Until when (clock) the inbox counts as up to date: the end of a long wait under way, or when it was emptied.
    inbox_fresh_until: u64,
    /// Keys of photos and videos received are forgotten before anything is kept (a bot that never opens them).
    media_keys_forgotten: bool,
    /// A guest (`restore_guest`): the events it receives stay in the state for the app; how many of them this one
    /// has returned already.
    guest_events: Option<usize>,
    /// The app acknowledges each batch it was handed itself (`acknowledge_only`): a call no longer acknowledges what
    /// the one before handed out, which another call running beside it may not have consumed yet.
    explicit_ack: bool,
    /// Inbox ids received and saved, not yet acknowledged at the hub: `sync_end` hands its events out first, and
    /// `after_sync` (or the next call that receives) acknowledges them. Lost with the process, they come again and are
    /// skipped as seen.
    unacked: Vec<u64>,
    /// Inbox ids a call received (its last batch, not full) and has not handed out yet: `finish` moves them to
    /// `unacked` once their events are saved and handed over, so no call acknowledges before the app has its events.
    received_ids: Vec<u64>,
    /// `sync_end` received: the work after it (receipts, retries, prekeys, the outbox, the storage, delivery tokens)
    /// waits for `after_sync`, or the next `sync_begin`.
    after_due: bool,
}

fn wall_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// The time and signature headers of a signed request (spec 6.2), by `key`: a device's auth key, or a backup door's.
fn signature_headers(
    key: &SigningKey,
    method: &str,
    path: &str,
    time: u64,
    body_hash: &[u8; 32],
) -> Vec<(String, String)> {
    let signature = key.sign(&yo_wire::request::to_be_signed_hash(method, path, time, body_hash));
    vec![(HEADER_TIME.to_owned(), time.to_string()), (HEADER_SIGNATURE.to_owned(), yo_wire::b64(&signature.to_bytes()))]
}

fn user_bytes(user: &str) -> Result<UserId> {
    unhex(user).and_then(|u| u.try_into().ok()).ok_or(Error::Protocol("user id"))
}

/// Whether two handles as shown name the same: a username by its skeleton (spec 3.3), so "yogpt" is "YoGPT".
fn same_handle(a: &str, b: &str) -> bool {
    yo_wire::handle::same_name(a, b)
}

/// Remembers how a contact is known. A username in another spelling takes the place of the one before: the newest
/// is the hub's word for how its owner writes it.
fn note_handle(contact: &mut Contact, shown: String) {
    match contact.handles.iter().position(|h| same_handle(h, &shown)) {
        Some(index) => contact.handles[index] = shown,
        None => contact.handles.push(shown),
    }
}

/// An event as the state keeps it, and back: the app-facing types carry no serde.
pub(crate) fn stored(event: Event) -> StoredEvent {
    let content = |content: Content| match content {
        Content::Yo => StoredContent::Yo,
        Content::Photo(media) => StoredContent::Photo(media.into()),
        Content::Video { media, duration_ms } => StoredContent::Video { media: media.into(), duration_ms },
        Content::Text(text) => StoredContent::Text(text),
        Content::Link(url) => StoredContent::Link(url),
        Content::Location { lat_e7, lon_e7 } => StoredContent::Location { lat_e7, lon_e7 },
        Content::Profile { name } => StoredContent::Profile(name),
        Content::Unknown(kind) => StoredContent::Unknown(kind),
        Content::Receipt => StoredContent::Unknown(yo_wire::content::TYPE_RECEIPT),
        Content::Retry => StoredContent::Unknown(yo_wire::content::TYPE_RETRY),
        Content::Group(_) => StoredContent::Unknown(yo_wire::content::TYPE_GROUP),
        Content::GroupAsk { .. } => StoredContent::Unknown(yo_wire::content::TYPE_GROUP_ASK),
    };
    match event {
        Event::NewChat { user } => StoredEvent::NewChat { user },
        Event::Message { user, sent_at_ms, content: c } => {
            StoredEvent::Message { user, sent_at_ms, content: content(c), own: false }
        }
        Event::SentElsewhere { user, sent_at_ms, content: c } => {
            StoredEvent::Message { user, sent_at_ms, content: content(c), own: true }
        }
        Event::Profile { user, name } => StoredEvent::Profile { user, name },
        Event::IdentityChanged { user } => StoredEvent::IdentityChanged { user },
        Event::GroupMessage { group, user, sent_at_ms, content: c } => {
            StoredEvent::GroupMessage { group, user, sent_at_ms, content: content(c), own: false }
        }
        Event::GroupSentElsewhere { group, sent_at_ms, content: c } => {
            StoredEvent::GroupMessage { group, user: String::new(), sent_at_ms, content: content(c), own: true }
        }
        Event::GroupChanged { group } => StoredEvent::GroupChanged { group },
    }
}

fn unstored(event: StoredEvent) -> Event {
    let content = |content: StoredContent| match content {
        StoredContent::Yo => Content::Yo,
        StoredContent::Photo(media) => Content::Photo(media.into()),
        StoredContent::Video { media, duration_ms } => Content::Video { media: media.into(), duration_ms },
        StoredContent::Text(text) => Content::Text(text),
        StoredContent::Link(url) => Content::Link(url),
        StoredContent::Location { lat_e7, lon_e7 } => Content::Location { lat_e7, lon_e7 },
        StoredContent::Profile(name) => Content::Profile { name },
        StoredContent::Unknown(kind) => Content::Unknown(kind),
    };
    match event {
        StoredEvent::NewChat { user } => Event::NewChat { user },
        StoredEvent::Message { user, sent_at_ms, content: c, own: false } => {
            Event::Message { user, sent_at_ms, content: content(c) }
        }
        StoredEvent::Message { user, sent_at_ms, content: c, own: true } => {
            Event::SentElsewhere { user, sent_at_ms, content: content(c) }
        }
        StoredEvent::Profile { user, name } => Event::Profile { user, name },
        StoredEvent::IdentityChanged { user } => Event::IdentityChanged { user },
        StoredEvent::GroupMessage { group, user, sent_at_ms, content: c, own: false } => {
            Event::GroupMessage { group, user, sent_at_ms, content: content(c) }
        }
        StoredEvent::GroupMessage { group, sent_at_ms, content: c, own: true, .. } => {
            Event::GroupSentElsewhere { group, sent_at_ms, content: content(c) }
        }
        StoredEvent::GroupChanged { group } => Event::GroupChanged { group },
    }
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
        Content::Profile { .. } | Content::Receipt | Content::Retry | Content::GroupAsk { .. } | Content::Unknown(_)
    )
}

impl Client {
    // ---- registration and state ----

    /// Registers this device with a username (spec 6.4). Without `account_secret`, a new account with a new account
    /// key: the name must be free. With the account key from the backup (spec 7), its user again, whatever the name:
    /// the user id is the same, the name the one it has now (`username`), its other devices are replaced. `409` when
    /// the name is taken by another key.
    pub fn register_username(
        transport: Arc<dyn Transport>,
        username: &str,
        account_secret: Option<[u8; 32]>,
        name: Option<String>,
        push: Push,
    ) -> Result<Client> {
        let mut client = Client::registering(transport, username, account_secret, name, push)?;
        client.finish_registration()?;
        Ok(client)
    }

    /// `register_username`, saved to `store` from the start: the account and device secrets and the request are saved
    /// before the request leaves, so a crash after the hub took it loses neither the account nor its device. A state
    /// saved so is finished by `finish_registration` after `restore` (`registering`).
    pub fn register_username_saved(
        transport: Arc<dyn Transport>,
        store: Arc<dyn Store>,
        key: [u8; 32],
        username: &str,
        account_secret: Option<[u8; 32]>,
        name: Option<String>,
        push: Push,
    ) -> Result<Client> {
        let mut client = Client::registering(transport, username, account_secret, name, push)?;
        client.persist_to(store, key)?;
        client.finish_registration()?;
        Ok(client)
    }

    /// A device with its keys and its registration (spec 6.4) made, not yet sent.
    fn registering(
        transport: Arc<dyn Transport>,
        username: &str,
        account_secret: Option<[u8; 32]>,
        name: Option<String>,
        push: Push,
    ) -> Result<Client> {
        let handle = Handle::parse(username)?;
        if handle.kind != KIND_USERNAME {
            return Err(Error::Protocol("not a username"));
        }
        let account = SigningKey::from_bytes(&account_secret.unwrap_or_else(identity::random));
        let (mut client, device) = Client::new_device(transport, &handle, account, name, push)?;
        let request = api::Register { username: handle.value.clone(), device };
        client.state.registering = Some(Registering { request: encode(&request), sent: false });
        client.state.username = Some(String::from_utf8_lossy(handle.value.as_slice()).into_owned());
        // A new account has nothing stored. A restored one takes its storage in once registered, so its blocks hold
        // before the first message; without network, before the first receive (`receive_all`).
        client.state.storage_fetched = account_secret.is_none();
        Ok(client)
    }

    /// Whether this device's registration has not been answered yet (a state saved before it, then restored):
    /// `finish_registration` before anything else.
    pub fn is_registering(&self) -> bool {
        self.state.registering.is_some()
    }

    /// Sends the registration (spec 6.4), again after a lost answer or a restart. A try sent before may have arrived:
    /// then this device is registered, and the hub says so to a lookup it signs (else `401`), so the same request
    /// is sent again only if it did not. Never a new key: that would find the name taken by itself.
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
                        if let Some(me) = found.devices.iter().find(|d| d.key == self.state.device_key) {
                            self.state.device_number = me.id;
                        }
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
        self.persist()
    }

    /// For tests: any request, signed by this device, as only a broken client would send it.
    #[cfg(feature = "testing")]
    pub fn signed_for_tests(&mut self, method: &str, path: &str, body: &[u8]) -> Result<Response> {
        self.signed(method, path, body)
    }

    /// The account key's secret (Ed25519 seed): what the backup seals (spec 7), so a new phone registers with it.
    pub fn account_secret(&self) -> [u8; 32] {
        self.state.account_secret
    }

    /// Stores the account's backup record at the hub (spec 7), replacing the one before: its old locator finds
    /// nothing from then on. `409` when another account's record has this locator.
    pub fn put_backup(&mut self, backup: &api::Backup) -> Result<()> {
        backup.check()?;
        let response = self.signed("PUT", api::BACKUP, &encode(backup))?;
        self.persist()?; // its signing time kept, as every signed request's
        match response.status {
            status::NO_CONTENT => Ok(()),
            _ => Err(refused(&response)),
        }
    }

    /// Whether a username is free (spec 6.3), asked while it is typed, before there is an account. `username` as it
    /// is written (NFC; `handle::canonical` keeps a field to that). False when it is taken, also in another spelling
    /// or as a name that looks like it, or reserved; `Refused(400)` when it is no username, `Refused(429)` when asked
    /// too often.
    pub fn username_free(transport: &dyn Transport, username: &str) -> Result<bool> {
        let response = transport.request("GET", &api::username_path(username), &[], &[]).map_err(Error::Network)?;
        match response.status {
            status::NO_CONTENT => Ok(true),
            status::CONFLICT => Ok(false),
            _ => Err(refused(&response)),
        }
    }

    /// A device's keys, certificate and first prekeys, and the proof of possession over the handle it registers
    /// with (spec 6.4).
    fn new_device(
        transport: Arc<dyn Transport>,
        handle: &Handle,
        account: SigningKey,
        name: Option<String>,
        push: Push,
    ) -> Result<(Client, api::NewDevice)> {
        let device_key: Key = identity::random();
        let device_id: HubDeviceId = identity::random();
        let auth = SigningKey::from_bytes(&identity::random());
        // A registration id as Signal's: 14 bits, never 0.
        let registration_id = u32::from(u16::from_be_bytes(identity::random()) & 0x3fff).max(1);
        let state = State {
            user_id: String::new(),
            username: None,
            account_secret: account.to_bytes(),
            auth_secret: auth.to_bytes(),
            device_id,
            device_key,
            device_number: 0,
            certificate: identity::certify(&account, &device_key).to_bytes(),
            registration_id,
            name,
            contacts: Default::default(),
            own: Vec::new(),
            outbox: VecDeque::new(),
            next_seq: 0,
            events: Vec::new(),
            handed: Vec::new(),
            seen: VecDeque::new(),
            prekeys: Default::default(),
            signed_until: 0,
            storage: Default::default(),
            storage_version: 0,
            storage_fetched: false,
            storage_dirty: false,
            storage_saved_at: 0,
            last_sent_ms: 0,
            received: VecDeque::new(),
            token_key: None,
            tokens: VecDeque::new(),
            tokens_after: 0,
            tokens_check: false,
            registering: None,
            groups: Default::default(),
            roster_asked: Default::default(),
            recovery_lookups: (0, 0),
            group_asks: Default::default(),
            group_held: Vec::new(),
            deferred: Default::default(),
        };
        let records = Records::new(&Default::default(), &state.account_secret, registration_id)?;
        let mut client = Client::with(transport, records, state);
        let prekeys = client.make_prekeys(PREKEY_TARGET, true)?;
        client.state.prekeys.left = PREKEY_TARGET;
        let auth_key = auth.verifying_key().to_bytes();
        let proof =
            account.sign(&api::registration_to_be_signed(handle, &device_id, &device_key, &auth_key)).to_bytes();
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
        let response = self.transport.request("POST", path, &[], body).map_err(Error::Network)?;
        if response.status != status::OK {
            return Err(refused(&response));
        }
        let registered: api::Registered = decode(&response.body)?;
        self.state.user_id = hex(&registered.user_id);
        self.state.device_number = registered.device_id;
        // A restored account keeps the name it has at the hub now, whatever the backup had (spec 6.4).
        self.state.username = registered.username.map(|name| String::from_utf8_lossy(name.as_slice()).into_owned());
        Ok(())
    }

    /// For fuzzing: processes one sealed envelope's payload as if it came from the inbox (one transaction, like
    /// `sync`).
    #[cfg(feature = "fuzz")]
    pub fn receive_for_fuzzing(&mut self, payload: &[u8]) -> Result<()> {
        self.atomic(|c| c.receive(payload))
    }

    /// Opens a sealed state.
    pub fn restore(transport: Arc<dyn Transport>, blob: &[u8], key: &[u8; 32]) -> Result<Client> {
        let (mut state, entries) = state::open(blob, key)?;
        // What the last call handed out may not have reached the app before the restart: it comes again.
        let handed = std::mem::take(&mut state.handed);
        state.events.splice(0..0, handed);
        let records = Records::new(&entries, &state.account_secret, state.registration_id)?;
        Ok(Client::with(transport, records, state))
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
            envelopes_unsaved: std::cell::Cell::new(false),
            inbox_fresh_until: 0,
            media_keys_forgotten: false,
            guest_events: None,
            explicit_ack: false,
            unacked: Vec::new(),
            received_ids: Vec::new(),
            after_due: false,
        }
    }

    /// Opens a sealed state as a guest: another process (a share or notification extension) using the state of the
    /// app that owns it, in turns (the platform's lock around each use). Unlike `restore`, what the app was handed
    /// stays handed, and the events this one receives are returned but stay in the state, so the app gets them too;
    /// a guest never acknowledges.
    pub fn restore_guest(transport: Arc<dyn Transport>, blob: &[u8], key: &[u8; 32]) -> Result<Client> {
        let (state, entries) = state::open(blob, key)?;
        let records = Records::new(&entries, &state.account_secret, state.registration_id)?;
        let mut client = Client::with(transport, records, state);
        client.guest_events = Some(client.state.events.len());
        Ok(client)
    }

    /// Takes in the state another process saved meanwhile (sharing it in turns, see `restore_guest`), in place of
    /// this one's: as if this process had stopped and started again from it, which the state is made to survive at
    /// any point (what leaves is saved first). What lives only in memory stays: the envelopes on their way
    /// (`delivering`; one the other process sent too goes twice, and is skipped as seen), the inbox's freshness, the
    /// signing clock. Unlike `restore`, what was handed out stays handed: this process may still be showing it.
    pub fn reload(&mut self, blob: &[u8]) -> Result<()> {
        let Some((_, key)) = &self.store else { return Err(Error::State) };
        let (state, entries) = state::open(blob, key)?;
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

    /// Stops saving: the state is gone or someone else's now (see `reload`).
    pub fn stop_persisting(&mut self) {
        self.store = None;
    }

    /// Saves the sealed state to `store` after every operation from now on, and now.
    pub fn persist_to(&mut self, store: Arc<dyn Store>, key: [u8; 32]) -> Result<()> {
        self.store = Some((store, key));
        self.persist()
    }

    /// The state sealed under `key`.
    pub fn export(&self, key: &[u8; 32]) -> Vec<u8> {
        state::seal(&self.state, self.records.export(), key)
    }

    /// For tests: the clock that decides when prekeys rotate and retries are due.
    pub fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
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

    /// One-time prekeys this device still holds the private keys of: of the scarcer kind.
    pub fn own_prekeys(&self) -> usize {
        self.records.prekeys.count().min(self.records.kyber.one_time())
    }

    /// Envelopes waiting to be taken by the hub; not those held for a blocked user (spec 4.8), which go only after
    /// an unblock.
    pub fn pending(&self) -> usize {
        self.state.outbox.iter().filter(|item| !self.blocked(&item.user)).count()
    }

    fn blocked(&self, user: &str) -> bool {
        self.state.contacts.get(user).is_some_and(|c| c.blocked)
    }

    fn persist(&self) -> Result<()> {
        if let Some((store, key)) = &self.store {
            store.save(self.export(key)).map_err(Error::Save)?;
        }
        self.envelopes_unsaved.set(false);
        Ok(())
    }

    fn push_event(&mut self, event: Event) {
        self.state.events.push(stored(event));
    }

    /// Runs `f` as one transaction: on an error, state (events included) and sessions are as before. Nothing that
    /// talks to the hub in a way the hub remembers may run inside: what it accepted cannot be rolled back.
    fn atomic<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let state = self.state.clone();
        // The records keep aside only what the operation changes (they nest as these transactions do).
        self.records.begin();
        let result = f(self);
        if result.is_ok() {
            self.records.commit();
        } else {
            self.records.rollback();
            // What the hub was asked stays asked: the limits on recovery lookups (spec 4.6) hold though the payload
            // that made them failed after.
            let (asked, lookups) = (std::mem::take(&mut self.state.roster_asked), self.state.recovery_lookups);
            self.state = state;
            self.state.roster_asked = asked;
            self.state.recovery_lookups = lookups;
        }
        result
    }

    /// The app has shown or stored the events the last call handed out: they will not come again after a crash. Without
    /// it, the next call that succeeds acknowledges them.
    pub fn acknowledge_events(&mut self) {
        if self.guest_events.is_none() && !self.state.handed.is_empty() {
            self.state.handed.clear();
            // Not saved: they come again after a crash, as without the acknowledgement.
            let _ = self.persist();
        }
    }

    /// From now on each call's events stay handed until the app acknowledges them (`acknowledge_only`), not only until
    /// the next call: for an app that makes calls side by side and consumes their events in its own time.
    pub fn acknowledge_explicitly(&mut self) {
        self.explicit_ack = true;
    }

    /// The app has shown or stored the events handed out that `acked` names (each named once, as handed): they will not
    /// come again after a crash. The others stay handed.
    pub fn acknowledge_only(&mut self, mut acked: impl FnMut(&Event) -> bool) {
        if self.guest_events.is_some() {
            return;
        }
        let before = self.state.handed.len();
        self.state.handed.retain(|event| !acked(&unstored(event.clone())));
        if self.state.handed.len() != before {
            // Not saved: they come again after a crash, as without the acknowledgement.
            let _ = self.persist();
        }
    }

    /// Ends a public operation: saves, hands out the events. Events are never lost: after an error they wait for the
    /// next call that succeeds.
    fn finish<T>(&mut self, result: Result<T>) -> Result<Vec<Event>> {
        if let Some(returned) = self.guest_events {
            // A guest: the events stay for the app; only those new since the last call are returned.
            let total = self.state.events.len();
            let result =
                result.map(|_| self.state.events[returned.min(total)..].iter().cloned().map(unstored).collect());
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
        // This call acknowledges what the last one handed out. What it hands out stays saved until the next call, so
        // a crash before the app has it brings it again.
        let result = result.map(|_| {
            let new = std::mem::take(&mut self.state.events);
            let returned = new.iter().cloned().map(unstored).collect();
            if self.explicit_ack {
                self.state.handed.extend(new);
            } else {
                self.state.handed = new;
            }
            returned
        });
        match self.persist() {
            // Not saved: nothing is handed out, it waits for the next call.
            Err(error) if result.is_ok() => {
                let handed = std::mem::take(&mut self.state.handed);
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

    /// Like `finish`, for receiving: what arrived before a passing trouble ("not now") is handed out now.
    fn finish_receiving<T>(&mut self, result: Result<T>) -> Result<Vec<Event>> {
        match result {
            Err(error) if retryable(&error) && !self.state.events.is_empty() => self.finish(Ok(())),
            other => self.finish(other),
        }
    }

    // ---- the hub ----

    /// The headers that sign a request, and its time.
    fn sign(&mut self, method: &str, path: &str, body: &[u8]) -> (Vec<(String, String)>, u64) {
        self.sign_hash(method, path, &sha256(body))
    }

    /// `sign` by the body's hash.
    fn sign_hash(&mut self, method: &str, path: &str, body_hash: &[u8; 32]) -> (Vec<(String, String)>, u64) {
        // Each signature once (the hub's replay cache): strictly increasing times, also across the processes that
        // share this state (the app and its extensions) and across a restart (`signed_until` is saved with it).
        let time = wall_ms().max(self.last_request_ms + 1).max(self.state.signed_until + 1);
        self.last_request_ms = time;
        self.state.signed_until = time;
        let key = SigningKey::from_bytes(&self.state.auth_secret);
        let mut headers = vec![(HEADER_DEVICE.to_owned(), yo_wire::b64(&self.state.device_id))];
        headers.extend(signature_headers(&key, method, path, time, body_hash));
        (headers, time)
    }

    /// A signed request. A `401` is answered once more, signed anew: the same request at the same millisecond from
    /// another process of this device (or one that crashed before saving the time it signed) is the same signature,
    /// which the hub takes once. A request refused so was not processed.
    fn signed(&mut self, method: &str, path: &str, body: &[u8]) -> Result<Response> {
        let (headers, _) = self.sign(method, path, body);
        let response = self.unsigned(method, path, &headers, body)?;
        if response.status != status::UNAUTHORIZED {
            return Ok(response);
        }
        let (headers, _) = self.sign(method, path, body);
        self.unsigned(method, path, &headers, body)
    }

    fn unsigned(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response> {
        self.transport.request(method, path, headers, body).map_err(Error::Network)
    }

    pub fn set_push(&mut self, push: Push) -> Result<()> {
        let response = self.signed("PUT", api::PUSH, &encode(&push))?;
        // Its signing time kept (spec 6.3: one clock for every process of the device).
        self.persist()?;
        match response.status {
            status::NO_CONTENT => Ok(()),
            _ => Err(refused(&response)),
        }
    }

    /// A lookup (spec 6.3): who a handle is, with its username as its owner writes it ("YoGPT"); with
    /// `LOOKUP_PREKEYS`, a bundle per device but those in `except`.
    fn lookup(&mut self, handle: &Handle, flags: u8, except: Vec<Key>) -> Result<(api::LookupResult, Option<String>)> {
        let body = encode(&api::Lookup { handle: handle.clone(), flags: flags | api::LOOKUP_USERNAME, except });
        let response = self.signed("POST", api::LOOKUP, &body)?;
        match response.status {
            status::OK => {}
            status::NOT_FOUND => return Err(Error::NotOnYo),
            _ => return Err(refused(&response)),
        }
        let named: api::LookupNamed = decode(&response.body)?;
        Ok((named.found, spelled(named.username)))
    }

    // ---- sessions ----

    fn my_account_key(&self) -> Key {
        SigningKey::from_bytes(&self.state.account_secret).verifying_key().to_bytes()
    }

    /// A user's account key and devices as this device knows them: a contact's, or this account's other devices.
    fn roster(&self, user: &str) -> Option<(Key, &[Slot])> {
        if user == self.state.user_id {
            return Some((self.my_account_key(), &self.state.own));
        }
        self.state.contacts.get(user).map(|c| (c.account_key, c.devices.as_slice()))
    }

    fn roster_mut(&mut self, user: &str) -> Option<&mut Vec<Slot>> {
        if user == self.state.user_id {
            return Some(&mut self.state.own);
        }
        self.state.contacts.get_mut(user).map(|c| &mut c.devices)
    }

    /// The local address of a user's device.
    fn address(&self, user: &str, slot: u8) -> Result<ProtocolAddress> {
        signal::address(&user_bytes(user)?, slot)
    }

    /// This device's address: its account's ACI and its number. Both ends bind their addresses into each message.
    fn local_address(&self) -> Result<ProtocolAddress> {
        self.address(&self.state.user_id, self.state.device_number)
    }

    /// Notes a device of a user with the number the hub gave it (a roster, or a payload from it that opened). A known
    /// device under another number moves: its session at the old address goes. One that took another's number keeps
    /// the address's session, which its message just made its own; a roster change deletes a gone device's session
    /// (`take_devices`).
    fn note_device(&mut self, user: &str, key: Key, slot: u8) -> Result<()> {
        DeviceId::new(slot).map_err(|_| Error::Protocol("device number"))?;
        let devices = self.roster_mut(user).ok_or(Error::Protocol("unknown user"))?;
        let old = devices.iter().find(|d| d.key == key).map(|d| d.slot);
        if old == Some(slot) {
            return Ok(());
        }
        devices.retain(|d| d.key != key && d.slot != slot);
        devices.push(Slot { key, slot });
        if let Some(old) = old {
            let address = self.address(user, old)?;
            self.records.delete_session(&address);
        }
        Ok(())
    }

    /// The number of a user's device as this device knows it.
    fn slot_of(&self, user: &str, key: &Key) -> Option<u8> {
        self.roster(user)?.1.iter().find(|d| d.key == *key).map(|d| d.slot)
    }

    /// Takes in a user's devices as the hub names them (a lookup, or a delivery's `409`, spec 4.4): sessions with
    /// devices it no longer has go, new ones get a slot (and a session with the next delivery). A new account key is
    /// an identity change for a contact (3.2), for this account a replacement of this device.
    fn take_devices(&mut self, user: &str, account_key: Key, roster: &[api::Device]) -> Result<()> {
        let keys: Vec<Key> = roster.iter().map(|d| d.key).collect();
        if user == self.state.user_id {
            if account_key != self.my_account_key() || !keys.contains(&self.state.device_key) {
                return Err(Error::Replaced);
            }
        } else {
            self.pin(user, account_key, None);
        }
        let mine = self.state.device_key;
        let gone: Vec<Slot> = self
            .roster(user)
            .map(|(_, devices)| devices.iter().filter(|d| !keys.contains(&d.key)).copied().collect())
            .unwrap_or_default();
        for device in gone {
            let address = self.address(user, device.slot)?;
            self.records.delete_session(&address);
        }
        if let Some(devices) = self.roster_mut(user) {
            devices.retain(|d| keys.contains(&d.key));
        }
        for device in roster.iter().filter(|d| d.key != mine) {
            self.note_device(user, device.key, device.id)?;
        }
        // What waits for the user is encrypted for its devices as they are now.
        for item in self.state.outbox.iter_mut().filter(|item| item.user == user) {
            item.envelope = None;
        }
        Ok(())
    }

    /// Whether a roster the hub names for a user leaves out a device this one knows (a new phone in its place): what
    /// that device sent before is read first (`read_before_roster`), while its session is still here.
    fn drops_a_device(&self, user: &str, roster: &[api::Device]) -> bool {
        self.roster(user).is_some_and(|(_, devices)| devices.iter().any(|d| !roster.iter().any(|r| r.key == d.key)))
    }

    /// Reads the inbox before a roster that drops a device is taken in: an envelope its device sent before it was
    /// replaced, waiting there, opens only in its session, which the roster deletes. "Not now": the roster waits too.
    fn read_before_roster(&mut self, user: &str, roster: &[api::Device]) -> Result<()> {
        if !self.drops_a_device(user, roster) {
            return Ok(());
        }
        self.receive_all(0)
    }

    /// Starts a session with each device of a bundle (spec 4.1): PQXDH, against the identity of the account key.
    fn take_bundles(&mut self, user: &str, account_key: Key, bundles: &[api::Bundle]) -> Result<()> {
        let identity = signal::identity_of(&account_key)?;
        for bundle in bundles {
            // Only for a device of the roster taken in with it.
            let Some(slot) = self.slot_of(user, &bundle.device_key) else { continue };
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
                // A device that made a bad bundle gets nothing; the others go on.
                self.drop_one(format!("bundle: {error}"));
            }
        }
        Ok(())
    }

    /// The devices of a user a message cannot go to now: no session yet, or one gone stale.
    fn without_session(&self, user: &str) -> Vec<Key> {
        let Some((_, devices)) = self.roster(user) else { return Vec::new() };
        devices
            .iter()
            .filter(|d| self.address(user, d.slot).map_or(true, |a| !self.records.usable(&a)))
            .map(|d| d.key)
            .collect()
    }

    /// Gets the sessions a user's waiting messages need (spec 4.1): bundles for its devices without one, from the
    /// hub. "Not now" leaves it for the next delivery.
    fn ensure_sessions(&mut self, user: &str) -> Result<()> {
        let missing = self.without_session(user);
        if missing.is_empty() {
            return Ok(());
        }
        self.fetch_bundles(user, &missing)
    }

    /// New sessions with these devices of a user, from bundles the hub hands out; the user's devices as it names them
    /// taken in on the way.
    fn fetch_bundles(&mut self, user: &str, missing: &[Key]) -> Result<()> {
        let have: Vec<Key> = self
            .roster(user)
            .map(|(_, devices)| devices.iter().map(|d| d.key).filter(|k| !missing.contains(k)).collect())
            .unwrap_or_default();
        let mut except = have;
        if user == self.state.user_id {
            except.push(self.state.device_key);
        }
        let (found, _) = self.lookup(&Handle::user(&user_bytes(user)?), api::LOOKUP_PREKEYS, except)?;
        if hex(&found.user_id) != user {
            return Err(Error::Protocol("the hub answered for another user"));
        }
        self.read_before_roster(user, &found.devices)?;
        self.take_devices(user, found.account_key, &found.devices)?;
        if let Some(contact) = self.state.contacts.get(user) {
            if contact.unconfirmed {
                return Err(Error::Unconfirmed);
            }
        }
        self.take_bundles(user, found.account_key, &found.bundles)?;
        self.note_outdated(user);
        Ok(())
    }

    /// Notes whether the hub named devices of a contact it has no prekeys for: an app too old for sessions.
    fn note_outdated(&mut self, user: &str) {
        let outdated = !self.without_session(user).is_empty();
        let now = (self.clock)();
        if let Some(contact) = self.state.contacts.get_mut(user) {
            contact.outdated_at = if outdated { now.max(1) } else { 0 };
        }
    }

    /// A contact noted as too old whose devices all have a session now (made by its own first message, after its
    /// update): it is not any more, without waiting for the hourly lookup.
    fn note_updated(&mut self, user: &str) {
        if self.state.contacts.get(user).is_some_and(|c| c.outdated_at > 0) && self.without_session(user).is_empty() {
            if let Some(contact) = self.state.contacts.get_mut(user) {
                contact.outdated_at = 0;
            }
        }
    }

    /// Encrypts a message for every device of a user (spec 4.4) and seals it to the user's account (6.1): one
    /// envelope, a copy per device.
    fn encrypt_for(&mut self, user: &str, plain: &[u8]) -> Result<(Vec<u8>, Vec<Key>)> {
        let (account_key, devices) = self.roster(user).map(|(k, d)| (k, d.to_vec())).ok_or(Error::Protocol("user"))?;
        if devices.is_empty() {
            return Err(Error::Protocol("no devices"));
        }
        let identity = signal::identity_of(&account_key)?;
        let local = self.local_address()?;
        let mut copies = Vec::new();
        for device in &devices {
            let address = self.address(user, device.slot)?;
            self.records.trust(&address, identity);
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
            copies.push(ForDevice {
                device_key: device.key,
                prekey: (message.message_type() == CiphertextMessageType::PreKey) as u8,
                message: message.serialize().to_vec().into(),
            });
        }
        let payload = Payload {
            account_key: self.my_account_key(),
            device_key: self.state.device_key,
            device_id: self.state.device_number,
            copies,
        };
        let envelope = seal::to(&account_key, &encode(&payload))?;
        Ok((envelope, devices.iter().map(|d| d.key).collect()))
    }

    /// Pins a contact's account key; another key than pinned for the same user is an identity change: its sessions
    /// and devices go with the old key.
    fn pin(&mut self, user: &str, account_key: Key, handle: Option<String>) {
        let contact =
            self.state.contacts.entry(user.to_owned()).or_insert_with(|| Contact { account_key, ..Contact::unknown() });
        if contact.account_key != account_key {
            let known = contact.account_key != [0; 32];
            contact.account_key = account_key;
            // A verified contact's new key is held until confirmed: the warning before the message (spec 3.4).
            contact.unconfirmed |= contact.verified;
            contact.verified = false;
            let devices = std::mem::take(&mut contact.devices);
            for device in devices {
                if let Ok(address) = self.address(user, device.slot) {
                    self.records.delete_session(&address);
                }
            }
            if known {
                self.push_event(Event::IdentityChanged { user: user.to_owned() });
            }
        }
        if let Some(handle) = handle {
            self.take_handle(user, handle);
        }
    }

    /// A handle the hub says is this user's now: no other contact is known by it any more (it was theirs once).
    fn take_handle(&mut self, user: &str, shown: String) {
        for (other, contact) in self.state.contacts.iter_mut() {
            if other != user {
                contact.handles.retain(|h| !same_handle(h, &shown));
            }
        }
        if let Some(contact) = self.state.contacts.get_mut(user) {
            note_handle(contact, shown);
        }
    }

    /// The user a handle names, if this device knows: by its id, or by a handle it found the contact with (a
    /// username in any spelling: "yogpt" finds the contact known as "YoGPT").
    fn known_user(&self, handle: &Handle) -> Option<String> {
        if handle.kind == KIND_USER {
            return Some(hex(handle.value.as_slice()));
        }
        let shown = handle.display();
        self.state
            .contacts
            .iter()
            .find(|(_, c)| c.handles.iter().any(|h| same_handle(h, &shown)))
            .map(|(user, _)| user.clone())
    }

    /// The own handles: sending to them is sending to oneself.
    fn is_me(&self, handle: &Handle) -> bool {
        match handle.kind {
            KIND_USER => hex(handle.value.as_slice()) == self.state.user_id,
            KIND_USERNAME => self.state.username.as_deref().is_some_and(|mine| same_handle(mine, &handle.display())),
            _ => false,
        }
    }

    /// Whether this device can write to a user: it knows the user's devices.
    fn has_chat(&self, user: &str) -> bool {
        self.state.contacts.get(user).is_some_and(|c| !c.devices.is_empty())
    }

    /// Starts writing to whoever `handle` names (spec 4.2): the hub's word for who it is, with a bundle per device;
    /// returns the user id.
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
            c.take_devices(&peer, found.account_key, &found.devices)?;
            c.take_bundles(&peer, found.account_key, &found.bundles)?;
            c.note_outdated(&peer);
            let contact = c.state.contacts.get_mut(&peer).ok_or(Error::Internal("contact"))?;
            contact.chat = true;
            contact.profile_due = c.state.name.is_some();
            c.persist()
        })?;
        Ok(peer)
    }

    /// Queues a message for a user, and for this account's other devices a copy that names the user (spec 4.4): one
    /// path with a group's messages (`queue`).
    fn send_message(&mut self, user: &str, content: Content) -> Result<()> {
        if self.state.contacts.get(user).is_some_and(|c| c.unconfirmed) {
            return Err(Error::Unconfirmed);
        }
        let shown = shown(&content);
        self.queue(&[user.to_owned()], None, content)?;
        if shown {
            if let Some(contact) = self.state.contacts.get_mut(user) {
                contact.chat = true;
            }
        }
        Ok(())
    }

    /// What waits goes first, and the inbox is taken in: the order in which things happened (spec 4.7). Offline, a
    /// message still goes into the outbox after it.
    fn catch_up(&mut self) -> Result<()> {
        match self.deliver().and_then(|()| self.receive_all(0)) {
            Err(error) if !retryable(&error) => Err(error),
            _ => Ok(()),
        }
    }

    /// Sends the outbox now, the message just queued and saved in it. What the hub cannot take now (no network, its
    /// limit, a full inbox, "not now") waits there: the send succeeded (spec 4.7), so an app never sends it again
    /// itself, a second time.
    pub(crate) fn deliver_now(&mut self) -> Result<()> {
        match self.deliver() {
            // A save that fails after it: the outbox as saved still has it, and a second copy is known by its
            // plaintext at the receiver.
            Err(error) if !retryable(&error) && !matches!(error, Error::Save(_)) => Err(error),
            _ => Ok(()),
        }
    }

    /// Makes the envelopes of what waits for `user` now, where its sessions are there and nothing of it waits on
    /// another flush (local work; what cannot be made now is made when it is sent, `deliveries`).
    fn encrypt_waiting(&mut self, user: &str) {
        if self.delivering.contains(user) || !self.without_session(user).is_empty() || self.blocked(user) {
            return;
        }
        let current: Vec<Key> = self.roster(user).map(|(_, d)| d.iter().map(|d| d.key).collect()).unwrap_or_default();
        if current.is_empty() {
            return;
        }
        for i in 0..self.state.outbox.len() {
            let item = &self.state.outbox[i];
            if item.user != user || item.envelope.is_some() || !self.still_in_group(item) {
                continue;
            }
            let plain = item.plain.clone();
            if let Ok((envelope, made_for)) = self.atomic(|c| c.encrypt_for(user, &plain)) {
                let item = &mut self.state.outbox[i];
                item.envelope = Some(envelope);
                item.made_for = made_for;
                self.envelopes_unsaved.set(true);
            }
        }
    }

    fn enqueue(&mut self, user: &str, plain: Vec<u8>) {
        self.state.next_seq += 1;
        self.state.outbox.push_back(Outgoing {
            seq: self.state.next_seq,
            user: user.to_owned(),
            plain,
            envelope: None,
            made_for: Vec::new(),
        });
    }

    /// Whether a message with this photo's or video's blob went to the user already in the same conversation (1:1:
    /// `group` none; else that group), among the last kept: the same blob sent to another group, or 1:1, is another
    /// message.
    pub(crate) fn sent_media(&self, user: &str, id: &[u8; 16], group: Option<&yo_wire::content::GroupId>) -> bool {
        self.state.contacts.get(user).is_some_and(|c| {
            c.sent.iter().any(|plaintext| {
                Message::decode(plaintext).is_ok_and(|m| {
                    m.group.as_ref().map(|g| &g.id) == group
                        && matches!(m.content, Content::Photo(media) | Content::Video { media, .. } if &media.id == id)
                })
            })
        })
    }

    // ---- delivery ----

    /// Sends the outbox in order, per user, and takes in the devices the hub names for any user whose devices this
    /// device saw otherwise (4.4), until the outbox moves.
    fn deliver(&mut self) -> Result<()> {
        for _ in 0..DEVICE_ROUNDS {
            self.prepare()?;
            let deliveries = self.deliveries()?;
            let answers = deliveries.send();
            if !self.delivered(&deliveries, answers)? {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Whether a queued message may still go: one of a group only while this account and its user are in the group
    /// (a copy for this account's other devices: while this account is).
    fn still_in_group(&self, item: &Outgoing) -> bool {
        let tag = decode::<yo_wire::envelope::Plain>(&item.plain)
            .ok()
            .and_then(|plain| Message::decode(plain.content.as_slice()).ok())
            .and_then(|m| m.group);
        tag.is_none_or(|tag| {
            self.state
                .groups
                .get(&hex(&tag.id))
                .is_some_and(|g| !g.left && (item.user == self.state.user_id || g.members.contains(&item.user)))
        })
    }

    /// The sessions the outbox needs, from the hub. A user it cannot get them for now waits.
    fn prepare(&mut self) -> Result<()> {
        let now = (self.clock)();
        let outdated: Vec<String> =
            self.state.contacts.iter().filter(|(_, c)| c.outdated_at > 0).map(|(user, _)| user.clone()).collect();
        for user in outdated {
            self.note_updated(&user);
        }
        let users: BTreeSet<String> = self
            .state
            .outbox
            .iter()
            .filter(|item| !self.blocked(&item.user) && !self.delivering.contains(&item.user))
            .filter(|item| self.state.contacts.get(&item.user).is_none_or(|c| now >= c.outdated_at + OUTDATED_RETRY_MS))
            .map(|item| item.user.clone())
            .collect();
        for user in users {
            match self.ensure_sessions(&user) {
                Ok(()) => {}
                Err(error) if retryable(&error) || error == Error::Unconfirmed => {}
                Err(error @ (Error::Save(_) | Error::Replaced)) => return Err(error),
                Err(Error::NotOnYo) => self.user_gone(&user),
                Err(error) => self.drop_one(format!("sessions: {error}")),
            }
        }
        Ok(())
    }

    /// What of the outbox can go now, in order per user, each envelope encrypted for the user's devices as they are
    /// and with its device check (4.4); its users are marked as on their way, so no other flush sends after them
    /// meanwhile. Local work only: `Deliveries::send` talks to the hub, `delivered` takes the answers.
    fn deliveries(&mut self) -> Result<Deliveries> {
        // Users whose envelopes another flush is sending, or that cannot be written to now: theirs wait, in order.
        let mut waiting = self.delivering.clone();
        let mut encrypted = false;
        let mut i = 0;
        while i < self.state.outbox.len() {
            let user = self.state.outbox[i].user.clone();
            // Nothing goes to a blocked user (4.8), nor to one whose new key waits for confirmation (3.4).
            let held = self.blocked(&user) || self.state.contacts.get(&user).is_some_and(|c| c.unconfirmed);
            if waiting.contains(&user) || held {
                waiting.insert(user);
                i += 1;
                continue;
            }
            // A group's Yo or media only while both are still in it: left since, or the user removed since, it does
            // not go (a change goes to everyone before and after; it carries no tag).
            if !self.still_in_group(&self.state.outbox[i]) {
                self.state.outbox.remove(i);
                self.note("a group's message after its member left".into());
                continue;
            }
            let current: Vec<Key> =
                self.roster(&user).map(|(_, d)| d.iter().map(|d| d.key).collect()).unwrap_or_default();
            if user == self.state.user_id && current.is_empty() {
                // No other devices of this account (any more): a copy for them is of use to nobody.
                self.state.outbox.remove(i);
                continue;
            }
            let item = &self.state.outbox[i];
            let fresh = item.envelope.is_some() && {
                let mut made = item.made_for.clone();
                let mut now = current.clone();
                made.sort_unstable();
                now.sort_unstable();
                made == now
            };
            if !fresh {
                if !self.without_session(&user).is_empty() {
                    waiting.insert(user);
                    i += 1;
                    continue;
                }
                let plain = item.plain.clone();
                match self.atomic(|c| c.encrypt_for(&user, &plain)) {
                    Ok((envelope, made_for)) => {
                        let item = &mut self.state.outbox[i];
                        item.envelope = Some(envelope);
                        item.made_for = made_for;
                        encrypted = true;
                        self.envelopes_unsaved.set(true);
                    }
                    Err(error) => {
                        self.note(format!("encrypt: {error}"));
                        waiting.insert(user);
                    }
                }
            }
            i += 1;
        }
        // Saved before anything leaves: a crash after the hub took an envelope must not bring back the ratchet state
        // that made it. A repeat after a crash is harmless, the receiver skips what it has seen.
        if encrypted || self.envelopes_unsaved.get() {
            self.persist()?;
        }
        let token_key =
            self.state.token_key.as_deref().and_then(unhex).and_then(|k| <[u8; token::NE]>::try_from(k).ok());
        let mut items = Vec::new();
        for item in &self.state.outbox {
            let Some(envelope) = &item.envelope else { continue };
            if waiting.contains(&item.user) {
                continue;
            }
            let Some((account_key, _)) = self.roster(&item.user) else { continue };
            let Ok(user) = user_bytes(&item.user) else { continue };
            // The hub's roster of this account holds this device too, which needs no copy.
            let mut named = item.made_for.clone();
            if item.user == self.state.user_id {
                named.push(self.state.device_key);
            }
            let hash = yo_wire::devices::hash(&account_key, &named);
            items.push(Delivery {
                seq: item.seq,
                user: item.user.clone(),
                path: api::deliver_path(&user),
                headers: vec![(api::HEADER_DEVICES.to_owned(), yo_wire::b64(&hash))],
                envelope: envelope.clone(),
                token: None,
            });
        }
        // One token per delivery while there are any, the oldest first: the hub counts it instead of the address.
        if let Some(key) = &token_key {
            for delivery in &mut items {
                while let Some(kept) = self.state.tokens.pop_front() {
                    if let Some(found) = tokens::unkept(&kept, key) {
                        delivery.headers.push(("authorization".to_owned(), found.authorization()));
                        delivery.token = Some(kept);
                        break;
                    }
                }
            }
        }
        self.delivering.extend(items.iter().map(|d| d.user.clone()));
        Ok(Deliveries { transport: self.transport.clone(), items, to: None, token_refused: Default::default() })
    }

    /// Takes the hub's answers to `Deliveries::send`: what it took leaves the outbox, what it refused for good is
    /// dropped, the rest waits for the next flush. A `409` names the user's devices as they are: taken in, and true,
    /// so the outbox goes again for them.
    fn delivered(&mut self, deliveries: &Deliveries, answers: Vec<Option<Answer>>) -> Result<bool> {
        let mut done = BTreeSet::new();
        let mut gone = BTreeSet::new();
        let mut changed = false;
        // Tokens that never reached the hub (held, or no answer) go back to the front of the stash, in their order. A
        // lost answer may hide a spent one: then the hub refuses it next time, and the delivery goes without.
        let unsent: Vec<String> = deliveries
            .items
            .iter()
            .zip(&answers)
            .filter(|(_, answer)| !matches!(answer, Some(Ok(_))))
            .filter_map(|(item, _)| item.token.clone())
            .collect();
        for kept in unsent.into_iter().rev() {
            self.state.tokens.push_front(kept);
        }
        if deliveries.token_refused.load(Ordering::Relaxed) {
            self.state.tokens_check = true;
        }
        let mut result = Ok(());
        for (item, answer) in deliveries.items.iter().zip(answers) {
            self.delivering.remove(&item.user);
            match answer {
                Some(Ok(response)) if response.status == status::ACCEPTED => {
                    done.insert(item.seq);
                }
                Some(Ok(response)) if response.status == status::CONFLICT => {
                    match decode::<api::Devices>(&response.body) {
                        Ok(devices) => match self
                            .read_before_roster(&item.user, &devices.devices)
                            .and_then(|()| self.take_devices(&item.user, devices.account_key, &devices.devices))
                        {
                            Ok(()) => changed = true,
                            // The inbox could not be read first: the envelope waits like any "not now".
                            Err(error) if retryable(&error) => self.note(format!("roster later: {error}")),
                            Err(error) => result = Err(error),
                        },
                        // The hub did not take it: kept, and tried again like any "not now".
                        Err(_) => self.note("hub answered 409 without devices".into()),
                    }
                }
                Some(Ok(response)) if response.status == status::NOT_FOUND && item.user != self.state.user_id => {
                    done.insert(item.seq);
                    gone.insert(item.user.clone());
                }
                Some(Ok(response)) if gone_for_good(response.status) => {
                    done.insert(item.seq);
                    self.drop_one(format!("hub refused delivery: {}", response.status));
                }
                // Rate limit, hub trouble, no network, or held behind such an answer: later, in the same order.
                _ => {}
            }
        }
        self.state.outbox.retain(|item| !done.contains(&item.seq));
        for user in gone {
            self.user_gone(&user);
        }
        result.map(|()| changed)
    }

    /// The hub knows no such user any more (a delivery's `404`): the account was deleted or its name released. Its
    /// sessions and what waits for it go; its handles are forgotten (the name may be someone else's now, a stranger to
    /// this device); sending to it is `NotOnYo` from now on.
    fn user_gone(&mut self, user: &str) {
        let Some(contact) = self.state.contacts.get_mut(user) else { return };
        contact.gone = true;
        contact.handles.clear();
        let devices = std::mem::take(&mut contact.devices);
        for device in devices {
            if let Ok(address) = self.address(user, device.slot) {
                self.records.delete_session(&address);
            }
        }
        self.state.outbox.retain(|item| item.user != user);
    }

    // ---- receiving ----

    /// Fetches and processes the inbox until it is empty. Transport failures stop it with nothing lost.
    fn receive_all(&mut self, wait_s: u64) -> Result<()> {
        self.storage_fetched()?;
        // What the hub would send again otherwise.
        self.acknowledge_inbox()?;
        let mut wait = wait_s;
        loop {
            let path = api::inbox_path(wait);
            let (headers, mut time) = self.sign("GET", &path, &[]);
            let mut response = self.unsigned("GET", &path, &headers, &[])?;
            if response.status == status::UNAUTHORIZED {
                // As `signed`: once more, signed anew.
                let (headers, again) = self.sign("GET", &path, &[]);
                (response, time) = (self.unsigned("GET", &path, &headers, &[])?, again);
            }
            // The last batch is acknowledged only after `finish` has saved and handed out its events.
            let (more, ids) = self.inbox(response, time, true)?;
            self.received_ids.extend(ids);
            if !more {
                self.inbox_fresh_until = (self.clock)() + INBOX_FRESH_MS;
                return Ok(());
            }
            wait = 0;
        }
    }

    /// The storage fetched once on this device before anything is received: its blocks hold from the first message
    /// on. "Not now" leaves the inbox for later.
    fn storage_fetched(&mut self) -> Result<()> {
        if self.state.storage_fetched {
            return Ok(());
        }
        match self.sync_storage() {
            Err(error) if retryable(&error) || error == Error::Replaced => Err(error),
            Err(error) => {
                self.drop_one(format!("storage: {error}"));
                self.state.storage_fetched = true;
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }

    /// Processes one inbox answer to a request signed at `requested_at` and acknowledges it; `true` when the hub has
    /// more. The answer may be older than the state (`sync` waits for it outside the app's lock while other calls
    /// run): envelopes already seen are skipped, an older prekey count is ignored. With `later`, an answer that is
    /// all there is and was all taken in is neither saved nor acknowledged here: its ids are returned, for the caller
    /// to acknowledge once it saved (`sync_end`).
    fn inbox(&mut self, response: Response, requested_at: u64, later: bool) -> Result<(bool, Vec<u64>)> {
        if response.status != status::OK {
            return Err(refused(&response));
        }
        let inbox: api::Inbox = decode(&response.body)?;
        if requested_at > self.prekeys_known_ms {
            self.state.prekeys.left = inbox.prekeys_left;
            self.prekeys_known_ms = requested_at;
        }
        let full = inbox.items.len() >= MAX_INBOX_FETCH;
        let mut done = Vec::new();
        let mut stopped = None;
        // Payloads kept for a roster that could not be asked: received again first (each may be kept again).
        for payload in std::mem::take(&mut self.state.deferred) {
            match self.atomic(|c| c.receive(&payload)) {
                Ok(()) => {}
                Err(error) if retryable(&error) => self.state.deferred.push_back(payload),
                Err(error) => self.drop_one(error.to_string()),
            }
        }
        for item in inbox.items {
            // Not sealed to this account (spec 6.1): of use to nobody here.
            let payload = match seal::open(&self.state.account_secret, item.envelope.as_slice()) {
                Ok(payload) => payload,
                Err(error) => {
                    self.drop_one(error.to_string());
                    done.push(item.id);
                    continue;
                }
            };
            match self.atomic(|c| c.receive(&payload)) {
                Ok(()) => {}
                // Not this envelope's fault: it stays at the hub and is read again next time.
                Err(error) if retryable(&error) => {
                    stopped = Some(error);
                    break;
                }
                Err(error) => self.drop_one(error.to_string()),
            }
            done.push(item.id);
        }
        if later && !full && stopped.is_none() {
            return Ok((false, done));
        }
        if !done.is_empty() {
            // Saved before the hub may forget: what was processed must survive a crash right after the ack.
            self.persist()?;
            if !self.ack(done)? {
                return Ok((false, Vec::new()));
            }
        }
        match stopped {
            Some(error) => Err(error),
            None => Ok((full, Vec::new())),
        }
    }

    /// Acknowledges inbox ids whose envelopes are saved as processed; `false` when the ack did not arrive. That costs
    /// only a repeat: the hub sends those envelopes again, and they are skipped as seen.
    fn ack(&mut self, ids: Vec<u64>) -> Result<bool> {
        match self.signed("POST", api::INBOX_ACK, &encode(&api::Ack { ids })) {
            Ok(response) if response.status == status::NO_CONTENT => Ok(true),
            Ok(response) if response.status == status::GONE => Err(Error::Replaced),
            _ => Ok(false),
        }
    }

    /// Acknowledges what `sync_end` received and saved (`unacked`).
    fn acknowledge_inbox(&mut self) -> Result<()> {
        if !self.unacked.is_empty() {
            let ids = std::mem::take(&mut self.unacked);
            self.ack(ids)?;
        }
        Ok(())
    }

    /// Marks a session message as processed: its hash, whatever envelope it came in.
    fn remember(&mut self, message: &[u8]) {
        self.state.seen.push_back(sha256(message));
        if self.state.seen.len() > SEEN {
            self.state.seen.pop_front();
        }
    }

    /// Whether the hub names this device of `user` at this number now (a lookup); its roster is taken in when it
    /// does.
    fn hub_names(&mut self, user: &str, account_key: &Key, device_key: &Key, id: u8) -> Result<bool> {
        match self.lookup(&Handle::user(&user_bytes(user)?), 0, Vec::new()) {
            Ok((found, _)) if found.account_key == *account_key => {
                let named = found.devices.iter().any(|d| d.key == *device_key && d.id == id);
                if named {
                    self.take_devices(user, found.account_key, &found.devices)?;
                }
                Ok(named)
            }
            Ok(_) | Err(Error::NotOnYo) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// A sender not pinned here, whose message did not open: pinned, with its devices, only when the hub (one lookup)
    /// lists the payload's device at its number for that account; else nothing is kept.
    fn pin_if_listed(
        &mut self,
        user: &str,
        account_key: Key,
        username: Option<String>,
        device_key: Key,
        id: u8,
    ) -> Result<bool> {
        match self.lookup(&Handle::user(&user_bytes(user)?), 0, Vec::new()) {
            Ok((found, _))
                if found.account_key == account_key
                    && found.devices.iter().any(|d| d.key == device_key && d.id == id) =>
            {
                self.pin(user, account_key, username);
                self.take_devices(user, found.account_key, &found.devices)?;
                Ok(true)
            }
            Ok(_) | Err(Error::NotOnYo) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Whether a payload may make this device ask the hub for `user`'s devices now (once a minute a user, and within
    /// the day's lookups for recovery).
    fn may_ask_roster(&mut self, user: &str) -> bool {
        let now = (self.clock)();
        let last = self.state.roster_asked.get(user).copied().unwrap_or(0);
        if last != 0 && now < last + ROSTER_ASK_EVERY_MS {
            return false;
        }
        if !self.recovery_lookup() {
            return false;
        }
        self.state.roster_asked.insert(user.to_owned(), now);
        true
    }

    /// Takes one of the day's lookups for recovery, if one is left.
    fn recovery_lookup(&mut self) -> bool {
        let day = (self.clock)() / (24 * 3600 * 1000);
        if self.state.recovery_lookups.0 != day {
            self.state.recovery_lookups = (day, 0);
        }
        if self.state.recovery_lookups.1 >= RECOVERY_LOOKUPS_PER_DAY {
            return false;
        }
        self.state.recovery_lookups.1 += 1;
        true
    }

    /// Who sent a payload: this account (another of its devices), a contact pinned to its key, or whom the hub says
    /// the key is (someone writing first). `None`: a key that is no user's now (a replaced device, or a forger).
    /// Who a payload's account key is: a contact, this account, or (asked of the hub) someone writing first, whose
    /// pin (`Unpinned`) is kept only once the message opened: anyone can seal a payload naming any account.
    fn sender(&mut self, account_key: Key) -> Result<Option<(String, Option<Unpinned>)>> {
        if account_key == self.my_account_key() {
            return Ok(Some((self.state.user_id.clone(), None)));
        }
        if let Some((user, _)) = self.state.contacts.iter().find(|(_, c)| c.account_key == account_key && !c.gone) {
            return Ok(Some((user.clone(), None)));
        }
        // Someone writing first, or a contact this state forgot (a state rolled back). A forger's payloads cost it a
        // delivery each (the hub's limits, 6.6); the lookup is the hub's (counted there too).
        match self.lookup(&Handle::account_key(&account_key), 0, Vec::new()) {
            Ok((found, username)) if found.account_key == account_key => {
                let user = hex(&found.user_id);
                Ok(Some((user, Some(Unpinned { account_key, username }))))
            }
            Ok(_) | Err(Error::NotOnYo) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Processes one payload, out of its seal (spec 4.4): the copy for this device, decrypted in the session with its
    /// sender's device.
    fn receive(&mut self, payload: &[u8]) -> Result<()> {
        let raw = payload;
        let payload: Payload = decode(payload)?;
        let mine = self.state.device_key;
        // Made before the sender knew this device (it was added since): not for it.
        let Some(copy) = payload.copies.into_iter().find(|c| c.device_key == mine) else { return Ok(()) };
        if payload.device_key == mine || self.state.seen.contains(&sha256(copy.message.as_slice())) {
            return Ok(());
        }
        let Some((user, unpinned)) = self.sender(payload.account_key)? else {
            self.drop_one("a sender that is no user now".into());
            return Ok(());
        };
        let own = user == self.state.user_id;
        // Another device of the user under this number here: one of the two was replaced, and only the hub knows
        // which (a replaced device can still deliver: deliveries are unsigned). Its roster decides before anything
        // opens: the session of a gone device goes, and a message from one is not opened (opening would make its
        // session the current one at an address now another device's). Nothing the payload claims is taken in
        // before its message opens: anyone can seal one.
        let other = self.roster(&user).is_some_and(|(_, devices)| {
            devices.iter().any(|d| d.slot == payload.device_id && d.key != payload.device_key)
        });
        if other {
            // Asked at most once a minute a user: a forger claiming numbers costs a lookup a minute, no more.
            if !self.may_ask_roster(&user) {
                // Asked a moment ago, or the day's lookups used up: kept and received again later (a contact's new
                // phone writing just after a forgery is not lost), up to `DEFERRED`.
                if self.state.deferred.len() < DEFERRED {
                    self.state.deferred.push_back(raw.to_vec());
                    self.note("a device under another's number: later".into());
                } else {
                    self.drop_one("a device the roster was asked of a moment ago".into());
                }
                return Ok(());
            }
            let (found, _) = self.lookup(&Handle::user(&user_bytes(&user)?), 0, Vec::new())?;
            if found.account_key == payload.account_key {
                self.take_devices(&user, found.account_key, &found.devices)?;
            }
            let current = found.devices.iter().any(|d| d.key == payload.device_key && d.id == payload.device_id);
            if found.account_key != payload.account_key || !current {
                self.note("a message from a replaced device".into());
                return Ok(());
            }
        }
        let address = self.address(&user, payload.device_id)?;
        let bytes = copy.message.as_slice();
        let message = match copy.prekey {
            1 => PreKeySignalMessage::try_from(bytes).map(CiphertextMessage::PreKeySignalMessage),
            0 => SignalMessage::try_from(bytes).map(CiphertextMessage::SignalMessage),
            _ => return Err(Error::Protocol("message type")),
        }
        .map_err(signal::signal)?;
        self.records.trust(&address, signal::identity_of(&payload.account_key)?);
        let local = self.local_address()?;
        let records = &mut self.records;
        let decrypted = signal::now(message_decrypt(
            &message,
            &address,
            &local,
            &mut records.sessions,
            &mut records.identities,
            &mut records.prekeys,
            &records.signed,
            &mut records.kyber,
            &mut rand::rng(),
        ));
        let plaintext = match decrypted {
            Ok(plaintext) => plaintext,
            Err(error) => {
                // A session out of step (spec 4.6): a new one with that device, and a retry, so it sends again. Not for
                // a device known here under another number (made up): taken in is nothing either way. Also a
                // "duplicate" whose bytes were not seen (above): made with message keys used before, by a sender whose
                // state went back; it never opens, and the sender's next ones would not either until its chain passed.
                // Only a device of the user's roster, at its number (the hub asked at most once a minute a user when
                // this device does not know it): the retry list stays as long as a roster (16), and a forger naming
                // made-up devices adds nothing to it.
                // Someone not pinned here (a state that went back and forgot them, or a forger naming a real
                // account): pinned for a retry only when the hub, asked at most once a minute a user, names that
                // device; else nothing of the payload is kept.
                let rostered = !own
                    && match unpinned {
                        None => {
                            self.slot_of(&user, &payload.device_key) == Some(payload.device_id)
                                || (self.may_ask_roster(&user)
                                    && self.hub_names(
                                        &user,
                                        &payload.account_key,
                                        &payload.device_key,
                                        payload.device_id,
                                    )?)
                        }
                        Some(Unpinned { account_key, ref username }) => {
                            self.may_ask_roster(&user)
                                && self.pin_if_listed(
                                    &user,
                                    account_key,
                                    username.clone(),
                                    payload.device_key,
                                    payload.device_id,
                                )?
                        }
                    };
                if rostered {
                    if let Some(contact) = self.state.contacts.get_mut(&user) {
                        if !contact.retry.contains(&payload.device_key) && contact.retry.len() < MAX_DEVICES {
                            contact.retry.push(payload.device_key);
                        }
                    }
                }
                self.drop_one(format!("session: {error}"));
                return Ok(());
            }
        };
        // It opened: from the account (its identity key), at that number. Which device key is there is the hub's to
        // say: one this device does not know is taken in only once the hub names it (asked without a bound: only the
        // account can make a message that opens); else it is a replaced device's, and dropped (4.6).
        self.remember(copy.message.as_slice());
        if let Some(Unpinned { account_key, username }) = unpinned {
            self.pin(&user, account_key, username);
        }
        if self.slot_of(&user, &payload.device_key) != Some(payload.device_id)
            && !self.hub_names(&user, &payload.account_key, &payload.device_key, payload.device_id)?
        {
            self.drop_one("a message from a replaced device".into());
            return Ok(());
        }
        self.note_device(&user, payload.device_key, payload.device_id)?;
        self.note_updated(&user);
        let plain: Plain = decode(&plaintext)?;
        let message = Message::decode(plain.content.as_slice())?;
        if own {
            return self.received_from_own(plain.to, plain.content.as_slice(), message);
        }
        if plain.to.is_some() {
            return Err(Error::Protocol("a copy for another"));
        }
        self.received(&user, plain.content.as_slice(), message)
    }

    /// Whether this plaintext from `user` comes for the first time (the last `SEEN` kept), noting it.
    fn first_time(&mut self, user: &str, plaintext: &[u8]) -> bool {
        let id = sha256(&[user.as_bytes(), plaintext].concat());
        if self.state.received.contains(&id) {
            return false;
        }
        self.state.received.push_back(id);
        if self.state.received.len() > SEEN {
            self.state.received.pop_front();
        }
        true
    }

    /// A message from one of this account's other devices: what it sent to `to`.
    fn received_from_own(&mut self, to: Option<UserId>, plaintext: &[u8], message: Message) -> Result<()> {
        // A copy encrypted again (a session or roster change) is still one: shown once, as from a contact.
        let me = self.state.user_id.clone();
        if !self.first_time(&me, plaintext) {
            return Ok(());
        }
        // Groups: one copy for this account's other devices, naming no one (spec 4.7).
        if to.is_none() {
            let me = self.state.user_id.clone();
            match (message.group, message.content) {
                (_, Content::Group(info)) => self.group_update(&me, info, true),
                (Some(tag), content) if shown(&content) => {
                    let group = state::hex(&tag.id);
                    let content = self.forget_keys(content);
                    self.push_event(Event::GroupSentElsewhere { group, sent_at_ms: message.sent_at_ms, content });
                }
                _ => {}
            }
            return Ok(());
        }
        let Some(to) = to else { return Ok(()) };
        if !shown(&message.content) {
            return Ok(());
        }
        let user = hex(&to);
        if user == self.state.user_id {
            return Err(Error::Protocol("a copy to oneself"));
        }
        let contact = self.state.contacts.entry(user.clone()).or_insert_with(Contact::unknown);
        contact.chat = true;
        let content = self.forget_keys(message.content);
        self.push_event(Event::SentElsewhere { user, sent_at_ms: message.sent_at_ms, content });
        Ok(())
    }

    /// The keys of photos and videos are dropped as they arrive, for a client that never opens them.
    fn forget_keys(&self, content: Content) -> Content {
        match content {
            Content::Photo(media) if self.media_keys_forgotten => Content::Photo(media::without_key(media)),
            Content::Video { media, duration_ms } if self.media_keys_forgotten => {
                Content::Video { media: media::without_key(media), duration_ms }
            }
            content => content,
        }
    }

    /// A message from a contact.
    fn received(&mut self, user: &str, plaintext: &[u8], message: Message) -> Result<()> {
        // A blocked user's messages are opened (their sessions go on) and dropped unseen (4.8); what they change of a
        // group still counts, so the group stays one.
        if self.blocked(user) && !matches!(message.content, Content::Group(_) | Content::GroupAsk { .. }) {
            return Ok(());
        }
        // Sent again on a retry (4.6) and seen already: once is enough. A sender's messages differ in their time at
        // least, so only a copy is the same.
        if !self.first_time(user, plaintext) {
            return Ok(());
        }
        // Everything but a receipt, a retry and an ask is answered by a receipt: the ratchet moves on also for a stream
        // of changes or names one way (spec 4.5).
        if !matches!(message.content, Content::Receipt | Content::Retry | Content::GroupAsk { .. }) {
            if let Some(contact) = self.state.contacts.get_mut(user) {
                contact.receipt_due = true;
            }
        }
        match message.content {
            Content::Receipt => Ok(()),
            Content::Group(info) => {
                self.group_update(user, info, false);
                Ok(())
            }
            Content::GroupAsk { id } => {
                self.group_asked(user, &state::hex(&id));
                Ok(())
            }
            content if message.group.is_some() => {
                let tag = message.group.expect("a group");
                self.group_yo(user, tag, message.sent_at_ms, content);
                Ok(())
            }
            Content::Retry => {
                let now = (self.clock)();
                let contact = self.state.contacts.get_mut(user).ok_or(Error::Internal("contact"))?;
                if now >= contact.resent_at + RESEND_EVERY_MS {
                    contact.resent_at = now;
                    self.send_again(user);
                }
                Ok(())
            }
            Content::Profile { name } => {
                if let Some(contact) = self.state.contacts.get_mut(user) {
                    contact.name = Some(name.clone());
                }
                self.push_event(Event::Profile { user: user.to_owned(), name });
                Ok(())
            }
            content => {
                let contact = self.state.contacts.get_mut(user).ok_or(Error::Internal("contact"))?;
                contact.receipt_due = true;
                if !std::mem::replace(&mut contact.chat, true) {
                    contact.profile_due = self.state.name.is_some();
                    self.push_event(Event::NewChat { user: user.to_owned() });
                }
                let content = self.forget_keys(content);
                self.push_event(Event::Message { user: user.to_owned(), sent_at_ms: message.sent_at_ms, content });
                Ok(())
            }
        }
    }

    /// A contact asked again (spec 4.6): what this device keeps of what it sent them goes once more, in order.
    fn send_again(&mut self, user: &str) {
        let Some(sent) = self.state.contacts.get(user).map(|c| c.sent.clone()) else { return };
        for message in sent {
            // A group's message only while both are still in it (left, or the user removed: not again).
            let in_group = Message::decode(&message).ok().and_then(|m| m.group).is_none_or(|tag| {
                self.state.groups.get(&hex(&tag.id)).is_some_and(|g| !g.left && g.members.iter().any(|m| m == user))
            });
            if in_group {
                self.enqueue(user, encode(&Plain { to: None, content: message.into() }));
            }
        }
    }

    /// The receipts and retries due (spec 4.5, 4.6): a receipt to each contact that wrote since the last, so its
    /// ratchet moves on; a new session with each device whose message did not open, and a retry in it.
    fn answer_due(&mut self) -> Result<()> {
        let now = (self.clock)();
        let due: Vec<(String, bool, Vec<Key>)> = self
            .state
            .contacts
            .iter()
            .filter(|(_, c)| !c.blocked && !c.gone && !c.unconfirmed)
            .map(|(user, c)| {
                let retry = if now >= c.retry_at + RETRY_EVERY_MS { c.retry.clone() } else { Vec::new() };
                // A receipt needs a session; a retry makes one (a device this one lost its sessions with).
                (user.clone(), c.receipt_due && !c.devices.is_empty(), retry)
            })
            .filter(|(_, receipt, retry)| *receipt || !retry.is_empty())
            .collect();
        for (user, receipt, retry) in due {
            let content = if retry.is_empty() {
                Content::Receipt
            } else if !self.recovery_lookup() {
                // The day's lookups for recovery are used up (forgeries, perhaps): the retry waits for the next day.
                if !receipt {
                    continue;
                }
                Content::Receipt
            } else {
                match self.fetch_bundles(&user, &retry) {
                    Ok(()) => {}
                    Err(error) if retryable(&error) => return Err(error),
                    Err(error) => self.note(format!("retry: {error}")),
                }
                Content::Retry
            };
            let content_was_retry = content == Content::Retry;
            self.atomic(|c| {
                c.send_message(&user, content)?;
                let contact = c.state.contacts.get_mut(&user).ok_or(Error::Internal("contact"))?;
                contact.receipt_due = false;
                if content_was_retry {
                    contact.retry.clear();
                    contact.retry_at = now;
                }
                Ok(())
            })?;
            let _ = receipt;
        }
        Ok(())
    }

    // ---- prekeys ----

    /// Makes `fresh` one-time prekeys of each kind and, with `rotate`, a new signed prekey and last resort (spec 4.1).
    fn make_prekeys(&mut self, fresh: u32, rotate: bool) -> Result<api::Prekeys> {
        let now = (self.clock)();
        let mut next = || {
            self.state.prekeys.next_id += 1;
            self.state.prekeys.next_id
        };
        let ids: Vec<u32> = (0..fresh * 2 + 2).map(|_| next()).collect();
        let mut prekeys = api::Prekeys { signed: None, last_resort: None, one_time: Vec::new(), kyber: Vec::new() };
        if rotate {
            prekeys.signed = Some(self.records.new_signed(ids[0], now)?);
            prekeys.last_resort = Some(self.records.new_kyber(ids[1], true)?);
            self.state.prekeys.signed_at = now;
        }
        for pair in ids[2..].chunks(2) {
            prekeys.one_time.push(self.records.new_prekey(pair[0])?);
            prekeys.kyber.push(self.records.new_kyber(pair[1], false)?);
        }
        Ok(prekeys)
    }

    /// Keeps the hub's prekeys (spec 4.1): one-time ones refilled below `PREKEY_REFILL_BELOW`, the signed prekey and
    /// the last resort replaced every week. Made and saved before they leave, sent again as they are until the hub
    /// answers: a lost answer must not cost the private keys of what it took.
    fn refill(&mut self) -> Result<()> {
        let (body, fresh) = match self.state.prekeys.upload.clone() {
            Some(upload) => upload,
            None => {
                let left = self.state.prekeys.left;
                let rotate = (self.clock)().saturating_sub(self.state.prekeys.signed_at) >= ROTATE_EVERY_MS;
                if left >= PREKEY_REFILL_BELOW && !rotate {
                    // Nothing to upload; retired keys still go when their month is over (spec 4.1).
                    if self.forget_retired() {
                        self.persist()?;
                    }
                    return Ok(());
                }
                let fresh = if left < PREKEY_REFILL_BELOW { PREKEY_TARGET - left } else { 0 };
                // Saved in the same transaction: a save that fails leaves no upload to send unsaved.
                self.atomic(|c| {
                    let prekeys = c.make_prekeys(fresh, rotate)?;
                    let upload = (encode(&prekeys), fresh);
                    c.state.prekeys.upload = Some(upload.clone());
                    c.persist()?;
                    Ok(upload)
                })?
            }
        };
        let response = self.signed("PUT", api::PREKEYS, &body)?;
        // `409` to the same upload again: the hub took it before and its answer was lost. Either way it is done.
        if response.status != status::NO_CONTENT && response.status != status::CONFLICT {
            if !retryable(&refused(&response)) {
                // Made anew next time, with a new signed prekey and last resort too: a rotation it carried is still due.
                self.state.prekeys.upload = None;
                self.state.prekeys.signed_at = 0;
            }
            return Err(refused(&response));
        }
        self.state.prekeys.upload = None;
        self.state.prekeys.left = self.state.prekeys.left.saturating_add(fresh);
        self.prekeys_known_ms = self.last_request_ms;
        // The hub hands out the newest signed prekey and last resort from now on: the ones before are retired.
        let now = wall_ms();
        for kind in ["sp", "kl"] {
            let ids = self.records.ids(kind);
            for id in ids.iter().take(ids.len().saturating_sub(1)) {
                if !self.state.prekeys.retired.iter().any(|(k, i, _)| k == kind && i == id) {
                    self.state.prekeys.retired.push((kind.to_owned(), *id, now));
                }
            }
        }
        let _ = self.forget_retired();
        self.records.keep_newest_one_time(ONE_TIME_KEPT);
        Ok(())
    }

    /// The private keys of signed prekeys and last resorts retired a month ago go (`RETIRED_KEPT_MS`).
    /// Whether any went.
    fn forget_retired(&mut self) -> bool {
        let now = wall_ms();
        let (old, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.state.prekeys.retired)
            .into_iter()
            .partition(|(_, _, at)| now.saturating_sub(*at) > RETIRED_KEPT_MS);
        let any = !old.is_empty();
        for (kind, id, _) in old {
            self.records.forget(&kind, id);
        }
        self.state.prekeys.retired = kept;
        any
    }

    /// Keeps the stash of delivery tokens (spec 6.10): below `TOKEN_REFILL_BELOW`, or after the hub refused one,
    /// the hub's key is read from its directory (a new key: the stash is dropped) and a batch issued, up to
    /// `TOKEN_TARGET`. Quietly: whatever fails (its daily limit, no network) leaves deliveries as they were, counted by
    /// address, and the next try is an hour later.
    fn refill_tokens(&mut self) {
        let now = (self.clock)();
        let low = self.state.tokens.len() < TOKEN_REFILL_BELOW;
        if !(low || self.state.tokens_check) || now < self.state.tokens_after {
            return;
        }
        if self.issue_tokens(now).is_none() {
            self.state.tokens_after = now + TOKENS_RETRY_MS;
        }
        // Not saved: tokens lost to a crash are only tokens.
        let _ = self.persist();
    }

    fn issue_tokens(&mut self, now: u64) -> Option<()> {
        let directory = self.unsigned("GET", token::DIRECTORY, &[], &[]).ok().filter(|r| r.status == status::OK)?;
        let key = tokens::directory_key(&directory.body, now / 1000)?;
        if self.state.token_key.as_deref() != Some(hex(&key).as_str()) {
            self.state.tokens.clear();
            self.state.token_key = Some(hex(&key));
        }
        self.state.tokens_check = false;
        if self.state.tokens.len() >= TOKEN_REFILL_BELOW {
            return Some(());
        }
        let wanted = (TOKEN_TARGET - self.state.tokens.len()).min(MAX_TOKENS_PER_REQUEST);
        let batch = tokens::request(&key, wanted)?;
        let response =
            self.signed("POST", api::TOKENS, &encode(&batch.request)).ok().filter(|r| r.status == status::OK)?;
        let issued = tokens::finish(&batch, &decode(&response.body).ok()?)?;
        self.state.tokens.extend(issued.iter().map(tokens::kept));
        Some(())
    }

    /// Delivery tokens in the stash (spec 6.10).
    pub fn tokens_left(&self) -> usize {
        self.state.tokens.len()
    }

    // ---- what the app calls ----

    /// Sends a Yo to "chris", or a user id from an event.
    pub fn send_yo(&mut self, to: &str) -> Result<Vec<Event>> {
        self.send(to, Content::Yo)
    }

    /// Sends a message: starts writing to the user if this device has not yet (spec 4.2).
    pub fn send(&mut self, to: &str, content: Content) -> Result<Vec<Event>> {
        let result = self.send_inner(to, content);
        self.finish_sent(result)
    }

    /// `finish` for a send: the message was queued and saved before it went (spec 4.7), so a later save that fails
    /// does not make it unsent. Its events wait for the next call; `Save` here would have the app send it again, a
    /// second message.
    pub(crate) fn finish_sent(&mut self, result: Result<()>) -> Result<Vec<Event>> {
        let queued = result.is_ok();
        match self.finish(result) {
            Err(Error::Save(_)) if queued => Ok(Vec::new()),
            other => other,
        }
    }

    fn send_inner(&mut self, to: &str, content: Content) -> Result<()> {
        let handle = Handle::parse(to)?;
        if self.is_me(&handle) {
            return Err(Error::Protocol("sending to oneself"));
        }
        if !shown(&content) {
            return Err(Error::Protocol("not content to send"));
        }
        self.catch_up()?;
        let user = self.send_target(&handle)?;
        self.check_sendable(user.as_deref())?;
        let peer = match user.filter(|u| self.has_chat(u)) {
            Some(user) => user,
            None => self.start_chat(&handle)?,
        };
        // A name not yet told goes first.
        self.send_profiles();
        // Saved with the message in it, or not made at all (spec 4.7).
        // Its envelope made in the same step when the sessions are there: one save before it leaves, not two.
        self.atomic(|c| {
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

    /// The user a send to `handle` goes to: a user id as it is; a username as the hub says now (who it is only,
    /// without spending a prekey), never as this device knew it, since a name may have changed hands since (3.3).
    /// So someone who wrote first, known by user id only, is found by name too. `None`: no contacts yet, the first
    /// lookup asks.
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

    /// `send` in three steps, so that a caller sharing the client behind a lock need not hold it while the message
    /// travels (sends to several friends and the inbox's wait then share the network): `send_begin` (locked) encrypts
    /// the message and takes what of the outbox can go, `Deliveries::send` (unlocked) posts it, `send_end` (locked)
    /// takes the answers. Only to a contact this device writes to already, with a session with each of its devices,
    /// neither blocked nor waiting for confirmation, and while the inbox is up to date: a long wait for it is under way
    /// (`sync_begin`), which brings what arrives at once, or it was emptied a moment ago. What the hub refuses for
    /// other devices is taken in by `send_end`. `None`, with nothing changed: `send` it instead.
    pub fn send_begin(&mut self, to: &str, content: Content) -> Option<Deliveries> {
        let handle = Handle::parse(to).ok()?;
        // By user id only: a name is asked of the hub (`send_target`).
        if handle.kind != KIND_USER
            || self.is_me(&handle)
            || !shown(&content)
            || (self.clock)() > self.inbox_fresh_until
        {
            return None;
        }
        let user = self.known_user(&handle)?;
        let contact = self.state.contacts.get(&user)?;
        if contact.blocked || contact.unconfirmed || contact.gone || contact.devices.is_empty() {
            return None;
        }
        if !self.without_session(&user).is_empty() {
            return None;
        }
        // A name not yet told goes first, as in `send`. Saved with the message in it, or not made at all: `send` then
        // fails on the save the same way.
        self.atomic(|c| {
            c.send_profiles();
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
        Deliveries { transport: self.transport.clone(), items: Vec::new(), to: None, token_refused: Default::default() }
    }

    /// The outbox, without the client, like `send_begin` without a message: for sending again what the hub did not take
    /// yet (its limit per sender, or no network). Ended with `send_end`.
    pub fn deliver_begin(&mut self) -> Result<Deliveries> {
        self.prepare()?;
        self.deliveries()
    }

    /// Envelopes not yet taken by the hub (`pending`).
    pub fn waiting(&self) -> usize {
        self.pending()
    }

    /// Takes the answers to `send_begin`'s deliveries (or to the ones before): `More` when the hub took them all and
    /// more of the outbox can go now (a message queued meanwhile behind them), to send and end the same way; else
    /// what `send` returns. What the hub did not take waits in the outbox for the next call that delivers.
    pub fn send_end(&mut self, deliveries: &Deliveries, answers: Vec<Option<Answer>>) -> Sent {
        let all_taken = answers
            .iter()
            .all(|answer| matches!(answer, Some(Ok(r)) if r.status == status::ACCEPTED || gone_for_good(r.status)));
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
        // As `send`: offline, the message waits in the outbox; to a user the hub no longer knows, it went nowhere. It
        // was saved when `send_begin` queued it: a save failing after is no reason to call it unsent (the app would
        // send a second one).
        let result = match result {
            Ok(()) | Err(Error::Network(_) | Error::Save(_)) => self.check_sendable(deliveries.to.as_deref()),
            Err(error) => Err(error),
        };
        Sent::Done(self.finish_sent(result))
    }

    /// What another process of this device received and saved for the app (the notification service, as its push
    /// came): handed out now, from the state, without asking the hub. Nothing waiting: nothing changes.
    pub fn saved_for_app(&mut self) -> Result<Vec<Event>> {
        if self.state.events.is_empty() {
            return Ok(Vec::new());
        }
        self.finish(Ok(()))
    }

    /// Receives: waits up to `wait_s` seconds for mail, processes all of it, delivers what is waiting. Returns as soon
    /// as what arrived is saved; the hub's ack and the work after it wait for `after_sync` or the next call.
    pub fn sync(&mut self, wait_s: u64) -> Result<Vec<Event>> {
        let wait = self.sync_begin(wait_s)?;
        let response = wait.send();
        self.sync_end(&wait, response)
    }

    /// `sync` in three steps, so that a caller sharing the client behind a lock need not hold it through the long
    /// poll: `sync_begin` (locked) delivers what is waiting and signs the inbox request, `InboxWait::send` (unlocked)
    /// waits for the hub, `sync_end` (locked) processes the answer. Other calls may run in between; the answer is
    /// processed against the state as it is then. The signature's time is checked when the hub gets the request,
    /// before the wait, so signing early is fine.
    pub fn sync_begin(&mut self, wait_s: u64) -> Result<InboxWait> {
        // What the last sync left (`after_sync`), then the outbox; a guest leaves the outbox to the app.
        let before = self.after_sync_inner().and_then(|()| match self.guest_events {
            Some(_) => Ok(()),
            None => self.deliver(),
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
        self.persist()?;
        // What arrives during the wait comes at once: until it ends, a send need not receive first.
        let until = (self.clock)() + wait_s * 1000 + INBOX_FRESH_MS;
        self.inbox_fresh_until = self.inbox_fresh_until.max(until);
        Ok(InboxWait { transport: self.transport.clone(), path, headers, time })
    }

    /// Processes what `InboxWait::send` brought and fetches the rest; saves and hands out the events. Acknowledging
    /// them at the hub, receipts, prekeys, delivering and writing the storage come after, off the way of the events:
    /// `after_sync`, else the next `sync_begin`. The hub is acknowledged only what was saved first.
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
        // The wait is over: the inbox is up to date only if its answer was taken in.
        self.inbox_fresh_until = if received.is_ok() { (self.clock)() + INBOX_FRESH_MS } else { 0 };
        // A guest does none of the work after (the app does): it only acknowledges.
        if received.is_ok() && self.guest_events.is_none() {
            self.after_due = true;
        }
        let events = self.finish_receiving(received);
        // Handed out only once saved: from now on the hub may forget them.
        if events.is_ok() {
            self.unacked.extend(saved_later);
        }
        events
    }

    /// The rest of a `sync`, once its events are handed over: acknowledges at the hub what it received (saved
    /// before), and for the app (not a guest) sends the receipts and retries due, refills prekeys and delivery tokens,
    /// delivers the outbox, writes the storage. Without it the next `sync_begin` does it first.
    pub fn after_sync(&mut self) -> Result<()> {
        let result = self.after_sync_inner();
        let saved = self.persist();
        result.and(saved)
    }

    fn after_sync_inner(&mut self) -> Result<()> {
        self.acknowledge_inbox()?;
        if !self.after_due {
            return Ok(());
        }
        // Due until all of it is done: a step that fails leaves it for the next call, not only the next receive.
        self.answer_due()?;
        self.refill()?;
        self.deliver()?;
        self.sync_storage_quietly()?;
        self.refill_tokens();
        self.after_due = false;
        Ok(())
    }

    /// Writes a change of the storage (friends, blocks, the icon) to the hub now, rather than with the next
    /// `sync_begin` or `maintenance`: for the app right after the change, beside the inbox's long wait. Quietly, as
    /// there ("not now" leaves it for later).
    pub fn write_storage(&mut self) -> Result<()> {
        let result = self.sync_storage_quietly();
        let saved = self.persist();
        result.and(saved)
    }

    /// What the app runs when it comes to the foreground: receive, answer, refill prekeys, deliver.
    pub fn maintenance(&mut self) -> Result<Vec<Event>> {
        let result = self.maintenance_inner();
        self.finish_receiving(result)
    }

    fn maintenance_inner(&mut self) -> Result<()> {
        // All of the work after a sync is done here.
        self.after_due = false;
        self.deliver()?;
        self.receive_all(0)?;
        self.answer_due()?;
        self.refill()?;
        self.refill_tokens();
        self.send_profiles();
        self.deliver()?;
        self.sync_storage_quietly()
    }

    /// Sets the name contacts see, and tells every contact this device writes to.
    pub fn set_name(&mut self, name: &str) -> Result<Vec<Event>> {
        let result = (|| {
            Message { sent_at_ms: 0, group: None, content: Content::Profile { name: name.to_owned() } }.encode()?;
            self.state.name = Some(name.to_owned());
            for contact in self.state.contacts.values_mut().filter(|c| c.chat) {
                contact.profile_due = true;
            }
            self.send_profiles();
            self.deliver()
        })();
        self.finish(result)
    }

    /// Tells the name to every contact it is due to that can take a message now: not to a blocked one (4.8), nor one
    /// whose new key waits for confirmation (3.4), nor one gone. The others keep it due, for the next send or
    /// `maintenance`.
    fn send_profiles(&mut self) {
        let Some(name) = self.state.name.clone() else { return };
        let due: Vec<String> = self
            .state
            .contacts
            .iter()
            .filter(|(_, c)| c.profile_due && !c.blocked && !c.unconfirmed && !c.gone && !c.devices.is_empty())
            .map(|(user, _)| user.clone())
            .collect();
        for user in due {
            let sent = self.atomic(|c| {
                c.send_message(&user, Content::Profile { name: name.clone() })?;
                c.state.contacts.get_mut(&user).ok_or(Error::Internal("contact"))?.profile_due = false;
                Ok(())
            });
            if let Err(error) = sent {
                self.note(format!("name: {error}"));
            }
        }
    }

    pub fn contacts(&self) -> Vec<ContactInfo> {
        self.state
            .contacts
            .iter()
            .map(|(user, c)| ContactInfo {
                user: user.clone(),
                handles: c.handles.clone(),
                name: c.name.clone(),
                verified: c.verified,
                unconfirmed: c.unconfirmed,
                blocked: c.blocked,
                gone: c.gone,
                outdated: c.outdated_at > 0,
            })
            .collect()
    }

    /// The sessions with a contact's devices (user id), with what each is like.
    pub fn sessions_with(&self, user: &str) -> Vec<SessionInfo> {
        let Some(contact) = self.state.contacts.get(user) else { return Vec::new() };
        contact
            .devices
            .iter()
            .map(|device| SessionInfo {
                device: hex(&device.key),
                usable: self.address(user, device.slot).is_ok_and(|a| self.records.usable(&a)),
                retrying: contact.retry.contains(&device.key),
            })
            .collect()
    }

    /// The account key a contact is pinned to (hex): what the safety number is made of (3.4).
    pub fn account_key_of(&self, user: &str) -> Option<String> {
        self.state.contacts.get(user).map(|c| hex(&c.account_key))
    }

    /// The contact a handle names, as this device knows it.
    fn contact_user(&self, who: &str) -> Result<String> {
        self.known_user(&Handle::parse(who)?).ok_or(Error::NotOnYo)
    }

    /// Who a name is, asked of the hub without the client, so a friend added by a typed name shows as its owner writes
    /// it before the first Yo: `whois_begin` (locked) signs the lookup (who it is only, no prekey), `Whois::send`
    /// (unlocked) asks. Nothing is kept: the name is not a contact until a message.
    pub fn whois_begin(&mut self, who: &str) -> Result<Whois> {
        let handle = Handle::parse(who)?;
        if self.is_me(&handle) {
            return Err(Error::Protocol("oneself"));
        }
        let body = encode(&api::Lookup { handle, flags: api::LOOKUP_USERNAME, except: Vec::new() });
        let (headers, _) = self.sign("POST", api::LOOKUP, &body);
        // The time signed saved before the request leaves: another process of this device signs later.
        self.persist()?;
        Ok(Whois { transport: self.transport.clone(), headers, body, me: self.state.user_id.clone() })
    }

    /// Takes in what `Whois::send` said a name is before a send to it by user id (the app's way to send to a name
    /// without holding the client, spec 4.2): a contact is known by the name from now on, and no other contact any
    /// more.
    pub fn whois_end(&mut self, name: &str, user: &str, username: Option<String>) -> Result<()> {
        let Ok(handle) = Handle::parse(name) else { return Ok(()) };
        if !self.state.contacts.contains_key(user) {
            return Ok(());
        }
        self.take_handle(user, username.unwrap_or_else(|| handle.display()));
        self.persist()
    }

    /// Whether `to` names a user by its id (hex), not by a name.
    pub fn is_user_id(to: &str) -> bool {
        Handle::parse(to).is_ok_and(|handle| handle.kind == KIND_USER)
    }

    /// The user a handle names: known to this device, else asked of the hub (who it is only, no prekey); with the
    /// handle to remember for it then, the username as its owner writes it if the hub tells.
    fn user_named(&mut self, handle: &Handle) -> Result<(String, Option<String>)> {
        if self.is_me(handle) {
            return Err(Error::Protocol("oneself"));
        }
        let (user, shown) = match self.known_user(handle) {
            Some(user) => (user, None),
            None => {
                let (found, username) = self.lookup(handle, 0, Vec::new())?;
                let shown = username.or_else(|| (handle.kind != KIND_USER).then(|| handle.display()));
                (hex(&found.user_id), shown)
            }
        };
        if user == self.state.user_id {
            return Err(Error::Protocol("oneself"));
        }
        Ok((user, shown))
    }

    /// The safety number with a contact (spec 3.4).
    pub fn safety_number(&self, who: &str) -> Result<String> {
        let user = self.contact_user(who)?;
        let contact = self.state.contacts.get(&user).ok_or(Error::NotOnYo)?;
        let mine = unhex(&self.state.user_id).unwrap_or_default();
        let theirs = unhex(&user).unwrap_or_default();
        Ok(identity::safety_number((&mine, &self.my_account_key()), (&theirs, &contact.account_key)))
    }

    /// Marks a contact verified (or not) after comparing safety numbers. Any contact can be verified (spec 3.4).
    pub fn set_verified(&mut self, who: &str, verified: bool) -> Result<()> {
        let user = self.contact_user(who)?;
        let contact = self.state.contacts.get_mut(&user).ok_or(Error::NotOnYo)?;
        contact.verified = verified;
        if verified {
            contact.unconfirmed = false;
        }
        self.persist()
    }

    /// Accepts a verified contact's new account key: sending to it goes on (unverified until compared again).
    pub fn confirm_identity(&mut self, who: &str) -> Result<()> {
        let user = self.contact_user(who)?;
        self.state.contacts.get_mut(&user).ok_or(Error::NotOnYo)?.unconfirmed = false;
        self.persist()
    }

    /// Blocks a user (by user id or any handle; one this device does not know is asked of the hub), or unblocks:
    /// nothing from them is shown, nothing is sent to them (spec 4.8).
    pub fn set_blocked(&mut self, who: &str, blocked: bool) -> Result<()> {
        let (user, shown) = self.user_named(&Handle::parse(who)?)?;
        self.apply_block(&user, blocked);
        if let Some(shown) = shown {
            note_handle(self.state.contacts.get_mut(&user).expect("blocked"), shown);
        }
        // Kept in the storage too, so a new phone has it (spec 6.9).
        self.storage_block(&user, blocked);
        self.persist()
    }

    /// Blocks or unblocks a user on this device. What came from them before and waits to be handed out (after a
    /// restart of the app) is not shown either.
    pub(crate) fn apply_block(&mut self, user: &str, blocked: bool) {
        self.state.contacts.entry(user.to_owned()).or_insert_with(Contact::unknown).blocked = blocked;
        if blocked {
            // From the events not handed out yet and from those handed out and not acknowledged, which a restart
            // hands out again: nothing of theirs comes after the block.
            let theirs = |e: &StoredEvent| {
                matches!(e, StoredEvent::Message { user: from, own: false, .. }
                    | StoredEvent::GroupMessage { user: from, own: false, .. } if from == user)
            };
            self.state.events.retain(|e| !theirs(e));
            self.state.handed.retain(|e| !theirs(e));
        }
    }

    /// Reports a user to the operator (by user id or any handle): `reason` is `api::REPORT_SPAM`, `REPORT_ABUSE` or
    /// `REPORT_OTHER` (spec 6.3). Nothing of what they sent goes with it.
    pub fn report(&mut self, who: &str, reason: u8) -> Result<()> {
        let (user, _) = self.user_named(&Handle::parse(who)?)?;
        let body = encode(&api::Report { user_id: user_bytes(&user)?, reason });
        let response = self.signed("POST", api::REPORT, &body)?;
        match response.status {
            status::NO_CONTENT => Ok(()),
            status::NOT_FOUND => Err(Error::NotOnYo),
            _ => Err(refused(&response)),
        }
    }

    /// Signs feedback on the app for the operator (spec 6.3), to send without the client (`FeedbackRequest::send`).
    /// The text goes as written, readable by the hub: not end-to-end encrypted. Empty text, or text or version too
    /// long, is refused here.
    pub fn feedback_begin(&mut self, text: &str, app_version: &str) -> Result<FeedbackRequest> {
        let feedback = api::Feedback { text: text.as_bytes().into(), app_version: app_version.as_bytes().into() };
        feedback.check().map_err(|_| Error::Protocol("feedback empty or too long"))?;
        let body = encode(&feedback);
        let (headers, _) = self.sign("POST", api::FEEDBACK, &body);
        self.persist()?;
        Ok(FeedbackRequest { transport: self.transport.clone(), headers, body })
    }

    /// `feedback_begin` and its send, holding the client throughout.
    pub fn send_feedback(&mut self, text: &str, app_version: &str) -> Result<()> {
        self.feedback_begin(text, app_version)?.send()
    }

    /// Deletes the account at the hub with everything it holds about it (spec 6.3): the name is free again, the other
    /// devices are out. This client is of no more use: the app forgets its state. `Replaced` when this device was out
    /// already (replaced, or its account deleted).
    pub fn delete_account(&mut self) -> Result<()> {
        let response = self.signed("DELETE", api::ACCOUNT, &[])?;
        match response.status {
            status::NO_CONTENT => Ok(()),
            _ => Err(refused(&response)),
        }
    }

    /// Claims a username, kept as written ("Chris": found as "chris" and "CHRIS" too), or gives it up with `None`.
    pub fn set_username(&mut self, name: Option<&str>) -> Result<()> {
        let canonical = match name {
            Some(name) => {
                let handle = Handle::parse(name)?;
                if handle.kind != KIND_USERNAME {
                    return Err(Error::Protocol("not a username"));
                }
                Some(String::from_utf8_lossy(handle.value.as_slice()).into_owned())
            }
            None => None,
        };
        let body = encode(&api::SetUsername { username: canonical.as_ref().map(|n| n.as_bytes().to_vec().into()) });
        let response = self.signed("PUT", api::USERNAME, &body)?;
        if response.status != status::NO_CONTENT {
            return Err(refused(&response));
        }
        self.state.username = canonical;
        self.persist()
    }
}
