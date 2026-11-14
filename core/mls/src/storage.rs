//! The account's storage at the hub (spec 6.9): what the user keeps beside the chats (friends with the name and
//! colour shown for them, blocks, the app's icon) in one blob sealed under a key derived from the account key, so a
//! new phone restored from the backup (or the iCloud Keychain) gets it back, and the hub reads none of it.
//!
//! Every field carries the time it was set; two versions merge field by field, the later one winning, and friends are
//! the union of both. The hub keeps a version number per user and takes a write only on top of the version it has,
//! so two writers never overwrite each other unseen: the loser fetches, merges and writes again.

use crate::error::{Error, Result};
use crate::Client;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::ChaCha20Poly1305;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use yo_wire::api::{self, status};
use yo_wire::{decode, encode};

/// HKDF-SHA256 without salt (RFC 5869), up to 32 bytes: the derivation spec 7 and 6.9 name.
pub fn hkdf(ikm: &[u8], info: &[u8], out: &mut [u8]) {
    let mut extract = <Hmac<sha2::Sha256> as Mac>::new_from_slice(&[0; 32]).expect("any key length");
    extract.update(ikm);
    let prk = extract.finalize().into_bytes();
    let mut expand = <Hmac<sha2::Sha256> as Mac>::new_from_slice(&prk).expect("any key length");
    expand.update(info);
    expand.update(&[1]);
    let okm = expand.finalize().into_bytes();
    out.copy_from_slice(&okm[..out.len()]);
}

/// The key the storage is sealed under: HKDF(the account key's secret, "yo storage v1").
pub fn key(account_secret: &[u8; 32]) -> [u8; 32] {
    let mut key = [0; 32];
    hkdf(account_secret, b"yo storage v1", &mut key);
    key
}

/// A value and when it was set (ms, the setting device's clock); in JSON `[value, at]`. Never set: `at` 0, left out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "(T, u64)", into = "(T, u64)")]
#[serde(bound(serialize = "T: Clone + Serialize", deserialize = "T: Deserialize<'de>"))]
pub struct Field<T> {
    pub value: T,
    pub at: u64,
}

impl<T> From<(T, u64)> for Field<T> {
    fn from((value, at): (T, u64)) -> Self {
        Field { value, at }
    }
}

impl<T> From<Field<T>> for (T, u64) {
    fn from(field: Field<T>) -> Self {
        (field.value, field.at)
    }
}

impl<T> Field<T> {
    fn unset(&self) -> bool {
        self.at == 0
    }
}

impl<T: Clone + Serialize> Field<T> {
    fn set(&mut self, value: T, at: u64) {
        self.value = value;
        self.at = at;
    }

    /// The later of two; equal times decided by the value, so both sides pick the same.
    fn merge(&mut self, other: &Field<T>) {
        let key = |f: &Field<T>| (f.at, serde_json::to_vec(&f.value).unwrap_or_default());
        if key(other) > key(self) {
            *self = other.clone();
        }
    }
}

/// A friend as the user keeps it. Short keys: a few hundred friends fit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Friend {
    /// The name the user gave the friend, if any.
    #[serde(default, rename = "n", skip_serializing_if = "Field::unset")]
    pub name: Field<Option<String>>,
    /// The colour the app shows the friend in (an index into its palette).
    #[serde(default, rename = "c", skip_serializing_if = "Field::unset")]
    pub color: Field<Option<u8>>,
    #[serde(default, rename = "b", skip_serializing_if = "Field::unset")]
    pub blocked: Field<bool>,
    /// Taken off the list (kept, so the removal reaches the other side of a merge).
    #[serde(default, rename = "r", skip_serializing_if = "Field::unset")]
    pub removed: Field<bool>,
    /// What a later version keeps here, carried along unread.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Friend {
    fn merge(&mut self, other: &Friend) {
        self.name.merge(&other.name);
        self.color.merge(&other.color);
        self.blocked.merge(&other.blocked);
        self.removed.merge(&other.removed);
        merge_extra(&mut self.extra, &other.extra);
    }
}

