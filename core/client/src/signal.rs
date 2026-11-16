//! The sessions (spec 4): libsignal's PQXDH and Double Ratchet with SPQR, over a store the client seals with its state.
//!
//! An account's devices share one identity key: the account key, as Signal's linked devices share
//! theirs. So every contact checks a session against the account key it pinned, and the hub has nothing to swap. A
//! peer device is a `ProtocolAddress`: the user id as an ACI and a number this device gives it (`Slot`), local only.

use crate::error::{Error, Result};
use crate::identity::Key;
use libsignal_protocol::{
    kem, Aci, DeviceId, Direction, GenericSignedPreKey as _, IdentityChange, IdentityKey, IdentityKeyPair,
    IdentityKeyStore, KeyPair, KyberPreKeyId, KyberPreKeyRecord, KyberPreKeyStore, PreKeyId, PreKeyRecord, PreKeyStore,
    ProtocolAddress, PublicKey, SessionRecord, SessionStore, SignalProtocolError, SignedPreKeyId, SignedPreKeyRecord,
    SignedPreKeyStore, Timestamp,
};
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};
use zeroize::Zeroize;

/// The Kyber prekeys' kind (libsignal's for PQXDH).
pub const KYBER: kem::KeyType = kem::KeyType::Kyber1024;
/// Base keys of messages made with a last resort Kyber prekey, remembered against a replay while that prekey is kept;
/// at most this many (beyond, the oldest go: a bound on the state, far above a month's last-resort sessions).
const LAST_RESORT_SEEN: usize = 20_000;

pub type Entries = BTreeMap<String, Vec<u8>>;

/// Entries that can undo what an operation changed (`Records::begin` … `rollback`, nested as the client's
/// transactions are): each key's value before its first change in the operation is kept, not the whole map, so a
/// transaction costs what it touches.
#[derive(Default)]
pub struct Journaled {
    entries: Entries,
    undo: Vec<BTreeMap<String, Option<Vec<u8>>>>,
}

impl std::ops::Deref for Journaled {
    type Target = Entries;
    fn deref(&self) -> &Entries {
        &self.entries
    }
}

impl From<Entries> for Journaled {
    fn from(entries: Entries) -> Self {
        Journaled { entries, undo: Vec::new() }
    }
}

impl Journaled {
    fn note(&mut self, key: &str) {
        if let Some(undo) = self.undo.last_mut() {
            if !undo.contains_key(key) {
                undo.insert(key.to_owned(), self.entries.get(key).cloned());
            }
        }
    }

    pub fn insert(&mut self, key: String, value: Vec<u8>) -> Option<Vec<u8>> {
        self.note(&key);
        self.entries.insert(key, value)
    }

    pub fn remove(&mut self, key: &str) -> Option<Vec<u8>> {
        self.note(key);
        self.entries.remove(key)
    }

    fn begin(&mut self) {
        self.undo.push(BTreeMap::new());
    }

    /// The operation stands: what it kept aside goes to the one around it (whose older value wins), or away.
    fn commit(&mut self) {
        let Some(undo) = self.undo.pop() else { return };
        match self.undo.last_mut() {
            Some(outer) => {
                for (key, old) in undo {
                    match outer.entry(key) {
                        std::collections::btree_map::Entry::Vacant(entry) => {
                            entry.insert(old);
                        }
                        std::collections::btree_map::Entry::Occupied(_) => {
                            if let Some(mut old) = old {
                                old.zeroize();
                            }
                        }
                    }
                }
            }
            None => {
                for (_, old) in undo {
                    if let Some(mut old) = old {
                        old.zeroize();
                    }
                }
            }
        }
    }

    fn rollback(&mut self) {
        for (key, old) in self.undo.pop().unwrap_or_default() {
            let now = match old {
                Some(old) => self.entries.insert(key, old),
                None => self.entries.remove(&key),
            };
            if let Some(mut now) = now {
                now.zeroize();
            }
        }
    }
}

