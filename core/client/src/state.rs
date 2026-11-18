//! What a device keeps (spec 2): its keys, contacts, the outbox and the session store, sealed as one blob under a
//! key the platform holds in its keychain.

use crate::error::{Error, Result};
use crate::identity::Key;
use crate::signal::Entries;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::ChaCha20Poly1305;
use serde::{Deserialize, Serialize};

/// Bytes in the sealed JSON as base64url text, not as an array of numbers (a third of the size, less to seal on every
/// save); the arrays of states saved before are read too.
mod compact {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};
    use std::collections::VecDeque;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Bytes {
        Text(String),
        Numbers(Vec<u8>),
    }

    impl Bytes {
        fn bytes<E: serde::de::Error>(self) -> Result<Vec<u8>, E> {
            match self {
                Bytes::Text(text) => yo_wire::unb64(&text).map_err(|_| E::custom("base64")),
                Bytes::Numbers(bytes) => Ok(bytes),
            }
        }
    }

    pub mod bytes {
        use super::*;
        pub fn serialize<S: Serializer>(value: &[u8], s: S) -> Result<S::Ok, S::Error> {
            s.serialize_str(&yo_wire::b64(value))
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
            Bytes::deserialize(d)?.bytes()
        }
    }

    pub mod optional {
        use super::*;
        pub fn serialize<S: Serializer>(value: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
            match value {
                Some(value) => s.serialize_some(&yo_wire::b64(value)),
                None => s.serialize_none(),
            }
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
            Option::<Bytes>::deserialize(d)?.map(Bytes::bytes).transpose()
        }
    }

    pub mod list {
        use super::*;
        pub fn serialize<S: Serializer>(value: &VecDeque<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
            s.collect_seq(value.iter().map(|v| yo_wire::b64(v)))
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<VecDeque<Vec<u8>>, D::Error> {
            Vec::<Bytes>::deserialize(d)?.into_iter().map(Bytes::bytes).collect()
        }
    }

    pub mod hashes {
        use super::*;
        pub fn serialize<S: Serializer>(value: &VecDeque<[u8; 32]>, s: S) -> Result<S::Ok, S::Error> {
            s.collect_seq(value.iter().map(|v| yo_wire::b64(v)))
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<VecDeque<[u8; 32]>, D::Error> {
            Vec::<Bytes>::deserialize(d)?
                .into_iter()
                .map(|b| b.bytes().and_then(|v: Vec<u8>| <[u8; 32]>::try_from(v).map_err(|_| D::Error::custom("hash"))))
                .collect()
        }
    }
}
use std::collections::{BTreeMap, VecDeque};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};
use yo_wire::DeviceId;
use zeroize::Zeroize;

/// 2: the account and auth keys are libsignal's (Curve25519). A state of 1 (Ed25519 keys) is brought up (`from_1`).
const STATE_VERSION: u8 = 2;
const AAD: &[u8] = b"yo state v2";

/// Resend plaintexts are read on nearly every transaction and changed only by a send.
/// Share their snapshot until changed, keeping rollback cheap without changing the JSON format.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SentCache(std::sync::Arc<VecDeque<Vec<u8>>>);
impl Clone for SentCache {
    fn clone(&self) -> Self {
        #[cfg(feature = "testing")]
        if crate::cost::copies_snapshots() {
            return Self(std::sync::Arc::new((*self.0).clone()));
        }
        Self(self.0.clone())
    }
}
impl std::ops::Deref for SentCache {
    type Target = VecDeque<Vec<u8>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for SentCache {
    fn deref_mut(&mut self) -> &mut Self::Target {
        std::sync::Arc::make_mut(&mut self.0)
    }
}
impl FromIterator<Vec<u8>> for SentCache {
    fn from_iter<T: IntoIterator<Item = Vec<u8>>>(iter: T) -> Self {
        Self(std::sync::Arc::new(iter.into_iter().collect()))
    }
}
impl Serialize for SentCache {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        compact::list::serialize(self, s)
    }
}
impl<'de> Deserialize<'de> for SentCache {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        compact::list::deserialize(d).map(|v| Self(std::sync::Arc::new(v)))
    }
}

