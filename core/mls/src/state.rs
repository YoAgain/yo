//! What a device keeps (spec 2): its keys, contacts, chats, the outbox and the MLS store, sealed as one blob under
//! a key the platform holds in its keychain.

use crate::error::{Error, Result};
use crate::profile::Entries;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::ChaCha20Poly1305;
use openmls_basic_credential::SignatureKeyPair;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};
use yo_wire::DeviceId;
use zeroize::Zeroize;

const STATE_VERSION: u8 = 3;
const AAD: &[u8] = b"yo state v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    /// The account key pinned on first contact, or the one confirmed after a change.
    pub account_key: [u8; 32],
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
    #[serde(default)]
    pub gone: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chat {
    /// The other account's user id (hex).
    pub peer: String,
    /// The epoch this device came into the group at: what was sent before is not for it.
    pub joined_epoch: u64,
    pub last_update_ms: u64,
    pub sent_since_update: u32,
    /// Joined by Welcome and not updated since.
    pub needs_update: bool,
    /// A commit sent to the hub whose answer never came: it may have been accepted. Retried as it is until the hub
    /// answers; meanwhile the group holds it as its pending commit and nothing is sent in it.
    pub inflight: Option<Inflight>,
    /// The plaintexts of the last messages sent in this chat: sent again in a restarted chat (spec 4.9).
    #[serde(default)]
    pub sent: VecDeque<Vec<u8>>,
    /// The `sent_at` of the last message received from the peer here: what a restart tells it.
    #[serde(default)]
    pub last_seen_ms: u64,
    /// This device can no longer follow the group (a commit it lacks is gone from the hub, or the hub refuses its
    /// commits): restarted at the next chance (spec 4.9). What waits to go in it goes in the new group.
    #[serde(default)]
    pub lost: bool,
    /// Made, or taken over, by a restart: it replaces the older chats with the peer.
    #[serde(default)]
    pub restarted: bool,
}

/// A commit on its way to the hub, with what to deliver once it is accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inflight {
    /// The request to `POST /v1/groups/{id}/commit`, as sent (it carries the commit's envelope).
    pub request: Vec<u8>,
    /// The commit's envelope, for every account in the group.
    pub commit: Vec<u8>,
    /// A repair's Welcome: for the user whose devices it adds (hex user id, envelope).
    pub welcome: Option<(String, Vec<u8>)>,
}

/// A KeyPackage this device made: its private keys stay in the MLS store until they are deleted here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnKeyPackage {
    pub hash_ref: Vec<u8>,
    pub created_at: u64,
    pub last_resort: bool,
    /// When a newer last resort replaced this one.
    pub retired_at: Option<u64>,
}

/// An envelope for one user, waiting for the hub to take it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outgoing {
    /// Its place in the order of everything this device sent: kept across repairs and restarts.
    pub seq: u64,
    /// The user it goes to (hex).
    pub user: String,
    /// The group it belongs to (hex): its device check is computed from the group when it is sent (spec 4.6).
    pub group: String,
    pub envelope: Vec<u8>,
    /// The user's devices it was encrypted for. When none of them is in the group any more (a repair replaced them),
    /// a message is encrypted anew in its place and anything else is of no use (spec 4.6).
    pub made_for: Vec<[u8; 32]>,
    /// For an application message: its plaintext, to encrypt it anew.
    pub plaintext: Option<Vec<u8>>,
}

/// An event as the state keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredEvent {
    NewChat { user: String },
    Message { user: String, sent_at_ms: u64, content: StoredContent, own: bool },
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
    pub certificate: Vec<u8>,
    pub signer: SignatureKeyPair,
    pub name: Option<String>,
    /// By hex user id.
    pub contacts: BTreeMap<String, Contact>,
    /// By hex group id.
    pub chats: BTreeMap<String, Chat>,
    /// Envelopes not yet taken by the hub, in order.
    pub outbox: VecDeque<Outgoing>,
    pub next_seq: u64,
    /// What happened and the app has not been handed yet: saved with the state, so nothing received is lost to a
    /// crash between the hub's ack and the app.
    pub events: Vec<StoredEvent>,
    /// Handed to the app by the last call: kept until the next call, which acknowledges them. After a crash they come
    /// again: once in normal operation, at least once across a crash.
    pub handed: Vec<StoredEvent>,
    /// Hashes of envelopes already processed, against redelivery.
    pub seen: VecDeque<[u8; 32]>,
    /// Groups whose Welcome came from a key that is no user's current one: what follows in them is discarded.
    pub ignored: VecDeque<[u8; 32]>,
    pub key_packages_left: u32,
    pub key_packages: Vec<OwnKeyPackage>,
    pub last_resort_at: u64,
    /// The time of the last signed request (spec 6.2): the next is signed later, whichever process signs it.
    #[serde(default)]
    pub signed_until: u64,
    /// The account's storage as this device has it (spec 6.9), merged with the hub's.
    #[serde(default)]
    pub storage: crate::storage::Storage,
    /// The hub's version the storage was last written or read as.
    #[serde(default)]
    pub storage_version: u64,
    /// Changed here since it was last written.
    #[serde(default)]
    pub storage_dirty: bool,
    /// When the hub last held all of it (wall clock, ms): written, or fetched with nothing left to write. 0: not yet.
    #[serde(default)]
    pub storage_saved_at: u64,
    /// Fetched from the hub on this device (once, before the first message is received).
    #[serde(default)]
    pub storage_fetched: bool,
    /// The time of the last message sent: each later one strictly later.
    #[serde(default)]
    pub last_sent_ms: u64,
    /// Hashes of the last messages received (sender and plaintext): a copy sent again after a restart is skipped.
    #[serde(default)]
    pub received: VecDeque<[u8; 32]>,
    /// Privacy Pass tokens for deliveries (spec 6.10): the hub's key they were made for (hex), and the tokens, the
    /// oldest first (`tokens::kept`).
    #[serde(default)]
    pub token_key: Option<String>,
    #[serde(default)]
    pub tokens: VecDeque<String>,
    /// No issuance is asked for before then (wall clock, ms): after one failed (an old hub without tokens, its limit,
    /// no network).
    #[serde(default)]
    pub tokens_after: u64,
    /// The hub refused a token: its key is asked for at the next refill, whatever the stash.
    #[serde(default)]
    pub tokens_check: bool,
}

impl Drop for State {
    fn drop(&mut self) {
        self.account_secret.zeroize();
        self.auth_secret.zeroize();
    }
}

/// One entry of the MLS store, TLS-encoded: the bulk of the state, kept compact.
#[derive(Debug, TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Entry {
    key: VLBytes,
    value: VLBytes,
}

/// The plaintext: version, the device state as JSON (small), the MLS store as TLS (large).
#[derive(Debug, TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Plain {
    version: u8,
    state: VLBytes,
    mls: Vec<Entry>,
}

pub fn seal(state: &State, mls: Entries, key: &[u8; 32]) -> Vec<u8> {
    let mut entries: Vec<_> = mls.into_iter().collect();
    entries.sort();
    let plain = Plain {
        version: STATE_VERSION,
        state: serde_json::to_vec(state).expect("state encodes").into(),
        mls: entries.into_iter().map(|(key, value)| Entry { key: key.into(), value: value.into() }).collect(),
    };
    let mut bytes = yo_wire::encode(&plain);
    wipe(plain.state);
    for entry in plain.mls {
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
    let entries =
        std::mem::take(&mut plain.mls).into_iter().map(|entry| (entry.key.into(), entry.value.into())).collect();
    wipe(plain.state);
    Ok((state.map_err(|_| Error::State)?, entries))
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