impl Drop for Journaled {
    fn drop(&mut self) {
        wipe(&mut self.entries);
        while !self.undo.is_empty() {
            self.commit();
        }
    }
}

/// Runs one of libsignal's store operations: they are `async`, but the store is in memory and never waits.
pub fn now<F: Future>(future: F) -> F::Output {
    let _cost = crate::cost::Span::new(0);
    match pin!(future).as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => unreachable!("an in-memory store never waits"),
    }
}

pub fn signal(error: SignalProtocolError) -> Error {
    Error::Session(error.to_string())
}

/// The account's identity key pair, the account key itself (spec 4.1): the sessions' and the seal's (6.1).
pub fn identity_pair(account_secret: &[u8; 32]) -> Result<IdentityKeyPair> {
    crate::identity::account_pair(account_secret)
}

/// The identity key of the account with this key: what its sessions must show.
pub fn identity_of(account_key: &Key) -> Result<IdentityKey> {
    let public = PublicKey::from_djb_public_key_bytes(account_key).map_err(|_| Error::Protocol("identity"))?;
    Ok(IdentityKey::new(public))
}

/// The address of a user's device here.
pub fn address(user: &[u8; 16], slot: u8) -> Result<ProtocolAddress> {
    let device = DeviceId::new(slot).map_err(|_| Error::Internal("device slot"))?;
    Ok(ProtocolAddress::new(Aci::from_uuid_bytes(*user).service_id_string(), device))
}

/// libsignal's store, as entries the state seals (spec 2): sessions and the identities seen by address, this
/// device's prekeys by id. Its identity key pair is the account's, never stored. One part per store trait, as
/// libsignal borrows several at once.
pub struct Records {
    pub sessions: Sessions,
    pub identities: Identities,
    pub prekeys: OneTime,
    pub signed: Signed,
    pub kyber: Kyber,
}

pub struct Sessions(Journaled);
pub struct OneTime(Journaled);

impl OneTime {
    /// One-time EC prekeys whose private keys this device holds.
    pub fn count(&self) -> usize {
        self.0.len()
    }
}
pub struct Signed(Journaled);

pub struct Identities {
    seen: Journaled,
    pair: IdentityKeyPair,
    registration_id: u32,
    /// The identity the next operation must see at an address (by name, the user): the pinned account key's.
    trusted: Option<(String, IdentityKey)>,
}

/// Base keys seen with each last resort: (its id, the key).
type Seen = VecDeque<(u32, Vec<u8>)>;

pub struct Kyber {
    /// One-time ones by `k/id`, last resorts by `kl/id`.
    keys: Journaled,
    /// Base keys seen with each last resort (its id, the key): a message made with one again is a replay.
    seen: Seen,
    /// `seen` as it was when each open operation began, kept at its first change in it (`Records::begin`).
    seen_before: Vec<Option<Seen>>,
}

impl Kyber {
    /// Keeps `seen` aside before its first change in an operation (`Records::begin`).
    fn keep_seen(&mut self) {
        if let Some(before @ None) = self.seen_before.last_mut() {
            *before = Some(self.seen.clone());
        }
    }

    /// One-time Kyber prekeys whose private keys this device holds.
    pub fn one_time(&self) -> usize {
        self.keys.keys().filter(|k| k.starts_with("k/")).count()
    }
}

fn session_key(address: &ProtocolAddress) -> String {
    format!("{}.{}", address.name(), u32::from(address.device_id()))
}

/// The entries under `prefix/`, without it.
fn part(entries: &Entries, prefix: &str) -> Entries {
    let prefix = format!("{prefix}/");
    entries.iter().filter_map(|(k, v)| Some((k.strip_prefix(&prefix)?.to_owned(), v.clone()))).collect()
}

fn wipe(entries: &mut Entries) {
    for value in entries.values_mut() {
        value.zeroize();
    }
}