/// Fields of a later version: kept; where both sides have one, the same choice on both (the larger value).
fn merge_extra(mine: &mut BTreeMap<String, serde_json::Value>, theirs: &BTreeMap<String, serde_json::Value>) {
    for (key, value) in theirs {
        let entry = mine.entry(key.clone()).or_insert_with(|| value.clone());
        if serde_json::to_vec(value).ok() > serde_json::to_vec(entry).ok() {
            *entry = value.clone();
        }
    }
}

/// The longest name kept for a friend, in bytes.
pub const MAX_FRIEND_NAME: usize = 64;

/// What is sealed: JSON, so a later version adds fields that an older one keeps (`extra`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Storage {
    /// The hub's version this one was written as: a hub that hands out an older one than a device has seen is
    /// noticed (and ignored).
    #[serde(default)]
    pub version: u64,
    /// By user id (hex).
    #[serde(default)]
    pub friends: BTreeMap<String, Friend>,
    /// The app's icon: none (the default) or its name ("Negative", "Original").
    #[serde(default)]
    pub icon: Field<Option<String>>,
    /// What a later version keeps here, carried along unread.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Storage {
    /// Both versions in one: the union of friends, each field the later of the two.
    pub fn merge(&mut self, other: &Storage) {
        for (user, friend) in &other.friends {
            self.friends.entry(user.clone()).or_default().merge(friend);
        }
        self.icon.merge(&other.icon);
        merge_extra(&mut self.extra, &other.extra);
        self.version = self.version.max(other.version);
    }
}

/// A nonce and a tag: what sealing adds.
const SEAL_OVERHEAD: usize = 12 + 16;

/// Pads `plain` like an envelope (spec 6.1) and seals it: nonce || ChaCha20-Poly1305 || tag, 1, 4, 16 or 60 KiB in
/// all (`SEALED_SIZES`), so the hub sees a size class. How the storage and a backup's sealed part are made (spec 6.9,
/// 7). `Error::Protocol("too large")` past the largest.
pub fn seal_padded(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    let sizes = yo_wire::limits::SEALED_SIZES.map(|size| size - SEAL_OVERHEAD);
    let padded = yo_wire::envelope::pad(plain, &sizes).map_err(|_| Error::Protocol("too large"))?;
    let nonce: [u8; 12] = crate::identity::random();
    let sealed =
        ChaCha20Poly1305::new(key.into()).encrypt(&nonce.into(), Payload { msg: &padded, aad }).expect("encryption");
    Ok([nonce.to_vec(), sealed].concat())
}

/// Opens what `seal_padded` sealed. Also what was sealed before the padding: JSON (spaces after it, or nothing),
/// which starts with `{` where the padded starts with its version, 1.
pub fn open_padded(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let (nonce, sealed) = sealed.split_at_checked(12)?;
    let plain = ChaCha20Poly1305::new(key.into()).decrypt(nonce.into(), Payload { msg: sealed, aad }).ok()?;
    if plain.first() != Some(&yo_wire::VERSION) {
        return Some(plain);
    }
    yo_wire::envelope::unpad(&plain, &[plain.len()]).ok().map(<[u8]>::to_vec)
}

/// Seals the storage for a user (associated data: "yo storage v1" || user id), padded (`seal_padded`).
pub fn seal(account_secret: &[u8; 32], user_id: &[u8], storage: &Storage) -> Result<Vec<u8>> {
    let plain = serde_json::to_vec(storage).expect("storage encodes");
    seal_padded(&key(account_secret), &[&b"yo storage v1"[..], user_id].concat(), &plain)
}

pub fn open(account_secret: &[u8; 32], user_id: &[u8], sealed: &[u8]) -> Result<Storage> {
    let plain = open_padded(&key(account_secret), &[&b"yo storage v1"[..], user_id].concat(), sealed)
        .ok_or(Error::Protocol("storage"))?;
    serde_json::from_slice(&plain).map_err(|_| Error::Protocol("storage"))
}

