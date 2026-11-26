//! The Swift and Kotlin API: a thin UniFFI layer over `yo_core`. Client operations may block on HTTP or SQLite.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use yo_core::backup::{Door, PasskeyDoor};
use yo_core::connect::ConnectionInfo;
use yo_core::groups::GroupView;
use yo_core::security::SecurityStatus;
use yo_core::shared;
use yo_core::statedb::{DbStore, Loaded};
use yo_core::storage::FriendInfo;
use yo_core::{Client, ContactInfo, Content, Event, SessionInfo};

uniffi::setup_scaffolding!();

mod android;
#[cfg(test)]
mod attestation_tests;
mod keychain;
mod list;
mod product;
pub use keychain::*;
pub use list::*;
pub use product::*;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum YoError {
    #[error("network: {message}")]
    Network { message: String },
    #[error("refused: {status}")]
    Refused { status: u16 },
    /// The phone's date and time are off beyond what the hub's time corrects (spec 4.1), or TLS refused the hub's
    /// certificate for its dates: the user checks them. Nothing was lost.
    #[error("the phone's clock is off")]
    Clock,
    #[error("not on Yo")]
    NotOnYo,
    #[error("blocked")]
    Blocked,
    /// A Yo to a group with no other member left: nobody to send it to (leave it instead).
    #[error("the group is empty")]
    GroupEmpty,
    /// The contact's app is too old: the Yo waits in the outbox until it is updated.
    #[error("the contact must update Yo")]
    Outdated,
    /// A verified contact's key changed: `confirm_identity` first.
    #[error("identity not confirmed")]
    Unconfirmed,
    #[error("this device was replaced")]
    Replaced,
    /// The account has the most devices (multi-device): register again with `replace` naming one of these.
    #[error("the account has the most devices")]
    DeviceLimit { devices: Vec<YoDevice> },
    #[error("protocol: {message}")]
    Protocol { message: String },
    #[error("state")]
    State,
    /// The state could not be saved: nothing that needed it left.
    #[error("save: {message}")]
    Save { message: String },
}

/// What a callback (`StateLock::lock`) threw, if it was no `YoError`.
impl From<uniffi::UnexpectedUniFFICallbackError> for YoError {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        YoError::Save { message: error.reason }
    }
}

impl From<yo_core::Error> for YoError {
    fn from(error: yo_core::Error) -> Self {
        use yo_core::Error as E;
        match error {
            E::Network(message) if message.starts_with(yo_core::net::CLOCK_FAILURE) => YoError::Clock,
            E::Network(message) => YoError::Network { message },
            E::Clock => YoError::Clock,
            E::Refused(status) => YoError::Refused { status },
            E::NotOnYo => YoError::NotOnYo,
            E::Blocked => YoError::Blocked,
            E::GroupEmpty => YoError::GroupEmpty,
            E::Outdated => YoError::Outdated,
            E::Unconfirmed => YoError::Unconfirmed,
            E::Replaced => YoError::Replaced,
            E::DeviceLimit(devices) => {
                YoError::DeviceLimit { devices: devices.into_iter().map(YoDevice::from).collect() }
            }
            E::State => YoError::State,
            E::Save(message) => YoError::Save { message },
            other => YoError::Protocol { message: other.to_string() },
        }
    }
}

fn protocol(what: &str) -> YoError {
    YoError::Protocol { message: what.into() }
}

#[derive(uniffi::Record)]
pub struct SupportMessage {
    pub id: u64,
    pub from_team: bool,
    pub text: String,
    pub created_at: u64,
    pub read_at: u64,
}

/// A thread another device of this account showed (`take_seen`): its notifications go here too.
#[derive(uniffi::Record)]
pub struct SeenThread {
    /// User id (32 hex digits) or group id (64 hex digits).
    pub thread: String,
    /// Shown through the message sent at this time (ms since 1970): what its notifications' ids end in.
    pub through_ms: u64,
}

/// One of this account's devices (`own_devices`): its number, when it registered and was last seen (ms since 1970,
/// to the hour), and the app it named ("ios/202611241344", empty if none).
#[derive(Debug, uniffi::Record)]
pub struct YoDevice {
    pub number: u8,
    pub created_at_ms: u64,
    pub last_seen_ms: u64,
    pub app: String,
}

impl From<yo_core::OwnDevice> for YoDevice {
    fn from(d: yo_core::OwnDevice) -> Self {
        YoDevice {
            number: d.number,
            created_at_ms: d.created_at,
            last_seen_ms: d.last_seen_at,
            app: String::from_utf8_lossy(d.app.as_slice()).into_owned(),
        }
    }
}

#[derive(uniffi::Record)]
pub struct SupportThread {
    pub messages: Vec<SupportMessage>,
    pub unread: u32,
}

#[derive(uniffi::Record)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// An HTTP answer, or `error` when there was none.
#[derive(uniffi::Record)]
pub struct HttpResult {
    pub status: u16,
    pub body: Vec<u8>,
    /// No answer: why. `clock: …` (`yo_core::net::CLOCK_FAILURE`) when TLS refused the hub's certificate for its
    /// dates, so the app can say to check the phone's date and time.
    pub error: Option<String>,
    /// The answer's headers; only the name check reads one (`Yo-Name-Min`). Empty from an app that passes none.
    #[uniffi(default = [])]
    pub headers: Vec<Header>,
}

/// HTTP to the hub, implemented by the app (URLSession, OkHttp). `path` is relative to the hub's base URL.
#[uniffi::export(with_foreign)]
pub trait HttpTransport: Send + Sync {
    fn request(&self, method: String, path: String, headers: Vec<Header>, body: Vec<u8>) -> HttpResult;
    /// The status of the hub's answer to a request that went wrapped (`use_hub`): the app sees only `200` outside.
    fn opened(&self, method: String, path: String, status: u16);
}

/// The turn for writing the shared `StateDb` between an app and its extensions, implemented by the app (iOS: a
/// file lock that expiring background time takes back). Taken around each step that may write, never for a read; the
/// database itself is locked only for a step's final compare-and-swap, never across a request.
#[uniffi::export(with_foreign)]
pub trait StateLock: Send + Sync {
    /// Takes the turn. Refused background time returns an error without a turn; `unlock` follows only a success.
    fn lock(&self) -> Result<(), YoError>;
    /// Whether the turn is still this process's, asked right before each write.
    fn held(&self) -> bool;
    /// Gives it back. `saved`: the state was written in the turn (the other processes may want to look).
    fn unlock(&self, saved: bool);
}

/// The turn held, given back when dropped (also when the step panics), saying whether the step wrote.
struct Turn<'a> {
    lock: &'a dyn StateLock,
    store: Option<Arc<DbStore>>,
}

impl<'a> Turn<'a> {
    fn take(lock: &'a dyn StateLock, store: Option<Arc<DbStore>>) -> Result<Self, YoError> {
        lock.lock()?;
        Ok(Turn { lock, store })
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        // Taken back early: the unwritten step is dropped and the next one starts again from the database.
        if let Some(store) = self.store.as_ref().filter(|_| !self.lock.held()) {
            store.took(None);
        }
        self.lock.unlock(self.store.as_ref().is_some_and(|store| store.take_saved()));
    }
}

/// Around each access to a `StateDb` (iOS: background time, so the app is never suspended inside SQLite with a lock on
/// the shared database, 0xdead10cc). `begin` false refuses the access; `end` follows every accepted one, its
/// connection closed.
#[uniffi::export(with_foreign)]
pub trait StateAccess: Send + Sync {
    fn begin(&self) -> bool;
    fn end(&self);
}

struct Access(Arc<dyn StateAccess>);

impl yo_core::statedb::Access for Access {
    fn begin(&self) -> bool {
        self.0.begin()
    }
    fn end(&self) {
        self.0.end()
    }
}

/// Receives a fixed error kind, suitable for counted diagnostics.
#[uniffi::export(with_foreign)]
pub trait StateDiagnostics: Send + Sync {
    fn failure(&self, kind: String);
}

/// The account's state, one SQLite database (WAL) the app and its extensions share, opened by each process.
#[derive(uniffi::Object)]
pub struct StateDb(Arc<yo_core::statedb::StateDb>);

#[uniffi::export]
impl StateDb {
    /// Opens the current database, creating an empty schema if needed.
    #[uniffi::constructor]
    pub fn open_fresh(
        path: String,
        state_key: Vec<u8>,
        diagnostics: Arc<dyn StateDiagnostics>,
    ) -> Result<Arc<Self>, YoError> {
        let report: yo_core::statedb::Report = Arc::new(move |kind| diagnostics.failure(kind));
        let db = yo_core::statedb::StateDb::open_fresh(path, key_of(state_key)?, report)?;
        Ok(Arc::new(Self(Arc::new(db))))
    }

    /// Every later access to this database runs within `access`.
    pub fn set_access(&self, access: Arc<dyn StateAccess>) {
        self.0.set_access(Some(Arc::new(Access(access))));
    }