impl Records {
    pub fn new(entries: &Entries, account_secret: &[u8; 32], registration_id: u32) -> Result<Self> {
        Ok(Records {
            sessions: Sessions(part(entries, "s").into()),
            identities: Identities {
                seen: part(entries, "i").into(),
                pair: identity_pair(account_secret)?,
                registration_id,
                trusted: None,
            },
            prekeys: OneTime(part(entries, "p").into()),
            signed: Signed(part(entries, "sp").into()),
            kyber: Kyber {
                seen_before: Vec::new(),
                keys: part(entries, "k").into(),
                seen: entries
                    .get("seen")
                    .map(|seen| {
                        seen.chunks(37)
                            .filter(|c| c.len() == 37)
                            .map(|c| (u32::from_be_bytes([c[0], c[1], c[2], c[3]]), c[4..].to_vec()))
                            .collect()
                    })
                    .unwrap_or_default(),
            },
        })
    }

    /// The entries to seal.
    pub fn export(&self) -> Entries {
        let mut out = Entries::new();
        for (prefix, entries) in [
            ("s", &*self.sessions.0),
            ("i", &*self.identities.seen),
            ("p", &*self.prekeys.0),
            ("sp", &*self.signed.0),
            ("k", &*self.kyber.keys),
        ] {
            out.extend(entries.iter().map(|(k, v)| (format!("{prefix}/{k}"), v.clone())));
        }
        let seen = self.kyber.seen.iter().flat_map(|(id, key)| id.to_be_bytes().into_iter().chain(key.iter().copied()));
        out.insert("seen".into(), seen.collect());
        out
    }

    /// Starts an operation that `rollback` can undo (`commit` ends it; operations nest): only what it changes is
    /// kept aside.
    pub fn begin(&mut self) {
        for journaled in self.journals() {
            journaled.begin();
        }
        self.kyber.seen_before.push(None);
    }

    pub fn commit(&mut self) {
        for journaled in self.journals() {
            journaled.commit();
        }
        if let Some(before) = self.kyber.seen_before.pop() {
            // The one around it keeps its own older copy, if it made one.
            if let (Some(seen), Some(outer @ None)) = (before, self.kyber.seen_before.last_mut()) {
                *outer = Some(seen);
            }
        }
    }

    /// The records as when the operation began.
    pub fn rollback(&mut self) {
        for journaled in self.journals() {
            journaled.rollback();
        }
        if let Some(Some(seen)) = self.kyber.seen_before.pop() {
            self.kyber.seen = seen;
        }
    }

    fn journals(&mut self) -> [&mut Journaled; 5] {
        [&mut self.sessions.0, &mut self.identities.seen, &mut self.prekeys.0, &mut self.signed.0, &mut self.kyber.keys]
    }

    /// Trusts `identity` at the addresses of `address`'s user for the next operation, and nothing else.
    pub fn trust(&mut self, address: &ProtocolAddress, identity: IdentityKey) {
        self.identities.trusted = Some((address.name().to_owned(), identity));
    }

    pub fn delete_session(&mut self, address: &ProtocolAddress) {
        self.sessions.0.remove(&session_key(address));
    }

    /// Signal's answer to a `DecryptionErrorMessage` (spec 4.6): the session with `address` archived if its current
    /// state still sends with `ratchet_key`, so the next message starts a new one. Whether it was.
    pub fn archive_if_current(&mut self, address: &ProtocolAddress, ratchet_key: &PublicKey) -> bool {
        let key = session_key(address);
        let Some(mut record) = self.sessions.0.get(&key).and_then(|bytes| SessionRecord::deserialize(bytes).ok())
        else {
            return false;
        };
        if !record.current_ratchet_key_matches(ratchet_key).unwrap_or(false) || record.archive_current_state().is_err()
        {
            return false;
        }
        match record.serialize() {
            Ok(bytes) => {
                self.sessions.0.insert(key, bytes);
                true
            }
            Err(_) => false,
        }
    }

