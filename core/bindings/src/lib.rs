//! The core for Swift and Kotlin (UniFFI). The apps supply HTTP and a place for one sealed blob; everything else
//! is `yo-core`. Every call blocks: run it off the main thread.

use std::sync::{Arc, Mutex, PoisonError};
use yo_core::{Client, Content, Event, Sent};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum YoError {
    #[error("network: {message}")]
    Network { message: String },
    #[error("refused: {status}")]
    Refused { status: u16 },
    #[error("not a phone number")]
    Phone,
    #[error("not on Yo")]
    NotOnYo,
    #[error("blocked")]
    Blocked,
    /// A verified contact's key changed: `confirm_identity` first.
    #[error("identity not confirmed")]
    Unconfirmed,
    #[error("this device was replaced")]
    Replaced,
    #[error("protocol: {message}")]
    Protocol { message: String },
    #[error("state")]
    State,
}

impl From<yo_core::Error> for YoError {
    fn from(error: yo_core::Error) -> Self {
        use yo_core::Error as E;
        match error {
            E::Network(message) => YoError::Network { message },
            E::Refused(status) => YoError::Refused { status },
            E::Phone => YoError::Phone,
            E::NotOnYo => YoError::NotOnYo,
            E::Blocked => YoError::Blocked,
            E::Unconfirmed => YoError::Unconfirmed,
            E::Replaced => YoError::Replaced,
            E::State => YoError::State,
            other => YoError::Protocol { message: other.to_string() },
        }
    }
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
    pub error: Option<String>,
}

/// HTTP to the hub, implemented by the app (URLSession, OkHttp). `path` is relative to the hub's base URL.
#[uniffi::export(with_foreign)]
pub trait HttpTransport: Send + Sync {
    fn request(&self, method: String, path: String, headers: Vec<Header>, body: Vec<u8>) -> HttpResult;
}

