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
use yo_wire::Mailbox;
use zeroize::Zeroize;

const STATE_VERSION: u8 = 2;
const AAD: &[u8] = b"yo state v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    /// The account key pinned on first contact.
    pub account_key: [u8; 32],
    /// The contact's certificate says its identity is backed by a passkey.
    pub passkey: bool,
    pub name: Option<String>,
    pub verified: bool,
    pub blocked: bool,
    /// Its device was replaced (410): look it up again before the next send.
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chat {
    /// The other account's phone number.
    pub peer: String,
    pub last_update_ms: u64,
    pub sent_since_update: u32,
    /// Joined by Welcome and not updated since.
    pub needs_update: bool,
    /// The peer's device was replaced: nothing is sent here any more, but what it sent before is still read,
    /// until the hub would have dropped it anyway.
    #[serde(default)]
    pub closed_at: Option<u64>,
    /// A commit sent to the hub whose answer never came: it may have been accepted. Retried as it is until the hub
    /// answers; meanwhile the group holds it as its pending commit and nothing is sent in it.
    #[serde(default)]
    pub inflight: Option<Inflight>,
    /// The hub does not order commits for this group (it never registered it, or the token is not ours): the chat
    /// works, without updates.
    #[serde(default)]
    pub no_updates: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inflight {
    /// The request to `POST /v1/groups/{id}/commit`, as sent.
    pub request: Vec<u8>,
    /// The commit as an MLSMessage, for the other members once the hub has accepted it.
    pub commit: Vec<u8>,
    pub mailboxes: Vec<Mailbox>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outgoing {
    pub mailbox: Mailbox,
    pub envelope: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct State {
    pub phone: String,
    pub flags: u8,
    pub account_secret: [u8; 32],
    pub auth_secret: [u8; 32],
    pub mailbox: Mailbox,
    pub certificate: Vec<u8>,
    pub signer: SignatureKeyPair,
    pub name: Option<String>,
    pub contacts: BTreeMap<String, Contact>,
    /// By hex group id.
    pub chats: BTreeMap<String, Chat>,
    /// Envelopes not yet accepted by the hub, in order.
    pub outbox: VecDeque<Outgoing>,
    /// Hashes of envelopes already processed, against redelivery.
    pub seen: VecDeque<[u8; 32]>,
    /// Groups whose Welcome was discarded as superseded: what follows in them is discarded quietly too.
    #[serde(default)]
    pub ignored: VecDeque<[u8; 32]>,
    pub key_packages_left: u32,
    #[serde(default)]
    pub key_packages: Vec<OwnKeyPackage>,
    pub last_resort_at: u64,
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