    /// Whether a message can go to `address` now: a session whose sending chain is good, made with PQXDH.
    pub fn usable(&self, address: &ProtocolAddress) -> bool {
        use libsignal_protocol::SessionUsabilityRequirements as Needs;
        self.sessions.0.get(&session_key(address)).and_then(|bytes| SessionRecord::deserialize(bytes).ok()).is_some_and(
            |record| {
                record
                    .has_usable_sender_chain(
                        std::time::SystemTime::now(),
                        Needs::NotStale | Needs::EstablishedWithPqxdh,
                    )
                    .unwrap_or(false)
            },
        )
    }

    /// The ids of the signed (`sp`) or last resort Kyber (`kl`) prekeys, oldest first (ids only grow).
    pub fn ids(&self, kind: &str) -> Vec<u32> {
        let entries = if kind == "sp" { &self.signed.0 } else { &self.kyber.keys };
        let prefix = if kind == "sp" { String::new() } else { format!("{kind}/") };
        let mut ids: Vec<u32> = entries.keys().filter_map(|key| key.strip_prefix(&prefix)?.parse().ok()).collect();
        ids.sort_unstable();
        ids
    }

    /// Deletes a signed (`sp`) or last resort Kyber (`kl`) prekey.
    pub fn forget(&mut self, kind: &str, id: u32) {
        if kind == "sp" {
            self.signed.0.remove(&id.to_string());
        } else {
            self.kyber.keys.remove(&format!("{kind}/{id}"));
            self.kyber.keep_seen();
            self.kyber.seen.retain(|(of, _)| *of != id);
        }
    }

    /// Keeps the newest `most` one-time prekeys of each kind; the private keys of older ones go.
    pub fn keep_newest_one_time(&mut self, most: usize) {
        let mut ids: Vec<u32> = self.prekeys.0.keys().filter_map(|k| k.parse().ok()).collect();
        ids.sort_unstable();
        for id in ids.iter().take(ids.len().saturating_sub(most)) {
            if let Some(mut gone) = self.prekeys.0.remove(&id.to_string()) {
                gone.zeroize();
            }
        }
        let mut ids: Vec<u32> = self.kyber.keys.keys().filter_map(|k| k.strip_prefix("k/")?.parse().ok()).collect();
        ids.sort_unstable();
        for id in ids.iter().take(ids.len().saturating_sub(most)) {
            if let Some(mut gone) = self.kyber.keys.remove(&format!("k/{id}")) {
                gone.zeroize();
            }
        }
    }

    /// A new one-time EC prekey, saved: its public part for the hub.
    pub fn new_prekey(&mut self, id: u32) -> Result<yo_wire::api::Prekey> {
        let pair = KeyPair::generate(&mut rand::rng());
        let record = PreKeyRecord::new(PreKeyId::from(id), &pair);
        self.prekeys.0.insert(id.to_string(), record.serialize().map_err(signal)?);
        Ok(yo_wire::api::Prekey { id, key: pair.public_key.serialize().to_vec().into() })
    }

    /// A new signed EC prekey, saved, signed by the identity key.
    pub fn new_signed(&mut self, id: u32, now_ms: u64) -> Result<yo_wire::api::SignedPrekey> {
        let pair = KeyPair::generate(&mut rand::rng());
        let public = pair.public_key.serialize();
        let signature = self
            .identities
            .pair
            .private_key()
            .calculate_signature(&public, &mut rand::rng())
            .map_err(|_| Error::Internal("signature"))?;
        let record =
            SignedPreKeyRecord::new(SignedPreKeyId::from(id), Timestamp::from_epoch_millis(now_ms), &pair, &signature);
        self.signed.0.insert(id.to_string(), record.serialize().map_err(signal)?);
        Ok(yo_wire::api::SignedPrekey { id, key: public.to_vec().into(), signature: signature.to_vec().into() })
    }

    /// A new Kyber prekey, saved, signed by the identity key: a one-time one or a last resort.
    pub fn new_kyber(&mut self, id: u32, last_resort: bool) -> Result<yo_wire::api::SignedPrekey> {
        let record = KyberPreKeyRecord::generate(KYBER, KyberPreKeyId::from(id), self.identities.pair.private_key())
            .map_err(signal)?;
        let public = record.public_key().map_err(signal)?.serialize();
        let signature = record.signature().map_err(signal)?;
        let prefix = if last_resort { "kl" } else { "k" };
        self.kyber.keys.insert(format!("{prefix}/{id}"), record.serialize().map_err(signal)?);
        Ok(yo_wire::api::SignedPrekey { id, key: public.to_vec().into(), signature: signature.into() })
    }
}

