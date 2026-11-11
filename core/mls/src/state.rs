//! What a device keeps (spec 2): its keys, contacts, chats, the outbox and the MLS store, sealed as one blob under
//! a key the platform holds in its keychain.

use crate::error::{Error, Result};
use crate::profile::Entries;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::ChaCha20Poly1305;
use openmls_basic_credential::SignatureKeyPair;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use yo_wire::Mailbox;
use zeroize::Zeroize;

const STATE_VERSION: u8 = 1;
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
    pub last_resort_at: u64,
}

impl Drop for State {
    fn drop(&mut self) {
        self.account_secret.zeroize();
        self.auth_secret.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
struct Sealed {
    version: u8,
    state: State,
    mls: Vec<(Vec<u8>, Vec<u8>)>,
}

pub fn seal(state: &State, mls: Entries, key: &[u8; 32]) -> Vec<u8> {
    let mut mls: Vec<_> = mls.into_iter().collect();
    mls.sort();
    let mut plain =
        serde_json::to_vec(&Sealed { version: STATE_VERSION, state: state.clone(), mls }).expect("state encodes");
    let nonce: [u8; 12] = crate::identity::random();
    let sealed = ChaCha20Poly1305::new(key.into())
        .encrypt(&nonce.into(), Payload { msg: &plain, aad: AAD })
        .expect("encryption");
    plain.zeroize();
    let mut out = nonce.to_vec();
    out.extend_from_slice(&sealed);
    out
}

pub fn open(blob: &[u8], key: &[u8; 32]) -> Result<(State, Entries)> {
    if blob.len() < 12 {
        return Err(Error::State);
    }
    let (nonce, sealed) = blob.split_at(12);
    let mut plain = ChaCha20Poly1305::new(key.into())
        .decrypt(nonce.into(), Payload { msg: sealed, aad: AAD })
        .map_err(|_| Error::State)?;
    let opened: std::result::Result<Sealed, _> = serde_json::from_slice(&plain);
    plain.zeroize();
    let sealed = opened.map_err(|_| Error::State)?;
    if sealed.version != STATE_VERSION {
        return Err(Error::State);
    }
    Ok((sealed.state, sealed.mls.into_iter().collect()))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
    (0..text.len()).step_by(2).map(|i| text.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect()
}