/// Keeps the sealed state, implemented by the app (a file; the key lives in the Keychain or Keystore).
#[uniffi::export(with_foreign)]
pub trait StateStore: Send + Sync {
    fn save(&self, blob: Vec<u8>);
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

struct Store(Arc<dyn StateStore>);

impl yo_core::Store for Store {
    fn save(&self, blob: Vec<u8>) {
        self.0.save(blob)
    }
}

/// A photo or video a Yo carries (spec 5): where the hub keeps it, encrypted, and what opens and checks it. The key
/// is the app's to keep until the photo is seen, then to forget.
#[derive(uniffi::Record, Clone)]
pub struct YoMedia {
    /// 16 bytes.
    pub id: Vec<u8>,
    /// 32 bytes.
    pub key: Vec<u8>,
    /// The ciphertext's SHA-256.
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
        let bad = || YoError::Protocol { message: "media".into() };
        Ok(yo_core::Media {
            id: m.id.try_into().map_err(|_| bad())?,
            key: m.key.try_into().map_err(|_| bad())?,
            sha256: m.sha256.try_into().map_err(|_| bad())?,
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

/// The chat a photo or video goes in: how many fetches its upload is for, and what opening it received.
#[derive(uniffi::Record)]
pub struct MediaChat {
    pub fetches: u32,
    pub events: Vec<YoEvent>,
}

/// Seals a photo or video (at most 10 MiB) under a key of its own, with a new id. No client: call it anywhere.
#[uniffi::export]
pub fn seal_media(plain: Vec<u8>, width: u16, height: u16) -> Result<SealedMedia, YoError> {
    let (media, ciphertext) = yo_core::media::seal(&plain, width, height)?;
    Ok(SealedMedia { media: media.into(), ciphertext })
}

/// Whether a refusal of `upload_media` means "later" (the account's quota for now, no room at the hub, no network):
/// keep the photo or video and try again.
#[uniffi::export]
pub fn media_retry_later(error: &YoError) -> bool {
    match error {
        YoError::Network { .. } => true,
        YoError::Refused { status } => *status == 429 || *status == 507 || *status >= 500,
        _ => false,
    }
}

#[derive(uniffi::Enum)]
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
    Text {
        text: String,
    },
    Link {
        url: String,
    },
    Location {
        lat_e7: i32,
        lon_e7: i32,
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
            Content::Text(text) => MessageKind::Text { text },
            Content::Link(url) => MessageKind::Link { url },
            Content::Location { lat_e7, lon_e7 } => MessageKind::Location { lat_e7, lon_e7 },
            Content::Unknown(kind) => MessageKind::Unsupported { kind },
        }
    }
}

#[derive(uniffi::Enum)]
/// `user` is the contact's user id: what `contacts()` describes and `send_yo` takes to answer.
pub enum YoEvent {
    NewChat { user: String },
    Message { user: String, sent_at_ms: u64, kind: MessageKind },
    SentElsewhere { user: String, sent_at_ms: u64, kind: MessageKind },
    Profile { user: String, name: String },
    IdentityChanged { user: String },
}

impl From<Event> for YoEvent {
    fn from(event: Event) -> Self {
        match event {
            Event::NewChat { user } => YoEvent::NewChat { user },
            Event::Message { user, sent_at_ms, content } => YoEvent::Message { user, sent_at_ms, kind: content.into() },
            Event::SentElsewhere { user, sent_at_ms, content } => {
                YoEvent::SentElsewhere { user, sent_at_ms, kind: content.into() }
            }
            Event::Profile { user, name } => YoEvent::Profile { user, name },
            Event::IdentityChanged { user } => YoEvent::IdentityChanged { user },
        }
    }
}

#[derive(uniffi::Record)]
pub struct YoContact {
    /// The user id.
    pub user: String,
    /// How this device knows the contact: "+491701234567", "Chris". A username as its owner writes it, as the hub
    /// tells, also for someone who wrote first: the first username is the name to show.
    pub handles: Vec<String>,
    pub name: Option<String>,
    pub verified: bool,
    /// The key changed after verification: confirm before sending.
    pub unconfirmed: bool,
    pub blocked: bool,
}

/// Who a name is (`YoClient::whois`).
#[derive(uniffi::Record)]
pub struct YoWhois {
    /// The user id.
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

/// Sends the SMS code; returns the number in E.164 digits.
#[uniffi::export]
pub fn request_code(transport: Arc<dyn HttpTransport>, phone: String) -> Result<String, YoError> {
    Ok(Client::request_code(&Http(transport), &phone)?)
}

/// The account's backup record (spec 7), as the app makes and opens it: K wrapped by the passkey (empty without one)
/// and by the recovery code, what K seals, and the code's locator (16 bytes, or empty).
#[derive(uniffi::Record)]
pub struct YoBackup {
    pub by_passkey: Vec<u8>,
    pub by_code: Vec<u8>,
    pub sealed: Vec<u8>,
    pub locator: Vec<u8>,
}

impl From<YoBackup> for yo_core::Backup {
    fn from(backup: YoBackup) -> Self {
        yo_core::Backup::new(backup.by_passkey, backup.by_code, backup.sealed, backup.locator)
    }
}

impl From<yo_core::Backup> for YoBackup {
    fn from(backup: yo_core::Backup) -> Self {
        YoBackup {
            by_passkey: backup.by_passkey.into(),
            by_code: backup.by_code.into(),
            sealed: backup.sealed.into(),
            locator: backup.locator.into(),
        }
    }
}

/// The backup record by the user id (a passkey's user handle) or the recovery code's locator, 16 bytes each; `None`
/// when there is none. Before there is an account on this phone.
#[uniffi::export]
pub fn fetch_backup(transport: Arc<dyn HttpTransport>, id: Vec<u8>) -> Result<Option<YoBackup>, YoError> {
    Ok(Client::fetch_backup(&Http(transport), &id)?.map(Into::into))
}

/// Whether a username is free, asked while it is typed; the name as written (NFC). False when it is taken, also in
/// another spelling or as a name that looks like it, or reserved. `Refused { status: 400 }`: no username; `429`: asked
/// too often.
#[uniffi::export]
pub fn username_free(transport: Arc<dyn HttpTransport>, username: String) -> Result<bool, YoError> {
    Ok(Client::username_free(&Http(transport), &username)?)
}

#[derive(uniffi::Object)]
pub struct YoClient {
    client: Mutex<Client>,
    /// The client's own transport, for what needs no client (fetching a photo or video): never behind the lock, so a
    /// fetch does not wait for a call that holds it across the network.
    transport: Arc<dyn yo_core::Transport>,
}

impl YoClient {
    fn new(client: Client) -> Arc<Self> {
        Arc::new(YoClient { transport: client.transport(), client: Mutex::new(client) })
    }