/// A friend as the app shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendInfo {
    pub user: String,
    pub name: Option<String>,
    pub color: Option<u8>,
    pub blocked: bool,
}

/// How often a write that lost to another writer is merged and tried again.
const WRITE_TRIES: usize = 4;

impl Client {
    fn user_id_bytes(&self) -> Vec<u8> {
        crate::state::unhex(&self.state.user_id).unwrap_or_default()
    }

    /// The friends kept in the storage (not those taken off the list), by user id.
    pub fn friends(&self) -> Vec<FriendInfo> {
        self.state
            .storage
            .friends
            .iter()
            .filter(|(_, f)| !f.removed.value)
            .map(|(user, f)| FriendInfo {
                user: user.clone(),
                name: f.name.value.clone(),
                color: f.color.value,
                blocked: f.blocked.value,
            })
            .collect()
    }

    /// Adds a friend to the list, or changes the name or colour shown for one. Kept at once, written to the hub with
    /// the next `sync` or `maintenance`.
    pub fn set_friend(&mut self, user: &str, name: Option<String>, color: Option<u8>) -> Result<()> {
        crate::user_bytes(user)?;
        if name.as_ref().is_some_and(|n| n.len() > MAX_FRIEND_NAME) {
            return Err(Error::Protocol("friend's name too long"));
        }
        let now = crate::wall_ms();
        let friend = self.state.storage.friends.entry(user.to_owned()).or_default();
        if friend.name.value != name {
            friend.name.set(name, now);
        }
        if friend.color.value != color {
            friend.color.set(color, now);
        }
        if friend.removed.value {
            friend.removed.set(false, now);
        }
        self.storage_changed()
    }

    /// Takes a friend off the list (a block stays as it is).
    pub fn remove_friend(&mut self, user: &str) -> Result<()> {
        let now = crate::wall_ms();
        let Some(friend) = self.state.storage.friends.get_mut(user) else { return Ok(()) };
        friend.removed.set(true, now);
        self.storage_changed()
    }

    /// The app's icon as kept in the storage: none for the default.
    pub fn app_icon(&self) -> Option<String> {
        self.state.storage.icon.value.clone()
    }

    /// When the hub last held all of the storage (wall clock, ms); none while a change waits to go up, or before
    /// the first sync.
    pub fn storage_saved_at(&self) -> Option<u64> {
        (self.state.storage_fetched && !self.state.storage_dirty && self.state.storage_saved_at > 0)
            .then_some(self.state.storage_saved_at)
    }

    pub fn set_app_icon(&mut self, icon: Option<String>) -> Result<()> {
        if self.state.storage.icon.value != icon {
            self.state.storage.icon.set(icon, crate::wall_ms());
            return self.storage_changed();
        }
        Ok(())
    }

    /// A block or unblock, kept in the storage too (`set_blocked`).
    pub(crate) fn storage_block(&mut self, user: &str, blocked: bool) {
        let friend = self.state.storage.friends.entry(user.to_owned()).or_default();
        if friend.blocked.value != blocked || friend.blocked.at == 0 {
            friend.blocked.set(blocked, crate::wall_ms());
            self.state.storage_dirty = true;
        }
    }

    fn storage_changed(&mut self) -> Result<()> {
        self.state.storage_dirty = true;
        self.persist()
    }

    /// The storage's blocks, applied to the contacts: what the merged storage says is what holds.
    fn apply_storage(&mut self) {
        let blocks: Vec<(String, bool)> = self
            .state
            .storage
            .friends
            .iter()
            .filter(|(_, f)| f.blocked.at > 0)
            .map(|(user, f)| (user.clone(), f.blocked.value))
            .collect();
        for (user, blocked) in blocks {
            if user == self.state.user_id {
                continue;
            }
            self.apply_block(&user, blocked);
        }
    }