/// A device of a user as this device knows it: its key, and its number among the user's devices, as the hub gave it
/// (spec 4.1): the session's address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub key: Key,
    pub slot: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    /// PINGTEST routing state; hidden only until explicitly added or written to.
    #[serde(default)]
    pub probe_service: bool,
    #[serde(default)]
    pub probe_reply_tag: [u8; 32],
    #[serde(default)]
    pub probe_pending: Option<[u8; 32]>,
    /// The account key pinned on first contact, or the one confirmed after a change.
    pub account_key: Key,
    pub name: Option<String>,
    pub verified: bool,
    /// The key changed after the contact was verified: nothing is sent until the person confirms it (spec 3.4).
    pub unconfirmed: bool,
    pub blocked: bool,
    /// How this device knows the contact: "Chris". A username as its owner writes it, as the hub tells (also for
    /// someone who wrote first), one per skeleton.
    pub handles: Vec<String>,
    /// The hub no longer knows the user (the account was deleted, or its name released): nothing more can reach it.
    /// Its handles are forgotten, so a name it had finds whoever has it now.
    pub gone: bool,
    /// The user's devices as the hub named them last (spec 4.4).
    pub devices: Vec<Slot>,
    /// Something was sent to the user or came from them: a chat the app shows.
    pub chat: bool,
    /// The own name changed and is not told to the user yet (`set_name`): it goes when it can.
    pub profile_due: bool,
    /// The plaintexts of the last messages sent to the user: one goes again when a `DecryptionErrorMessage` names
    /// its time (spec 4.6).
    pub sent: SentCache,
    /// Something came from the user since the last receipt (spec 4.5).
    pub receipt_due: bool,
    /// The `DecryptionErrorMessage`s due to its devices, for their messages that did not open here (spec 4.6).
    #[serde(default)]
    pub decryption_errors: Vec<DecryptionError>,
    /// When the hub last named devices of the user without prekeys (wall clock, ms; 0: never, or they have them
    /// now): an app too old for sessions, which must be updated. What waits for it stays (spec 4.2).
    pub outdated_at: u64,
    /// Accepted as a contact (spec 4.10): added or written to from here, or a request said yes to. Until then what
    /// comes from them is a request: no receipt, no name goes to them. Contacts from before requests are accepted.
    #[serde(default = "yes")]
    pub accepted: bool,
    /// Their profile key, as they sent it: its access key shown on deliveries to them, so they come as a contact's.
    /// A contact key from before is taken as their profile key, as they take theirs (32 bytes either way).
    #[serde(default, alias = "their_key")]
    pub profile_key: Option<[u8; 32]>,
    /// This account's profile key is still to go to them (accepted, a group's member, or a new key after a block).
    #[serde(default)]
    pub key_due: bool,
    /// The current profile key went to them (queued): not again until a new key.
    #[serde(default)]
    pub key_given: bool,
    /// When this account's key last went back to them for a message of theirs that came identified (wall clock, ms):
    /// once an hour at most (spec 4.10).
    #[serde(default)]
    pub key_resent_at: u64,
    /// The latest generation of their profile key seen (0: none told): a carried key of no later one, other than the
    /// one held, is not taken.
    #[serde(default)]
    pub key_gen: u32,
}

/// A `DecryptionErrorMessage` due to a device of a contact (spec 4.6): its device key, and the message as
/// libsignal's `PlaintextContent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecryptionError {
    pub device: Key,
    #[serde(with = "compact::bytes")]
    pub content: Vec<u8>,
}

/// What a field from before it was there is: true.
pub fn yes() -> bool {
    true
}

impl Contact {
    /// A contact known by its user id only, no key pinned yet.
    pub fn unknown() -> Contact {
        Contact {
            probe_service: false,
            probe_pending: None,
            probe_reply_tag: [0; 32],
            account_key: [0; 32],
            name: None,
            verified: false,
            unconfirmed: false,
            blocked: false,
            handles: Vec::new(),
            gone: false,
            devices: Vec::new(),
            chat: false,
            profile_due: false,
            sent: SentCache::default(),
            receipt_due: false,
            decryption_errors: Vec::new(),
            outdated_at: 0,
            accepted: false,
            profile_key: None,
            key_due: false,
            key_given: false,
            key_resent_at: 0,
            key_gen: 0,
        }
    }
}

/// This device's registration (`POST /v1/register`, spec 6.4), saved before it leaves and kept until the hub
/// answered: after a crash it is finished from the state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registering {
    #[serde(with = "compact::bytes")]
    pub request: Vec<u8>,
    /// Sent before: the hub may have taken it (then this device can sign, and is asked about first).
    pub sent: bool,
}

