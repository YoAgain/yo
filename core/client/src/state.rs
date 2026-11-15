//! What a device keeps (spec 2): its keys, contacts, the outbox and the session store, sealed as one blob under a
//! key the platform holds in its keychain.

use crate::error::{Error, Result};
use crate::identity::Key;
use crate::signal::Entries;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::ChaCha20Poly1305;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};
use yo_wire::DeviceId;
use zeroize::Zeroize;

const STATE_VERSION: u8 = 1;
const AAD: &[u8] = b"yo state v2";

/// A device of a user as this device knows it: its key, and its number among the user's devices, as the hub gave it
/// (spec 4.1): the session's address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub key: Key,
    pub slot: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
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
    /// The plaintexts of the last messages sent to the user: sent again when it asks (spec 4.6).
    pub sent: VecDeque<Vec<u8>>,
    /// Something came from the user since the last receipt (spec 4.5).
    pub receipt_due: bool,
    /// Its devices whose message did not open here: a new session with each, and a retry (spec 4.6).
    pub retry: Vec<Key>,
    /// When a retry was asked of the user last (wall clock, ms).
    pub retry_at: u64,
    /// When the user's retry was answered last (wall clock, ms).
    pub resent_at: u64,
    /// When the hub last named devices of the user without prekeys (wall clock, ms; 0: never, or they have them
    /// now): an app too old for sessions, which must be updated. What waits for it stays (spec 4.2).
    pub outdated_at: u64,
}

impl Contact {
    /// A contact known by its user id only, no key pinned yet.
    pub fn unknown() -> Contact {
        Contact {
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
            sent: VecDeque::new(),
            receipt_due: false,
            retry: Vec::new(),
            retry_at: 0,
            resent_at: 0,
            outdated_at: 0,
        }
    }
}

/// This device's registration (`POST /v1/register`, spec 6.4), saved before it leaves and kept until the hub
/// answered: after a crash it is finished from the state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registering {
    pub request: Vec<u8>,
    /// Sent before: the hub may have taken it (then this device can sign, and is asked about first).
    pub sent: bool,
}

/// A message for one user, waiting for the hub to take it (spec 4.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outgoing {
    /// Its place in the order of everything this device sent.
    pub seq: u64,
    /// The user it goes to (hex): a contact, or this account (a copy for its other devices).
    pub user: String,
    /// The session plaintext (`envelope::Plain`), sealed in the state: encrypted for the user's devices as they are
    /// when it goes.
    pub plain: Vec<u8>,
    /// Its envelope once encrypted, with the account key and devices it was made for: made again if they changed.
    pub envelope: Option<Vec<u8>>,
    pub made_for: Vec<Key>,
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

/// A group as this device has it (spec 4.7): kept for good, also once left (it shows as left).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    /// Its members by user id (hex), sorted, this account among them while it is in.
    pub members: Vec<String>,
    pub revision: u32,
    /// Who made this revision (hex): of two at the same revision, the lower one's stands.
    pub by: String,
    /// This account is no member of it (left, or removed).
    pub left: bool,
    /// The members of the revision this one was made from: who may make a rival change at the same revision (a member
    /// this revision removed, changing at once, is still heard, so both ends take the same one).
    #[serde(default)]
    pub before: Vec<String>,
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
    /// The time of the last message sent: each later one strictly later.
    pub last_sent_ms: u64,
    /// Hashes of the last messages received (sender and plaintext): a copy sent again on a retry is skipped.
    pub received: VecDeque<[u8; 32]>,
    /// Privacy Pass tokens for deliveries (spec 6.10): the hub's key they were made for (hex), and the tokens, the
    /// oldest first (`tokens::kept`).
    pub token_key: Option<String>,
    pub tokens: VecDeque<String>,
    /// No issuance is asked for before then (wall clock, ms): after one failed (its limit, no network).
    pub tokens_after: u64,
    /// The hub refused a token: its key is asked for at the next refill, whatever the stash.
    pub tokens_check: bool,
    /// This device's registration while the hub has not answered it: the user id is empty until then.
    pub registering: Option<Registering>,
    /// The groups this account is or was in, by id (hex).
    #[serde(default)]
    pub groups: BTreeMap<String, Group>,
    /// When a payload last made this device ask the hub for a user's devices (user hex → ms), and the day's lookups for
    /// recovery (day since the epoch, how many): kept with the state, so a restart or an extension does not start
    /// them anew (spec 4.6).
    #[serde(default)]
    pub roster_asked: BTreeMap<String, u64>,
    #[serde(default)]
    pub recovery_lookups: (u64, u32),
    /// Groups asked of a member whose Yo named a newer revision (group id, user, both hex): their answer is taken
    /// though they are not in the group as this device has it (added since).
    #[serde(default)]
    pub group_asks: BTreeSet<(String, String)>,
    /// Payloads from a device under another's number whose roster could not be asked now: received again later,
    /// not dropped (at most `DEFERRED`).
    #[serde(default)]
    pub deferred: VecDeque<Vec<u8>>,
}

impl Drop for State {
    fn drop(&mut self) {
        self.account_secret.zeroize();
        self.auth_secret.zeroize();
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
    let plain = Plain {
        version: STATE_VERSION,
        state: serde_json::to_vec(state).expect("state encodes").into(),
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
    let mut bytes = ChaCha20Poly1305::new(key.into())
        .decrypt(nonce.into(), Payload { msg: sealed, aad: AAD })
        .map_err(|_| Error::State)?;
    let decoded: std::result::Result<Plain, _> = yo_wire::decode(&bytes);
    bytes.zeroize();
    let mut plain = decoded.map_err(|_| Error::State)?;
    if plain.version != STATE_VERSION {
        return Err(Error::State);
    }
    let state: std::result::Result<State, _> = serde_json::from_slice(plain.state.as_slice());
    let entries = std::mem::take(&mut plain.records)
        .into_iter()
        .map(|entry| Ok((String::from_utf8(entry.key.into()).map_err(|_| Error::State)?, entry.value.into())))
        .collect::<Result<Entries>>();
    wipe(plain.state);
    Ok((state.map_err(|_| Error::State)?, entries?))
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