type R<T> = std::result::Result<T, SignalProtocolError>;

#[async_trait::async_trait(?Send)]
impl IdentityKeyStore for Identities {
    async fn get_identity_key_pair(&self) -> R<IdentityKeyPair> {
        Ok(self.pair)
    }

    async fn get_local_registration_id(&self) -> R<u32> {
        Ok(self.registration_id)
    }

    async fn save_identity(&mut self, address: &ProtocolAddress, identity: &IdentityKey) -> R<IdentityChange> {
        let bytes = identity.serialize().to_vec();
        let changed = self.seen.get(address.name()).is_some_and(|known| *known != bytes);
        self.seen.insert(address.name().to_owned(), bytes);
        Ok(IdentityChange::from_changed(changed))
    }

    /// Only the identity of the account key pinned for the user (spec 4.1): the client says which before each use.
    async fn is_trusted_identity(&self, address: &ProtocolAddress, identity: &IdentityKey, _: Direction) -> R<bool> {
        Ok(self.trusted.as_ref().is_some_and(|(name, trusted)| name == address.name() && trusted == identity))
    }

    async fn get_identity(&self, address: &ProtocolAddress) -> R<Option<IdentityKey>> {
        self.seen.get(address.name()).map(|bytes| IdentityKey::decode(bytes)).transpose()
    }
}

#[async_trait::async_trait(?Send)]
impl SessionStore for Sessions {
    async fn load_session(&self, address: &ProtocolAddress) -> R<Option<SessionRecord>> {
        self.0.get(&session_key(address)).map(|bytes| SessionRecord::deserialize(bytes)).transpose()
    }

