//! The Yo client core (spec/v1.md): one implementation of YoIP for iOS and Android.
//!
//! The platform supplies HTTP (`Transport`) and a place to keep one sealed blob (`Store`); everything else, from the
//! MLS groups to the order of commits, is here. Every operation either completes or leaves the state as it was,
//! except what the hub has already accepted, which is kept.

pub mod backup;
pub mod error;
pub mod identity;
pub mod media;
pub mod net;
pub mod profile;
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
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::storage::StorageProvider as _;
use profile::Provider;
use state::{
    hex, unhex, Chat, Contact, Creating, Inflight, Outgoing, OwnKeyPackage, Registering, State, StoredContent,
    StoredEvent,
};
use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use yo_wire::api::{self, status};
use yo_wire::content::Message;
use yo_wire::handle::{Handle, KIND_USER, KIND_USERNAME};
use yo_wire::limits::{
    KEY_PACKAGE_REFILL_BELOW, KEY_PACKAGE_TARGET, MAX_INBOX_FETCH, MAX_TOKENS_PER_REQUEST, TOKEN_REFILL_BELOW,
    TOKEN_TARGET,
};
use yo_wire::request::{HEADER_DEVICE, HEADER_SIGNATURE, HEADER_TIME};
use yo_wire::token;
use yo_wire::{decode, encode, envelope, sha256, DeviceId, UserId, EPOCH_TOKEN_LABEL, TOKEN_LEN};

/// An update is due after this long (spec 4.4)...
pub const UPDATE_AFTER_MS: u64 = 7 * 24 * 3600 * 1000;
/// ... or after this many messages sent.
pub const UPDATE_AFTER_MESSAGES: u32 = 100;
/// A send goes without receiving first (`send_begin`) only while the inbox is up to date: waited on, or processed
/// this recently.
const INBOX_FRESH_MS: u64 = 5_000;
/// The last resort KeyPackage is replaced this often.
const LAST_RESORT_EVERY_MS: u64 = 30 * 24 * 3600 * 1000;
/// A replaced last resort's private keys are kept this long: a Welcome made with it may still wait at the hub.
const RETIRED_KEY_PACKAGE_KEPT_MS: u64 = 31 * 24 * 3600 * 1000;
/// Envelope hashes remembered against redelivery.
const SEEN: usize = 2000;
/// Messages sent in a chat kept to send again if the peer restarts it (spec 4.9).
const KEPT_FOR_RESTART: usize = 64;
/// How often a registration whose answer did not come is tried again, with the same account key.
const REGISTER_TRIES: usize = 2;
/// Commits fetched at most at once: more than a month of a busy chat (older ones are gone from the hub anyway).
const MAX_COMMITS_FETCHED: usize = 1000;
/// How often a send repairs groups and tries again before it leaves the rest to the next call.
const REPAIR_ROUNDS: usize = 3;
/// After an issuance of tokens failed, the next is asked for this much later (spec 6.10).
const TOKENS_RETRY_MS: u64 = 3600 * 1000;

/// What became of a commit.
enum Committed {
    /// There was none to send.
    Nothing,
    Accepted,
    /// Another commit took the epoch, which the hub has now.
    Lost(u64),
    /// The hub does not order this group's commits, or not with this device's token: the chat is lost to this
    /// device and restarted (4.9).
    Refused,
    /// No answer: it may have been accepted.
    Unknown(String),
}

/// Errors that say "not now", not "never": the envelope or operation is tried again later. A `426` (this app too old,
/// spec 6.3) too: nothing goes until the app is updated, and the update sends what waits.
fn retryable(error: &Error) -> bool {
    match error {
        Error::Network(_) => true,
        Error::Refused(status) => {
            *status == status::RATE_LIMITED || *status == status::UPGRADE_REQUIRED || *status >= 500
        }
        _ => false,
    }
}

/// A group further behind the hub than one call fetches: "not now", what was fetched is kept.
fn more_commits() -> Error {
    Error::Network("more commits to fetch".into())
}

/// What happened, for the app to show. `user` is the contact's user id (hex): what `contacts()` describes and what
/// `send` takes to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Someone started a chat with this device.
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
}

/// One chat with a contact as MLS has it, for the app to show (spec 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatInfo {
    /// The group's id (hex).
    pub group: String,
    pub ciphersuite: String,
    /// The group's epoch now, and the one this device joined at.
    pub epoch: u64,
    pub joined_epoch: u64,
    /// Devices in the group (both accounts).
    pub members: u32,
    /// When this device last updated its keys here (ms; 0: not yet), post-compromise security (4.4).
    pub last_update_ms: u64,
    /// The chat takes updates (4.4): not while it is lost and waits for its restart.
    pub updates: bool,
    /// Taken over by a restart (4.9), or lost and waiting for one.
    pub restarted: bool,
    pub lost: bool,
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
    /// The user id and the username as its owner writes it (none from a hub before spellings). `NotOnYo`: nobody by
    /// that name. Blocks for the round trip.
    pub fn send(&self) -> Result<(String, Option<String>)> {
        let response =
            self.transport.request("POST", api::LOOKUP, &self.headers, &self.body).map_err(Error::Network)?;
        match response.status {
            status::OK => {}
            status::NOT_FOUND => return Err(Error::NotOnYo),
            _ => return Err(refused(&response)),
        }
        let (found, username) = match decode::<api::LookupNamed>(&response.body) {
            Ok(named) => (named.found, named.username.and_then(|n| String::from_utf8(n.into()).ok())),
            Err(_) => (decode::<api::LookupResult>(&response.body)?, None),
        };
        let user = hex(&found.user_id);
        if user == self.me {
            return Err(Error::Protocol("oneself"));
        }
        Ok((user, username.filter(|n| yo_wire::handle::check_username_form(n).is_ok())))
    }
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

/// A delivery the hub refused for good (4.6): the envelope is dropped, the user's next ones go on.
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
    group: String,
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
    provider: Provider,
    state: State,
    clock: Clock,
    last_request_ms: u64,
    /// The signing time of the request whose answer set `key_packages_left` last: an older answer, waited for
    /// outside the app's lock while another call refilled, does not overwrite it.
    key_packages_known_ms: u64,
    dropped: u64,
    drop_reasons: Vec<String>,
    /// Users whose envelopes are on their way to the hub (`deliveries`): nothing else to them goes meanwhile.
    delivering: BTreeSet<String>,
    /// Until when (clock) the inbox counts as up to date: the end of a long wait under way, or when it was emptied.
    inbox_fresh_until: u64,
    /// Keys of photos and videos received are forgotten before anything is kept (a bot that never opens them).
    media_keys_forgotten: bool,
    /// A guest (`keep_events`): the events it receives stay in the state for the app; how many of them this one has
    /// returned already.
    guest_events: Option<usize>,
    /// The app acknowledges each batch it was handed itself (`acknowledge_only`): a call no longer acknowledges what
    /// the one before handed out, which another call running beside it may not have consumed yet.
    explicit_ack: bool,
    /// Commits are being fetched (`fetch_commits`): what they bring is not fetched for again.
    fetching: bool,
    /// Commits fetched at most in one call (`MAX_COMMITS_FETCHED`; fewer in tests).
    fetch_at_most: usize,
    /// Inbox ids received and saved, not yet acknowledged at the hub: `sync_end` hands its events out first, and
    /// `after_sync` (or the next call that receives) acknowledges them. Lost with the process, they come again and are
    /// skipped as seen.
    unacked: Vec<u64>,
    /// `sync_end` received: the work after it (KeyPackages, the outbox, the storage, delivery tokens) waits for
    /// `after_sync`, or the next `sync_begin`.
    after_due: bool,
}