/// A message for one user, waiting for the hub to take it (spec 4.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outgoing {
    /// When queued (wall clock, milliseconds). Old states start their retention period when first pruned.
    #[serde(default)]
    pub queued_at: u64,
    /// Its place in the order of everything this device sent.
    pub seq: u64,
    /// The user it goes to (hex): a contact, or this account (a copy for its other devices).
    pub user: String,
    /// The session plaintext (`envelope::Plain`), sealed in the state: encrypted for the user's devices as they are
    /// when it goes. For `device`: a `PlaintextContent`, as it goes.
    #[serde(with = "compact::bytes")]
    pub plain: Vec<u8>,
    /// Only for this device of the user, outside the session: a `DecryptionErrorMessage` (spec 4.6).
    #[serde(default)]
    pub device: Option<Key>,
    /// A decryption-error resend: catch up silently even when its content was visible.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub non_urgent: bool,
    /// Its payload once encrypted (`envelope::Payload`), with the devices it was made for: made again if they
    /// changed. Sealed as it goes, with the sender certificate of then (spec 6.1).
    #[serde(default, with = "compact::optional")]
    pub payload: Option<Vec<u8>>,
    pub made_for: Vec<Key>,
    /// Refused (`403`, spec 4.10) though sent with the recipient's key: their new key may wait in the inbox, so it
    /// goes again after the next receive, with it or as a request. Only a request's `403` drops it.
    #[serde(default)]
    pub refused: bool,
}

/// This device's prekeys (spec 4.1): the ids handed out so far, when the signed and last resort ones were made, the
/// hub's count, an upload under way.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prekeys {
    pub next_id: u32,
    pub signed_at: u64,
    /// One-time prekeys of the scarcer kind the hub still has.
    pub left: u32,
    /// Made and not yet taken by the hub: sent again as it is until it answers (`PUT /v1/prekeys`, encoded), and
    /// how many one-time prekeys of each kind it brings.
    pub upload: Option<(Vec<u8>, u32)>,
    /// Signed prekeys (`sp`) and last resorts (`kl`) replaced at the hub, with when (wall clock, ms): their private keys
    /// go a month later, when no session started with them can still wait there.
    pub retired: Vec<(String, u32, u64)>,
}

/// A group as this device has it from the hub (spec 4.9): kept for good, also once left (it shows as left).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// Its master key (hex): what its id, its secret params and every ciphertext of it at the hub come from.
    pub key: String,
    pub name: String,
    /// Its members by user id (hex), sorted, this account among them while it is in.
    pub members: Vec<String>,
    /// The revision this device fetched (0: not yet).
    pub revision: u32,
    /// This account is no member of it (left, or removed).
    pub left: bool,
    /// Joined here: made by this account, added by one that had its profile key or by an accepted contact, or said
    /// yes to. Until then a group someone else added this account to is a request (spec 4.10).
    pub accepted: bool,
    /// This account's own entry at the hub has its profile key (else it was invited, by its user id alone).
    #[serde(default)]
    pub keyed: bool,
    /// The newest revision a message named (spec 4.9): above `revision`, the group is fetched again.
    #[serde(default)]
    pub named: u32,
    /// Group send endorsements for its members (spec 4.9): until when (seconds), and each member's (hex, compressed).
    #[serde(default)]
    pub endorsed_until: u64,
    #[serde(default)]
    pub endorsements: BTreeMap<String, String>,
}

/// An event as the state keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredEvent {
    NewChat { user: String },
    Message { user: String, sent_at_ms: u64, content: StoredContent, own: bool },
    GroupMessage { group: String, user: String, sent_at_ms: u64, content: StoredContent, own: bool },
    GroupChanged { group: String },
    Profile { user: String, name: String },
    IdentityChanged { user: String },
}

/// A photo or video as the state keeps it until the app has it (spec 5).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMedia {
    pub id: [u8; 16],
    pub key: [u8; 32],
    pub sha256: [u8; 32],
    pub size: u32,
    pub width: u16,
    pub height: u16,
}

impl std::fmt::Debug for StoredMedia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredMedia").field("size", &self.size).finish_non_exhaustive()
    }
}