    async fn store_session(&mut self, address: &ProtocolAddress, record: &SessionRecord) -> R<()> {
        self.0.insert(session_key(address), record.serialize()?);
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl PreKeyStore for OneTime {
    async fn get_pre_key(&self, id: PreKeyId) -> R<PreKeyRecord> {
        let bytes = self.0.get(&u32::from(id).to_string()).ok_or(SignalProtocolError::InvalidPreKeyId)?;
        PreKeyRecord::deserialize(bytes)
    }

    async fn save_pre_key(&mut self, id: PreKeyId, record: &PreKeyRecord) -> R<()> {
        self.0.insert(u32::from(id).to_string(), record.serialize()?);
        Ok(())
    }

    async fn remove_pre_key(&mut self, id: PreKeyId) -> R<()> {
        if let Some(mut gone) = self.0.remove(&u32::from(id).to_string()) {
            gone.zeroize();
        }
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl SignedPreKeyStore for Signed {
    async fn get_signed_pre_key(&self, id: SignedPreKeyId) -> R<SignedPreKeyRecord> {
        let bytes = self.0.get(&u32::from(id).to_string()).ok_or(SignalProtocolError::InvalidSignedPreKeyId)?;
        SignedPreKeyRecord::deserialize(bytes)
    }

    async fn save_signed_pre_key(&mut self, id: SignedPreKeyId, record: &SignedPreKeyRecord) -> R<()> {
        self.0.insert(u32::from(id).to_string(), record.serialize()?);
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl KyberPreKeyStore for Kyber {
    async fn get_kyber_pre_key(&self, id: KyberPreKeyId) -> R<KyberPreKeyRecord> {
        let id = u32::from(id);
        let bytes = self
            .keys
            .get(&format!("k/{id}"))
            .or_else(|| self.keys.get(&format!("kl/{id}")))
            .ok_or(SignalProtocolError::InvalidKyberPreKeyId)?;
        KyberPreKeyRecord::deserialize(bytes)
    }

    async fn save_kyber_pre_key(&mut self, id: KyberPreKeyId, record: &KyberPreKeyRecord) -> R<()> {
        self.keys.insert(format!("k/{}", u32::from(id)), record.serialize()?);
        Ok(())
    }

    /// A one-time Kyber prekey goes once used; a last resort stays, and a message made with it twice (the same base
    /// key) is a replay.
    async fn mark_kyber_pre_key_used(&mut self, id: KyberPreKeyId, _: SignedPreKeyId, base_key: &PublicKey) -> R<()> {
        if let Some(mut gone) = self.keys.remove(&format!("k/{}", u32::from(id))) {
            gone.zeroize();
            return Ok(());
        }
        let seen = (u32::from(id), base_key.serialize().to_vec());
        if self.seen.contains(&seen) {
            return Err(SignalProtocolError::InvalidMessage(
                libsignal_protocol::CiphertextMessageType::PreKey,
                "a last resort prekey message again".into(),
            ));
        }
        self.keep_seen();
        self.seen.push_back(seen);
        // A marker goes with its prekey; only past the bound (20,000 sessions started on last resorts within a
        // prekey's month) the oldest go before it.
        let keys = &self.keys;
        self.seen.retain(|(id, _)| keys.contains_key(&format!("kl/{id}")));
        while self.seen.len() > LAST_RESORT_SEEN {
            self.seen.pop_front();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_last_resort_replay_is_refused_while_its_prekey_is_kept_and_its_markers_go_with_it() {
        let mut kyber = Kyber { keys: Entries::new().into(), seen: VecDeque::new(), seen_before: Vec::new() };
        kyber.keys.insert("kl/1".into(), vec![1]);
        kyber.keys.insert("kl/2".into(), vec![2]);
        let base = KeyPair::generate(&mut rand::rng()).public_key;
        let (one, two) = (KyberPreKeyId::from(1), KyberPreKeyId::from(2));
        let signed = SignedPreKeyId::from(1);
        now(kyber.mark_kyber_pre_key_used(one, signed, &base)).unwrap();
        assert!(now(kyber.mark_kyber_pre_key_used(one, signed, &base)).is_err(), "the same base key again");
        kyber.keys.remove("kl/1");
        now(kyber.mark_kyber_pre_key_used(two, signed, &base)).unwrap();
        assert!(kyber.seen.iter().all(|(id, _)| *id == 2), "the gone prekey's markers went");
    }

    #[test]
    fn an_operation_undoes_what_it_changed_also_nested() {
        let mut j = Journaled::from(Entries::from([("a".into(), vec![1]), ("b".into(), vec![2])]));
        j.begin();
        j.insert("a".into(), vec![10]);
        j.begin();
        j.remove("b");
        j.insert("c".into(), vec![3]);
        j.rollback(); // the inner one: b back, c gone, a as the outer one left it
        assert_eq!(*j, Entries::from([("a".into(), vec![10]), ("b".into(), vec![2])]));
        j.begin();
        j.insert("a".into(), vec![11]);
        j.remove("b");
        j.commit(); // into the outer one, whose older values win
        j.rollback();
        assert_eq!(*j, Entries::from([("a".into(), vec![1]), ("b".into(), vec![2])]));
        j.begin();
        j.insert("d".into(), vec![4]);
        j.commit();
        assert_eq!(j.get("d"), Some(&vec![4]));
        assert!(j.undo.is_empty());
    }

    #[test]
    fn the_identity_key_is_the_account_key() {
        let secret = crate::identity::new_account_secret();
        let account_key = crate::identity::account_key(&identity_pair(&secret).unwrap());
        assert_eq!(*identity_pair(&secret).unwrap().identity_key(), identity_of(&account_key).unwrap());
        let other = crate::identity::new_account_secret();
        assert_ne!(*identity_pair(&other).unwrap().identity_key(), identity_of(&account_key).unwrap());
    }

    #[test]
    fn store_operations_complete_at_once() {
        assert_eq!(now(async { 5 }), 5);
    }
}