/// A group member as YoLS sees it.
struct Member {
    index: LeafNodeIndex,
    account_key: Key,
    device_key: Key,
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

fn mls<E: std::fmt::Debug>(error: E) -> Error {
    Error::Mls(format!("{error:?}"))
}

fn group_key(group: &GroupId) -> String {
    hex(group.as_slice())
}

fn group_of(key: &str) -> Option<GroupId> {
    unhex(key).map(|id| GroupId::from_slice(&id))
}

fn user_bytes(user: &str) -> Result<UserId> {
    unhex(user).and_then(|u| u.try_into().ok()).ok_or(Error::Protocol("user id"))
}

/// Whether two handles as shown name the same: a username by its skeleton (spec 3.3), so "yogpt" is "YoGPT".
fn same_handle(a: &str, b: &str) -> bool {
    yo_wire::handle::same_name(a, b)
}

/// Remembers how a contact is known. A username in another spelling takes the place of the one before: the newest
/// is the hub's word for how its owner writes it (or, from a hub that does not tell, what was typed).
fn note_handle(contact: &mut Contact, shown: String) {
    match contact.handles.iter().position(|h| same_handle(h, &shown)) {
        Some(index) => contact.handles[index] = shown,
        None => contact.handles.push(shown),
    }
}

/// An event as the state keeps it, and back: the app-facing types carry no serde.
fn stored(event: Event) -> StoredEvent {
    let content = |content: Content| match content {
        Content::Yo => StoredContent::Yo,
        Content::Photo(media) => StoredContent::Photo(media.into()),
        Content::Video { media, duration_ms } => StoredContent::Video { media: media.into(), duration_ms },
        Content::Text(text) => StoredContent::Text(text),
        Content::Link(url) => StoredContent::Link(url),
        Content::Location { lat_e7, lon_e7 } => StoredContent::Location { lat_e7, lon_e7 },
        Content::Profile { name } => StoredContent::Profile(name),
        Content::Unknown(kind) => StoredContent::Unknown(kind),
        Content::Restart { .. } => StoredContent::Unknown(yo_wire::content::TYPE_RESTART),
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
    }
}

fn refused(response: &Response) -> Error {
    match response.status {
        status::GONE => Error::Replaced,
        other => Error::Refused(other),
    }
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
                match self.lookup(&mine, false, Vec::new()) {
                    Ok(found) if found.device_keys.iter().any(|k| k.as_slice() == self.own_device_key()) => {
                        self.state.user_id = hex(&found.user_id);
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
                if let Ok((_, spelled)) = self.lookup_named(&me, false, Vec::new()) {
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

    /// A device's keys, certificate and first KeyPackages, and the proof of possession over the handle it registers
    /// with (spec 6.4).
    fn new_device(
        transport: Arc<dyn Transport>,
        handle: &Handle,
        account: SigningKey,
        name: Option<String>,
        push: Push,
    ) -> Result<(Client, api::NewDevice)> {
        let signer = SignatureKeyPair::new(SignatureScheme::ED25519).map_err(|_| Error::Internal("key"))?;
        let device_key: Key = signer.public().try_into().map_err(|_| Error::Internal("device key"))?;
        let device_id: DeviceId = identity::random();
        let auth = SigningKey::from_bytes(&identity::random());
        let state = State {
            user_id: String::new(),
            username: None,
            account_secret: account.to_bytes(),
            auth_secret: auth.to_bytes(),
            device_id,
            certificate: identity::certify(&account, &device_key).to_bytes(),
            signer,
            name,
            contacts: Default::default(),
            chats: Default::default(),
            outbox: VecDeque::new(),
            next_seq: 0,
            events: Vec::new(),
            handed: Vec::new(),
            seen: VecDeque::new(),
            ignored: VecDeque::new(),
            key_packages_left: KEY_PACKAGE_TARGET,
            key_packages: Vec::new(),
            last_resort_at: wall_ms(),
            refill: None,
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
        };
        let mut client = Client::with(transport, Provider::default(), state);
        let key_packages =
            (0..KEY_PACKAGE_TARGET).map(|_| client.key_package(false).map(Into::into)).collect::<Result<Vec<_>>>()?;
        let auth_key = auth.verifying_key().to_bytes();
        let proof =
            account.sign(&api::registration_to_be_signed(handle, &device_id, &device_key, &auth_key)).to_bytes();
        let device = api::NewDevice {
            device_id,
            device_key,
            certificate: client.state.certificate.clone().into(),
            auth_key,
            push,
            key_packages,
            last_resort: client.key_package(true)?.into(),
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
        // A restored account keeps the name it has at the hub now, whatever the backup had (spec 6.4).
        self.state.username = registered.username.map(|name| String::from_utf8_lossy(name.as_slice()).into_owned());
        Ok(())
    }

    /// For fuzzing: processes one envelope as if it came from the inbox (one transaction, like `sync`).
    #[cfg(feature = "fuzz")]
    pub fn receive_for_fuzzing(&mut self, sealed: &[u8]) -> Result<()> {
        self.atomic(|c| c.receive(sealed))
    }

    /// Opens a sealed state.
    pub fn restore(transport: Arc<dyn Transport>, blob: &[u8], key: &[u8; 32]) -> Result<Client> {
        let (mut state, entries) = state::open(blob, key)?;
        // What the last call handed out may not have reached the app before the restart: it comes again.
        let handed = std::mem::take(&mut state.handed);
        state.events.splice(0..0, handed);
        let provider = Provider::default();
        provider.restore(entries);
        Ok(Client::with(transport, provider, state))
    }

    fn with(transport: Arc<dyn Transport>, provider: Provider, state: State) -> Client {
        Client {
            transport,
            store: None,
            provider,
            state,
            clock: Arc::new(wall_ms),
            last_request_ms: 0,
            key_packages_known_ms: 0,
            dropped: 0,
            drop_reasons: Vec::new(),
            delivering: BTreeSet::new(),
            inbox_fresh_until: 0,
            media_keys_forgotten: false,
            guest_events: None,
            explicit_ack: false,
            fetching: false,
            fetch_at_most: MAX_COMMITS_FETCHED,
            unacked: Vec::new(),
            after_due: false,
        }
    }

    /// Opens a sealed state as a guest: another process (a share or notification extension) using the state of the
    /// app that owns it, in turns (the platform's lock around each use). Unlike `restore`, what the app was handed
    /// stays handed, and the events this one receives are returned but stay in the state, so the app gets them too;
    /// a guest never acknowledges.
    pub fn restore_guest(transport: Arc<dyn Transport>, blob: &[u8], key: &[u8; 32]) -> Result<Client> {
        let (state, entries) = state::open(blob, key)?;
        let provider = Provider::default();
        provider.restore(entries);
        let mut client = Client::with(transport, provider, state);
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
        self.provider.restore(entries);
        self.state = state;
        // Left to the hub's repeat: the state taken in says whether they are seen.
        self.unacked.clear();
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
        let mut snapshot = profile::Snapshot(self.provider.entries());
        state::seal(&self.state, std::mem::take(&mut snapshot.0), key)
    }

    /// For tests: fewer commits fetched in one call than `MAX_COMMITS_FETCHED`.
    #[cfg(feature = "testing")]
    pub fn fetch_at_most_for_tests(&mut self, commits: usize) {
        self.fetch_at_most = commits;
    }

    /// For tests: the clock that decides when updates are due.
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

    pub fn device_id(&self) -> DeviceId {
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

    /// KeyPackages whose private keys this device still holds.
    pub fn own_key_packages(&self) -> usize {
        self.state.key_packages.len()
    }

    /// Envelopes waiting to be taken by the hub; not those held for a blocked user (spec 4.8), which go only after
    /// an unblock.
    pub fn pending(&self) -> usize {
        self.state.outbox.iter().filter(|item| !self.blocked(&item.user)).count()
    }

    fn blocked(&self, user: &str) -> bool {
        self.state.contacts.get(user).is_some_and(|c| c.blocked)
    }

    /// Every chat: peer, group id, epoch, members. For tests and diagnostics.
    pub fn chats(&self) -> Vec<(String, [u8; 32], u64, usize)> {
        self.state
            .chats
            .iter()
            .filter_map(|(key, chat)| {
                let id: [u8; 32] = unhex(key)?.try_into().ok()?;
                let group = MlsGroup::load(self.provider.storage(), &GroupId::from_slice(&id)).ok()??;
                Some((chat.peer.clone(), id, group.epoch().as_u64(), group.members().count()))
            })
            .collect()
    }

    fn persist(&self) -> Result<()> {
        match &self.store {
            Some((store, key)) => store.save(self.export(key)).map_err(Error::Save),
            None => Ok(()),
        }
    }

    fn push_event(&mut self, event: Event) {
        self.state.events.push(stored(event));
    }

    /// Runs `f` as one transaction: on an error, state (events included) and MLS store are as before. Nothing that
    /// talks to the hub in a way the hub remembers may run inside: what it accepted cannot be rolled back.
    fn atomic<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let state = self.state.clone();
        let mut snapshot = profile::Snapshot(self.provider.entries());
        let result = f(self);
        if result.is_err() {
            self.state = state;
            self.provider.restore(std::mem::take(&mut snapshot.0));
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
        // This call acknowledges what the last one handed out. What it hands out stays saved until the next call, so
        // a crash before the app has it brings it again.
        if let Some(returned) = self.guest_events {
            // A guest: the events stay for the app; only those new since the last call are returned.
            let total = self.state.events.len();
            let result =
                result.map(|_| self.state.events[returned.min(total)..].iter().cloned().map(unstored).collect());
            let saved = self.persist();
            if result.is_ok() && saved.is_err() {
                return saved.map(|()| Vec::new());
            }
            self.guest_events = Some(total);
            return result;
        }
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
                Err(error)
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
        match response.status {
            status::NO_CONTENT => Ok(()),
            _ => Err(refused(&response)),
        }
    }

    fn lookup(&mut self, handle: &Handle, key_packages: bool, except: Vec<Key>) -> Result<api::LookupResult> {
        let body = encode(&api::Lookup { handle: handle.clone(), key_packages: key_packages as u8, except });
        let response = self.signed("POST", api::LOOKUP, &body)?;
        match response.status {
            status::OK => Ok(decode(&response.body)?),
            status::NOT_FOUND => Err(Error::NotOnYo),
            _ => Err(refused(&response)),
        }
    }

    /// A lookup that also asks for the user's username as its owner writes it ("YoGPT"), whatever spelling found
    /// it. A hub from before spellings answers without it (and, asked for KeyPackages, hands out none: then asked
    /// again the old way).
    fn lookup_named(
        &mut self,
        handle: &Handle,
        key_packages: bool,
        except: Vec<Key>,
    ) -> Result<(api::LookupResult, Option<String>)> {
        let flags = api::LOOKUP_USERNAME | if key_packages { api::LOOKUP_KEY_PACKAGES } else { 0 };
        let body = encode(&api::Lookup { handle: handle.clone(), key_packages: flags, except: except.clone() });
        let response = self.signed("POST", api::LOOKUP, &body)?;
        match response.status {
            status::OK => {}
            status::NOT_FOUND => return Err(Error::NotOnYo),
            // A hub from before spellings knows usernames in upper case only.
            status::BAD_REQUEST if handle.kind == KIND_USERNAME => {
                let upper = handle.display().to_uppercase();
                if upper == handle.display() {
                    return Err(refused(&response));
                }
                return Ok((self.lookup(&Handle::username(&upper), key_packages, except)?, None));
            }
            _ => return Err(refused(&response)),
        }
        if let Ok(named) = decode::<api::LookupNamed>(&response.body) {
            let username = named
                .username
                .and_then(|n| String::from_utf8(n.into()).ok())
                .filter(|n| yo_wire::handle::check_username_form(n).is_ok());
            return Ok((named.found, username));
        }
        let found: api::LookupResult = decode(&response.body)?;
        if key_packages && found.key_packages.is_empty() {
            return Ok((self.lookup(handle, true, except)?, None));
        }
        Ok((found, None))
    }

    // ---- MLS ----

    fn signer(&self) -> &SignatureKeyPair {
        &self.state.signer
    }

    fn own_device_key(&self) -> &[u8] {
        self.state.signer.public()
    }

    fn my_account_key(&self) -> Key {
        SigningKey::from_bytes(&self.state.account_secret).verifying_key().to_bytes()
    }

    fn credential(&self) -> CredentialWithKey {
        CredentialWithKey {
            credential: BasicCredential::new(self.state.certificate.clone()).into(),
            signature_key: self.state.signer.public().into(),
        }
    }

    /// Makes a KeyPackage and remembers it, so its private keys can be deleted when it can no longer be used.
    fn key_package(&mut self, last_resort: bool) -> Result<Vec<u8>> {
        let mut builder = KeyPackage::builder()
            .leaf_node_capabilities(profile::capabilities())
            .key_package_lifetime(Lifetime::new(profile::KEY_PACKAGE_LIFETIME_S));
        if last_resort {
            builder = builder.mark_as_last_resort();
        }
        let bundle = builder
            .build(profile::CIPHERSUITE, &self.provider, self.signer(), self.credential())
            .map_err(|_| Error::Internal("key package"))?;
        let hash_ref = bundle.key_package().hash_ref(self.provider.crypto()).map_err(mls)?;
        // A new last resort retires the one before only once the hub has it (`refill`).
        self.state.key_packages.push(OwnKeyPackage {
            hash_ref: hash_ref.tls_serialize_detached().map_err(mls)?,
            created_at: (self.clock)(),
            last_resort,
            retired_at: None,
        });
        bundle.key_package().tls_serialize_detached().map_err(|_| Error::Internal("key package"))
    }

    /// Deletes the private keys of KeyPackages that can no longer arrive in a Welcome: expired ones, and last resorts
    /// replaced long enough ago that no Welcome made with them can still wait at the hub.
    fn forget_key_packages(&mut self) {
        let now = (self.clock)();
        let lifetime_ms = profile::KEY_PACKAGE_LIFETIME_S * 1000 + 24 * 3600 * 1000;
        let (dead, alive): (Vec<_>, Vec<_>) = std::mem::take(&mut self.state.key_packages).into_iter().partition(|k| {
            now.saturating_sub(k.created_at) > lifetime_ms
                || k.retired_at.is_some_and(|at| now.saturating_sub(at) > RETIRED_KEY_PACKAGE_KEPT_MS)
        });
        for key_package in dead {
            if let Ok(hash_ref) = KeyPackageRef::tls_deserialize_exact(key_package.hash_ref.as_slice()) {
                let _ = self.provider.storage().delete_key_package(&hash_ref);
            }
        }
        self.state.key_packages = alive;
    }

    fn load(&self, group: &GroupId) -> Result<MlsGroup> {
        MlsGroup::load(self.provider.storage(), group).map_err(mls)?.ok_or(Error::Protocol("unknown group"))
    }

    fn token(&self, group: &MlsGroup) -> Result<[u8; TOKEN_LEN]> {
        let token = group
            .export_secret(self.provider.crypto(), EPOCH_TOKEN_LABEL, group.group_id().as_slice(), TOKEN_LEN)
            .map_err(mls)?;
        token.try_into().map_err(|_| Error::Internal("token"))
    }

    /// The members with the account keys their certificates name (checked when they joined).
    fn members(group: &MlsGroup) -> Vec<Member> {
        group
            .members()
            .filter_map(|m| {
                Some(Member {
                    index: m.index,
                    account_key: identity::claimed(&m.credential)?,
                    device_key: m.signature_key.as_slice().try_into().ok()?,
                })
            })
            .collect()
    }

    /// The members of one user in a v1 chat: those of the own account key, or all the others (the peer's).
    fn members_of(&self, group: &MlsGroup, user: &str) -> Vec<Member> {
        let mine = self.my_account_key();
        let own = user == self.state.user_id;
        Self::members(group).into_iter().filter(|m| (m.account_key == mine) == own).collect()
    }

    /// The users a group's envelopes go to: the peer, and oneself when the group holds other own devices (4.6).
    fn recipients(&self, group: &MlsGroup, peer: &str) -> Vec<String> {
        let own_device = self.own_device_key();
        let mut users = vec![peer.to_owned()];
        if self.members_of(group, &self.state.user_id).iter().any(|m| m.device_key != own_device) {
            users.push(self.state.user_id.clone());
        }
        users
    }

    /// The device check of a delivery (4.6): the user's devices as the group holds them now.
    fn devices_hash(&self, group: &MlsGroup, user: &str) -> Option<[u8; 32]> {
        let theirs = self.members_of(group, user);
        let account_key = theirs.first()?.account_key;
        let keys: Vec<Key> = theirs.iter().map(|m| m.device_key).collect();
        Some(yo_wire::devices::hash(&account_key, &keys))
    }

    /// Queues an envelope for users, each with the devices it is encrypted for: their members in `group`, the group
    /// state the envelope was made in.
    fn enqueue(&mut self, group: &MlsGroup, users: &[String], sealed: &[u8], plaintext: Option<&[u8]>) {
        for user in users {
            let made_for = self.members_of(group, user).iter().map(|m| m.device_key).collect();
            self.state.next_seq += 1;
            self.state.outbox.push_back(Outgoing {
                seq: self.state.next_seq,
                user: user.clone(),
                group: group_key(group.group_id()),
                envelope: sealed.to_vec(),
                made_for,
                plaintext: plaintext.map(<[u8]>::to_vec),
            });
        }
    }

    /// Pins a contact's account key; another key than pinned for the same user is an identity change.
    fn pin(&mut self, user: &str, account_key: Key, handle: Option<String>) {
        let contact = self.state.contacts.entry(user.to_owned()).or_insert(Contact {
            account_key,
            name: None,
            verified: false,
            unconfirmed: false,
            blocked: false,
            handles: Vec::new(),
            gone: false,
        });
        if contact.account_key != account_key {
            let known = contact.account_key != [0; 32];
            contact.account_key = account_key;
            // A verified contact's new key is held until confirmed: the warning before the message (spec 3.4).
            contact.unconfirmed |= contact.verified;
            contact.verified = false;
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

    /// The chat to send to `user` on: the one with the smallest group id (spec 4.5).
    fn chat_with(&self, user: &str) -> Option<GroupId> {
        self.state.chats.iter().find(|(_, chat)| chat.peer == user).and_then(|(key, _)| group_of(key))
    }

    fn drop_chat(&mut self, key: &str) {
        self.state.chats.remove(key);
        self.state.outbox.retain(|item| item.group != key);
        if let Some(group) = group_of(key) {
            if let Ok(Some(mut group)) = MlsGroup::load(self.provider.storage(), &group) {
                let _ = group.delete(self.provider.storage());
            }
        }
    }

    /// Validates KeyPackages a lookup handed out: each made for one of the user's devices under its account key.
    fn checked_key_packages(&self, found: &api::LookupResult) -> Result<Vec<KeyPackage>> {
        found
            .key_packages
            .iter()
            .map(|bytes| {
                let key_package = KeyPackageIn::tls_deserialize_exact(bytes.as_slice())
                    .map_err(mls)?
                    .validate(self.provider.crypto(), ProtocolVersion::Mls10)
                    .map_err(mls)?;
                if key_package.ciphersuite() != profile::CIPHERSUITE {
                    return Err(Error::Protocol("suite"));
                }
                let leaf = key_package.leaf_node();
                let device_key = leaf.signature_key().as_slice();
                let account_key = identity::member(leaf.credential(), device_key)?;
                if account_key != found.account_key || !found.device_keys.iter().any(|k| k.as_slice() == device_key) {
                    return Err(Error::Protocol("key package names someone else"));
                }
                Ok(key_package)
            })
            .collect()
    }

    /// Creates a chat with whoever `handle` names (spec 4.2); returns the user id and the group.
    fn start_chat(&mut self, handle: &Handle) -> Result<(String, GroupId)> {
        let (found, username) = self.lookup_named(handle, true, Vec::new())?;
        let peer = hex(&found.user_id);
        if handle.kind == KIND_USER && found.user_id.as_slice() != handle.value.as_slice() {
            return Err(Error::Protocol("the hub answered for another user"));
        }
        if peer == self.state.user_id {
            return Err(Error::Protocol("sending to oneself"));
        }
        let key_packages = self.checked_key_packages(&found)?;
        if key_packages.is_empty() {
            return Err(Error::Protocol("no key package"));
        }
        // The name as its owner writes it; from a hub that does not tell, as it was typed.
        let shown = username.or_else(|| (handle.kind != KIND_USER).then(|| handle.display()));
        // Pinned outside the transaction that makes the chat: a verified contact's new key stays marked.
        self.pin(&peer, found.account_key, shown);
        if self.state.contacts.get(&peer).is_some_and(|c| c.unconfirmed) {
            return Err(Error::Unconfirmed);
        }
        // Made and saved first, registered outside the transaction: what the hub took is never rolled back (4.7).
        let group_id = self.atomic(|c| c.create_chat(&peer, &key_packages).and_then(|g| c.persist().map(|()| g)))?;
        match self.register_group(&group_id) {
            // Not now, or not saved before it left (so it did not): the chat waits, its Welcome and messages with it,
            // for the next delivery.
            Err(error) if !retryable(&error) && !matches!(error, Error::Save(_)) => return Err(error),
            _ => {}
        }
        Ok((peer, group_id))
    }

    /// Creates the group for a chat with `peer` and queues its Welcome and profile (spec 4.2), all held until the
    /// group is registered at the hub (`register_group`).
    fn create_chat(&mut self, peer: &str, key_packages: &[KeyPackage]) -> Result<GroupId> {
        let id: [u8; 32] = identity::random();
        let group_id = GroupId::from_slice(&id);
        let mut group = MlsGroup::new_with_group_id(
            &self.provider,
            self.signer(),
            &profile::create_config()?,
            group_id.clone(),
            self.credential(),
        )
        .map_err(mls)?;
        let (_, welcome, _) = group.add_members(&self.provider, self.signer(), key_packages).map_err(mls)?;
        group.merge_pending_commit(&self.provider).map_err(mls)?;
        let token = self.token(&group)?;
        let create = api::GroupCreate { group_id: id, epoch: group.epoch().as_u64(), token_hash: sha256(&token) };
        let sealed = envelope::seal(&welcome.to_bytes().map_err(mls)?)?;
        self.state.chats.insert(
            group_key(&group_id),
            Chat {
                peer: peer.to_owned(),
                joined_epoch: group.epoch().as_u64(),
                last_update_ms: (self.clock)(),
                sent_since_update: 0,
                needs_update: false,
                inflight: None,
                sent: VecDeque::new(),
                last_seen_ms: 0,
                lost: false,
                restarted: false,
                profile_due: false,
                creating: Some(Creating { request: encode(&create), sent: false }),
            },
        );
        self.enqueue(&group, &[peer.to_owned()], &sealed, None);
        if let Some(name) = self.state.name.clone() {
            self.send_in(&group_id, Content::Profile { name }, false)?;
        }
        Ok(group_id)
    }

    /// Registers a group made here at the hub (spec 4.2) with the request it was made with, again until the hub
    /// answers. A `409` to a request sent before is the answer to that one, lost; to the first, the id is someone
    /// else's and the chat goes, with what waits in it.
    fn register_group(&mut self, group_id: &GroupId) -> Result<()> {
        let key = group_key(group_id);
        let Some(creating) = self.state.chats.get(&key).and_then(|c| c.creating.clone()) else { return Ok(()) };
        if !creating.sent {
            if let Some(pending) = self.state.chats.get_mut(&key).and_then(|c| c.creating.as_mut()) {
                pending.sent = true;
            }
            self.persist()?;
        }
        let response = self.unsigned("POST", api::GROUPS, &[], &creating.request)?;
        let error = refused(&response);
        match response.status {
            status::CREATED => {}
            status::CONFLICT if creating.sent => {}
            _ if retryable(&error) => return Err(error),
            _ => {
                self.drop_chat(&key);
                return Err(error);
            }
        }
        if let Some(chat) = self.state.chats.get_mut(&key) {
            chat.creating = None;
        }
        Ok(())
    }

    /// Registers the groups made here that the hub has not answered for yet, before a delivery.
    fn register_pending(&mut self) -> Result<()> {
        let pending: Vec<GroupId> = self
            .state
            .chats
            .iter()
            .filter(|(_, chat)| chat.creating.is_some())
            .filter_map(|(key, _)| group_of(key))
            .collect();
        for group in pending {
            match self.register_group(&group) {
                Ok(()) => {}
                Err(error) if retryable(&error) => {}
                Err(error @ Error::Save(_)) => return Err(error),
                Err(error) => self.drop_one(format!("group: {error}")),
            }
        }
        Ok(())
    }

    // ---- commits ----

    fn update_due(&self, group: &GroupId) -> bool {
        self.state.chats.get(&group_key(group)).is_some_and(|chat| {
            !chat.lost
                && chat.creating.is_none()
                && (chat.needs_update
                    || (self.clock)().saturating_sub(chat.last_update_ms) >= UPDATE_AFTER_MS
                    || chat.sent_since_update >= UPDATE_AFTER_MESSAGES)
        })
    }

    /// Records a staged commit as the group's inflight commit and saves it; `resolve` then hands it to the hub (spec
    /// 4.3). From here it is kept, whatever happens to the answer.
    fn prepare(&mut self, group: &MlsGroup, bundle: &CommitMessageBundle, welcome_for: Option<&str>) -> Result<()> {
        let group_id = group.group_id().clone();
        let epoch = group.epoch().as_u64();
        let token = self.token(group)?;
        let next = group
            .pending_commit()
            .ok_or(Error::Internal("pending commit"))?
            .export_secret(self.provider.crypto(), EPOCH_TOKEN_LABEL, group_id.as_slice(), TOKEN_LEN)
            .map_err(mls)?;
        let commit = envelope::seal(&bundle.commit().to_bytes().map_err(mls)?)?;
        let welcome = match (welcome_for, bundle.welcome()) {
            (Some(user), Some(welcome)) => {
                let message = MlsMessageOut::from_welcome(welcome.clone(), ProtocolVersion::Mls10);
                Some((user.to_owned(), envelope::seal(&message.to_bytes().map_err(mls)?)?))
            }
            _ => None,
        };
        let request =
            encode(&api::Commit { epoch, token, next_token_hash: sha256(&next), envelope: commit.clone().into() });
        self.state.chats.get_mut(&group_key(&group_id)).ok_or(Error::Protocol("unknown group"))?.inflight =
            Some(Inflight { request, commit, welcome });
        // Saved before it leaves: after a crash the commit is still there to be repeated (spec 4.3).
        self.persist()
    }

    /// Sends a group's inflight commit (again) and acts on the answer. The hub answers a repeat of an accepted commit
    /// with 200, so a lost answer costs only the repeat.
    fn resolve(&mut self, group_id: &GroupId) -> Result<Committed> {
        let key = group_key(group_id);
        let Some(chat) = self.state.chats.get(&key) else { return Ok(Committed::Nothing) };
        let (peer, Some(inflight)) = (chat.peer.clone(), chat.inflight.clone()) else {
            return Ok(Committed::Nothing);
        };
        let id: yo_wire::GroupId = group_id.as_slice().try_into().map_err(|_| Error::Protocol("group id"))?;
        let response = match self.unsigned("POST", &api::commit_path(&id), &[], &inflight.request) {
            Ok(response) => response,
            Err(error) => return Ok(Committed::Unknown(error.to_string())),
        };
        let mut group = self.load(group_id)?;
        let outcome = match response.status {
            status::OK => {
                // The commit goes to the members of the epoch it leaves; a repair's Welcome first, so a new device
                // joins before the commit it cannot use arrives.
                let recipients = self.recipients(&group, &peer);
                let before = self.load(group_id)?;
                group.merge_pending_commit(&self.provider).map_err(mls)?;
                if let Some((user, welcome)) = &inflight.welcome {
                    self.enqueue(&group, std::slice::from_ref(user), welcome, None);
                }
                self.enqueue(&before, &recipients, &inflight.commit, None);
                Committed::Accepted
            }
            status::CONFLICT => {
                // Read first: an answer that cannot be read leaves the commit as it was, to be repeated.
                let Ok(epoch) = decode::<api::Epoch>(&response.body) else {
                    return Ok(Committed::Unknown("unreadable answer to a commit".into()));
                };
                group.clear_pending_commit(self.provider.storage()).map_err(mls)?;
                Committed::Lost(epoch.epoch)
            }
            status::FORBIDDEN | status::NOT_FOUND => {
                group.clear_pending_commit(self.provider.storage()).map_err(mls)?;
                Committed::Refused
            }
            // Rate limit or hub trouble: the answer is still open.
            other => return Ok(Committed::Unknown(format!("hub answered {other}"))),
        };
        let now = (self.clock)();
        if let Some(chat) = self.state.chats.get_mut(&key) {
            chat.inflight = None;
            match outcome {
                Committed::Accepted => {
                    chat.last_update_ms = now;
                    chat.sent_since_update = 0;
                    chat.needs_update = false;
                }
                // Its updates are not given up: a new group takes its place (4.9).
                Committed::Refused => chat.lost = true,
                _ => {}
            }
        }
        Ok(outcome)
    }

    /// After a lost race: receives the winning commit, and fetches it from the hub (spec 4.3) when it is not in the
    /// inbox, until the group stands at `epoch` (or is lost). More commits than one call fetches are "not now": what
    /// was fetched is kept, the rest comes with the next call.
    fn catch_up(&mut self, group_id: &GroupId, epoch: u64) -> Result<()> {
        self.receive_all(0)?;
        self.fetch_commits(group_id, epoch)?.then_some(()).ok_or_else(more_commits)
    }

    /// Fetches and processes the commits of a group from its epoch up to `epoch` (spec 4.3): after a lost race, or
    /// for a message from an epoch this device has not reached. A commit the hub had and no longer has (`410`, past
    /// its 30 days) marks the chat lost (4.9); one it never had (`404`: the message's epoch was made up) is a refusal.
    /// Each fetched envelope must move the group on, and is processed (each in a transaction of its own, so what was
    /// fetched stays) without fetching again. False when more remain than one call fetches.
    fn fetch_commits(&mut self, group_id: &GroupId, epoch: u64) -> Result<bool> {
        let id: yo_wire::GroupId = group_id.as_slice().try_into().map_err(|_| Error::Protocol("group id"))?;
        for _ in 0..self.fetch_at_most {
            let current = self.load(group_id)?.epoch().as_u64();
            if current >= epoch {
                return Ok(true);
            }
            let response = self.unsigned("GET", &api::commit_fetch_path(&id, current), &[], &[])?;
            match response.status {
                status::OK => {}
                status::GONE => {
                    if let Some(chat) = self.state.chats.get_mut(&group_key(group_id)) {
                        chat.lost = true;
                    }
                    return Ok(true);
                }
                _ => return Err(refused(&response)),
            }
            self.atomic(|c| {
                c.fetching = true;
                let received = c.receive(&response.body);
                c.fetching = false;
                received?;
                if c.load(group_id)?.epoch().as_u64() <= current {
                    return Err(Error::Protocol("a fetched commit did not move the group on"));
                }
                // The same commit also comes through the inbox: then it is skipped as seen.
                c.remember(&response.body);
                Ok(())
            })?;
        }
        Ok(self.load(group_id)?.epoch().as_u64() >= epoch)
    }

    /// Before an envelope is processed: a message from an epoch of a group this device has not reached makes it fetch
    /// the commits it lacks (4.3), outside the envelope's transaction, so they stay whatever becomes of it.
    fn fetch_before(&mut self, sealed: &[u8]) -> Result<()> {
        let Some((group_id, epoch)) = Self::private_epoch(sealed) else { return Ok(()) };
        let Some(chat) = self.state.chats.get(&group_key(&group_id)) else { return Ok(()) };
        if chat.lost || epoch <= self.load(&group_id)?.epoch().as_u64() {
            return Ok(());
        }
        // Gone from the hub (past their 30 days): the chat is lost to this device and restarted (4.9).
        self.fetch_commits(&group_id, epoch)?.then_some(()).ok_or_else(more_commits)
    }

    /// The group and epoch of a private message, read without opening it.
    fn private_epoch(sealed: &[u8]) -> Option<(GroupId, u64)> {
        let message = MlsMessageIn::tls_deserialize_exact(envelope::open(sealed).ok()?).ok()?;
        match message.extract() {
            MlsMessageBodyIn::PrivateMessage(private) => {
                let message: ProtocolMessage = private.into();
                Some((message.group_id().clone(), message.epoch().as_u64()))
            }
            _ => None,
        }
    }

    /// Restarts every chat lost to this device (spec 4.9): a new group with the peer, the old one dropped; what was
    /// waiting to go is sent there, and a `Restart` tells the peer what of theirs arrived last, so it sends the rest
    /// again. "Not now" leaves it for the next call.
    fn restart_lost(&mut self) -> Result<()> {
        let lost: Vec<String> =
            self.state.chats.iter().filter(|(_, chat)| chat.lost).map(|(key, _)| key.clone()).collect();
        for key in lost {
            let Some(old) = self.state.chats.get(&key).cloned() else { continue };
            let peer = old.peer.clone();
            // A blocked peer's lost chat waits, with what is held for it, and is restarted after the unblock (4.8);
            // only a gone peer's is dropped.
            match self.state.contacts.get(&peer) {
                Some(contact) if contact.gone => {
                    self.drop_chat(&key);
                    continue;
                }
                Some(contact) if contact.blocked => continue,
                _ => {}
            }
            let restarted = self.start_chat(&Handle::user(&user_bytes(&peer)?)).map(|(_, group)| group);
            let group = match restarted {
                Ok(group) => group,
                // Not now, or not saved (a full disk): the old chat and what waits in it stay for the next call.
                Err(error) if retryable(&error) || matches!(error, Error::Save(_) | Error::Replaced) => {
                    return Err(error)
                }
                // Restarted once the person confirmed the new key; what waits is kept until then.
                Err(Error::Unconfirmed) => continue,
                Err(error) => {
                    self.drop_one(format!("restart: {error}"));
                    self.drop_chat(&key);
                    continue;
                }
            };
            let waiting = self.waiting_plaintexts(&key);
            self.atomic(|c| {
                let restart =
                    Message { sent_at_ms: wall_ms(), content: Content::Restart { last_seen_ms: old.last_seen_ms } };
                c.send_plaintext(&group, &restart.encode()?)?;
                for plaintext in &waiting {
                    c.send_plaintext(&group, plaintext)?;
                }
                if let Some(chat) = c.state.chats.get_mut(&group_key(&group)) {
                    // Until the peer answers here, a restart of its own (at the same time) leaves this one standing.
                    chat.restarted = true;
                    chat.last_seen_ms = old.last_seen_ms;
                    // What was sent in the old chat, should the peer ask for it again (its own restart).
                    let mut sent = old.sent.clone();
                    sent.extend(chat.sent.drain(..));
                    while sent.len() > KEPT_FOR_RESTART {
                        sent.pop_front();
                    }
                    chat.sent = sent;
                }
                Ok(())
            })?;
            self.drop_chat(&key);
            // What the peer still sends there, before it takes in the restart, comes again in the new chat.
            if let Some(id) = unhex(&key).and_then(|id| <[u8; 32]>::try_from(id).ok()) {
                self.ignore(id);
            }
        }
        Ok(())
    }

    /// The messages of a chat still in the outbox, in order, once each (one envelope per recipient).
    fn waiting_plaintexts(&self, key: &str) -> Vec<Vec<u8>> {
        let mut seen = BTreeSet::new();
        self.state
            .outbox
            .iter()
            .filter(|item| item.group == key)
            .filter_map(|item| item.plaintext.clone())
            .filter(|plaintext| seen.insert(plaintext.clone()))
            .collect()
    }

    /// The peer restarted the chat in `group` (spec 4.9): the older chats with it go, and what was sent in them is
    /// sent again here, with what was still waiting to go. All of it: the peer's `last_seen` is the latest it got,
    /// not all it got (one may have come after a later one, or never); it skips what it has seen.
    fn restarted_by_peer(&mut self, group: &GroupId, peer: &str) -> Result<()> {
        let key = group_key(group);
        let others: Vec<String> = self
            .state
            .chats
            .iter()
            .filter(|(k, chat)| chat.peer == peer && **k != key)
            .map(|(k, _)| k.clone())
            .collect();
        let mut again: Vec<Vec<u8>> = Vec::new();
        for other in &others {
            let sent = self.state.chats[other].sent.iter().filter(|plaintext| {
                Message::decode(plaintext)
                    .is_ok_and(|m| !matches!(m.content, Content::Profile { .. } | Content::Restart { .. }))
            });
            for plaintext in sent.cloned().chain(self.waiting_plaintexts(other)) {
                if !again.contains(&plaintext) {
                    again.push(plaintext);
                }
            }
        }
        // The older chats go; one this device restarted itself at the same time and the peer has not answered in
        // stays (both are kept, the smaller id used, 4.5).
        // In the order they were sent, whichever chat they were in.
        again.sort_by_key(|plaintext| Message::decode(plaintext).map_or(0, |m| m.sent_at_ms));
        let older: Vec<&String> = others.iter().filter(|k| !self.state.chats[*k].restarted).collect();
        for other in older {
            self.drop_chat(other);
        }
        for plaintext in again {
            self.send_plaintext(group, &plaintext)?;
        }
        Ok(())
    }

    fn remember(&mut self, sealed: &[u8]) {
        self.state.seen.push_back(sha256(sealed));
        if self.state.seen.len() > SEEN {
            self.state.seen.pop_front();
        }
    }

    /// Stages an update (spec 4.4) as the group's inflight commit.
    fn stage_update(&mut self, group_id: &GroupId) -> Result<()> {
        let mut group = self.load(group_id)?;
        let bundle = group.self_update(&self.provider, self.signer(), LeafNodeParameters::default()).map_err(mls)?;
        self.prepare(&group, &bundle, None)
    }

    /// Settles an inflight commit and updates the chat if due. `Err` when a commit's answer is still open, or the
    /// group is behind the hub's and could not catch up: it cannot be used until then, nothing is encrypted in an
    /// epoch already left.
    fn ensure_fresh(&mut self, group: &GroupId) -> Result<()> {
        match self.resolve(group)? {
            Committed::Unknown(error) => return Err(Error::Network(error)),
            Committed::Lost(epoch) => self.catch_up(group, epoch)?,
            _ => {}
        }
        for _ in 0..4 {
            if !self.update_due(group) {
                return Ok(());
            }
            // Staged in a transaction, sent outside it: what the hub accepted is never rolled back.
            let committed = self.atomic(|c| c.stage_update(group)).and_then(|()| self.resolve(group));
            match committed {
                Ok(Committed::Accepted | Committed::Refused | Committed::Nothing) => return Ok(()),
                Ok(Committed::Unknown(error)) => return Err(Error::Network(error)),
                Ok(Committed::Lost(epoch)) => self.catch_up(group, epoch)?,
                // An update that cannot be made does not stop the chat.
                Err(error) if retryable(&error) => return Ok(()),
                Err(error) => {
                    self.drop_one(format!("update: {error}"));
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn send_in(&mut self, group_id: &GroupId, content: Content, count: bool) -> Result<()> {
        // A photo or video already sent to this peer: its blob id is random, so a message with it is this one, sent
        // again by an app that lost its answer (a crash before it took it off its queue). It goes once.
        if let Content::Photo(media) | Content::Video { media, .. } = &content {
            if self.sent_media(group_id, &media.id) {
                return Ok(());
            }
        }
        // Strictly increasing: no two messages of a device are the same plaintext, and a restart's `last_seen` (4.9)
        // tells them apart.
        let sent_at_ms = wall_ms().max(self.state.last_sent_ms + 1);
        self.state.last_sent_ms = sent_at_ms;
        let plaintext = Message { sent_at_ms, content }.encode()?;
        self.send_plaintext(group_id, &plaintext)?;
        if count {
            if let Some(chat) = self.state.chats.get_mut(&group_key(group_id)) {
                chat.sent_since_update += 1;
            }
        }
        Ok(())
    }

    /// Whether a message with this photo's or video's blob went to the group's peer already, in any chat with them
    /// (among the last `KEPT_FOR_RESTART` sent in each).
    fn sent_media(&self, group_id: &GroupId, id: &[u8; 16]) -> bool {
        let Some(peer) = self.state.chats.get(&group_key(group_id)).map(|c| &c.peer) else { return false };
        self.state.chats.values().filter(|chat| &chat.peer == peer).flat_map(|chat| &chat.sent).any(|plaintext| {
            matches!(Message::decode(plaintext).map(|m| m.content),
                Ok(Content::Photo(media) | Content::Video { media, .. }) if &media.id == id)
        })
    }

    /// Encrypts a message in a group. The one place messages are made, so the one place a verified contact's
    /// unconfirmed new key is checked (spec 3.4).
    fn encrypt(&mut self, group: &mut MlsGroup, plaintext: &[u8]) -> Result<Vec<u8>> {
        let peer =
            self.state.chats.get(&group_key(group.group_id())).ok_or(Error::Protocol("unknown group"))?.peer.clone();
        if self.state.contacts.get(&peer).is_some_and(|c| c.unconfirmed) {
            return Err(Error::Unconfirmed);
        }
        let message = group.create_message(&self.provider, self.signer(), plaintext).map_err(mls)?;
        Ok(envelope::seal(&message.to_bytes().map_err(mls)?)?)
    }

    /// Encrypts a message in a group and queues it for every recipient.
    fn send_plaintext(&mut self, group_id: &GroupId, plaintext: &[u8]) -> Result<()> {
        let mut group = self.load(group_id)?;
        let sealed = self.encrypt(&mut group, plaintext)?;
        let peer = self.state.chats.get(&group_key(group_id)).ok_or(Error::Protocol("unknown group"))?.peer.clone();
        let users = self.recipients(&group, &peer);
        self.enqueue(&group, &users, &sealed, Some(plaintext));
        if let Some(chat) = self.state.chats.get_mut(&group_key(group_id)) {
            chat.sent.push_back(plaintext.to_vec());
            if chat.sent.len() > KEPT_FOR_RESTART {
                chat.sent.pop_front();
            }
        }
        Ok(())
    }

    /// An envelope whose devices are all gone from the group (a repair replaced them): a message is encrypted anew for
    /// the devices there now; anything else is of use to nobody. `Ok(None)`: drop it.
    fn renew(&mut self, item: &mut Outgoing, group_id: &GroupId) -> Result<Option<()>> {
        let Some(plaintext) = item.plaintext.clone() else { return Ok(None) };
        let (sealed, made_for) = self.atomic(|c| {
            let mut group = c.load(group_id)?;
            let sealed = c.encrypt(&mut group, &plaintext)?;
            let made_for = c.members_of(&group, &item.user).iter().map(|m| m.device_key).collect::<Vec<_>>();
            Ok((sealed, made_for))
        })?;
        item.envelope = sealed;
        item.made_for = made_for;
        Ok(Some(()))
    }

    // ---- delivery ----

    /// Sends the outbox in order, per user. Returns the deliveries the hub refused because the user's devices are
    /// other than the group's (4.6): group, user, the user's current devices.
    fn flush(&mut self) -> Result<Vec<(String, String, api::Devices)>> {
        let deliveries = self.deliveries()?;
        let answers = deliveries.send();
        Ok(self.delivered(&deliveries, answers))
    }

    /// What of the outbox can go now, in order per user, each envelope with its device check (4.6); its users are
    /// marked as on their way, so no other flush sends after them meanwhile. Local work only: `Deliveries::send`
    /// talks to the hub, `delivered` takes the answers.
    fn deliveries(&mut self) -> Result<Deliveries> {
        // Saved before anything leaves: a crash after the hub took an envelope must not bring back the ratchet state
        // that made it. A repeat after a crash is harmless, the receiver skips what it has seen.
        if !self.state.outbox.is_empty() {
            self.persist()?;
        }
        let mut items = Vec::new();
        // Users whose envelopes another flush is sending: theirs wait, in their order.
        let mut waiting = self.delivering.clone();
        // One token per delivery while there are any, the oldest first: the hub counts it instead of the address.
        let token_key =
            self.state.token_key.as_deref().and_then(unhex).and_then(|k| <[u8; token::NE]>::try_from(k).ok());
        // Walked in place, so every save in between holds the whole outbox.
        let mut i = 0;
        while i < self.state.outbox.len() {
            let item = &self.state.outbox[i];
            // Nothing goes to a blocked user (4.8): what waits for them stays, in its order, until an unblock.
            if waiting.contains(&item.user) || self.blocked(&item.user) {
                i += 1;
                continue;
            }
            // A lost chat's envelopes wait for its restart, which sends its messages in the new group (4.9); a new
            // chat's wait until the hub has its group (4.2).
            if self.state.chats.get(&item.group).is_some_and(|c| c.lost || c.creating.is_some()) {
                waiting.insert(item.user.clone());
                i += 1;
                continue;
            }
            let Some((group_id, group)) = group_of(&item.group).and_then(|id| self.load(&id).ok().map(|g| (id, g)))
            else {
                self.state.outbox.remove(i);
                self.drop_one("delivery for a group that is gone".into());
                continue;
            };
            let present: Vec<Key> = self.members_of(&group, &item.user).iter().map(|m| m.device_key).collect();
            if !item.made_for.is_empty() && !item.made_for.iter().any(|key| present.contains(key)) {
                let mut item = item.clone();
                match self.renew(&mut item, &group_id) {
                    // Renewed for the devices there now, it goes behind the repair's Welcome and commit, which those
                    // devices need first: in this round still, at the back, and the messages queued for that user
                    // after it go behind it again, so the user gets them in the order sent.
                    Ok(Some(())) => {
                        self.state.outbox.remove(i);
                        let (user, group) = (item.user.clone(), item.group.clone());
                        let (later, rest): (VecDeque<Outgoing>, VecDeque<Outgoing>) = self
                            .state
                            .outbox
                            .split_off(i)
                            .into_iter()
                            .partition(|o| o.user == user && o.group == group && o.plaintext.is_some());
                        self.state.outbox.extend(rest);
                        self.state.outbox.push_back(item);
                        self.state.outbox.extend(later);
                        // Saved before it leaves, like everything sent: the renewed ratchet must not come back.
                        if let Err(error) = self.persist() {
                            self.restore_tokens(&items);
                            return Err(error);
                        }
                    }
                    Ok(None) => {
                        self.state.outbox.remove(i);
                    }
                    // Held for a contact whose new key waits for confirmation: later, in the same order.
                    Err(_) => {
                        waiting.insert(item.user);
                        i += 1;
                    }
                }
                continue;
            }
            // Renewing a message moves its ratchet, not the members: the group's roster as loaded is still the one.
            let target = user_bytes(&item.user).ok().zip(self.devices_hash(&group, &item.user));
            let Some((user, hash)) = target else {
                self.state.outbox.remove(i);
                self.drop_one("delivery to nobody in the group".into());
                continue;
            };
            let mut delivery = Delivery {
                seq: item.seq,
                user: item.user.clone(),
                group: item.group.clone(),
                path: api::deliver_path(&user),
                headers: vec![(api::HEADER_DEVICES.to_owned(), yo_wire::b64(&hash))],
                envelope: item.envelope.clone(),
                token: None,
            };
            if let Some(key) = &token_key {
                while let Some(kept) = self.state.tokens.pop_front() {
                    if let Some(found) = tokens::unkept(&kept, key) {
                        delivery.headers.push(("authorization".to_owned(), found.authorization()));
                        delivery.token = Some(kept);
                        break;
                    }
                }
            }
            items.push(delivery);
            i += 1;
        }
        self.delivering.extend(items.iter().map(|d| d.user.clone()));
        Ok(Deliveries { transport: self.transport.clone(), items, to: None, token_refused: Default::default() })
    }

    fn no_deliveries(&self) -> Deliveries {
        Deliveries { transport: self.transport.clone(), items: Vec::new(), to: None, token_refused: Default::default() }
    }

    /// Tokens taken for deliveries that never left go back to the front of the stash, in their order.
    fn restore_tokens(&mut self, items: &[Delivery]) {
        for kept in items.iter().rev().filter_map(|item| item.token.clone()) {
            self.state.tokens.push_front(kept);
        }
    }

    /// Takes the hub's answers to `Deliveries::send`: what it took leaves the outbox, what it refused for good is
    /// dropped, the rest waits for the next flush. Returns the deliveries refused for other devices (see `flush`).
    fn delivered(
        &mut self,
        deliveries: &Deliveries,
        answers: Vec<Option<Answer>>,
    ) -> Vec<(String, String, api::Devices)> {
        let mut mismatches: Vec<(String, String, api::Devices)> = Vec::new();
        let mut done = BTreeSet::new();
        let mut gone = BTreeSet::new();
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
        for (item, answer) in deliveries.items.iter().zip(answers) {
            self.delivering.remove(&item.user);
            match answer {
                Some(Ok(response)) if response.status == status::ACCEPTED => {
                    done.insert(item.seq);
                }
                Some(Ok(response)) if response.status == status::CONFLICT => {
                    match decode::<api::Devices>(&response.body) {
                        Ok(devices) => {
                            if !mismatches.iter().any(|(g, u, _)| *g == item.group && *u == item.user) {
                                mismatches.push((item.group.clone(), item.user.clone(), devices));
                            }
                        }
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
        mismatches
    }

    /// The hub knows no such user any more (a delivery's `404`): the account was deleted or its name released. Its
    /// chats and what waits for it go; its handles are forgotten (the name may be someone else's now, a stranger to
    /// this device); sending to it is `NotOnYo` from now on.
    fn user_gone(&mut self, user: &str) {
        let Some(contact) = self.state.contacts.get_mut(user) else { return };
        contact.gone = true;
        contact.handles.clear();
        let chats: Vec<String> =
            self.state.chats.iter().filter(|(_, chat)| chat.peer == user).map(|(key, _)| key.clone()).collect();
        for key in chats {
            self.drop_chat(&key);
        }
    }

    /// Flushes, and repairs every group a delivery found out of line with a user's devices, until the outbox moves.
    fn deliver(&mut self) -> Result<()> {
        self.register_pending()?;
        for _ in 0..REPAIR_ROUNDS {
            let mismatches = self.flush()?;
            if mismatches.is_empty() {
                return Ok(());
            }
            self.repair_all(mismatches)?;
        }
        Ok(())
    }

    /// Repairs the groups a delivery found out of line with a user's devices; the outbox is left for the next flush.
    fn repair_all(&mut self, mismatches: Vec<(String, String, api::Devices)>) -> Result<()> {
        for (group, user, devices) in mismatches {
            match self.repair(&group, &user, &devices) {
                Ok(()) => {}
                Err(error) if retryable(&error) || error == Error::Unconfirmed => return Err(error),
                Err(error) => self.drop_one(format!("repair: {error}")),
            }
        }
        Ok(())
    }

    /// Brings a group in line with a user's devices at the hub (spec 4.6): one commit removing the devices the user
    /// no longer has and adding the missing ones. The envelopes waiting for that user stay in the saved outbox, in
    /// their order; the next flush sends each as it is if a device it was made for remains, and encrypts a message
    /// anew in its place otherwise.
    fn repair(&mut self, key: &str, user: &str, devices: &api::Devices) -> Result<()> {
        let group_id = group_of(key).ok_or(Error::Protocol("group id"))?;
        // Pinned outside any transaction: a verified contact's new key stays marked unconfirmed even though the
        // repair stops for it.
        if user != self.state.user_id {
            self.pin(user, devices.account_key, None);
            if self.state.contacts.get(user).is_some_and(|c| c.unconfirmed) {
                return Err(Error::Unconfirmed);
            }
        }
        self.repair_commit(&group_id, user, devices)
    }

    /// The commit of a repair, ordered by the hub. A lost race is caught up and computed again; nothing that talks to
    /// the hub runs in a transaction, so an accepted commit is never rolled back.
    fn repair_commit(&mut self, group_id: &GroupId, user: &str, devices: &api::Devices) -> Result<()> {
        for _ in 0..REPAIR_ROUNDS {
            // A commit of this device still waiting for its answer comes first.
            match self.resolve(group_id)? {
                Committed::Unknown(error) => return Err(Error::Network(error)),
                Committed::Lost(epoch) => self.catch_up(group_id, epoch)?,
                _ => {}
            }
            if !self.atomic(|c| c.stage_repair(group_id, user, devices))? {
                return Ok(());
            }
            match self.resolve(group_id)? {
                // Refused: the chat is lost and restarted with the user's devices as they are (4.9).
                Committed::Accepted | Committed::Refused => return Ok(()),
                Committed::Lost(epoch) => self.catch_up(group_id, epoch)?,
                Committed::Unknown(error) => return Err(Error::Network(error)),
                Committed::Nothing => return Err(Error::Protocol("repair refused")),
            }
        }
        Err(Error::Network("a repair kept losing to other commits".into()))
    }

    /// Stages the repair commit for a user as the group's inflight commit; false when the group already matches.
    fn stage_repair(&mut self, group_id: &GroupId, user: &str, devices: &api::Devices) -> Result<bool> {
        let mut group = self.load(group_id)?;
        let theirs = self.members_of(&group, user);
        let stays = |m: &Member| m.account_key == devices.account_key && devices.device_keys.contains(&m.device_key);
        let remove: Vec<LeafNodeIndex> = theirs.iter().filter(|m| !stays(m)).map(|m| m.index).collect();
        let present: Vec<Key> = theirs.iter().filter(|m| stays(m)).map(|m| m.device_key).collect();
        if user == self.state.user_id {
            // The own account: a new key means this device was replaced; and a device does not remove itself.
            if devices.account_key != self.my_account_key() || remove.contains(&group.own_leaf_index()) {
                return Err(Error::Replaced);
            }
        }
        let key_packages = if devices.device_keys.iter().any(|k| !present.contains(k)) {
            let found = self.lookup(&Handle::user(&user_bytes(user)?), true, present.clone())?;
            if found.account_key != devices.account_key {
                // The user changed again meanwhile: the next delivery brings the newest devices.
                return Err(Error::Network("devices changed during a repair".into()));
            }
            self.checked_key_packages(&found)?
        } else {
            Vec::new()
        };
        if remove.is_empty() && key_packages.is_empty() {
            return Ok(false);
        }
        let bundle = group
            .commit_builder()
            .propose_removals(remove)
            .propose_adds(key_packages)
            .load_psks(self.provider.storage())
            .map_err(mls)?
            .build(self.provider.rand(), self.provider.crypto(), self.signer(), |_| true)
            .map_err(mls)?
            .stage_commit(&self.provider)
            .map_err(mls)?;
        self.prepare(&group, &bundle, Some(user))?;
        Ok(true)
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
            if !self.inbox(response, time, false)?.0 {
                self.inbox_fresh_until = (self.clock)() + INBOX_FRESH_MS;
                return Ok(());
            }
            wait = 0;
        }
    }

    /// The storage fetched once on this device before anything is received: its blocks hold from the first message
    /// on. "Not now" leaves the inbox for later; a hub without storage is left alone.
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
    /// run): envelopes already seen are skipped, an older KeyPackage count is ignored. With `later`, an answer that is
    /// all there is and was all taken in is neither saved nor acknowledged here: its ids are returned, for the caller
    /// to acknowledge once it saved (`sync_end`).
    fn inbox(&mut self, response: Response, requested_at: u64, later: bool) -> Result<(bool, Vec<u64>)> {
        if response.status != status::OK {
            return Err(refused(&response));
        }
        let inbox: api::Inbox = decode(&response.body)?;
        if requested_at > self.key_packages_known_ms {
            self.state.key_packages_left = inbox.key_packages_left;
            self.key_packages_known_ms = requested_at;
        }
        let full = inbox.items.len() >= MAX_INBOX_FETCH;
        let mut done = Vec::new();
        let mut stopped = None;
        for item in inbox.items {
            let hash = sha256(item.envelope.as_slice());
            if !self.state.seen.contains(&hash) {
                let envelope = item.envelope.as_slice();
                match self.fetch_before(envelope).and_then(|()| self.atomic(|c| c.receive(envelope))) {
                    Ok(()) => {}
                    // Not this envelope's fault: it stays at the hub and is read again next time.
                    Err(error) if retryable(&error) => {
                        stopped = Some(error);
                        break;
                    }
                    Err(error) => self.drop_one(error.to_string()),
                }
                self.remember(item.envelope.as_slice());
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

    fn receive(&mut self, sealed: &[u8]) -> Result<()> {
        let bytes = envelope::open(sealed)?;
        let message = MlsMessageIn::tls_deserialize_exact(bytes).map_err(mls)?;
        match message.extract() {
            MlsMessageBodyIn::Welcome(welcome) => self.join(welcome),
            MlsMessageBodyIn::PrivateMessage(private) => self.private(private.into()),
            _ => Err(Error::Protocol("wire format")),
        }
    }

    fn ignore(&mut self, group: [u8; 32]) {
        self.state.ignored.push_back(group);
        if self.state.ignored.len() > 200 {
            self.state.ignored.pop_front();
        }
    }

    /// Someone started a chat with this device, or a repair added it to one (spec 4.2, 4.6).
    fn join(&mut self, welcome: Welcome) -> Result<()> {
        if welcome.ciphersuite() != profile::CIPHERSUITE {
            return Err(Error::Protocol("suite"));
        }
        let staged = match StagedWelcome::new_from_welcome(&self.provider, &profile::join_config(), welcome, None) {
            Ok(staged) => staged,
            // A Welcome for other devices of this account (a repair adding them): not this device's.
            Err(WelcomeError::NoMatchingKeyPackage) => return Ok(()),
            Err(error) => return Err(mls(error)),
        };
        let id: [u8; 32] =
            staged.group_context().group_id().as_slice().try_into().map_err(|_| Error::Protocol("group id"))?;
        if self.state.chats.contains_key(&hex(&id)) {
            return Ok(());
        }
        let mine = self.my_account_key();
        let own_device = self.own_device_key().to_vec();
        let mut peer: Option<Key> = None;
        let mut me = false;
        for member in staged.members() {
            let account_key = identity::member(&member.credential, &member.signature_key)?;
            if account_key == mine {
                me |= member.signature_key == own_device;
                continue;
            }
            match peer {
                None => peer = Some(account_key),
                Some(key) if key == account_key => {}
                // v1: a chat between exactly two accounts.
                Some(_) => return Err(Error::Protocol("more than two accounts")),
            }
        }
        let peer_key = peer.ok_or(Error::Protocol("nobody else"))?;
        let sender = staged
            .members()
            .find(|m| m.index == staged.welcome_sender_index())
            .and_then(|m| identity::claimed(&m.credential))
            .ok_or(Error::Protocol("sender"))?;
        // The Welcome comes from the peer, or from one's own other device.
        if !me || (sender != peer_key && sender != mine) {
            return Err(Error::Protocol("welcome"));
        }
        // Who is it? A contact pinned to this key, or the hub's answer for the key: nobody can start a chat in someone
        // else's name. A key that is no user's current key is a replaced device's (or a forger's): not joined, and
        // remembered (not rolled back), so that what follows in that group is discarded too.
        let known = self.state.contacts.iter().find(|(_, c)| c.account_key == peer_key).map(|(u, _)| u.clone());
        // Someone who wrote first: the hub tells their username too, so the app can show who it is.
        let (peer, username) = match known {
            Some(user) => (user, None),
            None => match self.lookup_named(&Handle::account_key(&peer_key), false, Vec::new()) {
                Ok((found, username)) if found.account_key == peer_key => (hex(&found.user_id), username),
                Ok(_) | Err(Error::NotOnYo) => {
                    self.ignore(id);
                    return Ok(());
                }
                Err(error) => return Err(error),
            },
        };
        // A blocked user's chat is joined quietly: nothing of it is shown while the block holds, and it works again
        // after an unblock (ignored, it would stay dead for good: the sender keeps writing in it).
        let blocked = self.blocked(&peer);
        self.pin(&peer, peer_key, username);
        let group = staged.into_group(&self.provider).map_err(mls)?;
        if group.ciphersuite() != profile::CIPHERSUITE {
            return Err(Error::Protocol("group"));
        }
        self.state.chats.insert(
            hex(&id),
            Chat {
                peer: peer.clone(),
                joined_epoch: group.epoch().as_u64(),
                last_update_ms: (self.clock)(),
                sent_since_update: 0,
                needs_update: true,
                inflight: None,
                sent: VecDeque::new(),
                last_seen_ms: 0,
                lost: false,
                restarted: false,
                profile_due: false,
                creating: None,
            },
        );
        if !blocked {
            self.push_event(Event::NewChat { user: peer });
        }
        Ok(())
    }

    fn private(&mut self, message: ProtocolMessage) -> Result<()> {
        let group_id = message.group_id().clone();
        let key = group_key(&group_id);
        let Some(chat) = self.state.chats.get(&key) else {
            // A group whose Welcome came from a replaced identity: discarded quietly, as expected.
            if self.state.ignored.iter().any(|g| g.as_slice() == group_id.as_slice()) {
                return Ok(());
            }
            return Err(Error::Protocol("unknown group"));
        };
        let (peer, inflight, joined) = (chat.peer.clone(), chat.inflight.is_some(), chat.joined_epoch);
        let message_epoch = message.epoch().as_u64();
        // What was sent before this device joined (a repair added it after): not for it.
        if message_epoch < joined {
            return Ok(());
        }
        // From an epoch this device has not reached: its commits were fetched before (`fetch_before`), unless the
        // chat is lost (restarted, 4.9). A fetched envelope does not fetch again.
        if message_epoch > self.load(&group_id)?.epoch().as_u64() {
            if self.fetching {
                return Err(Error::Protocol("a fetched envelope from a later epoch"));
            }
            if self.state.chats.get(&key).is_some_and(|c| c.lost) {
                return Ok(());
            }
        }
        let mut group = self.load(&group_id)?;
        let before = Self::members(&group);
        let group_epoch = group.epoch().as_u64();
        let processed = group
            .process_message(&self.provider, message)
            .map_err(|e| Error::Mls(format!("{e:?} (message epoch {message_epoch}, group epoch {group_epoch})")))?;
        let sender = identity::claimed(processed.credential()).ok_or(Error::Protocol("sender"))?;
        let from_me = sender == self.my_account_key();
        match processed.into_content() {
            // This device's own message, come back by the own account: already known.
            ProcessedMessageContent::OwnPrivateMessage | ProcessedMessageContent::OwnPendingCommit => Ok(()),
            ProcessedMessageContent::ApplicationMessage(application) => {
                let bytes = application.into_bytes();
                let message = Message::decode(&bytes)?;
                if self.blocked(&peer) {
                    return Ok(());
                }
                if !from_me {
                    // Sent again after a restart (4.9) and seen already: once is enough. A sender's messages differ
                    // in their time at least, so only a copy is the same.
                    let id = sha256(&[peer.as_bytes(), &bytes].concat());
                    if self.state.received.contains(&id) {
                        return Ok(());
                    }
                    self.state.received.push_back(id);
                    if self.state.received.len() > SEEN {
                        self.state.received.pop_front();
                    }
                    if let Some(chat) = self.state.chats.get_mut(&key) {
                        chat.last_seen_ms = chat.last_seen_ms.max(message.sent_at_ms);
                        // The peer answers in a chat this device restarted: it has the restart.
                        chat.restarted = false;
                    }
                }
                if let Content::Restart { .. } = message.content {
                    if !from_me {
                        self.restarted_by_peer(&group_id, &peer)?;
                    }
                    return Ok(());
                }
                let content = match message.content {
                    Content::Photo(media) if self.media_keys_forgotten => Content::Photo(media::without_key(media)),
                    Content::Video { media, duration_ms } if self.media_keys_forgotten => {
                        Content::Video { media: media::without_key(media), duration_ms }
                    }
                    content => content,
                };
                let event = match content {
                    Content::Profile { name } if !from_me => {
                        if let Some(contact) = self.state.contacts.get_mut(&peer) {
                            contact.name = Some(name.clone());
                        }
                        Event::Profile { user: peer, name }
                    }
                    Content::Profile { .. } => return Ok(()),
                    content if from_me => Event::SentElsewhere { user: peer, sent_at_ms: message.sent_at_ms, content },
                    content => Event::Message { user: peer, sent_at_ms: message.sent_at_ms, content },
                };
                self.push_event(event);
                Ok(())
            }
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                let replacement = self.check_commit(&staged, &sender, &before, &peer)?;
                // Checked: if this device's own commit was waiting for an answer, the hub took this one instead.
                if inflight {
                    group.clear_pending_commit(self.provider.storage()).map_err(mls)?;
                    if let Some(chat) = self.state.chats.get_mut(&key) {
                        chat.inflight = None;
                    }
                }
                group.merge_staged_commit(&self.provider, *staged).map_err(mls)?;
                if !group.is_active() {
                    // This device was removed: the chat is over for it.
                    self.drop_chat(&key);
                    return Ok(());
                }
                if let Some(account_key) = replacement {
                    self.pin(&peer, account_key, None);
                }
                Ok(())
            }
            _ => Err(Error::Protocol("proposals are not used")),
        }
    }

    /// A commit may change membership and keys only (spec 4.3): adds of an account key already in the group, or of the
    /// peer's new key that replaces all devices of its old one (the hub must say it is the peer's current key); a new
    /// leaf only with its sender's key. Returns the peer's new account key, if it is replaced.
    fn check_commit(
        &mut self,
        staged: &StagedCommit,
        sender: &Key,
        before: &[Member],
        peer: &str,
    ) -> Result<Option<Key>> {
        let allowed_kind =
            |p: &QueuedProposal| matches!(p.proposal(), Proposal::Add(_) | Proposal::Update(_) | Proposal::Remove(_));
        if !staged.queued_proposals().all(allowed_kind) {
            return Err(Error::Protocol("proposal type not allowed"));
        }
        let keys_before: BTreeSet<Key> = before.iter().map(|m| m.account_key).collect();
        let removed: BTreeSet<LeafNodeIndex> =
            staged.remove_proposals().map(|r| r.remove_proposal().removed()).collect();
        let mut replacement = None;
        for add in staged.add_proposals() {
            let leaf = add.add_proposal().key_package().leaf_node();
            let account_key = identity::member(leaf.credential(), leaf.signature_key().as_slice())?;
            if keys_before.contains(&account_key) || replacement == Some(account_key) {
                continue;
            }
            if replacement.is_some() {
                return Err(Error::Protocol("added a stranger"));
            }
            let found = self.lookup(&Handle::account_key(&account_key), false, Vec::new())?;
            if hex(&found.user_id) != peer || found.account_key != account_key {
                return Err(Error::Protocol("added a stranger"));
            }
            replacement = Some(account_key);
        }
        if let Some(new_key) = replacement {
            // The old key's devices all go, and none comes back: one user, one key in a chat.
            let mine = self.my_account_key();
            if before.iter().any(|m| m.account_key != mine && !removed.contains(&m.index)) {
                return Err(Error::Protocol("replacement keeps the old key"));
            }
            for add in staged.add_proposals() {
                let leaf = add.add_proposal().key_package().leaf_node();
                let account_key = identity::member(leaf.credential(), leaf.signature_key().as_slice())?;
                if account_key != new_key && account_key != mine {
                    return Err(Error::Protocol("replacement brings back the old key"));
                }
            }
        }
        if let Some(leaf) = staged.update_path_leaf_node() {
            let account_key = identity::member(leaf.credential(), leaf.signature_key().as_slice())?;
            if account_key != *sender {
                return Err(Error::Protocol("leaf of someone else"));
            }
        }
        Ok(replacement)
    }

    fn refill(&mut self) -> Result<()> {
        let refill = match self.state.refill.clone() {
            // One made before whose upload failed: the same again, not another batch of private keys per try.
            Some(refill) => refill,
            None => {
                let left = self.state.key_packages_left;
                let last_resort = (self.clock)().saturating_sub(self.state.last_resort_at) >= LAST_RESORT_EVERY_MS;
                if left >= KEY_PACKAGE_REFILL_BELOW && !last_resort {
                    return Ok(());
                }
                let fresh = if left < KEY_PACKAGE_REFILL_BELOW { KEY_PACKAGE_TARGET - left } else { 0 };
                // Made and saved before they leave: a lost answer must not cost the private keys of packages the hub
                // took.
                let refill = self.atomic(|c| {
                    let key_packages =
                        (0..fresh).map(|_| c.key_package(false).map(Into::into)).collect::<Result<Vec<_>>>()?;
                    let last_resort_package = if last_resort { Some(c.key_package(true)?.into()) } else { None };
                    let body = encode(&api::KeyPackages { key_packages, last_resort: last_resort_package });
                    let refill = state::Refill { body, fresh, last_resort };
                    c.state.refill = Some(refill.clone());
                    Ok(refill)
                })?;
                self.persist()?;
                refill
            }
        };
        let response = self.signed("PUT", api::KEY_PACKAGES, &refill.body)?;
        // `409` to the same upload again: the hub took it before and its answer was lost (it refuses a KeyPackage it
        // has seen), or it holds as many as it keeps. Either way this one is done.
        if response.status == status::BAD_REQUEST {
            // Refused as it is (its packages past their lifetime, after a long time offline): made anew next time.
            // Their keys stay, for a Welcome made with one the hub had already handed out.
            self.state.refill = None;
            return Err(refused(&response));
        }
        if response.status != status::NO_CONTENT && response.status != status::CONFLICT {
            return Err(refused(&response));
        }
        self.state.refill = None;
        self.state.key_packages_left = self.state.key_packages_left.saturating_add(refill.fresh);
        self.key_packages_known_ms = self.last_request_ms;
        if refill.last_resort {
            // The hub hands out the new one from now on: the ones before (also one made for an upload that failed)
            // are retired now, not when they were replaced here, and kept while a Welcome made with them may wait.
            let now = (self.clock)();
            self.state.last_resort_at = now;
            let newest = self.state.key_packages.iter().rposition(|k| k.last_resort);
            for (i, older) in self.state.key_packages.iter_mut().enumerate() {
                if older.last_resort && older.retired_at.is_none() && Some(i) != newest {
                    older.retired_at = Some(now);
                }
            }
        }
        Ok(())
    }

    /// Keeps the stash of delivery tokens (spec 6.10): below `TOKEN_REFILL_BELOW`, or after the hub refused one,
    /// the hub's key is read from its directory (a new key: the stash is dropped) and a batch issued, up to
    /// `TOKEN_TARGET`. Quietly: whatever fails (a hub without tokens, its daily limit, no network) leaves deliveries
    /// as they were, counted by address, and the next try is an hour later.
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

    /// Sends a message: starts a chat if there is none, updates it if due (spec 4).
    pub fn send(&mut self, to: &str, content: Content) -> Result<Vec<Event>> {
        let result = self.send_inner(to, content);
        self.finish(result)
    }

    fn send_inner(&mut self, to: &str, content: Content) -> Result<()> {
        let handle = Handle::parse(to)?;
        if self.is_me(&handle) {
            return Err(Error::Protocol("sending to oneself"));
        }
        if let Content::Unknown(_) = content {
            return Err(Error::Protocol("unknown content"));
        }
        // Receive first: a commit waiting in the inbox would otherwise leave this message in an epoch the peer may
        // already have left. Offline, the message still goes into the outbox.
        match self.deliver().and_then(|()| self.receive_all(0)).and_then(|()| self.restart_lost()) {
            Ok(()) => {}
            Err(error) if retryable(&error) => {}
            Err(error) => return Err(error),
        }
        let user = self.send_target(&handle)?;
        self.check_sendable(user.as_deref())?;
        let (peer, group) = match user.as_ref().and_then(|u| self.chat_with(u).map(|g| (u.clone(), g))) {
            Some(chat) => chat,
            None => self.start_chat(&handle)?,
        };
        self.ensure_fresh(&group)?;
        // Lost while it was brought up to date (a commit gone from the hub): restarted, the message goes there.
        let group = if self.state.chats.get(&group_key(&group)).is_some_and(|c| c.lost) {
            self.restart_lost()?;
            self.chat_with(&peer).ok_or(Error::Internal("restarted chat"))?
        } else {
            group
        };
        // A name not yet told goes first.
        self.send_profiles();
        // Saved with the message in it, or not made at all (spec 4.7).
        self.atomic(|c| c.send_in(&group, content, true).and_then(|()| c.persist()))?;
        match self.deliver() {
            Ok(()) | Err(Error::Network(_)) => {}
            Err(error) => return Err(error),
        }
        // The hub knows the user no more: the message went nowhere.
        self.check_sendable(Some(&peer))
    }

    /// The user a send to `handle` goes to: a user id as it is; a username as the hub says now (who it is only,
    /// without spending a KeyPackage), never as this device knew it, since a name may have changed hands since (3.3).
    /// So someone who wrote first, known by user id only, is found by name too, in the chat there is. `None`: no
    /// contacts yet, the new chat's lookup asks.
    pub(crate) fn send_target(&mut self, handle: &Handle) -> Result<Option<String>> {
        if handle.kind == KIND_USER {
            return Ok(Some(hex(handle.value.as_slice())));
        }
        if self.state.contacts.is_empty() {
            return Ok(None);
        }
        let (found, username) = self.lookup_named(handle, false, Vec::new())?;
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
    /// takes the answers. Only for a chat in use that needs nothing first (no commit open or due, the contact neither
    /// blocked nor waiting for confirmation), and while the inbox is up to date: a long wait for it is under way
    /// (`sync_begin`), which brings what arrives at once, or it was emptied a moment ago. Then the send need not
    /// receive first (spec 4.7): a commit crossing it is what the peer's past epochs are kept for. What the hub refuses
    /// for other devices is repaired in `send_end`. `None`, with nothing changed: `send` it instead.
    pub fn send_begin(&mut self, to: &str, content: Content) -> Option<Deliveries> {
        let handle = Handle::parse(to).ok()?;
        // By user id only: a name is asked of the hub (`send_target`).
        if handle.kind != KIND_USER
            || self.is_me(&handle)
            || matches!(content, Content::Unknown(_))
            || (self.clock)() > self.inbox_fresh_until
        {
            return None;
        }
        let user = self.known_user(&handle)?;
        let contact = self.state.contacts.get(&user)?;
        if contact.blocked || contact.unconfirmed || contact.gone {
            return None;
        }
        let group = self.chat_with(&user)?;
        if self.state.chats.get(&group_key(&group))?.inflight.is_some() || self.update_due(&group) {
            return None;
        }
        // Saved with the message in it, or not made at all: `send` then fails on the save the same way.
        self.atomic(|c| c.send_in(&group, content, true).and_then(|()| c.persist())).ok()?;
        let mut deliveries = self.deliveries().unwrap_or_else(|_| self.no_deliveries());
        deliveries.to = Some(user);
        Some(deliveries)
    }

    /// The outbox, without the client, like `send_begin` without a message: for sending again what the hub did not take
    /// yet (its limit per sender, or no network). Ended with `send_end`.
    pub fn deliver_begin(&mut self) -> Result<Deliveries> {
        // As `deliver`: a new chat's group the hub has not answered for yet first, so its Welcome can go.
        self.register_pending()?;
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
        let mismatches = self.delivered(deliveries, answers);
        let result = if !mismatches.is_empty() {
            self.repair_all(mismatches).and_then(|()| self.deliver())
        } else {
            if all_taken && self.check_sendable(deliveries.to.as_deref()).is_ok() {
                match self.deliveries() {
                    Ok(mut more) if !more.is_empty() => {
                        more.to = deliveries.to.clone();
                        return Sent::More(more);
                    }
                    Ok(_) => Ok(()),
                    Err(error) => Err(error),
                }
            } else {
                Ok(())
            }
        };
        // As `send`: offline, the message waits in the outbox; to a user the hub no longer knows, it went nowhere.
        let result = match result {
            Ok(()) | Err(Error::Network(_)) => self.check_sendable(deliveries.to.as_deref()),
            Err(error) => Err(error),
        };
        Sent::Done(self.finish(result))
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
    /// them at the hub, refilling KeyPackages, delivering and writing the storage come after, off the way of the
    /// events: `after_sync`, else the next `sync_begin`. The hub is acknowledged only what was saved first.
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
    /// before), and for the app (not a guest) restarts lost chats, refills KeyPackages and delivery tokens, delivers
    /// the outbox, writes the storage. Without it the next `sync_begin` does it first. What it brings (rarely an
    /// event) is saved and comes with the next call.
    pub fn after_sync(&mut self) -> Result<()> {
        let result = self.after_sync_inner();
        let saved = self.persist();
        result.and(saved)
    }

    fn after_sync_inner(&mut self) -> Result<()> {
        self.acknowledge_inbox()?;
        if !std::mem::take(&mut self.after_due) {
            return Ok(());
        }
        self.restart_lost()?;
        self.refill()?;
        self.deliver()?;
        self.sync_storage_quietly()?;
        self.refill_tokens();
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

    /// What the app runs when it comes to the foreground: receive, refill KeyPackages, update due chats.
    pub fn maintenance(&mut self) -> Result<Vec<Event>> {
        let result = self.maintenance_inner();
        self.finish_receiving(result)
    }

    fn maintenance_inner(&mut self) -> Result<()> {
        // All of the work after a sync is done here.
        self.after_due = false;
        self.deliver()?;
        self.receive_all(0)?;
        self.restart_lost()?;
        self.refill()?;
        self.refill_tokens();
        // A blocked peer's chats get no update: nothing goes to them (4.8).
        let groups: Vec<GroupId> = self
            .state
            .chats
            .iter()
            .filter(|(_, chat)| !self.blocked(&chat.peer))
            .filter_map(|(key, _)| group_of(key))
            .collect();
        // One chat's trouble does not keep the others from their updates.
        let mut first_error = None;
        for group in groups {
            if let Err(error) = self.ensure_fresh(&group) {
                first_error.get_or_insert(error);
            }
        }
        self.send_profiles();
        self.forget_key_packages();
        if let Err(error) = self.deliver().and_then(|()| self.sync_storage_quietly()) {
            first_error.get_or_insert(error);
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Sets the name contacts see, and tells every chat.
    pub fn set_name(&mut self, name: &str) -> Result<Vec<Event>> {
        let result = (|| {
            Message { sent_at_ms: 0, content: Content::Profile { name: name.to_owned() } }.encode()?;
            self.state.name = Some(name.to_owned());
            for chat in self.state.chats.values_mut() {
                chat.profile_due = true;
            }
            self.send_profiles();
            match self.deliver() {
                // The name is set; that contact has it once the new key is confirmed.
                Err(Error::Unconfirmed) => Ok(()),
                other => other,
            }
        })();
        self.finish(result)
    }

    /// Tells the name to every chat it is due in that can take a message now: not to a blocked peer (4.8), nor one
    /// whose new key waits for confirmation (3.4), nor in a group whose commit waits for its answer or that is lost.
    /// The others keep it due, for the next send or `maintenance`.
    fn send_profiles(&mut self) {
        let Some(name) = self.state.name.clone() else { return };
        let due: Vec<String> = self
            .state
            .chats
            .iter()
            .filter(|(_, chat)| chat.profile_due && chat.inflight.is_none() && !chat.lost && !self.blocked(&chat.peer))
            .map(|(key, _)| key.clone())
            .collect();
        for key in due {
            let Some(group) = group_of(&key) else { continue };
            let sent = self.atomic(|c| {
                c.send_in(&group, Content::Profile { name: name.clone() }, false)?;
                c.state.chats.get_mut(&key).ok_or(Error::Protocol("unknown group"))?.profile_due = false;
                Ok(())
            });
            if let Err(error) = sent {
                if error != Error::Unconfirmed {
                    self.note(format!("name: {error}"));
                }
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
            })
            .collect()
    }

    /// The chats with a contact (user id), newest restart first as they are kept; with what MLS says of each.
    pub fn chats_with(&self, user: &str) -> Vec<ChatInfo> {
        self.state
            .chats
            .iter()
            .filter(|(_, chat)| chat.peer == user)
            .map(|(id, chat)| {
                let group = group_of(id).and_then(|id| self.load(&id).ok());
                ChatInfo {
                    group: id.clone(),
                    ciphersuite: format!("{:?}", profile::CIPHERSUITE),
                    epoch: group.as_ref().map_or(0, |g| g.epoch().as_u64()),
                    joined_epoch: chat.joined_epoch,
                    members: group.as_ref().map_or(0, |g| g.members().count() as u32),
                    last_update_ms: chat.last_update_ms,
                    updates: !chat.lost,
                    restarted: chat.restarted,
                    lost: chat.lost,
                }
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
    /// it before the first Yo: `whois_begin` (locked) signs the lookup (who it is only, no KeyPackage),
    /// `Whois::send` (unlocked) asks. Nothing is kept: the name is not a contact until a chat.
    pub fn whois_begin(&mut self, who: &str) -> Result<Whois> {
        let handle = Handle::parse(who)?;
        if self.is_me(&handle) {
            return Err(Error::Protocol("oneself"));
        }
        let body = encode(&api::Lookup { handle, key_packages: api::LOOKUP_USERNAME, except: Vec::new() });
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

    /// The user a handle names: known to this device, else asked of the hub (who it is only, no KeyPackage); with
    /// the handle to remember for it then, the username as its owner writes it if the hub tells.
    fn user_named(&mut self, handle: &Handle) -> Result<(String, Option<String>)> {
        if self.is_me(handle) {
            return Err(Error::Protocol("oneself"));
        }
        let (user, shown) = match self.known_user(handle) {
            Some(user) => (user, None),
            None => {
                let (found, username) = self.lookup_named(handle, false, Vec::new())?;
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
    /// nothing from them is shown, no chat with them is joined, nothing is sent to them (spec 4.8).
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
    /// restart) is not shown either.
    pub(crate) fn apply_block(&mut self, user: &str, blocked: bool) {
        self.state.contacts.entry(user.to_owned()).or_insert_with(Client::unknown_contact).blocked = blocked;
        if blocked {
            self.state
                .events
                .retain(|e| !matches!(e, StoredEvent::Message { user: from, own: false, .. } if from == user));
        }
    }

    /// A contact known by its user id only, no key pinned yet.
    pub(crate) fn unknown_contact() -> Contact {
        Contact {
            account_key: [0; 32],
            name: None,
            verified: false,
            unconfirmed: false,
            blocked: false,
            handles: Vec::new(),
            gone: false,
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