impl From<yo_wire::content::Media> for StoredMedia {
    fn from(m: yo_wire::content::Media) -> Self {
        StoredMedia { id: m.id, key: m.key, sha256: m.sha256, size: m.size, width: m.width, height: m.height }
    }
}

impl From<StoredMedia> for yo_wire::content::Media {
    fn from(m: StoredMedia) -> Self {
        yo_wire::content::Media {
            id: m.id,
            key: m.key,
            sha256: m.sha256,
            size: m.size,
            width: m.width,
            height: m.height,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredContent {
    Yo,
    Photo(StoredMedia),
    Video { media: StoredMedia, duration_ms: u32 },
    Text(String),
    Link(String),
    Location { lat_e7: i32, lon_e7: i32 },
    Profile(String),
    Unknown(u16),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct State {
    /// The own user id (hex), as the hub gave it.
    pub user_id: String,
    pub username: Option<String>,
    pub account_secret: [u8; 32],
    pub auth_secret: [u8; 32],
    /// The hub's name for this device's inbox: never given to other users.
    pub device_id: DeviceId,
    /// Its name among the account's devices, which senders encrypt for and the certificate vouches for (spec 3).
    pub device_key: Key,
    /// Its number among the account's devices, as the hub gave it at registration: its sessions' address.
    pub device_number: u8,
    #[serde(with = "compact::bytes")]
    pub certificate: Vec<u8>,
    /// Its registration id in its sessions.
    pub registration_id: u32,
    pub name: Option<String>,
    /// By hex user id.
    pub contacts: BTreeMap<String, Contact>,
    /// The account's other devices, as this device knows them (spec 4.4).
    pub own: Vec<Slot>,
    /// Messages not yet taken by the hub, in order.
    pub outbox: VecDeque<Outgoing>,
    pub next_seq: u64,
    /// What happened and the app has not been handed yet: saved with the state, so nothing received is lost to a
    /// crash between the hub's ack and the app.
    pub events: Vec<StoredEvent>,
    /// Handed to the app by the last call: kept until the next call, which acknowledges them. After a crash they come
    /// again: once in normal operation, at least once across a crash.
    pub handed: Vec<StoredEvent>,
    /// Hashes of session messages already processed, against redelivery.
    #[serde(with = "compact::hashes")]
    pub seen: VecDeque<[u8; 32]>,
    pub prekeys: Prekeys,
    /// The time of the last signed request (spec 6.2): the next is signed later, whichever process signs it.
    pub signed_until: u64,
    /// The account's storage as this device has it (spec 6.9), merged with the hub's.
    pub storage: crate::storage::Storage,
    /// The hub's version the storage was last written or read as.
    pub storage_version: u64,
    /// Changed here since it was last written.
    pub storage_dirty: bool,
    /// When the hub last held all of it (wall clock, ms): written, or fetched with nothing left to write. 0: not yet.
    pub storage_saved_at: u64,
    /// Fetched from the hub on this device (once, before the first message is received).
    pub storage_fetched: bool,
    /// The key the storage was sealed under before the account key was libsignal's (from its Ed25519 seed, spec
    /// 6.9): a storage of then opens with it until this device has written it under the account's key of now.
    #[serde(default)]
    pub legacy_storage_key: Option<[u8; 32]>,
    /// The time of the last message sent: each later one strictly later.
    pub last_sent_ms: u64,
    /// Hashes of the last messages received (sender and plaintext): a copy sent again (4.6) is skipped.
    #[serde(with = "compact::hashes")]
    pub received: VecDeque<[u8; 32]>,
    /// This account's profile key (spec 4.10): its contacts show its access key on deliveries; none yet before the
    /// first sync. The contact key from before is kept as it (the contacts have it already); only its check is told
    /// anew (`access_key_told` starts false).
    #[serde(default, alias = "contact_key")]
    pub profile_key: Option<[u8; 32]>,
    /// Its generation: the account's count of its profile keys (1 the first), on every content message with it, so a
    /// contact knows which of two is the later (spec 4.10). Kept in the storage with the key.
    #[serde(default)]
    pub profile_key_gen: u32,
    /// Whether strangers may send requests at all.
    #[serde(default = "yes")]
    pub requests_open: bool,
    /// The hub has the check of `profile_key`'s access key and `requests_open` as they are.
    #[serde(default)]
    pub access_key_told: bool,
    /// This device's sender certificate from the hub (spec 6.1), and when (this clock, ms) it came: used for a day
    /// less `sealed::REFRESH_MS` of this clock from then, whatever time the clock shows.
    #[serde(default)]
    pub sender_certificate: Option<(Vec<u8>, u64)>,
    /// The latest this clock showed since then: a clock set back is noticed (the certificate is fetched anew), it
    /// never stretches the day.
    #[serde(default)]
    pub certificate_clock: u64,
    /// This device's registration while the hub has not answered it: the user id is empty until then.
    pub registering: Option<Registering>,
    /// The groups this account is or was in, by id (hex, the group's identifier at the hub). Groups of before the
    /// hub kept them (the key `groups`) are gone.
    #[serde(default, rename = "hub_groups")]
    pub groups: BTreeMap<String, Group>,
    /// When a payload last made this device ask the hub for a user's devices (user hex → ms), and the day's lookups for
    /// recovery (day since the epoch, how many): kept with the state, so a restart or an extension does not start
    /// them anew (spec 4.6).
    #[serde(default)]
    pub roster_asked: BTreeMap<String, u64>,
    #[serde(default)]
    pub recovery_lookups: (u64, u32),
    /// Group Yos waiting for their group to be fetched (group id, the event): shown once the group from the hub has
    /// their sender in it, dropped once it does not (at most `groups::MAX_HELD`).
    #[serde(default)]
    pub group_held: Vec<(String, StoredEvent)>,
    /// Payloads from a device under another's number whose roster could not be asked now: received again later,
    /// not dropped (at most `DEFERRED`).
    #[serde(default)]
    #[serde(with = "compact::list")]
    pub deferred: VecDeque<Vec<u8>>,
}

impl Drop for State {
    fn drop(&mut self) {
        self.account_secret.zeroize();
        self.auth_secret.zeroize();
        self.legacy_storage_key.zeroize();
    }
}

/// One record of the session store, TLS-encoded: the bulk of the state, kept compact.
#[derive(Debug, TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Entry {
    key: VLBytes,
    value: VLBytes,
}

/// The plaintext: version, the device state as JSON (small), the session store as TLS (large).
#[derive(Debug, TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Plain {
    version: u8,
    state: VLBytes,
    records: Vec<Entry>,
}

pub fn seal(state: &State, records: Entries, key: &[u8; 32]) -> Vec<u8> {
    let json = {
        let _cost = crate::cost::Span::new(1);
        serde_json::to_vec(state).expect("state encodes")
    };
    let _cost = crate::cost::Span::new(2);
    let plain = Plain {
        version: STATE_VERSION,
        state: json.into(),
        records: records
            .into_iter()
            .map(|(key, value)| Entry { key: key.into_bytes().into(), value: value.into() })
            .collect(),
    };
    let mut bytes = yo_wire::encode(&plain);
    wipe(plain.state);
    for entry in plain.records {
        wipe(entry.value);
    }
    let nonce: [u8; 12] = crate::identity::random();
    let sealed = ChaCha20Poly1305::new(key.into())
        .encrypt(&nonce.into(), Payload { msg: &bytes, aad: AAD })
        .expect("encryption");
    bytes.zeroize();
    let mut out = nonce.to_vec();
    out.extend_from_slice(&sealed);
    out
}

pub fn open(blob: &[u8], key: &[u8; 32]) -> Result<(State, Entries)> {
    if blob.len() < 12 {
        return Err(Error::State);
    }
    let (nonce, sealed) = blob.split_at(12);
    let mut bytes = {
        let _cost = crate::cost::Span::new(2);
        ChaCha20Poly1305::new(key.into())
            .decrypt(nonce.into(), Payload { msg: sealed, aad: AAD })
            .map_err(|_| Error::State)?
    };
    let decoded: std::result::Result<Plain, _> = yo_wire::decode(&bytes);
    bytes.zeroize();
    let mut plain = decoded.map_err(|_| Error::State)?;
    if plain.version != STATE_VERSION && plain.version != 1 {
        return Err(Error::State);
    }
    let state: std::result::Result<State, _> = {
        let _cost = crate::cost::Span::new(1);
        serde_json::from_slice(plain.state.as_slice())
    };
    let state = match (state, plain.version) {
        (Ok(state), 1) => from_1(state),
        (state, _) => state.map_err(|_| Error::State),
    };
    let entries = std::mem::take(&mut plain.records)
        .into_iter()
        .map(|entry| Ok((String::from_utf8(entry.key.into()).map_err(|_| Error::State)?, entry.value.into())))
        .collect::<Result<Entries>>();
    wipe(plain.state);
    let mut state = state?;
    for contact in state.contacts.values_mut().filter(|contact| contact.probe_service) {
        // Older clients marked the public PING account as the probe service.
        if !contact.handles.iter().any(|handle| crate::same_handle(handle, crate::PROBE_HANDLE)) {
            contact.probe_service = false;
        }
    }
    Ok((state, entries?))
}

/// A state from before the keys were libsignal's: the Ed25519 seeds of the account key and the auth key become the
/// clamped scalars their X25519 forms were (the identity its sessions have), the contacts' pinned keys their
/// Curve25519 form (as the hub converted them),
/// the device certificate signed anew, and the storage written again under the key the new secret gives. A
/// registration still unanswered (signed by the old key) does not come over: the app signs in again.
fn from_1(mut state: State) -> Result<State> {
    if state.registering.is_some() {
        return Err(Error::State);
    }
    // The storage at the hub is sealed under the seed's key: opened with it until written again (`storage_dirty`).
    state.legacy_storage_key = Some(crate::storage::key(&state.account_secret));
    state.account_secret = crate::identity::legacy_account_secret(&state.account_secret);
    // The auth key the same way: the hub converted its public half (schema 3).
    state.auth_secret = crate::identity::legacy_account_secret(&state.auth_secret);
    for contact in state.contacts.values_mut() {
        if contact.account_key != [0; 32] {
            contact.account_key = crate::identity::legacy_account_key(&contact.account_key).unwrap_or([0; 32]);
        }
    }
    let pair = crate::identity::account_pair(&state.account_secret)?;
    state.certificate = crate::identity::certify(&pair, &state.device_key).to_bytes();
    state.storage_dirty = true;
    Ok(state)
}

fn wipe(bytes: VLBytes) {
    let mut bytes: Vec<u8> = bytes.into();
    bytes.zeroize();
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
    (0..text.len()).step_by(2).map(|i| text.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn resend_snapshot_is_independent_and_json_unchanged() {
        let mut cache: super::SentCache = [vec![1, 2, 3]].into_iter().collect();
        let snapshot = cache.clone();
        cache.push_back(vec![4]);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(cache.len(), 2);
        assert_eq!(serde_json::to_string(&snapshot).unwrap(), "[\"AQID\"]");
        let old: super::SentCache = serde_json::from_str("[[1,2,3]]").unwrap();
        assert_eq!(snapshot, old);
    }

    #[test]
    fn a_contact_key_from_before_is_the_profile_key() {
        use super::Contact;
        let contact = Contact { profile_key: Some([7; 32]), ..Contact::unknown() };
        let json = serde_json::to_string(&contact).unwrap().replace("\"profile_key\"", "\"their_key\"");
        assert!(json.contains("their_key"));
        assert_eq!(serde_json::from_str::<Contact>(&json).unwrap().profile_key, Some([7; 32]));
    }

    use super::compact;
    use std::collections::VecDeque;

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct T {
        #[serde(with = "compact::list")]
        sent: VecDeque<Vec<u8>>,
        #[serde(with = "compact::hashes")]
        seen: VecDeque<[u8; 32]>,
        #[serde(default, with = "compact::optional")]
        envelope: Option<Vec<u8>>,
    }

    #[test]
    fn bytes_are_saved_as_text_and_read_as_numbers_too() {
        let t = T { sent: [vec![1, 2, 3]].into(), seen: [[7; 32]].into(), envelope: Some(vec![9]) };
        let json = serde_json::to_string(&t).unwrap();
        assert!(!json.contains("[1,2,3]") && json.contains("\"AQID\""), "{json}");
        assert_eq!(serde_json::from_str::<T>(&json).unwrap(), t);
        let old = format!(r#"{{"sent":[[1,2,3]],"seen":[{:?}],"envelope":[9]}}"#, [7u8; 32]);
        assert_eq!(serde_json::from_str::<T>(&old).unwrap(), t, "a state saved before");
        assert_eq!(serde_json::from_str::<T>(r#"{"sent":[],"seen":[]}"#).unwrap().envelope, None);
    }
}