    /// Takes in a version from the hub: merged with this device's, applied.
    fn take_storage(&mut self, version: u64, sealed: &[u8]) -> Result<()> {
        let theirs = open(&self.state.account_secret, &self.user_id_bytes(), sealed)?;
        // Sealed with the version it was written as: an older blob under a newer number is a hub replaying it.
        if theirs.version != version {
            return Err(Error::Protocol("storage version"));
        }
        self.state.storage.merge(&theirs);
        self.state.storage_version = self.state.storage_version.max(version);
        // What this device had and the hub's did not (or had older) goes back up.
        if self.state.storage != (Storage { version: self.state.storage.version, ..theirs }) {
            self.state.storage_dirty = true;
        }
        self.apply_storage();
        Ok(())
    }

    /// Brings the storage in line with the hub's: fetched once on this device (before anything is received, so a
    /// restored block holds from the first message on), then written when it changed. A hub without storage (an older
    /// one) is left alone.
    pub(crate) fn sync_storage(&mut self) -> Result<()> {
        if !self.state.storage_fetched {
            let response = self.signed("GET", api::STORAGE, &[])?;
            match response.status {
                status::OK => {
                    let stored: api::Storage = decode(&response.body)?;
                    // Not ours to open, or replayed (a hub that mixed it up, or damage): this device's stands and
                    // takes its place with the next write. Only this account's devices could read it, so nothing
                    // readable is lost; the hub could as well have answered that there is none.
                    if let Err(error) = self.take_storage(stored.version, stored.data.as_slice()) {
                        self.drop_one(format!("storage: {error}"));
                        self.state.storage_version = stored.version;
                    }
                }
                status::NOT_FOUND => {}
                status::GONE => return Err(Error::Replaced),
                other if other == status::RATE_LIMITED || other >= 500 => return Err(Error::Refused(other)),
                // An older hub without storage: nothing to fetch.
                _ => {}
            }
            // Blocks this device had before the storage (an app from before it) go up with the first write.
            let blocked: Vec<String> =
                self.state.contacts.iter().filter(|(_, c)| c.blocked).map(|(u, _)| u.clone()).collect();
            for user in blocked {
                if self.state.storage.friends.get(&user).is_none_or(|f| f.blocked.at == 0) {
                    self.storage_block(&user, true);
                }
            }
            self.state.storage_fetched = true;
            self.persist()?;
        }
        for _ in 0..WRITE_TRIES {
            if !self.state.storage_dirty {
                // Nothing to write: the hub holds it all (a new account's empty storage too).
                if self.state.storage_saved_at == 0 {
                    self.state.storage_saved_at = crate::wall_ms();
                    self.persist()?;
                }
                return Ok(());
            }
            let mut next = self.state.storage.clone();
            next.version = self.state.storage_version + 1;
            let data = self.sealed_within_limit(&mut next)?;
            let body = encode(&api::Storage { version: self.state.storage_version, data: data.into() });
            let response = self.signed("PUT", api::STORAGE, &body)?;
            match response.status {
                status::NO_CONTENT => {
                    self.state.storage_version = next.version;
                    self.state.storage.version = next.version;
                    self.state.storage_dirty = false;
                    self.state.storage_saved_at = crate::wall_ms();
                    self.persist()?;
                }
                // Another writer was first: its version, merged, goes up again.
                status::CONFLICT => {
                    let stored: api::Storage = decode(&response.body)?;
                    self.state.storage_version = stored.version;
                    if let Err(error) = self.take_storage(stored.version, stored.data.as_slice()) {
                        self.drop_one(format!("storage: {error}"));
                    }
                    self.state.storage_dirty = true;
                    self.persist()?;
                }
                status::GONE => return Err(Error::Replaced),
                other => return Err(Error::Refused(other)),
            }
        }
        Ok(())
    }

    /// The storage sealed, within the hub's limit: friends taken off the list are forgotten first, the longest gone
    /// first, until it fits (a removal older than the other writers' last read is no longer needed to merge).
    fn sealed_within_limit(&mut self, next: &mut Storage) -> Result<Vec<u8>> {
        loop {
            if let Ok(data) = seal(&self.state.account_secret, &self.user_id_bytes(), next) {
                self.state.storage.friends.clone_from(&next.friends);
                return Ok(data);
            }
            let oldest = next
                .friends
                .iter()
                .filter(|(_, f)| f.removed.value && !f.blocked.value)
                .min_by_key(|(_, f)| f.removed.at)
                .map(|(user, _)| user.clone())
                .ok_or(Error::Protocol("storage full"))?;
            next.friends.remove(&oldest);
        }
    }

