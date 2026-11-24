//! The Swift and Kotlin API: a thin UniFFI layer over `yo_core`. Client operations may block on HTTP or SQLite.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use yo_core::backup::{Door, PasskeyDoor};
use yo_core::groups::GroupView;
use yo_core::security::SecurityStatus;
use yo_core::shared;
use yo_core::statedb::{DbStore, Loaded};
use yo_core::storage::FriendInfo;
use yo_core::{Client, ContactInfo, Content, Event, SessionInfo};

uniffi::setup_scaffolding!();

mod android;
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
    /// The phone's date and time are off beyond what the hub's time corrects (spec 6.2), or TLS refused the hub's
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
}

/// HTTP to the hub, implemented by the app (URLSession, OkHttp). `path` is relative to the hub's base URL.
#[uniffi::export(with_foreign)]
pub trait HttpTransport: Send + Sync {
    fn request(&self, method: String, path: String, headers: Vec<Header>, body: Vec<u8>) -> HttpResult;
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

struct Http(Arc<dyn HttpTransport>);

impl yo_core::Transport for Http {
    fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<yo_core::Response, String> {
        let headers = headers.iter().map(|(name, value)| Header { name: name.clone(), value: value.clone() }).collect();
        let result = self.0.request(method.to_owned(), path.to_owned(), headers, body.to_vec());
        match result.error {
            Some(error) => Err(error),
            None => Ok(yo_core::Response { status: result.status, body: result.body }),
        }
    }
}

/// A photo or video a Yo carries (spec 5); the app keeps the key until it is seen. UniFFI has no fixed-size arrays.
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

/// A photo or video sealed under a new key: what its Yo carries, and the ciphertext to upload (spec 6.8).
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

/// Whether an error means "later", not "never" (spec 6.6, `yo_core::retry_later`).
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
            // Never an event (spec 4.5, 4.6).
            Content::Receipt
            | Content::Group(_)
            | Content::ProfileKey(_)
            | Content::Probe(_)
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

/// The backup record (spec 7) as the hub has it: K wrapped by the passkey (or empty) and the recovery key, what K
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

/// A new recovery key (spec 7): libsignal's account entropy pool, 64 characters a–z and 0–9.
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
/// request is taken (spec 6.10), none: as before.
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
    let opened = Client::open_backup(&Http(transport), user_id.as_deref(), &door, &hub_keys)?;
    Ok(opened.map(|o| YoOpenedBackup { k: o.k.to_vec(), sealed: o.plain, record: o.record.into() }))
}

/// Whether a request goes without a signature (spec 6.2), by the core's rule.
#[uniffi::export]
pub fn anonymous_request(method: String, path: String) -> bool {
    yo_core::anonymous_request(&method, &path)
}

/// Whether a username (NFC) is free, also in other spellings and look-alikes. `Refused` 400: no username; 429:
/// asked too often.
#[uniffi::export]
pub fn username_free(transport: Arc<dyn HttpTransport>, username: String) -> Result<bool, YoError> {
    Ok(Client::username_free(&Http(transport), &username)?)
}

/// A new install id (128 random bits, 32 hex digits) for help: the support thread, reports, crash and error reports
/// (spec 6.4). The app keeps it on the phone alone and sends it with nothing else; Send Crashes off deletes it.
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
    Ok(yo_core::help::send_message(&Http(transport), &install, &text, &app_version)?)
}

/// The install's thread with the Yo team (empty before its first message).
#[uniffi::export]
pub fn help_thread(transport: Arc<dyn HttpTransport>, install: String) -> Result<SupportThread, YoError> {
    let thread = yo_core::help::thread(&Http(transport), &install)?;
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
    Ok(yo_core::help::read(&Http(transport), &install, through)?)
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
    Ok(yo_core::help::send_diagnostic(&Http(transport), &install, kind, report.as_bytes())?)
}

/// What may be typed as a username (spec 3.3), so the app's field can never disagree with the hub.
#[uniffi::export]
pub fn username_canonical(typed: String) -> String {
    yo_core::handle::canonical(&typed)
}

/// Whether a name is a username as the hub takes it (spec 3.3).
#[uniffi::export]
pub fn username_valid(name: String) -> bool {
    yo_core::handle::check_username_form(&name).is_ok()
}

/// What two usernames are compared by (spec 3.3): one skeleton, one name.
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
        let http: Arc<dyn yo_core::Transport> = Arc::new(Http(transport));
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
    /// same account's registration still unanswered there is finished instead, with the device it made.
    #[uniffi::constructor]
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
    ) -> Result<Arc<Self>, YoError> {
        let key = key_of(state_key)?;
        let account = account_secret.map(key_of).transpose()?;
        let seed = legacy_seed.map(key_of).transpose()?;
        let legacy = legacy_storage_key.map(key_of).transpose()?.or(seed.map(|seed| yo_core::storage::key(&seed)));
        let push = yo_core::Push { platform: push_platform, token: push_token.into_bytes().into() };
        let mut turn = first_turn(&lock)?;
        let http: Arc<dyn yo_core::Transport> = Arc::new(Http(transport));
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
        let client = Client::register_username_saved(http, store, key, &username, account, legacy, name, push)?;
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

    /// `waiting` but for requests past their bucket: the "Waiting to Send" banner's count (those wait quietly).
    pub fn waiting_unheld(&self) -> u32 {
        self.look(|c| c.waiting_unheld()).map_or(0, |n| n.try_into().unwrap_or(u32::MAX))
    }

    /// What waits for `user` (id) is a request past its bucket (spec 4.10): it goes at the hub's time, or at once
    /// with their yes. The bar says "Waiting to Send", no banner: nothing went wrong.
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
    /// never leaves an old door able to open what is sealed from then on (spec 7). A rotation with a passkey asks it
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
        match self.with(|c| c.sync_end(&wait, response)).and_then(|received| Ok(received?)) {
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
            Ok((user, username)) => Ok(Some(YoWhois { user, username })),
            Err(yo_core::Error::NotOnYo) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// A signed read-only hub check, without holding the client during the request.
    pub fn status_probe(&self) -> Result<u16, YoError> {
        let probe = self.with(|c| c.status_probe_begin())??;
        Ok(probe.status()?)
    }

    /// Pins the hub's public keys (spec 6.10): answers handing out keys count only signed by one of them.
    pub fn set_hub_keys(&self, keys: Vec<Vec<u8>>) -> Result<(), YoError> {
        let keys = keys_of(keys)?;
        self.configure(|c| c.set_hub_keys(keys))
    }

    /// Pins the sender-certificate trust roots (spec 6.1); without one every delivery goes identified.
    pub fn set_trust_roots(&self, keys: Vec<Vec<u8>>) -> Result<(), YoError> {
        let keys = keys_of(keys)?;
        self.configure(|c| c.set_trust_roots(keys))
    }

    /// Pins the hub's zkgroup public params (spec 4.9). Without them no group is made, changed or fetched.
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

    /// The safety number's QR code payload (spec 3.4), shown for the contact to scan.
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

    /// Says yes to a request (spec 4.10).
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

    /// The friends the account's storage keeps (spec 6.9): after a restore, the list to show.
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
