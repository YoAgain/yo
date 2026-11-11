//! The Yo client core (spec/v1.md): one implementation of the protocol for iOS and Android.
//!
//! The platform supplies HTTP (`Transport`) and a place to keep one sealed blob (`Store`); everything else, from
//! the MLS groups to the order of commits, is here. Every operation either completes or leaves the state as it
//! was, except what the hub has already accepted, which is kept.

pub mod error;
pub mod identity;
pub mod net;
pub mod profile;
pub mod state;

pub use error::{Error, Result};
pub use net::{Response, Transport};
pub use yo_wire::api::Push;
pub use yo_wire::content::Content;

use ed25519_dalek::{Signer as _, SigningKey};
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::storage::StorageProvider as _;
use profile::Provider;
use state::{hex, Chat, Contact, Inflight, Outgoing, OwnKeyPackage, State};
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use yo_wire::api::{self, status};
use yo_wire::content::Message;
use yo_wire::handle::{Handle, KIND_PHONE, KIND_USER, KIND_USERNAME};
use yo_wire::limits::{KEY_PACKAGE_REFILL_BELOW, KEY_PACKAGE_TARGET, MAX_INBOX_FETCH};
use yo_wire::request::{HEADER_DEVICE, HEADER_SIGNATURE, HEADER_TIME};
use yo_wire::{decode, encode, envelope, phone, sha256, Mailbox, EPOCH_TOKEN_LABEL, TOKEN_LEN};

/// An update is due after this long (spec 4.4)...
pub const UPDATE_AFTER_MS: u64 = 7 * 24 * 3600 * 1000;
/// ... or after this many messages sent.
pub const UPDATE_AFTER_MESSAGES: u32 = 100;
/// The last resort KeyPackage is replaced this often.
const LAST_RESORT_EVERY_MS: u64 = 30 * 24 * 3600 * 1000;
/// A closed chat is still read this long: the hub keeps envelopes 30 days.
const CLOSED_CHAT_KEPT_MS: u64 = 31 * 24 * 3600 * 1000;
/// Envelope hashes remembered against redelivery.
const SEEN: usize = 2000;

/// What became of a commit.
enum Committed {
    /// There was none to send.
    Nothing,
    Accepted,
    /// Another commit took the epoch.
    Lost,
    /// The hub does not order this group's commits.
    Refused,
    /// No answer: it may have been accepted.
    Unknown(String),
}

/// Errors that say "not now", not "never": the envelope or operation is tried again later.
fn retryable(error: &Error) -> bool {
    match error {
        Error::Network(_) => true,
        Error::Refused(status) => *status == status::RATE_LIMITED || *status >= 500,
        _ => false,
    }
}