    /// `sync_storage` where it is a side matter: "not now" waits for the next call, a refusal is noted.
    pub(crate) fn sync_storage_quietly(&mut self) -> Result<()> {
        match self.sync_storage() {
            Err(Error::Replaced) => Err(Error::Replaced),
            Err(error) if crate::retryable(&error) => Ok(()),
            Err(error) => {
                self.drop_one(format!("storage: {error}"));
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn friend(name: &str, at: u64) -> Friend {
        Friend { name: Field { value: Some(name.into()), at }, ..Friend::default() }
    }

    #[test]
    fn merging_takes_the_union_and_the_later_field() {
        let mut a = Storage::default();
        a.friends.insert("aa".into(), friend("Leo", 5));
        a.icon = Field { value: Some("Negative".into()), at: 9 };
        let mut b = Storage::default();
        b.friends.insert("aa".into(), Friend { blocked: Field { value: true, at: 7 }, ..friend("Leonard", 3) });
        b.friends.insert("bb".into(), friend("Mia", 1));
        b.icon = Field { value: Some("Original".into()), at: 4 };
        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab, ba, "the same either way");
        assert_eq!(ab.friends.len(), 2);
        assert_eq!(ab.friends["aa"].name.value.as_deref(), Some("Leo"), "the later name");
        assert!(ab.friends["aa"].blocked.value, "the block from the other side");
        assert_eq!(ab.icon.value.as_deref(), Some("Negative"));
    }

    #[test]
    fn a_sealed_storage_opens_only_with_its_key_and_user() {
        let mut s = Storage::default();
        s.friends.insert("aa".into(), friend("Secret Friend", 1));
        s.extra.insert("later".into(), serde_json::json!({"kept": true}));
        let sealed = seal(&[1; 32], b"user", &s).unwrap();
        assert_eq!(sealed.len(), 1024, "padded");
        assert!(!sealed.windows(6).any(|w| w == b"Secret"));
        assert_eq!(open(&[1; 32], b"user", &sealed).unwrap(), s, "unknown fields carried along");
        assert!(open(&[2; 32], b"user", &sealed).is_err());
        assert!(open(&[1; 32], b"other", &sealed).is_err());
    }

    #[test]
    fn sealed_sizes_are_buckets_and_old_blobs_still_open() {
        let key = [3; 32];
        for (len, size) in [(1, 1024), (991, 1024), (992, 4096), (4063, 4096), (16000, 16384), (61407, 61440)] {
            let sealed = seal_padded(&key, b"ad", &vec![b'x'; len]).unwrap();
            assert_eq!(sealed.len(), size, "{len} bytes");
            assert_eq!(open_padded(&key, b"ad", &sealed).unwrap(), vec![b'x'; len]);
        }
        assert!(seal_padded(&key, b"ad", &[b'x'; 61408]).is_err(), "past the largest");
        assert!(open_padded(&key, b"other", &seal_padded(&key, b"ad", b"x").unwrap()).is_none());
        // Sealed before the padding: JSON with spaces after it.
        let nonce = [7; 12];
        let old = ChaCha20Poly1305::new(&key.into())
            .encrypt(&nonce.into(), Payload { msg: b"{\"version\":3}   ", aad: b"ad" })
            .unwrap();
        let old = [nonce.to_vec(), old].concat();
        assert_eq!(open_padded(&key, b"ad", &old).unwrap(), b"{\"version\":3}   ");
    }

    #[test]
    fn hkdf_is_rfc_5869() {
        // RFC 5869, test case 3 (no salt, no info): the first 32 bytes of its OKM.
        let mut out = [0; 32];
        hkdf(&[0x0b; 22], b"", &mut out);
        assert_eq!(crate::state::hex(&out), "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d");
    }
}