    /// Whether there is a state (an account on this phone).
    pub fn has_state(&self) -> Result<bool, YoError> {
        Ok(self.0.word()?.is_some())
    }

    /// Whether the state signed in (its registration answered): an old install's migration is done only then.
    pub fn registered(&self) -> Result<bool, YoError> {
        Ok(self.0.registered()?)
    }

    /// Signed out: the state goes. Every client on it is closed at its next step.
    pub fn clear(&self) -> Result<(), YoError> {
        Ok(self.0.clear()?)
    }
}

/// A client on a `StateDb`: its store, and the turn its writing steps take when processes share it.
struct Shared {
    store: Arc<DbStore>,
    lock: Option<Arc<dyn StateLock>>,
}

impl Shared {
    fn new(db: &StateDb, word: Option<String>, lock: Option<Arc<dyn StateLock>>) -> Self {
        let held = lock.clone().map(|lock| Box::new(move || lock.held()) as Box<dyn Fn() -> bool + Send + Sync>);
        Shared { store: Arc::new(DbStore::new(db.0.clone(), word, held)), lock }
    }
}

/// The turn for a step that writes from the start (opening, registering), before its store is known.
fn first_turn(lock: &Option<Arc<dyn StateLock>>) -> Result<Option<Turn<'_>>, YoError> {
    lock.as_deref().map(|lock| Turn::take(lock, None)).transpose()
}

/// The hub the app talks to (`use_hub`): over Oblivious HTTP to its pinned key (spec 4.1) unless direct.
static HUB: Mutex<Option<(String, bool)>> = Mutex::new(None);

/// Sets the hub's address. Every later request to a hub with a pinned OHTTP key (`yo_core::ohttp::PINNED`) goes
/// wrapped to `/v1/ohttp`, never falling back to direct requests; `direct` (debug builds only) sends them as before.
/// Before it is called, or for a hub without a pinned key, requests go direct.
#[uniffi::export]
pub fn use_hub(url: String, direct: bool) {
    *HUB.lock().unwrap_or_else(PoisonError::into_inner) = Some((url, direct));
}

/// The app's transport as the core's: wrapped in Oblivious HTTP for a pinned hub (`use_hub`), else direct.
fn wire(transport: Arc<dyn HttpTransport>) -> Arc<dyn yo_core::Transport> {
    let hub = HUB.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let keys = match hub {
        Some((url, false)) => yo_core::ohttp::pinned(&url),
        _ => Vec::new(),
    };
    if keys.is_empty() {
        return Arc::new(Http(transport));
    }
    let line = |anonymous| -> Arc<dyn yo_core::Transport> { Arc::new(Line { app: transport.clone(), anonymous }) };
    Arc::new(Wrapped { app: transport.clone(), ohttp: yo_core::ohttp::Ohttp::new(line(false), line(true), keys) })
}

/// On a wrapped request (`POST /v1/ohttp`) only, for the app and never sent: which connection it takes
/// (`signed`/`anonymous`, spec 1.1) and the request inside (`METHOD /path`), for the app's own timeouts and marks.
pub const HEADER_LINE: &str = "yo-ohttp-line";
pub const HEADER_INNER: &str = "yo-ohttp-inner";
const OHTTP_PATH: &str = "/v1/ohttp";