    fn with<T>(&self, f: impl FnOnce(&mut Client) -> T) -> T {
        f(&mut self.client.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// A message, its way to the hub without holding the client where it can (`send_yo`).
    fn send_content(&self, to: String, content: Content) -> Result<Vec<YoEvent>, YoError> {
        match self.with(|c| c.send_begin(&to, content.clone())) {
            Some(deliveries) => self.deliver_from(deliveries),
            None => events(self.with(|c| c.send(&to, content))),
        }
    }

    /// Posts deliveries without holding the client, and what may go after them, until done.
    fn deliver_from(&self, mut deliveries: yo_core::Deliveries) -> Result<Vec<YoEvent>, YoError> {
        loop {
            let answers = deliveries.send();
            match self.with(|c| c.send_end(&deliveries, answers)) {
                Sent::More(more) => deliveries = more,
                Sent::Done(result) => return events(result),
            }
        }
    }
}

#[uniffi::export]
impl YoClient {
    /// Registers this device with the SMS code. `state_key`: 32 bytes the app keeps in its keychain.
    #[uniffi::constructor]
    #[allow(clippy::too_many_arguments)]
    pub fn register(
        transport: Arc<dyn HttpTransport>,
        store: Arc<dyn StateStore>,
        state_key: Vec<u8>,
        phone: String,
        code: String,
        name: Option<String>,
        push_platform: u8,
        push_token: String,
    ) -> Result<Arc<Self>, YoError> {
        let key = key_of(state_key)?;
        let push = yo_core::Push { platform: push_platform, token: push_token.into_bytes().into() };
        let mut client = Client::register(Arc::new(Http(transport)), &phone, &code, name, push)?;
        client.persist_to(Arc::new(Store(store)), key);
        Ok(Self::new(client))
    }

    /// Registers this device with a username (spec 6.4). Without `account_secret`, a new account: the name must be
    /// free. With the 32-byte account secret from the backup (spec 7), the name's account again, same user id, its
    /// other device replaced. `Refused { status: 409 }`: the name is taken.
    #[uniffi::constructor]
    #[allow(clippy::too_many_arguments)]
    pub fn register_username(
        transport: Arc<dyn HttpTransport>,
        store: Arc<dyn StateStore>,
        state_key: Vec<u8>,
        username: String,
        account_secret: Option<Vec<u8>>,
        name: Option<String>,
        push_platform: u8,
        push_token: String,
    ) -> Result<Arc<Self>, YoError> {
        let key = key_of(state_key)?;
        let account = account_secret.map(key_of).transpose()?;
        let push = yo_core::Push { platform: push_platform, token: push_token.into_bytes().into() };
        let mut client = Client::register_username(Arc::new(Http(transport)), &username, account, name, push)?;
        client.persist_to(Arc::new(Store(store)), key);
        Ok(Self::new(client))
    }

    /// Opens the state the store saved last.
    #[uniffi::constructor]
    pub fn restore(
        transport: Arc<dyn HttpTransport>,
        store: Arc<dyn StateStore>,
        state_key: Vec<u8>,
        blob: Vec<u8>,
    ) -> Result<Arc<Self>, YoError> {
        let key = key_of(state_key)?;
        let mut client = Client::restore(Arc::new(Http(transport)), &blob, &key)?;
        client.persist_to(Arc::new(Store(store)), key);
        Ok(Self::new(client))
    }

    pub fn phone(&self) -> String {
        self.with(|c| c.phone().to_owned())
    }

    /// To "+49 170 …", "chris", or a user id from an event. In a chat in use, the way to the hub does not hold the
    /// client: other calls run meanwhile, sends to other friends and the inbox's wait among them.
    pub fn send_yo(&self, to: String) -> Result<Vec<YoEvent>, YoError> {
        self.send_content(to, Content::Yo)
    }

    /// Sends again what the hub did not take yet (`waiting`): its limit per sender (`429`) or no network. Without
    /// holding the client.
    pub fn deliver(&self) -> Result<Vec<YoEvent>, YoError> {
        let deliveries = self.with(|c| c.deliver_begin());
        self.deliver_from(deliveries)
    }

    /// Envelopes the hub has not taken yet: `deliver` them again in a while.
    pub fn waiting(&self) -> u32 {
        self.with(|c| c.waiting()).try_into().unwrap_or(u32::MAX)
    }

    /// The events the last call returned are shown or stored: they will not come again after a crash.
    pub fn acknowledge_events(&self) {
        self.with(|c| c.acknowledge_events())
    }

    pub fn user_id(&self) -> String {
        self.with(|c| c.user_id().to_owned())
    }

    /// The username as its owner writes it ("José").
    pub fn username(&self) -> Option<String> {
        self.with(|c| c.username().map(str::to_owned))
    }

    /// The account key's 32-byte secret: what the backup seals, for `register_username` on a new phone.
    pub fn account_secret(&self) -> Vec<u8> {
        self.with(|c| c.account_secret().to_vec())
    }

    /// Stores the account's backup record at the hub, replacing the one before (its old locator then finds nothing).
    pub fn put_backup(&self, backup: YoBackup) -> Result<(), YoError> {
        Ok(self.with(|c| c.put_backup(&backup.into()))?)
    }

    /// Claims a username, or gives it up with `None`.
    pub fn set_username(&self, name: Option<String>) -> Result<(), YoError> {
        Ok(self.with(|c| c.set_username(name.as_deref()))?)
    }

    /// Waits up to `wait_s` seconds for mail and processes it: for the foreground, and the notification extension.
    /// The wait does not hold the client: other calls run meanwhile.
    pub fn sync(&self, wait_s: u64) -> Result<Vec<YoEvent>, YoError> {
        let wait = self.with(|c| c.sync_begin(wait_s))?;
        let response = wait.send();
        events(self.with(|c| c.sync_end(&wait, response)))
    }

    /// When the app comes to the foreground.
    pub fn maintenance(&self) -> Result<Vec<YoEvent>, YoError> {
        events(self.with(|c| c.maintenance()))
    }

    pub fn set_name(&self, name: String) -> Result<Vec<YoEvent>, YoError> {
        events(self.with(|c| c.set_name(&name)))
    }

    pub fn set_push(&self, platform: u8, token: String) -> Result<(), YoError> {
        let push = yo_core::Push { platform, token: token.into_bytes().into() };
        Ok(self.with(|c| c.set_push(push))?)
    }

    pub fn contacts(&self) -> Vec<YoContact> {
        self.with(|c| c.contacts())
            .into_iter()
            .map(|c| YoContact {
                user: c.user,
                handles: c.handles,
                name: c.name,
                verified: c.verified,
                unconfirmed: c.unconfirmed,
                blocked: c.blocked,
            })
            .collect()
    }

    /// Who a name is, as the hub says ("yogpt" is YoGPT's user id and "YoGPT"), without a chat; `None`: nobody by
    /// that name. The question to the hub does not hold the client.
    pub fn whois(&self, name: String) -> Result<Option<YoWhois>, YoError> {
        let whois = self.with(|c| c.whois_begin(&name))?;
        match whois.send() {
            Ok((user, username)) => Ok(Some(YoWhois { user, username })),
            Err(yo_core::Error::NotOnYo) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn safety_number(&self, who: String) -> Result<String, YoError> {
        Ok(self.with(|c| c.safety_number(&who))?)
    }

    pub fn set_verified(&self, who: String, verified: bool) -> Result<(), YoError> {
        Ok(self.with(|c| c.set_verified(&who, verified))?)
    }

    /// Accepts a verified contact's new key, so sending goes on.
    pub fn confirm_identity(&self, who: String) -> Result<(), YoError> {
        Ok(self.with(|c| c.confirm_identity(&who))?)
    }

    /// Blocks (or unblocks) a user by user id or handle: nothing from them is shown, nothing is sent to them.
    pub fn set_blocked(&self, who: String, blocked: bool) -> Result<(), YoError> {
        Ok(self.with(|c| c.set_blocked(&who, blocked))?)
    }

    /// Reports a user to the operator. `NotOnYo`: no such user.
    pub fn report(&self, who: String, reason: ReportReason) -> Result<(), YoError> {
        Ok(self.with(|c| c.report(&who, reason.into()))?)
    }

    /// Deletes the account at the hub with everything it holds about it; then forget this client and its state.
    pub fn delete_account(&self) -> Result<(), YoError> {
        Ok(self.with(|c| c.delete_account())?)
    }

    /// The chat a photo or video to `to` goes in, opened first if there is none yet (a new friend), with how many
    /// fetches its upload is for, and what opening it received (to be shown like any).
    pub fn media_chat(&self, to: String) -> Result<MediaChat, YoError> {
        let (fetches, received) = self.with(|c| match c.media_fetches(&to) {
            Some(fetches) => Ok((fetches, Vec::new())),
            None => c.open_chat(&to),
        })?;
        Ok(MediaChat { fetches, events: received.into_iter().map(Into::into).collect() })
    }

    /// Uploads a sealed photo or video for `fetches` fetches (`media_chat`), before its Yo (`send_photo`,
    /// `send_video`). Signed under the client, sent without it: other calls go on meanwhile. `media_retry_later`
    /// tells which refusals mean "later".
    pub fn upload_media(&self, media: YoMedia, ciphertext: Vec<u8>, fetches: u32) -> Result<(), YoError> {
        let media: yo_core::Media = media.try_into()?;
        let upload = self.with(|c| c.upload(&media, fetches));
        Ok(upload.send(&ciphertext)?)
    }

    /// A Yo with a photo whose blob is uploaded; on its way like `send_yo`.
    pub fn send_photo(&self, to: String, media: YoMedia) -> Result<Vec<YoEvent>, YoError> {
        self.send_content(to, Content::Photo(media.try_into()?))
    }

    /// A Yo with a video whose blob is uploaded; on its way like `send_yo`.
    pub fn send_video(&self, to: String, media: YoMedia, duration_ms: u32) -> Result<Vec<YoEvent>, YoError> {
        self.send_content(to, Content::Video { media: media.try_into()?, duration_ms })
    }

    /// Fetches a photo or video a Yo carried and opens it, without the client (not even waiting for its lock: as soon
    /// as the Yo is there, beside the inbox's wait and sends); `None` when the hub no longer has it (fetched by all its
    /// devices, or past its 30 days). Each fetch counts at the hub: keep what comes.
    pub fn fetch_media(&self, media: YoMedia) -> Result<Option<Vec<u8>>, YoError> {
        let media: yo_core::Media = media.try_into()?;
        Ok(yo_core::media::fetch_open(self.transport.as_ref(), &media)?)
    }
}

/// Why a user is reported.
#[derive(uniffi::Enum)]
pub enum ReportReason {
    Spam,
    Abuse,
    Other,
}

impl From<ReportReason> for u8 {
    fn from(reason: ReportReason) -> Self {
        match reason {
            ReportReason::Spam => yo_core::REPORT_SPAM,
            ReportReason::Abuse => yo_core::REPORT_ABUSE,
            ReportReason::Other => yo_core::REPORT_OTHER,
        }
    }
}