/// What happened, for the app to show. `user` is the contact's user id (hex): what `contacts()` describes and
/// what `send` takes to answer.
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
    /// A contact's identity changed: new device without passkey, or something worse. Verification is reset.
    IdentityChanged { user: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactInfo {
    /// The user id (hex).
    pub user: String,
    /// How this device knows the contact: "+491701234567", "CHRIS".
    pub handles: Vec<String>,
    pub name: Option<String>,
    pub verified: bool,
    /// Verification is offered only for an identity that survives a device change (spec 3).
    pub can_verify: bool,
    pub blocked: bool,
}

/// Where the sealed state goes after every operation.
pub trait Store: Send + Sync {
    fn save(&self, blob: Vec<u8>);
}

pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub struct Client {
    transport: Arc<dyn Transport>,
    store: Option<(Arc<dyn Store>, [u8; 32])>,
    provider: Provider,
    state: State,
    clock: Clock,
    events: Vec<Event>,
    last_request_ms: u64,
    dropped: u64,
    drop_reasons: Vec<String>,
}

fn wall_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn mls<E: std::fmt::Debug>(error: E) -> Error {
    Error::Mls(format!("{error:?}"))
}

fn group_key(group: &GroupId) -> String {
    hex(group.as_slice())
}

fn refused(response: &Response) -> Error {
    match response.status {
        status::GONE => Error::Replaced,
        other => Error::Refused(other),
    }
}

impl Client {
    // ---- registration and state ----

    /// Asks the hub to send a code to the number; returns the number in E.164 digits.
    pub fn request_code(transport: &dyn Transport, phone_input: &str) -> Result<String> {
        let digits = phone::normalize(phone_input)?;
        let body = encode(&api::VerifyStart { phone: digits.as_bytes().to_vec().into() });
        let response = transport.request("POST", api::VERIFY_START, &[], &body).map_err(Error::Network)?;
        match response.status {
            status::NO_CONTENT => Ok(digits),
            _ => Err(refused(&response)),
        }
    }

    /// Registers this device with the code from the SMS: a new account key. The number keeps its user id if it
    /// had one (a new phone); any device of it registered before is replaced.
    pub fn register(
        transport: Arc<dyn Transport>,
        phone_input: &str,
        code: &str,
        name: Option<String>,
        push: Push,
    ) -> Result<Client> {
        let digits = phone::normalize(phone_input)?;
        let account = SigningKey::from_bytes(&identity::random());
        let signer = SignatureKeyPair::new(SignatureScheme::ED25519).map_err(|_| Error::Internal("key"))?;
        let mailbox: Mailbox = identity::random();
        let certificate = identity::certify(&account, signer.public(), &mailbox, 0)?;
        let state = State {
            phone: digits.clone(),
            user_id: String::new(),
            username: None,
            flags: 0,
            account_secret: account.to_bytes(),
            auth_secret: identity::random(),
            mailbox,
            certificate: certificate.to_bytes(),
            signer,
            name,
            contacts: Default::default(),
            chats: Default::default(),
            outbox: VecDeque::new(),
            seen: VecDeque::new(),
            ignored: VecDeque::new(),
            key_packages: Vec::new(),
            key_packages_left: KEY_PACKAGE_TARGET,
            last_resort_at: wall_ms(),
        };
        let mut client = Client::with(transport, Provider::default(), state);
        let key_packages =
            (0..KEY_PACKAGE_TARGET).map(|_| client.key_package(false).map(Into::into)).collect::<Result<Vec<_>>>()?;
        let request = api::VerifyCheck {
            phone: digits.as_bytes().to_vec().into(),
            code: code.as_bytes().to_vec().into(),
            certificate: client.state.certificate.clone().into(),
            auth_key: SigningKey::from_bytes(&client.state.auth_secret).verifying_key().to_bytes(),
            push,
            key_packages,
            last_resort: client.key_package(true)?.into(),
        };
        let response =
            client.transport.request("POST", api::VERIFY_CHECK, &[], &encode(&request)).map_err(Error::Network)?;
        if response.status != status::OK {
            return Err(refused(&response));
        }
        let registered: api::Registered = decode(&response.body)?;
        client.state.user_id = hex(&registered.user_id);
        Ok(client)
    }

    /// For fuzzing: processes one envelope as if it came from the inbox (one transaction, like `sync`).
    #[cfg(feature = "fuzz")]
    pub fn receive_for_fuzzing(&mut self, sealed: &[u8]) -> Result<()> {
        self.atomic(|c| c.receive(sealed))
    }

    /// Opens a sealed state.
    pub fn restore(transport: Arc<dyn Transport>, blob: &[u8], key: &[u8; 32]) -> Result<Client> {
        let (state, entries) = state::open(blob, key)?;
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
            events: Vec::new(),
            last_request_ms: 0,
            dropped: 0,
            drop_reasons: Vec::new(),
        }
    }

    /// Saves the sealed state to `store` after every operation from now on, and now.
    pub fn persist_to(&mut self, store: Arc<dyn Store>, key: [u8; 32]) {
        self.store = Some((store, key));
        self.persist();
    }

    /// The state sealed under `key`.
    pub fn export(&self, key: &[u8; 32]) -> Vec<u8> {
        let mut snapshot = profile::Snapshot(self.provider.entries());
        state::seal(&self.state, std::mem::take(&mut snapshot.0), key)
    }

    /// For tests: the clock that decides when updates are due.
    pub fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
    }

    pub fn phone(&self) -> &str {
        &self.state.phone
    }

    /// The own user id (hex): stable across new phones.
    pub fn user_id(&self) -> &str {
        &self.state.user_id
    }

    pub fn username(&self) -> Option<&str> {
        self.state.username.as_deref()
    }

    pub fn mailbox(&self) -> Mailbox {
        self.state.mailbox
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
        if self.drop_reasons.len() < 20 {
            self.drop_reasons.push(reason);
        }
    }

    /// KeyPackages whose private keys this device still holds.
    pub fn own_key_packages(&self) -> usize {
        self.state.key_packages.len()
    }

    /// Envelopes waiting to be accepted by the hub.
    pub fn pending(&self) -> usize {
        self.state.outbox.len()
    }

    /// Every chat: peer, group id, epoch. For tests and diagnostics.
    pub fn chats(&self) -> Vec<(String, [u8; 32], u64)> {
        self.state
            .chats
            .iter()
            .filter_map(|(key, chat)| {
                let id: [u8; 32] = state::unhex(key)?.try_into().ok()?;
                let group = MlsGroup::load(self.provider.storage(), &GroupId::from_slice(&id)).ok()??;
                Some((chat.peer.clone(), id, group.epoch().as_u64()))
            })
            .collect()
    }

    fn persist(&self) {
        if let Some((store, key)) = &self.store {
            store.save(self.export(key));
        }
    }

    fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Runs `f` as one transaction: on an error, state, MLS store and events are as before.
    fn atomic<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let state = self.state.clone();
        let mut snapshot = profile::Snapshot(self.provider.entries());
        let events = self.events.len();
        let result = f(self);
        if result.is_err() {
            self.state = state;
            self.provider.restore(std::mem::take(&mut snapshot.0));
            self.events.truncate(events);
        }
        result
    }

    /// Ends a public operation: saves, hands out the events. Events are never lost: after an error they wait
    /// for the next call that succeeds.
    fn finish<T>(&mut self, result: Result<T>) -> Result<Vec<Event>> {
        self.persist();
        match result {
            Ok(_) => Ok(self.take_events()),
            Err(error) => Err(error),
        }
    }

    /// Like `finish`, for receiving: what arrived before a passing trouble ("not now") is handed out now.
    fn finish_receiving<T>(&mut self, result: Result<T>) -> Result<Vec<Event>> {
        match result {
            Err(error) if retryable(&error) && !self.events.is_empty() => self.finish(Ok(())),
            other => self.finish(other),
        }
    }

    // ---- the hub ----

    fn signed(&mut self, method: &str, path: &str, body: &[u8]) -> Result<Response> {
        // Each signature once (the hub's replay cache): strictly increasing times.
        let time = wall_ms().max(self.last_request_ms + 1);
        self.last_request_ms = time;
        let key = SigningKey::from_bytes(&self.state.auth_secret);
        let signature = key.sign(&yo_wire::request::to_be_signed(method, path, time, body));
        let headers = [
            (HEADER_DEVICE.to_owned(), yo_wire::b64(&self.state.mailbox)),
            (HEADER_TIME.to_owned(), time.to_string()),
            (HEADER_SIGNATURE.to_owned(), yo_wire::b64(&signature.to_bytes())),
        ];
        self.transport.request(method, path, &headers, body).map_err(Error::Network)
    }

    fn unsigned(&self, path: &str, body: &[u8]) -> Result<Response> {
        self.transport.request("POST", path, &[], body).map_err(Error::Network)
    }

    pub fn set_push(&mut self, push: Push) -> Result<()> {
        let response = self.signed("PUT", api::PUSH, &encode(&push))?;
        match response.status {
            status::NO_CONTENT => Ok(()),
            _ => Err(refused(&response)),
        }
    }

    fn lookup(&mut self, handle: &Handle, key_packages: bool) -> Result<api::LookupResult> {
        let body = encode(&api::Lookup { handle: handle.clone(), key_packages: key_packages as u8 });
        let response = self.signed("POST", api::LOOKUP, &body)?;
        match response.status {
            status::OK => Ok(decode(&response.body)?),
            status::NOT_FOUND => Err(Error::NotOnYo),
            _ => Err(refused(&response)),
        }
    }

    // ---- MLS ----

    fn signer(&self) -> &SignatureKeyPair {
        &self.state.signer
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
        let now = (self.clock)();
        if last_resort {
            for older in self.state.key_packages.iter_mut().filter(|k| k.last_resort && k.retired_at.is_none()) {
                older.retired_at = Some(now);
            }
        }
        self.state.key_packages.push(OwnKeyPackage {
            hash_ref: hash_ref.tls_serialize_detached().map_err(mls)?,
            created_at: now,
            last_resort,
            retired_at: None,
        });
        bundle.key_package().tls_serialize_detached().map_err(|_| Error::Internal("key package"))
    }

    /// Deletes the private keys of KeyPackages that can no longer arrive in a Welcome: expired ones, and last
    /// resorts replaced long enough ago that no Welcome made with them can still wait at the hub.
    fn forget_key_packages(&mut self) {
        let now = (self.clock)();
        let lifetime_ms = profile::KEY_PACKAGE_LIFETIME_S * 1000 + 24 * 3600 * 1000;
        let (dead, alive): (Vec<_>, Vec<_>) = std::mem::take(&mut self.state.key_packages).into_iter().partition(|k| {
            now.saturating_sub(k.created_at) > lifetime_ms
                || k.retired_at.is_some_and(|at| now.saturating_sub(at) > CLOSED_CHAT_KEPT_MS)
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

    /// The mailboxes of every member but this device.
    fn others(&self, group: &MlsGroup) -> Vec<Mailbox> {
        group
            .members()
            .filter_map(|m| identity::claimed(&m.credential))
            .map(|body| body.mailbox)
            .filter(|mailbox| *mailbox != self.state.mailbox)
            .collect()
    }

    fn enqueue(&mut self, mailboxes: &[Mailbox], message: &MlsMessageOut) -> Result<()> {
        let bytes = message.to_bytes().map_err(mls)?;
        let sealed = envelope::seal(&bytes)?;
        for mailbox in mailboxes {
            self.state.outbox.push_back(Outgoing { mailbox: *mailbox, envelope: sealed.clone() });
        }
        Ok(())
    }

    fn my_account_key(&self) -> [u8; 32] {
        SigningKey::from_bytes(&self.state.account_secret).verifying_key().to_bytes()
    }

    /// Pins a contact's account key; a different one than pinned for the same user is an identity change.
    fn pin(&mut self, user: &str, account_key: [u8; 32], passkey: bool, handle: Option<String>) {
        let contact = self.state.contacts.entry(user.to_owned()).or_insert(Contact {
            account_key,
            passkey,
            name: None,
            verified: false,
            blocked: false,
            stale: false,
            handles: Vec::new(),
        });
        if contact.account_key != account_key {
            let known = contact.account_key != [0; 32];
            contact.account_key = account_key;
            contact.verified = false;
            if known {
                self.events.push(Event::IdentityChanged { user: user.to_owned() });
            }
        }
        let contact = self.state.contacts.get_mut(user).expect("inserted");
        contact.passkey = passkey;
        contact.stale = false;
        if let Some(handle) = handle {
            if !contact.handles.contains(&handle) {
                contact.handles.push(handle);
            }
        }
    }

    /// The user a handle names, if this device knows: by its id, or by a handle it found the contact with.
    fn known_user(&self, handle: &Handle) -> Option<String> {
        if handle.kind == KIND_USER {
            return Some(hex(handle.value.as_slice()));
        }
        let shown = handle.display();
        self.state.contacts.iter().find(|(_, c)| c.handles.contains(&shown)).map(|(user, _)| user.clone())
    }

    /// The own handles: sending to them is sending to oneself.
    fn is_me(&self, handle: &Handle) -> bool {
        match handle.kind {
            KIND_USER => hex(handle.value.as_slice()) == self.state.user_id,
            KIND_PHONE => handle.value.as_slice() == self.state.phone.as_bytes(),
            KIND_USERNAME => self.state.username.as_deref().map(str::as_bytes) == Some(handle.value.as_slice()),
            _ => false,
        }
    }

    /// The chat to send to `peer` on: the one with the smallest group id (spec 4.5).
    fn chat_with(&self, peer: &str) -> Option<GroupId> {
        if self.state.contacts.get(peer).is_some_and(|c| c.stale) {
            return None;
        }
        self.state
            .chats
            .iter()
            .find(|(_, chat)| chat.peer == peer && chat.closed_at.is_none())
            .and_then(|(key, _)| state::unhex(key))
            .map(|id| GroupId::from_slice(&id))
    }

    fn drop_chat(&mut self, key: &str) {
        self.state.chats.remove(key);
        if let Some(id) = state::unhex(key) {
            if let Ok(Some(mut group)) = MlsGroup::load(self.provider.storage(), &GroupId::from_slice(&id)) {
                let _ = group.delete(self.provider.storage());
            }
        }
    }

    /// Creates a chat with whoever `handle` names (spec 4.2); returns the user id and the group.
    fn start_chat(&mut self, handle: &Handle) -> Result<(String, GroupId)> {
        let found = self.lookup(handle, true)?;
        let peer = hex(&found.user_id);
        if peer == self.state.user_id {
            return Err(Error::Protocol("sending to oneself"));
        }
        if found.key_packages.is_empty() {
            return Err(Error::Protocol("no key package"));
        }
        let mut key_packages = Vec::new();
        let mut passkey = true;
        for bytes in &found.key_packages {
            let key_package = KeyPackageIn::tls_deserialize_exact(bytes.as_slice())
                .map_err(mls)?
                .validate(self.provider.crypto(), ProtocolVersion::Mls10)
                .map_err(mls)?;
            if key_package.ciphersuite() != profile::CIPHERSUITE {
                return Err(Error::Protocol("suite"));
            }
            let leaf = key_package.leaf_node();
            let body = identity::member(leaf.credential(), leaf.signature_key().as_slice())?;
            if body.account_key != found.account_key {
                return Err(Error::Protocol("key package names someone else"));
            }
            passkey &= body.has_passkey();
            key_packages.push(key_package);
        }
        let shown = (handle.kind != KIND_USER).then(|| handle.display());
        self.pin(&peer, found.account_key, passkey, shown);
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
        let (_, welcome, _) = group.add_members(&self.provider, self.signer(), &key_packages).map_err(mls)?;
        group.merge_pending_commit(&self.provider).map_err(mls)?;
        let token = self.token(&group)?;
        let create = api::GroupCreate { group_id: id, epoch: group.epoch().as_u64(), token_hash: sha256(&token) };
        let response = self.unsigned(api::GROUPS, &encode(&create))?;
        if response.status != 201 {
            return Err(refused(&response));
        }
        let mailboxes = self.others(&group);
        self.enqueue(&mailboxes, &welcome)?;
        self.state.chats.insert(
            group_key(&group_id),
            Chat {
                peer: peer.clone(),
                last_update_ms: (self.clock)(),
                sent_since_update: 0,
                needs_update: false,
                closed_at: None,
                inflight: None,
                no_updates: false,
            },
        );
        if let Some(name) = self.state.name.clone() {
            self.send_in(&group_id, Content::Profile { name }, false)?;
        }
        Ok((peer, group_id))
    }

    fn update_due(&self, group: &GroupId) -> bool {
        self.state.chats.get(&group_key(group)).is_some_and(|chat| {
            chat.closed_at.is_none()
                && !chat.no_updates
                && (chat.needs_update
                    || (self.clock)().saturating_sub(chat.last_update_ms) >= UPDATE_AFTER_MS
                    || chat.sent_since_update >= UPDATE_AFTER_MESSAGES)
        })
    }

    /// Commits an update (spec 4.3, 4.4).
    fn update(&mut self, group_id: &GroupId) -> Result<Committed> {
        let mut group = self.load(group_id)?;
        let epoch = group.epoch().as_u64();
        let token = self.token(&group)?;
        let bundle = group.self_update(&self.provider, self.signer(), LeafNodeParameters::default()).map_err(mls)?;
        let next = group
            .pending_commit()
            .ok_or(Error::Internal("pending commit"))?
            .export_secret(self.provider.crypto(), EPOCH_TOKEN_LABEL, group_id.as_slice(), TOKEN_LEN)
            .map_err(mls)?;
        let inflight = Inflight {
            request: encode(&api::Commit { epoch, token, next_token_hash: sha256(&next) }),
            commit: bundle.commit().to_bytes().map_err(mls)?,
            mailboxes: self.others(&group),
        };
        // From here the commit may reach the hub: it is kept, whatever happens to the answer.
        self.state.chats.get_mut(&group_key(group_id)).ok_or(Error::Protocol("unknown group"))?.inflight =
            Some(inflight);
        self.resolve(group_id)
    }

    /// Sends a group's inflight commit (again) and acts on the answer. The hub answers a repeat of an accepted
    /// commit with 200, so a lost answer costs only the repeat.
    fn resolve(&mut self, group_id: &GroupId) -> Result<Committed> {
        let key = group_key(group_id);
        let Some(inflight) = self.state.chats.get(&key).and_then(|c| c.inflight.clone()) else {
            return Ok(Committed::Nothing);
        };
        let id: yo_wire::GroupId = group_id.as_slice().try_into().map_err(|_| Error::Protocol("group id"))?;
        let response = match self.unsigned(&api::commit_path(&id), &inflight.request) {
            Ok(response) => response,
            Err(error) => return Ok(Committed::Unknown(error.to_string())),
        };
        let mut group = self.load(group_id)?;
        let outcome = match response.status {
            status::OK => {
                group.merge_pending_commit(&self.provider).map_err(mls)?;
                let sealed = envelope::seal(&inflight.commit)?;
                for mailbox in &inflight.mailboxes {
                    self.state.outbox.push_back(Outgoing { mailbox: *mailbox, envelope: sealed.clone() });
                }
                Committed::Accepted
            }
            status::CONFLICT => {
                group.clear_pending_commit(self.provider.storage()).map_err(mls)?;
                Committed::Lost
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
                Committed::Refused => chat.no_updates = true,
                _ => {}
            }
        }
        Ok(outcome)
    }

    /// Settles an inflight commit and updates the chat if due. A lost race (409) is resolved by receiving the
    /// winner's commit. `Err` only when a commit's answer is still open: the group cannot be used until it is.
    fn ensure_fresh(&mut self, group: &GroupId) -> Result<()> {
        if let Committed::Unknown(error) = self.resolve(group)? {
            return Err(Error::Network(error));
        }
        for _ in 0..4 {
            if !self.update_due(group) {
                return Ok(());
            }
            match self.atomic(|c| c.update(group)) {
                Ok(Committed::Accepted | Committed::Refused | Committed::Nothing) => return Ok(()),
                Ok(Committed::Unknown(error)) => return Err(Error::Network(error)),
                Ok(Committed::Lost) => match self.receive_all(1) {
                    Ok(()) => {}
                    Err(error) if retryable(&error) => return Ok(()),
                    Err(error) => return Err(error),
                },
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
        let mut group = self.load(group_id)?;
        let bytes = Message { sent_at_ms: wall_ms(), content }.encode()?;
        let message = group.create_message(&self.provider, self.signer(), &bytes).map_err(mls)?;
        let mailboxes = self.others(&group);
        self.enqueue(&mailboxes, &message)?;
        if count {
            if let Some(chat) = self.state.chats.get_mut(&group_key(group_id)) {
                chat.sent_since_update += 1;
            }
        }
        Ok(())
    }

    // ---- delivery ----

    /// Sends the outbox in order, per mailbox. Returns the peers whose device is gone (410).
    fn flush(&mut self) -> BTreeSet<String> {
        // Saved before anything leaves: a crash after the hub took an envelope must not bring back the ratchet
        // state that made it. A repeat after a crash is harmless, the receiver skips what it has seen.
        if !self.state.outbox.is_empty() {
            self.persist();
        }
        let mut gone_peers = BTreeSet::new();
        let mut gone = BTreeSet::new();
        let mut waiting = BTreeSet::new();
        let mut kept = VecDeque::new();
        while let Some(item) = self.state.outbox.pop_front() {
            if gone.contains(&item.mailbox) {
                continue;
            }
            if waiting.contains(&item.mailbox) {
                kept.push_back(item);
                continue;
            }
            match self.unsigned(&api::deliver_path(&item.mailbox), &item.envelope) {
                Ok(response) if response.status == status::ACCEPTED => {}
                Ok(response) if response.status == status::GONE => {
                    gone.insert(item.mailbox);
                    gone_peers.extend(self.mailbox_gone(&item.mailbox));
                }
                Ok(response)
                    if matches!(response.status, status::NOT_FOUND | status::BAD_REQUEST | status::TOO_LARGE) =>
                {
                    self.drop_one(format!("hub refused delivery: {}", response.status));
                }
                // Rate limit, hub trouble, no network: later, in the same order.
                _ => {
                    waiting.insert(item.mailbox);
                    kept.push_back(item);
                }
            }
        }
        kept.extend(self.state.outbox.drain(..));
        self.state.outbox = kept;
        gone_peers
    }

    /// A member's device was replaced: its chats are closed for sending, the contact must be looked up again.
    fn mailbox_gone(&mut self, mailbox: &Mailbox) -> Vec<String> {
        let mut peers = Vec::new();
        let keys: Vec<String> = self.state.chats.keys().cloned().collect();
        for key in keys {
            let Some(id) = state::unhex(&key) else { continue };
            let Ok(group) = self.load(&GroupId::from_slice(&id)) else { continue };
            if self.others(&group).contains(mailbox) {
                let now = (self.clock)();
                if let Some(chat) = self.state.chats.get_mut(&key) {
                    peers.push(chat.peer.clone());
                    chat.closed_at.get_or_insert(now);
                }
            }
        }
        for peer in &peers {
            if let Some(contact) = self.state.contacts.get_mut(peer) {
                contact.stale = true;
            }
        }
        peers
    }

    // ---- receiving ----

    /// Fetches and processes the inbox until it is empty. Transport failures stop it with nothing lost.
    fn receive_all(&mut self, wait_s: u64) -> Result<()> {
        let mut wait = wait_s;
        loop {
            let response = self.signed("GET", &api::inbox_path(wait), &[])?;
            if response.status != status::OK {
                return Err(refused(&response));
            }
            let inbox: api::Inbox = decode(&response.body)?;
            self.state.key_packages_left = inbox.key_packages_left;
            let full = inbox.items.len() >= MAX_INBOX_FETCH;
            let mut done = Vec::new();
            let mut stopped = None;
            for item in inbox.items {
                let hash = sha256(item.envelope.as_slice());
                if !self.state.seen.contains(&hash) {
                    match self.atomic(|c| c.receive(item.envelope.as_slice())) {
                        Ok(()) => {}
                        // Not this envelope's fault: it stays at the hub and is read again next time.
                        Err(error) if retryable(&error) => {
                            stopped = Some(error);
                            break;
                        }
                        Err(error) => self.drop_one(error.to_string()),
                    }
                    self.state.seen.push_back(hash);
                    if self.state.seen.len() > SEEN {
                        self.state.seen.pop_front();
                    }
                }
                done.push(item.id);
            }
            if !done.is_empty() {
                // Saved before the hub may forget: what was processed must survive a crash right after the ack.
                self.persist();
                // An ack that does not arrive costs only a repeat: the hub sends those envelopes again, and they
                // are skipped as seen.
                match self.signed("POST", api::INBOX_ACK, &encode(&api::Ack { ids: done })) {
                    Ok(response) if response.status == status::NO_CONTENT => {}
                    Ok(response) if response.status == status::GONE => return Err(Error::Replaced),
                    _ => return Ok(()),
                }
            }
            if let Some(error) = stopped {
                return Err(error);
            }
            if !full {
                return Ok(());
            }
            wait = 0;
        }
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

    /// Someone started a chat with this device (spec 4.2).
    fn join(&mut self, welcome: Welcome) -> Result<()> {
        if welcome.ciphersuite() != profile::CIPHERSUITE {
            return Err(Error::Protocol("suite"));
        }
        let staged =
            StagedWelcome::new_from_welcome(&self.provider, &profile::join_config(), welcome, None).map_err(mls)?;
        let mut peer: Option<([u8; 32], bool)> = None;
        let mut me = false;
        let account_key = self.my_account_key();
        for member in staged.members() {
            let body = identity::member(&member.credential, &member.signature_key)?;
            if body.account_key == account_key {
                me |= body.mailbox == self.state.mailbox;
                continue;
            }
            match &peer {
                None => peer = Some((body.account_key, body.has_passkey())),
                Some((key, _)) if *key == body.account_key => {}
                // v1: a chat between exactly two accounts.
                Some(_) => return Err(Error::Protocol("more than two accounts")),
            }
        }
        let (peer_key, passkey) = peer.ok_or(Error::Protocol("nobody else"))?;
        let sender = staged
            .members()
            .find(|m| m.index == staged.welcome_sender_index())
            .and_then(|m| identity::claimed(&m.credential))
            .ok_or(Error::Protocol("sender"))?;
        if !me || sender.account_key != peer_key {
            return Err(Error::Protocol("welcome"));
        }
        let ignore = |c: &mut Self| -> Result<()> {
            // Kept, not rolled back: the group is remembered as ignored, and what follows in it is discarded.
            let id: [u8; 32] =
                staged.group_context().group_id().as_slice().try_into().map_err(|_| Error::Protocol("group id"))?;
            c.ignore(id);
            Ok(())
        };
        // Who is it? A contact already pinned to this key, or the hub's answer for the key: nobody can start a chat
        // in someone else's name. A key that is no user's current key is a replaced device's (or a forger's):
        // either way the chat is not joined.
        let known =
            self.state.contacts.iter().find(|(_, c)| !c.stale && c.account_key == peer_key).map(|(u, _)| u.clone());
        let peer = match known {
            Some(user) => user,
            None => match self.lookup(&Handle::account_key(&peer_key), false) {
                Ok(found) if found.account_key == peer_key => hex(&found.user_id),
                Ok(_) | Err(Error::NotOnYo) => return ignore(self),
                Err(error) => return Err(error),
            },
        };
        if self.state.contacts.get(&peer).is_some_and(|c| c.blocked) {
            return ignore(self);
        }
        self.pin(&peer, peer_key, passkey, None);
        let group = staged.into_group(&self.provider).map_err(mls)?;
        if group.ciphersuite() != profile::CIPHERSUITE || group.group_id().as_slice().len() != 32 {
            return Err(Error::Protocol("group"));
        }
        self.state.chats.insert(
            group_key(group.group_id()),
            Chat {
                peer: peer.clone(),
                last_update_ms: (self.clock)(),
                sent_since_update: 0,
                needs_update: true,
                closed_at: None,
                inflight: None,
                no_updates: false,
            },
        );
        self.events.push(Event::NewChat { user: peer });
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
        let peer = chat.peer.clone();
        let inflight = chat.inflight.is_some();
        let mut group = self.load(&group_id)?;
        // The accounts in the group, as their certificates were checked when they joined.
        let accounts: Vec<[u8; 32]> =
            group.members().filter_map(|m| identity::claimed(&m.credential)).map(|body| body.account_key).collect();
        // A commit from someone else for this epoch means the hub took theirs: ours, still unanswered, lost.
        if message.content_type() == ContentType::Commit && inflight {
            group.clear_pending_commit(self.provider.storage()).map_err(mls)?;
            if let Some(chat) = self.state.chats.get_mut(&key) {
                chat.inflight = None;
            }
        }
        let (message_epoch, group_epoch) = (message.epoch().as_u64(), group.epoch().as_u64());
        let processed = group
            .process_message(&self.provider, message)
            .map_err(|e| Error::Mls(format!("{e:?} (message epoch {message_epoch}, group epoch {group_epoch})")))?;
        let sender = identity::claimed(processed.credential()).ok_or(Error::Protocol("sender"))?;
        let from_me = sender.account_key == self.my_account_key();
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(application) => {
                let message = Message::decode(&application.into_bytes())?;
                if self.state.contacts.get(&peer).is_some_and(|c| c.blocked) {
                    return Ok(());
                }
                match message.content {
                    Content::Profile { name } if !from_me => {
                        if let Some(contact) = self.state.contacts.get_mut(&peer) {
                            contact.name = Some(name.clone());
                        }
                        self.events.push(Event::Profile { user: peer, name });
                    }
                    Content::Profile { .. } => {}
                    content if from_me => {
                        self.events.push(Event::SentElsewhere { user: peer, sent_at_ms: message.sent_at_ms, content })
                    }
                    content => self.events.push(Event::Message { user: peer, sent_at_ms: message.sent_at_ms, content }),
                }
                Ok(())
            }
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                self.check_commit(&staged, &sender, &accounts)?;
                group.merge_staged_commit(&self.provider, *staged).map_err(mls)?;
                if !group.is_active() {
                    self.drop_chat(&key);
                }
                Ok(())
            }
            _ => Err(Error::Protocol("proposals are not used")),
        }
    }

    /// A commit may add devices only of the accounts already in the group, and a new leaf must be its sender's.
    fn check_commit(
        &self,
        staged: &StagedCommit,
        sender: &yo_wire::cert::CertificateBody,
        accounts: &[[u8; 32]],
    ) -> Result<()> {
        let allowed = |body: &yo_wire::cert::CertificateBody| accounts.contains(&body.account_key);
        // v1 commits change membership and keys, nothing else: no extensions, PSKs, re-inits.
        let allowed_kind =
            |p: &QueuedProposal| matches!(p.proposal(), Proposal::Add(_) | Proposal::Update(_) | Proposal::Remove(_));
        if !staged.queued_proposals().all(allowed_kind) {
            return Err(Error::Protocol("proposal type not allowed"));
        }
        for add in staged.add_proposals() {
            let leaf = add.add_proposal().key_package().leaf_node();
            let body = identity::member(leaf.credential(), leaf.signature_key().as_slice())?;
            if !allowed(&body) {
                return Err(Error::Protocol("added a stranger"));
            }
        }
        if let Some(leaf) = staged.update_path_leaf_node() {
            let body = identity::member(leaf.credential(), leaf.signature_key().as_slice())?;
            if body.account_key != sender.account_key || !allowed(&body) {
                return Err(Error::Protocol("leaf of someone else"));
            }
        }
        Ok(())
    }

    fn refill(&mut self) -> Result<()> {
        let left = self.state.key_packages_left;
        let last_resort_due = (self.clock)().saturating_sub(self.state.last_resort_at) >= LAST_RESORT_EVERY_MS;
        if left >= KEY_PACKAGE_REFILL_BELOW && !last_resort_due {
            return Ok(());
        }
        self.atomic(|c| {
            let fresh = if left < KEY_PACKAGE_REFILL_BELOW { KEY_PACKAGE_TARGET - left } else { 0 };
            let key_packages = (0..fresh).map(|_| c.key_package(false).map(Into::into)).collect::<Result<Vec<_>>>()?;
            let last_resort = if last_resort_due { Some(c.key_package(true)?.into()) } else { None };
            let response =
                c.signed("PUT", api::KEY_PACKAGES, &encode(&api::KeyPackages { key_packages, last_resort }))?;
            if response.status != status::NO_CONTENT {
                return Err(refused(&response));
            }
            c.state.key_packages_left = left + fresh;
            if last_resort_due {
                c.state.last_resort_at = (c.clock)();
            }
            Ok(())
        })
    }

    // ---- what the app calls ----

    /// Sends a Yo to "+49 170 …", "chris", or a user id from an event.
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
        let mut user = self.known_user(&handle);
        if user.as_ref().and_then(|u| self.state.contacts.get(u)).is_some_and(|c| c.blocked) {
            return Err(Error::Blocked);
        }
        if let Content::Unknown(_) = content {
            return Err(Error::Protocol("unknown content"));
        }
        self.flush();
        // Receive first: a commit waiting in the inbox would otherwise leave this message in an epoch the peer
        // may already have left behind. Offline, the message still goes into the outbox.
        match self.receive_all(0) {
            Ok(()) | Err(Error::Network(_)) => {}
            Err(error) => return Err(error),
        }
        // A handle this device does not know may still name a contact (someone who wrote first is known by user
        // id only): ask who it is, without spending a KeyPackage, so the existing chat is used and not a second one.
        if user.is_none() && handle.kind != KIND_USER && !self.state.contacts.is_empty() {
            let found = self.lookup(&handle, false)?;
            let id = hex(&found.user_id);
            if id == self.state.user_id {
                return Err(Error::Protocol("sending to oneself"));
            }
            if let Some(contact) = self.state.contacts.get_mut(&id) {
                let shown = handle.display();
                if !contact.handles.contains(&shown) {
                    contact.handles.push(shown);
                }
            }
            user = Some(id);
        }
        if user.as_ref().and_then(|u| self.state.contacts.get(u)).is_some_and(|c| c.blocked) {
            return Err(Error::Blocked);
        }
        for _ in 0..2 {
            let (peer, group) = match user.as_ref().and_then(|u| self.chat_with(u).map(|g| (u.clone(), g))) {
                Some(found) => found,
                None => self.atomic(|c| c.start_chat(&handle))?,
            };
            if self.state.contacts.get(&peer).is_some_and(|c| c.blocked) {
                return Err(Error::Blocked);
            }
            self.ensure_fresh(&group)?;
            let content = content.clone();
            self.atomic(|c| c.send_in(&group, content, true))?;
            // The peer's device was replaced while this was in flight: a new chat, the message again.
            if !self.flush().contains(&peer) {
                return Ok(());
            }
            user = Some(peer);
        }
        Ok(())
    }

    /// Receives: waits up to `wait_s` seconds for mail, processes all of it, delivers what is waiting.
    pub fn sync(&mut self, wait_s: u64) -> Result<Vec<Event>> {
        self.flush();
        let mut result = self.receive_all(wait_s);
        if result.is_ok() {
            result = self.refill();
        }
        self.flush();
        self.finish_receiving(result)
    }

    /// What the app runs when it comes to the foreground: receive, refill KeyPackages, update due chats.
    pub fn maintenance(&mut self) -> Result<Vec<Event>> {
        let result = self.maintenance_inner();
        self.finish_receiving(result)
    }

    fn maintenance_inner(&mut self) -> Result<()> {
        self.flush();
        self.receive_all(0)?;
        self.refill()?;
        let now = (self.clock)();
        let expired: Vec<String> = self
            .state
            .chats
            .iter()
            .filter(|(_, chat)| chat.closed_at.is_some_and(|at| now.saturating_sub(at) > CLOSED_CHAT_KEPT_MS))
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            self.drop_chat(&key);
        }
        let groups: Vec<GroupId> =
            self.state.chats.keys().filter_map(|key| state::unhex(key)).map(|id| GroupId::from_slice(&id)).collect();
        // One chat's trouble does not keep the others from their updates.
        let mut first_error = None;
        for group in groups {
            if let Err(error) = self.ensure_fresh(&group) {
                first_error.get_or_insert(error);
            }
        }
        self.forget_key_packages();
        self.flush();
        first_error.map_or(Ok(()), Err)
    }

    /// Sets the name contacts see, and tells every chat.
    pub fn set_name(&mut self, name: &str) -> Result<Vec<Event>> {
        let result = (|| {
            Message { sent_at_ms: 0, content: Content::Profile { name: name.to_owned() } }.encode()?;
            self.state.name = Some(name.to_owned());
            let groups: Vec<GroupId> = self
                .state
                .chats
                .iter()
                .filter(|(_, chat)| chat.closed_at.is_none())
                .filter_map(|(key, _)| state::unhex(key))
                .map(|id| GroupId::from_slice(&id))
                .collect();
            for group in groups {
                self.atomic(|c| c.send_in(&group, Content::Profile { name: name.to_owned() }, false))?;
            }
            self.flush();
            Ok(())
        })();
        self.finish(result)
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
                can_verify: c.passkey,
                blocked: c.blocked,
            })
            .collect()
    }

    /// The contact a handle names, as this device knows it.
    fn contact_user(&self, who: &str) -> Result<String> {
        let user = self.known_user(&Handle::parse(who)?).ok_or(Error::NotOnYo)?;
        Ok(user)
    }

    /// The safety number with a contact (spec 3).
    pub fn safety_number(&self, who: &str) -> Result<String> {
        let user = self.contact_user(who)?;
        let contact = self.state.contacts.get(&user).ok_or(Error::NotOnYo)?;
        let mine = state::unhex(&self.state.user_id).unwrap_or_default();
        let theirs = state::unhex(&user).unwrap_or_default();
        Ok(identity::safety_number((&mine, &self.my_account_key()), (&theirs, &contact.account_key)))
    }

    /// Marks a contact verified after comparing safety numbers; only for identities backed by a passkey.
    pub fn set_verified(&mut self, who: &str, verified: bool) -> Result<()> {
        let user = self.contact_user(who)?;
        let contact = self.state.contacts.get_mut(&user).ok_or(Error::NotOnYo)?;
        if verified && !contact.passkey {
            return Err(Error::Protocol("identity not backed by a passkey"));
        }
        contact.verified = verified;
        self.persist();
        Ok(())
    }

    /// Blocks a contact (by user id, or a handle this device knows them by): nothing from them is shown, no chat
    /// with them is joined.
    pub fn set_blocked(&mut self, who: &str, blocked: bool) -> Result<()> {
        let user = self.contact_user(who)?;
        let contact = self.state.contacts.entry(user).or_insert(Contact {
            account_key: [0; 32],
            passkey: false,
            name: None,
            verified: false,
            blocked,
            stale: true,
            handles: Vec::new(),
        });
        contact.blocked = blocked;
        self.persist();
        Ok(())
    }

    /// Claims a username ("chris": found as CHRIS), or gives it up with `None`.
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
        self.persist();
        Ok(())
    }
}
