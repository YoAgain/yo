//! The core for Swift and Kotlin (UniFFI). The apps supply HTTP and a place for one sealed blob; everything else
//! is `yo-core`. Every call blocks: run it off the main thread.

use std::sync::{Arc, Mutex, PoisonError};
use yo_core::{Client, Content, Event};

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

#[derive(uniffi::Enum)]
pub enum MessageKind {
    Yo,
    Text { text: String },
    Link { url: String },
    Location { lat_e7: i32, lon_e7: i32 },
    Unsupported { kind: u16 },
}

impl From<Content> for MessageKind {
    fn from(content: Content) -> Self {
        match content {
            Content::Yo | Content::Profile { .. } => MessageKind::Yo,
            Content::Text(text) => MessageKind::Text { text },
            Content::Link(url) => MessageKind::Link { url },
            Content::Location { lat_e7, lon_e7 } => MessageKind::Location { lat_e7, lon_e7 },
            Content::Unknown(kind) => MessageKind::Unsupported { kind },
        }
    }
}

#[derive(uniffi::Enum)]
pub enum YoEvent {
    NewChat { phone: String },
    Message { phone: String, sent_at_ms: u64, kind: MessageKind },
    SentElsewhere { phone: String, sent_at_ms: u64, kind: MessageKind },
    Profile { phone: String, name: String },
    IdentityChanged { phone: String },
}

impl From<Event> for YoEvent {
    fn from(event: Event) -> Self {
        match event {
            Event::NewChat { phone } => YoEvent::NewChat { phone },
            Event::Message { phone, sent_at_ms, content } => YoEvent::Message { phone, sent_at_ms, kind: content.into() },
            Event::SentElsewhere { phone, sent_at_ms, content } => {
                YoEvent::SentElsewhere { phone, sent_at_ms, kind: content.into() }
            }
            Event::Profile { phone, name } => YoEvent::Profile { phone, name },
            Event::IdentityChanged { phone } => YoEvent::IdentityChanged { phone },
        }
    }
}

#[derive(uniffi::Record)]
pub struct YoContact {
    pub phone: String,
    pub name: Option<String>,
    pub verified: bool,
    pub can_verify: bool,
    pub blocked: bool,
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

#[derive(uniffi::Object)]
pub struct YoClient {
    client: Mutex<Client>,
}

impl YoClient {
    fn with<T>(&self, f: impl FnOnce(&mut Client) -> T) -> T {
        f(&mut self.client.lock().unwrap_or_else(PoisonError::into_inner))
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
        Ok(Arc::new(YoClient { client: Mutex::new(client) }))
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
        Ok(Arc::new(YoClient { client: Mutex::new(client) }))
    }

    pub fn phone(&self) -> String {
        self.with(|c| c.phone().to_owned())
    }

    pub fn send_yo(&self, phone: String) -> Result<Vec<YoEvent>, YoError> {
        events(self.with(|c| c.send_yo(&phone)))
    }

    /// Waits up to `wait_s` seconds for mail and processes it: for the foreground, and the notification extension.
    pub fn sync(&self, wait_s: u64) -> Result<Vec<YoEvent>, YoError> {
        events(self.with(|c| c.sync(wait_s)))
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
            .map(|c| YoContact { phone: c.phone, name: c.name, verified: c.verified, can_verify: c.can_verify, blocked: c.blocked })
            .collect()
    }

    pub fn safety_number(&self, phone: String) -> Result<String, YoError> {
        Ok(self.with(|c| c.safety_number(&phone))?)
    }

    pub fn set_verified(&self, phone: String, verified: bool) -> Result<(), YoError> {
        Ok(self.with(|c| c.set_verified(&phone, verified))?)
    }

    pub fn set_blocked(&self, phone: String, blocked: bool) -> Result<(), YoError> {
        Ok(self.with(|c| c.set_blocked(&phone, blocked))?)
    }
}