std::thread_local! {
    /// The request `Wrapped` is sending on this thread, for its line's hint.
    static INNER: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

struct Wrapped {
    app: Arc<dyn HttpTransport>,
    ohttp: yo_core::ohttp::Ohttp,
}

impl yo_core::Transport for Wrapped {
    fn authenticated(&self) -> bool {
        yo_core::Transport::authenticated(&self.ohttp)
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<yo_core::Response, String> {
        self.request_headers(method, path, headers, body).map(|(response, _)| response)
    }

    fn request_headers(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(yo_core::Response, Vec<(String, String)>), String> {
        INNER.with(|inner| *inner.borrow_mut() = format!("{method} {path}"));
        let answer = yo_core::Transport::request_headers(&self.ohttp, method, path, headers, body);
        INNER.with(|inner| inner.borrow_mut().clear());
        // The hub's own answer, for the app as a direct one (426: this build is too old).
        if let Ok((response, _)) = &answer {
            self.app.opened(method.to_owned(), path.to_owned(), response.status);
        }
        answer
    }
}

/// One line of Oblivious HTTP over the app's transport, its connection named for the app (`HEADER_LINE`).
struct Line {
    app: Arc<dyn HttpTransport>,
    anonymous: bool,
}

impl yo_core::Transport for Line {
    fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<yo_core::Response, String> {
        let mut headers = headers.to_vec();
        if path == OHTTP_PATH {
            let line = if self.anonymous { "anonymous" } else { "signed" };
            headers.push((HEADER_LINE.to_owned(), line.to_owned()));
            headers.push((HEADER_INNER.to_owned(), INNER.with(|inner| inner.borrow().clone())));
        }
        yo_core::Transport::request(&Http(self.app.clone()), method, path, &headers, body)
    }
}

struct Http(Arc<dyn HttpTransport>);

impl yo_core::Transport for Http {
    fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<yo_core::Response, String> {
        self.request_headers(method, path, headers, body).map(|(response, _)| response)
    }

    fn request_headers(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(yo_core::Response, Vec<(String, String)>), String> {
        let headers = headers.iter().map(|(name, value)| Header { name: name.clone(), value: value.clone() }).collect();
        let result = self.0.request(method.to_owned(), path.to_owned(), headers, body.to_vec());
        match result.error {
            Some(error) => Err(error),
            None => Ok((
                yo_core::Response { status: result.status, body: result.body },
                result.headers.into_iter().map(|h| (h.name, h.value)).collect(),
            )),
        }
    }
}

/// A photo or video a Yo carries (spec 3.2); the app keeps the key until it is seen. UniFFI has no fixed-size arrays.
#[derive(uniffi::Record, Clone, PartialEq)]
pub struct YoMedia {
    pub id: Vec<u8>,
    pub key: Vec<u8>,
    pub sha256: Vec<u8>,
    /// The ciphertext's size.
    pub size: u32,
    pub width: u16,
    pub height: u16,
}

impl From<yo_core::Media> for YoMedia {
    fn from(m: yo_core::Media) -> Self {
        YoMedia {
            id: m.id.to_vec(),
            key: m.key.to_vec(),
            sha256: m.sha256.to_vec(),
            size: m.size,
            width: m.width,
            height: m.height,
        }
    }
}

impl TryFrom<YoMedia> for yo_core::Media {
    type Error = YoError;

    fn try_from(m: YoMedia) -> Result<Self, YoError> {
        let bad = |_| protocol("media");
        Ok(yo_core::Media {
            id: m.id.try_into().map_err(bad)?,
            key: m.key.try_into().map_err(bad)?,
            sha256: m.sha256.try_into().map_err(bad)?,
            size: m.size,
            width: m.width,
            height: m.height,
        })
    }
}

/// A photo or video sealed under a new key: what its Yo carries, and the ciphertext to upload (spec 4.3).
#[derive(uniffi::Record)]
pub struct SealedMedia {
    pub media: YoMedia,
    pub ciphertext: Vec<u8>,
}

/// A group made: its id, and what the inbox brought while making it.
#[derive(uniffi::Record)]
pub struct CreatedGroup {
    pub id: String,
    pub events: Vec<YoEvent>,
}

/// Seals a photo or video (at most 10 MiB) under a key of its own, with a new id. No client: call it anywhere.
#[uniffi::export]
pub fn seal_media(plain: Vec<u8>, width: u16, height: u16) -> Result<SealedMedia, YoError> {
    let (media, ciphertext) = yo_core::media::seal(&plain, width, height)?;
    Ok(SealedMedia { media: media.into(), ciphertext })
}

/// Whether an error means "later", not "never" (spec 3.2, `yo_core::retry_later`).
#[uniffi::export]
pub fn retry_later(error: &YoError) -> bool {
    match error {
        YoError::Network { .. } | YoError::Save { .. } | YoError::Clock => true,
        YoError::Refused { status } => yo_core::retryable(&yo_core::Error::Refused(*status)),
        _ => false,
    }
}

#[derive(uniffi::Enum)]
pub enum SendTarget {
    Contact { id: String },
    Group { id: String },
}

impl From<SendTarget> for shared::Target {
    fn from(target: SendTarget) -> Self {
        match target {
            SendTarget::Contact { id } => shared::Target::User(id),
            SendTarget::Group { id } => shared::Target::Group(id),
        }
    }
}

/// A target as an app keeps it: a user id, a name, or "group:" and the group's id.
#[uniffi::export]
pub fn send_target(to: String) -> SendTarget {
    match shared::Target::parse(&to) {
        shared::Target::User(id) => SendTarget::Contact { id },
        shared::Target::Group(id) => SendTarget::Group { id },
    }
}

#[derive(uniffi::Enum, PartialEq)]
pub enum MessageKind {
    Yo,
    /// A Yo with a photo (JPEG or HEIC).
    Photo {
        media: YoMedia,
    },
    /// A Yo with a short video (MP4 or QuickTime).
    Video {
        media: YoMedia,
        duration_ms: u32,
    },
    Unsupported {
        kind: u16,
    },
}

impl From<Content> for MessageKind {
    fn from(content: Content) -> Self {
        match content {
            Content::Yo | Content::Profile { .. } => MessageKind::Yo,
            Content::Photo(media) => MessageKind::Photo { media: media.into() },
            Content::Video { media, duration_ms } => MessageKind::Video { media: media.into(), duration_ms },
            Content::Unknown(kind) => MessageKind::Unsupported { kind },
            // Never an event (spec 3.2).
            Content::Receipt
            | Content::Group(_)
            | Content::ProfileKey(_)
            | Content::Probe(_)
            | Content::Sync(_)
            | Content::Connect { .. }
            | Content::ProbeReply(_) => MessageKind::Unsupported { kind: 0 },
        }
    }
}

impl TryFrom<MessageKind> for Content {
    type Error = YoError;

    fn try_from(kind: MessageKind) -> Result<Self, YoError> {
        Ok(match kind {
            MessageKind::Yo => Content::Yo,
            MessageKind::Photo { media } => Content::Photo(media.try_into()?),
            MessageKind::Video { media, duration_ms } => Content::Video { media: media.try_into()?, duration_ms },
            MessageKind::Unsupported { .. } => return Err(protocol("content kind")),
        })
    }
}

/// `user` is the contact's user id: what `contacts()` describes and `send` takes to answer.
#[derive(uniffi::Enum, PartialEq)]
pub enum YoEvent {
    NewChat {
        user: String,
    },
    Message {
        user: String,
        sent_at_ms: u64,
        kind: MessageKind,
    },
    Profile {
        user: String,
        name: String,
    },
    IdentityChanged {
        user: String,
    },
    /// A Yo from `user` to a group (id): `groups()` describes it.
    GroupMessage {
        group: String,
        user: String,
        sent_at_ms: u64,
        kind: MessageKind,
    },
    /// A group came, changed or was left: read `groups()` again.
    GroupChanged {
        group: String,
    },
    /// The hub lists a device more for this account (`user`: its own id), never for a contact: for the app to show.
    DeviceAdded {
        user: String,
    },
    /// A service answered: read `connections()` again.
    ConnectionChanged {
        user: String,
    },
    /// Another device of this account sent this to `user`: the thread shows it as sent ("Sent Yo!").
    Sent {
        user: String,
        sent_at_ms: u64,
        kind: MessageKind,
    },
}

impl From<Event> for YoEvent {
    fn from(event: Event) -> Self {
        match event {
            Event::NewChat { user } => YoEvent::NewChat { user },
            Event::Message { user, sent_at_ms, content } => YoEvent::Message { user, sent_at_ms, kind: content.into() },
            Event::Profile { user, name } => YoEvent::Profile { user, name },
            Event::IdentityChanged { user } => YoEvent::IdentityChanged { user },
            Event::GroupMessage { group, user, sent_at_ms, content } => {
                YoEvent::GroupMessage { group, user, sent_at_ms, kind: content.into() }
            }
            Event::GroupChanged { group } => YoEvent::GroupChanged { group },
            Event::DeviceAdded { user } => YoEvent::DeviceAdded { user },
            Event::ConnectionChanged { user } => YoEvent::ConnectionChanged { user },
            Event::Sent { user, sent_at_ms, content } => YoEvent::Sent { user, sent_at_ms, kind: content.into() },
        }
    }
}

// The core's own view types, exported as they are under the names the apps know.

#[uniffi::remote(Record)]
#[uniffi(name = "YoContact")]
pub struct ContactInfo {
    pub user: String,
    pub handles: Vec<String>,
    pub name: Option<String>,
    pub verified: bool,
    pub unconfirmed: bool,
    pub blocked: bool,
    pub gone: bool,
    pub outdated: bool,
    pub accepted: bool,
    pub bot: bool,
}

#[uniffi::remote(Record)]
#[uniffi(name = "YoConnection")]
pub struct ConnectionInfo {
    pub user: String,
    pub name: String,
    pub connected: bool,
    pub waiting: bool,
    pub refused: bool,
    pub ending: bool,
}

#[uniffi::remote(Record)]
#[uniffi(name = "YoGroup")]
pub struct GroupView {
    pub id: String,
    pub name: String,
    pub members: Vec<String>,
    pub revision: u32,
    pub left: bool,
    pub accepted: bool,
}

#[uniffi::remote(Record)]
pub struct SecurityStatus {
    pub account_key: String,
    pub device_key: String,
    pub prekeys_at_hub: u32,
    pub refill_pending: bool,
    pub signed_prekey_days: u64,
    pub devices: u32,
    pub pq_sessions: u32,
    pub contacts: u32,
    pub verified: u32,
    pub hub_signed: bool,
}

#[uniffi::remote(Record)]
#[uniffi(name = "YoSession")]
pub struct SessionInfo {
    pub device: String,
    pub usable: bool,
    pub retrying: bool,
}

#[uniffi::remote(Record)]
#[uniffi(name = "YoFriend")]
pub struct FriendInfo {
    pub user: String,
    pub name: Option<String>,
    pub color: Option<u8>,
    pub blocked: bool,
}

/// Who a name is (`YoClient::whois`).
#[derive(uniffi::Record)]
pub struct YoWhois {
    pub user: String,
    /// The username as its owner writes it ("YoGPT"); none from a hub before spellings.
    pub username: Option<String>,
    /// The account declares itself a bot.
    pub bot: bool,
}

fn events(result: yo_core::Result<Vec<Event>>) -> Result<Vec<YoEvent>, YoError> {
    Ok(result?.into_iter().map(Into::into).collect())
}

fn key_of(key: Vec<u8>) -> Result<[u8; 32], YoError> {
    key.try_into().map_err(|_| YoError::State)
}

fn keys_of(keys: Vec<Vec<u8>>) -> Result<Vec<[u8; 32]>, YoError> {
    keys.into_iter().map(key_of).collect()
}

/// The backup record (spec 2.4) as the hub has it: K wrapped by the passkey (or empty) and the recovery key, what K
/// seals, the locator (16 bytes or empty), and the doors' auth keys.
#[derive(uniffi::Record, Clone)]
pub struct YoBackup {
    pub by_passkey: Vec<u8>,
    pub by_code: Vec<u8>,
    pub sealed: Vec<u8>,
    pub locator: Vec<u8>,
    pub auth_passkey: Vec<u8>,
    pub auth_code: Vec<u8>,
}

impl From<YoBackup> for yo_core::Backup {
    fn from(b: YoBackup) -> Self {
        yo_core::Backup {
            version: yo_core::BACKUP_VERSION,
            by_passkey: b.by_passkey.into(),
            by_code: b.by_code.into(),
            sealed: b.sealed.into(),
            locator: b.locator.into(),
            auth_passkey: b.auth_passkey.into(),
            auth_code: b.auth_code.into(),
        }
    }
}

impl From<yo_core::Backup> for YoBackup {
    fn from(b: yo_core::Backup) -> Self {
        YoBackup {
            by_passkey: b.by_passkey.into(),
            by_code: b.by_code.into(),
            sealed: b.sealed.into(),
            locator: b.locator.into(),
            auth_passkey: b.auth_passkey.into(),
            auth_code: b.auth_code.into(),
        }
    }
}

/// What opens the backup: the passkey's PRF output, or the recovery key (64 characters, `recovery_key_parse`).
#[derive(uniffi::Enum)]
pub enum YoDoor {
    Passkey { prf: Vec<u8> },
    RecoveryKey { key: String },
}

fn recovery_key(key: &str) -> Result<yo_core::backup::AccountEntropyPool, YoError> {
    yo_core::backup::parse_recovery_key(key).ok_or(YoError::State)
}

/// A new recovery key (spec 2.4): libsignal's account entropy pool, 64 characters a–z and 0–9.
#[uniffi::export]
pub fn new_recovery_key() -> String {
    yo_core::backup::new_recovery_key().to_string()
}

/// The recovery key in what was pasted, typed or scanned, as its 64 characters; `None` when it is none.
#[uniffi::export]
pub fn recovery_key_parse(typed: String) -> Option<String> {
    yo_core::backup::parse_recovery_key(&typed).map(|key| key.to_string())
}

/// A backup opened by a door: K, what K sealed (the app's JSON), and the record as the hub has it.
#[derive(uniffi::Record)]
pub struct YoOpenedBackup {
    pub k: Vec<u8>,
    pub sealed: Vec<u8>,
    pub record: YoBackup,
}

/// The passkey's door in a record being made (`YoClient::put_backup`).
#[derive(uniffi::Enum)]
pub enum YoPasskeyDoor {
    /// None: the recovery key alone opens it.
    None,
    /// A passkey, by its PRF output.
    New { prf: Vec<u8> },
    /// As in this record, under the same recovery key (K the same): its wrap and its auth key, or none.
    Keep { record: YoBackup },
}

/// Fetches and opens the backup record a door opens, by user id when given, else by the recovery key's locator.
/// `hub_keys`: the hub's pinned keys (as `YoClient::set_hub_keys`); with any, only an answer the hub signed for this
/// request is taken (spec 4.1), none: as before.
#[uniffi::export]
pub fn open_backup(
    transport: Arc<dyn HttpTransport>,
    user_id: Option<Vec<u8>>,
    door: YoDoor,
    hub_keys: Vec<Vec<u8>>,
) -> Result<Option<YoOpenedBackup>, YoError> {
    let hub_keys = keys_of(hub_keys)?;
    let code;
    let door = match &door {
        YoDoor::Passkey { prf } => Door::Passkey(prf),
        YoDoor::RecoveryKey { key } => {
            code = recovery_key(key)?;
            Door::Code(&code)
        }
    };
    let opened = Client::open_backup(&*wire(transport), user_id.as_deref(), &door, &hub_keys)?;
    Ok(opened.map(|o| YoOpenedBackup { k: o.k.to_vec(), sealed: o.plain, record: o.record.into() }))
}

/// Whether a request goes without a signature (spec 4.1), by the core's rule.
#[uniffi::export]
pub fn anonymous_request(method: String, path: String) -> bool {
    yo_core::anonymous_request(&method, &path)
}

/// Whether a username (NFC) is free, also in other spellings and look-alikes. `Refused` 400: no username; 429:
/// asked too often.
#[uniffi::export]
pub fn username_free(transport: Arc<dyn HttpTransport>, username: String) -> Result<bool, YoError> {
    Ok(Client::username_free(&*wire(transport), &username)?)
}

/// A link this device opened (spec 2.5): show `text` (as a QR code, or to paste), keep it for `link_wait`.
#[derive(Debug, uniffi::Record)]
pub struct YoLinkRequest {
    pub id: String,
    pub secret: Vec<u8>,
    pub text: String,
}

/// A link read from another device's text: nothing sent until the owner confirms (`YoClient::link_approve`).
#[derive(Debug, uniffi::Record)]
pub struct YoLinkOffer {
    pub id: String,
    pub key: Vec<u8>,
}

/// What the old device handed over: register with it (`register_username_db` with `username`, `account_secret`
/// and `legacy_storage_key`) as an added device.
#[derive(Debug, uniffi::Record)]
pub struct YoLinked {
    pub username: String,
    pub account_secret: Vec<u8>,
    pub legacy_storage_key: Option<Vec<u8>>,
}

/// New device: opens a link at the hub with a one-time key (spec 2.5). `Refused` 404: the hub adds no devices.
#[uniffi::export]
pub fn link_start(transport: Arc<dyn HttpTransport>) -> Result<YoLinkRequest, YoError> {
    let r = yo_core::link::start(&*wire(transport))?;
    Ok(YoLinkRequest { id: r.id, secret: r.secret.to_vec(), text: r.text })
}

/// New device: waits up to `wait_s` (≤ 60) for the bundle; `None` none yet, `Refused` 404 the link is over.
#[uniffi::export]
pub fn link_wait(
    transport: Arc<dyn HttpTransport>,
    request: YoLinkRequest,
    wait_s: u64,
) -> Result<Option<YoLinked>, YoError> {
    let request = yo_core::link::Request { id: request.id, secret: key_of(request.secret)?, text: request.text };
    Ok(yo_core::link::wait(&*wire(transport), &request, wait_s)?.map(|l| YoLinked {
        username: l.username,
        account_secret: l.account_secret.to_vec(),
        legacy_storage_key: l.legacy_storage_key.map(|k| k.to_vec()),
    }))
}

/// Old device: reads a link's text (`yo-link:…`); the app asks the owner before `link_approve`.
#[uniffi::export]
pub fn link_offer(text: String) -> Result<YoLinkOffer, YoError> {
    let o = yo_core::link::offer(&text)?;
    Ok(YoLinkOffer { id: o.id, key: o.key.to_vec() })
}

/// A device attestation for `Register` (spec 4.2): the platform's payload (1 App Attest, 2 Play Integrity) over
/// the unsigned challenge from `attest_open_challenge`.
#[derive(uniffi::Record)]
pub struct YoAttest {
    pub platform: u8,
    pub payload: Vec<u8>,
    pub challenge: Vec<u8>,
}

/// A name check: `status` 204 free, 409 taken, 400 no username or too short, 429 asked too often; `name_min` the
/// hub's minimum for this caller (`Yo-Name-Min`), `None` from a hub before it.
#[derive(uniffi::Record)]
pub struct YoNameCheck {
    pub status: u16,
    pub name_min: Option<u32>,
}

/// An unsigned attestation challenge before registering (spec 4.2); `None`: attestation is off at the hub.
#[uniffi::export]
pub fn attest_open_challenge(transport: Arc<dyn HttpTransport>) -> Result<Option<Vec<u8>>, YoError> {
    Ok(Client::attest_open_challenge(&*wire(transport))?)
}

/// A name check with the hub's minimum (spec 2.2). `challenge`: the attested unsigned challenge, `None` without.
#[uniffi::export]
pub fn username_check(
    transport: Arc<dyn HttpTransport>,
    username: String,
    challenge: Option<Vec<u8>>,
) -> Result<YoNameCheck, YoError> {
    let check = Client::username_check(&*wire(transport), &username, challenge.as_deref())?;
    Ok(YoNameCheck { status: check.status, name_min: check.name_min })
}

/// What the sign-up name field shows (`yo_core::name_field`).
#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum YoNamePhase {
    Empty,
    Checking,
    Ok,
    TooShort,
    Taken,
    Invalid,
    Offline,
}

/// What a tap on Sign Up does: go, wait for the answer (button spinner), or shake.
#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum YoSignUp {
    Go,
    Wait,
    Shake,
}

/// The name field for the UI: spinner only while `Checking` (hint empty); `min` the hub's minimum ("Minimum N
/// characters"); `bad` the forbidden characters typed ("No spaces", "No @"); `decided` a pending Sign Up's answer.
#[derive(uniffi::Record)]
pub struct YoNameState {
    pub phase: YoNamePhase,
    pub min: Option<u32>,
    pub bad: Vec<String>,
    pub signup_pending: bool,
    pub decided: Option<YoSignUp>,
}

fn sign_up_of(s: yo_core::name_field::SignUp) -> YoSignUp {
    use yo_core::name_field::SignUp;
    match s {
        SignUp::Go => YoSignUp::Go,
        SignUp::Wait => YoSignUp::Wait,
        SignUp::Shake => YoSignUp::Shake,
    }
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The sign-up name field: one per welcome screen, made when it shows so the device check is usually back
/// before the name is typed. Blocking calls (`start_device_check`, `check`) go off the main thread.
#[derive(uniffi::Object)]
pub struct YoNameField {
    transport: Arc<dyn HttpTransport>,
    field: Mutex<yo_core::name_field::NameField>,
    changed: std::sync::Condvar,
    challenge: Mutex<Option<Vec<u8>>>,
}

#[uniffi::export]
impl YoNameField {
    #[uniffi::constructor]
    pub fn new(transport: Arc<dyn HttpTransport>) -> Arc<Self> {
        Arc::new(YoNameField {
            transport,
            field: Mutex::new(yo_core::name_field::NameField::new(wall_ms())),
            changed: std::sync::Condvar::new(),
            challenge: Mutex::new(None),
        })
    }

    /// Fetches the unsigned challenge for the platform to attest over (App Attest, Play Integrity); `None`
    /// (off at the hub, no answer): the device is not verifiable, nothing to attest.
    pub fn start_device_check(&self) -> Option<Vec<u8>> {
        let challenge = Client::attest_open_challenge(&*wire(self.transport.clone())).ok().flatten();
        match &challenge {
            Some(c) => *self.challenge.lock().unwrap_or_else(PoisonError::into_inner) = Some(c.clone()),
            None => self.device_done(0, None),
        }
        challenge
    }

    /// The platform's attestation over the challenge (1 App Attest, 2 Play Integrity); `None` for any failure.
    pub fn device_done(&self, platform: u8, payload: Option<Vec<u8>>) {
        let challenge = self.challenge.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let attested = match (payload, challenge) {
            (Some(payload), Some(challenge)) => Some(yo_core::Attested { platform, payload, challenge }),
            _ => None,
        };
        self.lock().device_done(attested, wall_ms());
        self.changed.notify_all();
    }

    /// The text changed: the check number to run with `check` (after the app's typing pause), or `None` when
    /// the state is already known (empty, forbidden characters). `own_key`: this device keeps a new-account key for
    /// the text (its own interrupted sign-up), so taken counts as free.
    pub fn typed(&self, text: String, own_key: bool) -> Option<u64> {
        let mut field = self.lock();
        field.typed_own(&text, own_key);
        let seq = (field.view().phase == yo_core::name_field::Phase::Checking).then(|| field.seq());
        self.changed.notify_all();
        seq
    }

    /// Runs check `seq` at the hub, waiting for the device check (at most `DEVICE_TIMEOUT_MS`); a newer text
    /// drops it. Returns the state after.
    pub fn check(&self, seq: u64) -> YoNameState {
        let mut field = self.lock();
        let query = loop {
            if field.seq() != seq {
                return Self::state_of(&field, None);
            }
            if let Some(q) = field.tick(wall_ms()).or_else(|| field.due_query()) {
                break q;
            }
            if field.view().phase != yo_core::name_field::Phase::Checking {
                return Self::state_of(&field, None);
            }
            field = self
                .changed
                .wait_timeout(field, std::time::Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        };
        drop(field);
        let answer = Client::username_check(&*wire(self.transport.clone()), &query.text, query.challenge.as_deref())
            .map(|c| (c.status, c.name_min))
            .map_err(drop);
        let mut field = self.lock();
        let decided = field.answered(query.seq, answer).map(sign_up_of);
        Self::state_of(&field, decided)
    }

    /// Sign Up tapped: `Wait` while checking, then `check`'s `decided` says.
    pub fn sign_up(&self) -> YoSignUp {
        sign_up_of(self.lock().sign_up())
    }

    pub fn state(&self) -> YoNameState {
        Self::state_of(&self.lock(), None)
    }

    /// The attestation for `register_username_db`, if the device attested.
    pub fn attestation(&self) -> Option<YoAttest> {
        self.lock().attestation().map(|a| YoAttest { platform: a.platform, payload: a.payload, challenge: a.challenge })
    }

    /// How long the device check took (ms), once done.
    pub fn device_ms(&self) -> Option<u64> {
        self.lock().device_ms
    }
}

impl YoNameField {
    fn lock(&self) -> MutexGuard<'_, yo_core::name_field::NameField> {
        self.field.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn state_of(field: &yo_core::name_field::NameField, decided: Option<YoSignUp>) -> YoNameState {
        use yo_core::name_field::Phase;
        let view = field.view();
        let phase = match view.phase {
            Phase::Empty => YoNamePhase::Empty,
            Phase::Checking => YoNamePhase::Checking,
            Phase::Ok => YoNamePhase::Ok,
            Phase::TooShort => YoNamePhase::TooShort,
            Phase::Taken => YoNamePhase::Taken,
            Phase::Invalid => YoNamePhase::Invalid,
            Phase::Offline => YoNamePhase::Offline,
        };
        YoNameState { phase, min: view.min, bad: view.bad, signup_pending: view.signup_pending, decided }
    }
}

/// A new install id (128 random bits, 32 hex digits) for help: the support thread, reports, crash and error reports
/// (spec 4). The app keeps it on the phone alone and sends it with nothing else; Send Crashes off deletes it.
#[uniffi::export]
pub fn new_install_id() -> String {
    yo_core::help::new_install_id()
}

/// A message to the Yo team by the install id. `Refused` 429: the day's are used up, or the thread is full.
#[uniffi::export]
pub fn help_send(
    transport: Arc<dyn HttpTransport>,
    install: String,
    text: String,
    app_version: String,
) -> Result<(), YoError> {
    Ok(yo_core::help::send_message(&*wire(transport), &install, &text, &app_version)?)
}

/// The install's thread with the Yo team (empty before its first message).
#[uniffi::export]
pub fn help_thread(transport: Arc<dyn HttpTransport>, install: String) -> Result<SupportThread, YoError> {
    let thread = yo_core::help::thread(&*wire(transport), &install)?;
    let messages = thread.messages.into_iter().map(|m| SupportMessage {
        id: m.id,
        from_team: m.from_team == 1,
        text: String::from_utf8_lossy(m.text.as_slice()).into_owned(),
        created_at: m.created_at,
        read_at: m.read_at,
    });
    Ok(SupportThread { unread: thread.unread, messages: messages.collect() })
}

/// The team's answers read, through the id shown last.
#[uniffi::export]
pub fn help_read(transport: Arc<dyn HttpTransport>, install: String, through: u64) -> Result<(), YoError> {
    Ok(yo_core::help::read(&*wire(transport), &install, through)?)
}

/// A crash (`crash` true) or error report, the JSON the app made, by the install id: the hub's status (204 taken,
/// 4xx never taken, 429 and 5xx later again). Err: no answer.
#[uniffi::export]
pub fn help_diagnostic(
    transport: Arc<dyn HttpTransport>,
    install: String,
    crash: bool,
    report: String,
) -> Result<u16, YoError> {
    let kind = if crash { yo_core::help::Diagnostic::Crash } else { yo_core::help::Diagnostic::Error };
    Ok(yo_core::help::send_diagnostic(&*wire(transport), &install, kind, report.as_bytes())?)
}

/// What may be typed as a username (spec 2.2), so the app's field can never disagree with the hub.
#[uniffi::export]
pub fn username_canonical(typed: String) -> String {
    yo_core::handle::canonical(&typed)
}

/// Whether a name is a username as the hub takes it (spec 2.2).
#[uniffi::export]
pub fn username_valid(name: String) -> bool {
    yo_core::handle::check_username_form(&name).is_ok()
}

/// What two usernames are compared by (spec 2.2): one skeleton, one name.
#[uniffi::export]
pub fn username_skeleton(name: String) -> String {
    yo_core::handle::skeleton(&name)
}

#[derive(uniffi::Object)]
pub struct YoClient {
    client: Mutex<Client>,
    /// For fetching media without the client's lock, so a fetch never waits behind a network call.
    transport: Arc<dyn yo_core::Transport>,
    /// Each step takes in what another process wrote first.
    shared: Shared,
    /// The state went away under this client (`client`).
    closed: AtomicBool,
    probe: Mutex<()>,
}

impl YoClient {
    fn on(client: Client, shared: Shared) -> Arc<Self> {
        Arc::new(YoClient {
            transport: client.transport(),
            client: Mutex::new(client),
            shared,
            closed: AtomicBool::new(false),
            probe: Mutex::new(()),
        })
    }

    /// The client unless closed; with `catch_up`, on the database's current state. A client whose state is gone or
    /// another account's is closed: it stops saving and every step fails with `State`.
    fn client(&self, catch_up: bool) -> Result<MutexGuard<'_, Client>, YoError> {
        let mut client = self.client.lock().unwrap_or_else(PoisonError::into_inner);
        if self.closed.load(Ordering::Relaxed) {
            return Err(YoError::State);
        }
        if !catch_up {
            return Ok(client);
        }
        let store = &self.shared.store;
        let current = match client.load_db(store)? {
            Loaded::Same => true,
            Loaded::Newer { word, state, records } => {
                let taken = client.reload_snapshot(*state, records).is_ok();
                if taken {
                    store.took(Some(word));
                }
                taken
            }
            Loaded::Gone => false,
        };
        if !current {
            client.stop_persisting();
            store.took(None);
            self.closed.store(true, Ordering::Relaxed);
            return Err(YoError::State);
        }
        Ok(client)
    }

    /// One step that may write. The turn comes before the client's mutex, so local readers run while the writer waits.
    fn with<T>(&self, f: impl FnOnce(&mut Client) -> T) -> Result<T, YoError> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(YoError::State);
        }
        let shared = &self.shared;
        let _turn = shared.lock.as_deref().map(|lock| Turn::take(lock, Some(shared.store.clone()))).transpose()?;
        Ok(f(&mut *self.client(true)?))
    }

    /// The outbox sent without the turn; what does not go now waits for the next round.
    fn deliver_waiting(&self) -> Vec<Event> {
        match self.with(|c| c.deliver_begin()) {
            Ok(Ok(deliveries)) if !deliveries.is_empty() => shared::deliver(self, deliveries).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// A step that only reads the last committed state, without the turn.
    fn look<T>(&self, f: impl FnOnce(&Client) -> T) -> Result<T, YoError> {
        Ok(f(&*self.client(true)?))
    }

    /// Process-local settings: neither the turn nor the database.
    fn configure<T>(&self, f: impl FnOnce(&mut Client) -> T) -> Result<T, YoError> {
        Ok(f(&mut *self.client(false)?))
    }

    /// What never changes once registered (the user id, the account key): also for the main thread.
    fn read<T>(&self, f: impl FnOnce(&Client) -> T) -> T {
        f(&self.client.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The bindings' steps: the turn, then the client on the database's current state.
impl yo_core::shared::Locked for YoClient {
    type Error = YoError;

    fn step<T>(&self, step: impl FnOnce(&mut Client) -> T) -> Result<T, YoError> {
        self.with(step)
    }

    fn forget(&self, deliveries: &yo_core::Deliveries) {
        _ = self.configure(|c| c.forget_deliveries(deliveries));
    }
}

#[uniffi::export]
impl YoClient {
    /// A silent Yo-to-Yo probe. Each step gives the client back; inbox consumers go on receiving meanwhile.
    pub fn round_trip_probe(&self, target: String, timeout_ms: u64) -> Result<Option<u64>, YoError> {
        let _probe = self.probe.try_lock().map_err(|_| YoError::State)?;
        let mut probe = self.with(|c| c.round_trip_probe_begin(&target, timeout_ms))??;
        let began = probe.began;
        loop {
            let Some(mut reply) = probe.receive(timeout_ms) else { return Ok(None) };
            let done = self.with(|c| c.round_trip_probe_poll(&mut reply))??;
            let elapsed = began.elapsed().as_millis() as u64;
            if done || elapsed >= timeout_ms {
                return Ok((done && elapsed < timeout_ms).then_some(elapsed));
            }
            probe = self.with(|c| c.round_trip_probe_wait(&reply, timeout_ms - elapsed))??;
        }
    }

    /// Connects a service (its name) with another one: the code of its connect link, once the user allowed it.
    pub fn connect_service(&self, service: String, code: String) -> Result<(), YoError> {
        let code = yo_core::unb64(&code).ok().and_then(|c| c.try_into().ok()).ok_or(YoError::State)?;
        self.with(|c| c.connect_service(&service, code))??;
        Ok(())
    }

    /// Declines it instead: the code of its connect link, so whoever waits is told.
    pub fn decline_service(&self, service: String, code: String) -> Result<(), YoError> {
        let code = yo_core::unb64(&code).ok().and_then(|c| c.try_into().ok()).ok_or(YoError::State)?;
        self.with(|c| c.decline_service(&service, code))??;
        Ok(())
    }

    /// The services connected with another one, waiting, refused or ending.
    pub fn connections(&self) -> Vec<ConnectionInfo> {
        self.look(|c| c.connections()).unwrap_or_default()
    }

    /// Ends a service's connection (its user id).
    pub fn disconnect_service(&self, user: String) -> Result<(), YoError> {
        self.with(|c| c.disconnect_service(&user))??;
        Ok(())
    }

    /// Signed out: the client stops after the step under way and every later step fails with `State`.
    pub fn close(&self) {
        let mut client = self.client.lock().unwrap_or_else(PoisonError::into_inner);
        client.stop_persisting();
        self.closed.store(true, Ordering::Relaxed);
    }

    /// Opens the state in `db` without the turn. `lock`: the turn writing steps take (none: one process). `guest`: an
    /// extension that leaves received events for the app. Only when opening changes the state does it take the turn and
    /// open again. `State`: no state, or not with this key.
    #[uniffi::constructor]
    pub fn open_db(
        transport: Arc<dyn HttpTransport>,
        db: Arc<StateDb>,
        state_key: Vec<u8>,
        guest: bool,
        lock: Option<Arc<dyn StateLock>>,
    ) -> Result<Arc<Self>, YoError> {
        let key = key_of(state_key)?;
        let http: Arc<dyn yo_core::Transport> = wire(transport);
        let open = || -> Result<(Client, bool, Shared), YoError> {
            let Loaded::Newer { word, state, records } = db.0.load(None, &key)? else {
                return Err(YoError::State);
            };
            let (client, changed) = Client::from_snapshot(http.clone(), *state, records, guest)?;
            Ok((client, changed, Shared::new(&db, Some(word), lock.clone())))
        };
        let (mut client, changed, shared) = open()?;
        if guest && client.is_registering() {
            return Err(YoError::State); // the app's to finish
        }
        if guest || !(changed || client.is_registering()) {
            client.keep_in(shared.store.clone(), key);
            return Ok(Self::on(client, shared));
        }
        let mut turn = first_turn(&lock)?;
        let (mut client, _, shared) = open()?;
        if let Some(turn) = &mut turn {
            turn.store = Some(shared.store.clone());
        }
        client.persist_to(shared.store.clone(), key)?;
        client.finish_registration()?;
        drop(turn);
        Ok(Self::on(client, shared))
    }

    /// Registers a username into `db` under the turn, replacing what is there; saved before the request leaves. The
    /// same account's registration still unanswered there is finished instead, with the device it made; `replace`
    /// (after `DeviceLimit`, the owner's choice) names the account's device that goes for it.
    #[uniffi::constructor(default(attest = None, replace = None))]
    #[allow(clippy::too_many_arguments)]
    pub fn register_username_db(
        transport: Arc<dyn HttpTransport>,
        db: Arc<StateDb>,
        state_key: Vec<u8>,
        lock: Option<Arc<dyn StateLock>>,
        username: String,
        account_secret: Option<Vec<u8>>,
        legacy_seed: Option<Vec<u8>>,
        legacy_storage_key: Option<Vec<u8>>,
        name: Option<String>,
        push_platform: u8,
        push_token: String,
        attest: Option<YoAttest>,
        replace: Option<u8>,
    ) -> Result<Arc<Self>, YoError> {
        let key = key_of(state_key)?;
        let attest =
            attest.map(|a| yo_core::Attested { platform: a.platform, payload: a.payload, challenge: a.challenge });
        let account = account_secret.map(key_of).transpose()?;
        let seed = legacy_seed.map(key_of).transpose()?;
        let legacy = legacy_storage_key.map(key_of).transpose()?.or(seed.map(|seed| yo_core::storage::key(&seed)));
        let push = yo_core::Push { platform: push_platform, token: push_token.into_bytes().into() };
        let mut turn = first_turn(&lock)?;
        let http: Arc<dyn yo_core::Transport> = wire(transport);
        // This account's registration saved and not answered yet (a lost answer, a kill): finished as it is. A new
        // device would replace the one the hub may have taken already, and what came for it since.
        if let (Some(secret), Ok(Loaded::Newer { word, state, records })) = (account, db.0.load(None, &key)) {
            let (mut client, _) = Client::from_snapshot(http.clone(), *state, records, false)?;
            if client.is_registering() && client.account_secret() == secret {
                let shared = Shared::new(&db, Some(word), lock.clone());
                if let Some(turn) = &mut turn {
                    turn.store = Some(shared.store.clone());
                }
                client.persist_to(shared.store.clone(), key)?;
                if let Some(number) = replace {
                    client.replace_on_register(number)?;
                }
                client.finish_registration()?;
                drop(turn);
                return Ok(Self::on(client, shared));
            }
        }
        let shared = Shared::new(&db, db.0.word()?, lock.clone());
        if let Some(turn) = &mut turn {
            turn.store = Some(shared.store.clone());
        }
        let store = shared.store.clone();
        let client = Client::register_username_saved(http, store, key, &username, account, legacy, name, push, attest)?;
        drop(turn);
        Ok(Self::on(client, shared))
    }

    /// Sends again what the hub did not take yet (`waiting`), without holding the client.
    pub fn deliver(&self) -> Result<Vec<YoEvent>, YoError> {
        let deliveries = self.with(|c| c.deliver_begin())??;
        events(Ok(shared::deliver(self, deliveries)?))
    }

    /// A Yo, with a photo or video or not. Its way to the hub does not hold the client where a session is in place.
    pub fn send(&self, target: SendTarget, kind: MessageKind) -> Result<Vec<YoEvent>, YoError> {
        let content = Content::try_from(kind)?;
        events(Ok(shared::send(self, &target.into(), content)?))
    }

    /// Uploads a sealed photo or video for its Yo to `target`: the chat or the group's sessions opened first if
    /// needed, the upload without the client. What opening received.
    pub fn upload_media_to(
        &self,
        target: SendTarget,
        media: YoMedia,
        ciphertext: Vec<u8>,
    ) -> Result<Vec<YoEvent>, YoError> {
        let media = media.try_into()?;
        events(Ok(shared::upload(self, &target.into(), &media, &ciphertext)?))
    }

    /// Envelopes the hub has not taken yet: `deliver` them again in a while.
    pub fn waiting(&self) -> u32 {
        self.look(|c| c.waiting()).map_or(0, |n| n.try_into().unwrap_or(u32::MAX))
    }

    /// The "Waiting to Send" banner's count: what the user sent and the hub should have taken by now (not on its way,
    /// no receipts or keys, nothing one recipient's own state holds).
    pub fn waiting_unheld(&self) -> u32 {
        self.look(|c| c.waiting_unheld()).map_or(0, |n| n.try_into().unwrap_or(u32::MAX))
    }

    /// What waits for `user` (id) waits for them alone: a request past its bucket (spec 3.3: at the hub's time, or at
    /// once with their yes), a full inbox, no session to be had. The bar says "Waiting to Send", no banner.
    pub fn request_held(&self, user: String) -> bool {
        self.look(|c| c.request_held(&user)).unwrap_or(false)
    }

    /// Yos received since the app last showed them, by the app or the notification service: the badge.
    pub fn unread(&self) -> u32 {
        self.look(|c| c.unread()).unwrap_or(0)
    }

    /// The app showed every Yo received: the badge is empty.
    pub fn mark_read(&self) {
        _ = self.with(|c| c.mark_read());
    }

    /// These events are shown or stored: they will not come again after a crash.
    pub fn acknowledge(&self, events: Vec<YoEvent>) {
        let mut left = events;
        _ = self.with(|c| {
            c.acknowledge_only(|handed| {
                let handed = YoEvent::from(handed.clone());
                left.iter().position(|event| *event == handed).map(|at| left.swap_remove(at)).is_some()
            })
        });
    }

    pub fn user_id(&self) -> String {
        self.read(|c| c.user_id().to_owned())
    }

    /// The username as its owner writes it ("José").
    pub fn username(&self) -> Option<String> {
        self.read(|c| c.username().map(str::to_owned))
    }

    /// The account key's 32-byte secret: what the backup seals, for registering on a new phone.
    pub fn account_secret(&self) -> Vec<u8> {
        self.read(|c| c.account_secret().to_vec())
    }

    /// Stores the backup record, replacing the one before: `sealed` (the app's JSON) under K, K wrapped by the recovery
    /// key and as `passkey` says. K is the core's: the kept record's with `Keep` (under the same recovery key, else
    /// `State`), a new one with `New` or `None`, so a door changed (a new recovery key, a passkey added or removed)
    /// never leaves an old door able to open what is sealed from then on (spec 2.4). A rotation with a passkey asks it
    /// again (`New`).
    pub fn put_backup(&self, sealed: Vec<u8>, recovery_key: String, passkey: YoPasskeyDoor) -> Result<(), YoError> {
        let code = self::recovery_key(&recovery_key)?;
        let kept: yo_core::Backup;
        let (k, door) = match &passkey {
            YoPasskeyDoor::None => (yo_core::backup::new_k(), PasskeyDoor::None),
            YoPasskeyDoor::New { prf } => (yo_core::backup::new_k(), PasskeyDoor::New(prf)),
            YoPasskeyDoor::Keep { record } => {
                kept = record.clone().into();
                let opened = yo_core::backup::open(kept.clone(), &Door::Code(&code)).ok_or(YoError::State)?;
                (opened.k.clone(), PasskeyDoor::Keep(&kept))
            }
        };
        let record = yo_core::backup::make(&k, &sealed, &code, door)?;
        Ok(self.with(|c| c.put_backup(&record))??)
    }

    /// What the notification service (or the share extension) received and saved for the app, without the hub.
    pub fn saved_events(&self) -> Result<Vec<YoEvent>, YoError> {
        events(self.with(|c| c.saved_for_app())?)
    }

    /// Waits up to `wait_s` seconds for mail without holding the client and processes it; the hub's ack and the work
    /// after it go first in the next `sync`.
    pub fn sync(&self, wait_s: u64) -> Result<Vec<YoEvent>, YoError> {
        self.sync_with(wait_s, false)
    }

    /// `sync`, its events handed out before they are saved, so the app shows them first; `save_received` saves them
    /// right after. Nothing is acknowledged at the hub before that save: an app that dies first fetches them again.
    /// Only for an app whose database no other process writes (Android; never with iOS's notification service).
    pub fn sync_unsaved(&self, wait_s: u64) -> Result<Vec<YoEvent>, YoError> {
        self.sync_with(wait_s, true)
    }

    /// Saves what `sync_unsaved` handed out.
    pub fn save_received(&self) -> Result<(), YoError> {
        Ok(self.with(|c| c.save_handed())??)
    }

    fn sync_with(&self, wait_s: u64, unsaved: bool) -> Result<Vec<YoEvent>, YoError> {
        let (wait, deliveries) = self.with(|c| c.sync_begin_shared(wait_s))??;
        // The outbox goes as `deliver` does, without the turn: under it, a slow network kept the notification service
        // and every other step waiting as long as the envelopes took (past their 10 s, the push showed unread), and a
        // long turn is one iOS may suspend the app in (0xdead10cc). What does not go now waits for the next sync.
        // A fetch that does not wait (a push's pass) sends it beside the inbox's request, not before: the receipt for the
        // last Yo no longer holds up the new one (a round trip, ~150 ms on a phone). A long poll sends it first, so what
        // it delivered is not held for the poll's seconds.
        let (mut delivered, response) = if wait_s == 0 {
            std::thread::scope(|scope| {
                let delivering = deliveries.map(|deliveries| scope.spawn(|| shared::deliver(self, deliveries)));
                let response = wait.send();
                let delivered = delivering.and_then(|delivering| delivering.join().ok()?.ok()).unwrap_or_default();
                (delivered, response)
            })
        } else {
            let delivered = match deliveries {
                Some(deliveries) => shared::deliver(self, deliveries).unwrap_or_default(),
                None => Vec::new(),
            };
            (delivered, wait.send())
        };
        let end =
            |c: &mut Client| if unsaved { c.sync_end_unsaved(&wait, response) } else { c.sync_end(&wait, response) };
        match self.with(end).and_then(|received| Ok(received?)) {
            Ok(received) => {
                delivered.extend(received);
                events(Ok(delivered))
            }
            Err(error) => {
                // What the delivery handed out is not lost with the inbox: the next call hands it out again.
                if !delivered.is_empty() {
                    _ = self.with(|c| c.hand_back(&delivered));
                }
                Err(error)
            }
        }
    }

    /// Writes a change of the friends, blocks or settings to the hub now, not with the inbox's next answer.
    pub fn write_storage(&self) -> Result<(), YoError> {
        Ok(self.with(|c| c.write_storage())??)
    }

    /// When the app comes to the foreground. The outbox goes before and after as `deliver` does, without the turn: under
    /// it, a slow network kept the notification service waiting past its 10 s (SharedAccess 35, the push shown
    /// unread) and the app's own steps with it.
    pub fn maintenance(&self) -> Result<Vec<YoEvent>, YoError> {
        let mut received = self.deliver_waiting();
        match self.with(|c| c.maintenance_shared()).and_then(|events| Ok(events?)) {
            Ok(events) => received.extend(events),
            Err(error) => {
                // What the delivery handed out is not lost with the rest: the next call hands it out again.
                if !received.is_empty() {
                    _ = self.with(|c| c.hand_back(&received));
                }
                return Err(error);
            }
        }
        // What the maintenance queued (receipts, a profile key for contacts).
        received.extend(self.deliver_waiting());
        events(Ok(received))
    }

    pub fn attest_due(&self, now_ms: u64) -> bool {
        self.look(|c| c.attest_due(now_ms)).unwrap_or(false)
    }

    pub fn attest_challenge(&self) -> Result<Option<Vec<u8>>, YoError> {
        let challenge = self.with(|c| c.attest_challenge_begin())??.attest_challenge()?;
        if challenge.is_none() {
            self.with(|c| c.attest_off())??;
        }
        Ok(challenge)
    }

    pub fn attest_send(&self, platform: u8, payload: Vec<u8>) -> Result<u8, YoError> {
        let verdict =
            self.with(|c| c.attest_send_begin(platform, payload)).and_then(|request| Ok(request?.attest_send()?));
        self.with(|c| c.attest_outcome(verdict.as_ref().ok().copied()))??;
        verdict
    }

    pub fn attest_failed(&self, now_ms: u64) -> Result<(), YoError> {
        Ok(self.with(|c| c.attest_failed(now_ms))??)
    }

    pub fn set_push(&self, platform: u8, token: String) -> Result<(), YoError> {
        let push = yo_core::Push { platform, token: token.into_bytes().into() };
        Ok(self.with(|c| c.set_push(push))??)
    }

    pub fn contacts(&self) -> Vec<ContactInfo> {
        self.look(|c| c.contacts()).unwrap_or_default()
    }

    /// Who a name is, as the hub says, without holding the client; `None`: nobody by that name.
    pub fn whois(&self, name: String) -> Result<Option<YoWhois>, YoError> {
        // A name no one can own (an invite link's malformed name) is nobody, not a failed lookup to retry.
        let whois = match self.with(|c| c.whois_begin(&name))? {
            Err(yo_core::Error::Protocol("format")) => return Ok(None),
            begun => begun?,
        };
        match whois.send() {
            Ok((user, username, bot)) => {
                self.with(|c| c.note_bot(&user, bot))??;
                Ok(Some(YoWhois { user, username, bot }))
            }
            Err(yo_core::Error::NotOnYo) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// A signed read-only hub check, without holding the client during the request.
    pub fn status_probe(&self) -> Result<u16, YoError> {
        let probe = self.with(|c| c.status_probe_begin())??;
        Ok(probe.status()?)
    }

    /// Pins the hub's public keys (spec 4.1): answers handing out keys count only signed by one of them.
    pub fn set_hub_keys(&self, keys: Vec<Vec<u8>>) -> Result<(), YoError> {
        let keys = keys_of(keys)?;
        self.configure(|c| c.set_hub_keys(keys))
    }

    /// Pins the sender-certificate trust roots (spec 4.1); without one every delivery goes identified.
    pub fn set_trust_roots(&self, keys: Vec<Vec<u8>>) -> Result<(), YoError> {
        let keys = keys_of(keys)?;
        self.configure(|c| c.set_trust_roots(keys))
    }

    /// Pins the hub's zkgroup public params (spec 3.4). Without them no group is made, changed or fetched.
    pub fn set_zk_params(&self, params: Vec<u8>) -> Result<(), YoError> {
        Ok(self.configure(|c| c.set_zk_params(&params))??)
    }

    /// This device's security now: keys at the hub, sessions, verified contacts.
    pub fn security_status(&self) -> SecurityStatus {
        self.look(|c| c.security_status()).unwrap_or_default()
    }

    /// The session with a contact's device (user id), if any.
    pub fn sessions_with(&self, user: String) -> Result<Vec<SessionInfo>, YoError> {
        self.look(|c| c.sessions_with(&user))
    }

    /// The account key a contact is pinned to (hex), as it was first seen or last confirmed.
    pub fn account_key_of(&self, user: String) -> Option<String> {
        self.look(|c| c.account_key_of(&user)).ok().flatten()
    }

    pub fn safety_number(&self, who: String) -> Result<String, YoError> {
        Ok(self.look(|c| c.safety_number(&who))??)
    }

    /// The safety number's QR code payload (spec 2.3), shown for the contact to scan.
    pub fn safety_code(&self, who: String) -> Result<Vec<u8>, YoError> {
        Ok(self.look(|c| c.safety_code(&who))??)
    }

    /// Whether a code scanned off the contact's screen (the QR code's own bytes) matches.
    pub fn safety_code_matches(&self, who: String, scanned: Vec<u8>) -> Result<bool, YoError> {
        Ok(self.look(|c| c.safety_code_matches(&who, &scanned))??)
    }

    pub fn set_verified(&self, who: String, verified: bool) -> Result<(), YoError> {
        Ok(self.with(|c| c.set_verified(&who, verified))??)
    }

    /// Accepts a verified contact's new key, so sending goes on.
    pub fn confirm_identity(&self, who: String) -> Result<(), YoError> {
        Ok(self.with(|c| c.confirm_identity(&who))??)
    }

    /// Blocks (or unblocks) a user by user id or handle: nothing from them is shown, nothing is sent to them.
    pub fn set_blocked(&self, who: String, blocked: bool) -> Result<(), YoError> {
        Ok(self.with(|c| c.set_blocked(&who, blocked))??)
    }

    /// The threads shown here (user or group ids, hex): this account's other devices let their notifications of them
    /// go, through the newest message received here (spec 3.2).
    pub fn mark_seen(&self, threads: Vec<String>) -> Result<(), YoError> {
        Ok(self.with(|c| c.mark_seen(&threads))??)
    }

    /// Threads (user or group id, hex) another device of this account showed, through the message sent then (ms):
    /// their notifications of messages sent up to then go here too. Each is handed out once.
    pub fn take_seen(&self) -> Result<Vec<SeenThread>, YoError> {
        let due = self.with(|c| c.take_seen())??;
        Ok(due.into_iter().map(|(thread, through_ms)| SeenThread { thread, through_ms }).collect())
    }

    /// Says yes to a request (spec 3.3).
    pub fn accept(&self, who: String) -> Result<Vec<YoEvent>, YoError> {
        events(self.with(|c| c.accept(&who))?)
    }

    /// Says no to a request: gone from the list; a later Yo from them is a request again.
    pub fn decline(&self, who: String) -> Result<(), YoError> {
        Ok(self.with(|c| c.decline(&who))??)
    }

    /// Says yes to a group someone not yet a contact added this account to.
    pub fn accept_group(&self, group: String) -> Result<Vec<YoEvent>, YoError> {
        events(self.with(|c| c.accept_group(&group))?)
    }

    /// Whether strangers may send requests at all: off, the hub refuses them.
    pub fn set_requests_open(&self, open: bool) -> Result<(), YoError> {
        Ok(self.with(|c| c.set_requests_open(open))??)
    }

    pub fn requests_open(&self) -> bool {
        self.look(|c| c.requests_open()).unwrap_or(true)
    }

    /// The friends the account's storage keeps (spec 4.3): after a restore, the list to show.
    pub fn friends(&self) -> Vec<FriendInfo> {
        self.look(|c| c.friends()).unwrap_or_default()
    }

    /// Adds a friend (by user id) to the stored list, or changes its name or colour; to the hub with `write_storage`.
    pub fn set_friend(&self, user: String, name: Option<String>, color: Option<u8>) -> Result<(), YoError> {
        Ok(self.with(|c| c.set_friend(&user, name, color))??)
    }

    pub fn remove_friend(&self, user: String) -> Result<(), YoError> {
        Ok(self.with(|c| c.remove_friend(&user))??)
    }

    /// When the hub last held all of the storage (ms since 1970); none while a change waits to go up.
    pub fn storage_saved_at(&self) -> Result<Option<u64>, YoError> {
        self.look(|c| c.storage_saved_at())
    }

    /// The app's icon as stored ("Negative", "Original"); none for the default.
    pub fn app_icon(&self) -> Option<String> {
        self.look(|c| c.app_icon()).ok().flatten()
    }

    // The two setters keep a change that did not save in memory: it goes with the next save.

    pub fn set_app_icon(&self, icon: Option<String>) {
        _ = self.with(|c| c.set_app_icon(icon));
    }

    /// Left-handed mode as stored; none until chosen.
    pub fn left_handed(&self) -> Option<bool> {
        self.look(|c| c.left_handed()).ok().flatten()
    }

    pub fn set_left_handed(&self, left_handed: bool) {
        _ = self.with(|c| c.set_left_handed(left_handed));
    }

    /// Reports a user to the operator by the install id (`new_install_id`), not by this account. `NotOnYo`: no such
    /// user.
    pub fn report(&self, who: String, reason: ReportReason, install: String) -> Result<(), YoError> {
        let reason = match reason {
            ReportReason::Spam => yo_core::REPORT_SPAM,
            ReportReason::Abuse => yo_core::REPORT_ABUSE,
            ReportReason::Other => yo_core::REPORT_OTHER,
        };
        Ok(self.with(|c| c.report(&who, reason, &install))??)
    }

    /// Deletes the account at the hub with everything it holds about it; then forget this client and its state.
    pub fn delete_account(&self) -> Result<(), YoError> {
        Ok(self.with(|c| c.delete_account())??)
    }

    /// Old device, after the owner's tap (spec 2.5): seals this account to the new device and leaves it at the link.
    pub fn link_approve(&self, offer: YoLinkOffer) -> Result<(), YoError> {
        let offer = yo_core::link::Offer { id: offer.id, key: key_of(offer.key)? };
        Ok(self.with(|c| c.link_approve(&offer))??)
    }

    /// This account's devices as the hub lists them, for the owner to choose one to remove (multi-device).
    pub fn own_devices(&self) -> Result<Vec<YoDevice>, YoError> {
        Ok(self.with(|c| c.own_devices())??.into_iter().map(YoDevice::from).collect())
    }

    /// Removes another device of this account: it gets `Replaced` from then on and forgets what it held.
    pub fn remove_device(&self, number: u8) -> Result<(), YoError> {
        Ok(self.with(|c| c.remove_device(number))??)
    }

    /// Fetches, verifies and opens media without the client. The app stores it, then acknowledges it (`ack_media`).
    pub fn fetch_media(&self, media: YoMedia) -> Result<Option<Vec<u8>>, YoError> {
        Ok(yo_core::media::fetch_open(self.transport.as_ref(), &media.try_into()?)?)
    }

    /// Acknowledges a blob only after its decrypted file is durably stored.
    pub fn ack_media(&self, id: Vec<u8>) -> Result<(), YoError> {
        let id = id.try_into().map_err(|_| protocol("blob id"))?;
        Ok(yo_core::media::ack(self.transport.as_ref(), &id)?)
    }

    pub fn groups(&self) -> Result<Vec<GroupView>, YoError> {
        self.look(|c| c.groups())
    }

    /// Makes a group of this account and `members` (at most 19) and tells them: its id and what the inbox brought.
    pub fn create_group(&self, name: String, members: Vec<String>) -> Result<CreatedGroup, YoError> {
        let (id, received) = self.with(|c| c.create_group(&name, &members))??;
        Ok(CreatedGroup { id, events: received.into_iter().map(Into::into).collect() })
    }

    pub fn rename_group(&self, group: String, name: String) -> Result<Vec<YoEvent>, YoError> {
        events(self.with(|c| c.rename_group(&group, &name))?)
    }

    pub fn add_to_group(&self, group: String, users: Vec<String>) -> Result<Vec<YoEvent>, YoError> {
        events(self.with(|c| c.add_to_group(&group, &users))?)
    }

    pub fn remove_from_group(&self, group: String, users: Vec<String>) -> Result<Vec<YoEvent>, YoError> {
        events(self.with(|c| c.remove_from_group(&group, &users))?)
    }

    pub fn leave_group(&self, group: String) -> Result<Vec<YoEvent>, YoError> {
        events(self.with(|c| c.leave_group(&group))?)
    }
}

#[cfg(feature = "test-support")]
impl YoClient {
    /// For tests: the core client itself, as a step that may write (the turn taken, the state caught up).
    pub fn core_for_tests<T>(&self, f: impl FnOnce(&mut Client) -> T) -> T {
        self.with(f).expect("the client is open")
    }
}

/// Why a user is reported.
#[derive(uniffi::Enum)]
pub enum ReportReason {
    Spam,
    Abuse,
    Other,
}
