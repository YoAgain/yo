//! The account's storage at the hub (spec 6.9): friends with their name and colour, blocks, the app's icon,
//! left-handed mode, profile keys and groups' master keys, sealed under a key from the account key so a restored phone
//! gets them back and the hub reads none of it.
//! Every field carries the time it was set; versions merge field by field, the later winning, friends as the union.
//! The hub takes a write only on top of its current version, so the loser fetches, merges and writes again.

use crate::error::{Error, Result};
use crate::Client;
#[cfg(test)]
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
#[cfg(test)]
use chacha20poly1305::ChaCha20Poly1305;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use yo_wire::api::{self, status};
use yo_wire::{decode, encode};

/// HKDF-SHA256 without salt (RFC 5869), up to 32 bytes: the derivation spec 7 and 6.9 name.
pub fn hkdf(ikm: &[u8], info: &[u8], out: &mut [u8]) {
    hkdf::Hkdf::<sha2_11::Sha256>::new(None, ikm).expand(info, out).expect("valid HKDF output length");
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
    pub(crate) fn unset(&self) -> bool {
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

/// Fields of a later version, merged without knowing them (spec 6.9): a `[value, at]` field by its time, then its
/// value; an object field by field; anything else by the larger value.
fn merge_extra(mine: &mut BTreeMap<String, serde_json::Value>, theirs: &BTreeMap<String, serde_json::Value>) {
    for (key, value) in theirs {
        mine.entry(key.clone()).and_modify(|entry| merge_value(entry, value)).or_insert_with(|| value.clone());
    }
}

fn merge_value(mine: &mut serde_json::Value, theirs: &serde_json::Value) {
    use serde_json::Value;
    // A field: (true, its time, its value); anything else: (false, 0, itself). So a field wins over what is none.
    let key = |v: &Value| {
        let bytes = |v: &Value| serde_json::to_vec(v).unwrap_or_default();
        match v {
            Value::Array(pair) if pair.len() == 2 && pair[1].is_u64() => {
                (true, pair[1].as_u64().unwrap_or(0), bytes(&pair[0]))
            }
            other => (false, 0, bytes(other)),
        }
    };
    if let (Value::Object(ours), Value::Object(other)) = (&mut *mine, theirs) {
        for (key, value) in other {
            ours.entry(key.clone()).and_modify(|entry| merge_value(entry, value)).or_insert_with(|| value.clone());
        }
    } else if key(theirs) > key(mine) {
        *mine = theirs.clone();
    }
}

/// A group this account is in, as Signal's GroupV2Record: its master key, so a new phone fetches it at once. Kept
/// after leaving (`removed`), so the leaving survives a merge.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupEntry {
    /// The master key (base64url): the same for the group's every version, never changed.
    #[serde(default, rename = "k", skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// Joined (Signal's whitelisted): else a request (spec 4.10).
    #[serde(default, rename = "a", skip_serializing_if = "Field::unset")]
    pub accepted: Field<bool>,
    /// This account is in it no more.
    #[serde(default, rename = "r", skip_serializing_if = "Field::unset")]
    pub removed: Field<bool>,
    /// What a later version keeps here, carried along unread.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl GroupEntry {
    fn merge(&mut self, other: &GroupEntry) {
        if self.key.is_empty() {
            self.key.clone_from(&other.key);
        }
        self.accepted.merge(&other.accepted);
        self.removed.merge(&other.removed);
        merge_extra(&mut self.extra, &other.extra);
    }
}

/// The longest name kept for a friend, in bytes.
pub const MAX_FRIEND_NAME: usize = 64;

/// What is sealed: JSON, so a later version adds fields that an older one keeps (`extra`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Storage {
    /// The hub's version this was written as: an older one handed out later is noticed and ignored.
    #[serde(default)]
    pub version: u64,
    /// By user id (hex).
    #[serde(default)]
    pub friends: BTreeMap<String, Friend>,
    /// The app's icon: none (the default) or its name ("Negative", "Original").
    #[serde(default)]
    pub icon: Field<Option<String>>,
    /// Left-handed mode: none until a device chooses it.
    #[serde(default)]
    pub left_handed: Field<Option<bool>>,
    /// This account's profile key (base64url), so a restored phone keeps its contacts' deliveries good (spec 4.10).
    #[serde(default, rename = "pk", skip_serializing_if = "Field::unset")]
    pub profile_key: Field<Option<String>>,
    /// Its generation (spec 4.10), set with it.
    #[serde(default, rename = "pg", skip_serializing_if = "Field::unset")]
    pub profile_key_gen: Field<u32>,
    /// Contacts' profile keys (base64url) by user id (hex), none once forgotten. The first to go when it would not fit.
    #[serde(default, rename = "k", skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, Field<Option<String>>>,
    /// The groups this account is or was in, by group id (hex), as Signal's storage keeps them.
    #[serde(default, rename = "g", skip_serializing_if = "BTreeMap::is_empty")]
    pub groups: BTreeMap<String, GroupEntry>,
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
        self.left_handed.merge(&other.left_handed);
        self.profile_key.merge(&other.profile_key);
        self.profile_key_gen.merge(&other.profile_key_gen);
        for (user, key) in &other.keys {
            self.keys.entry(user.clone()).or_default().merge(key);
        }
        for (id, group) in &other.groups {
            self.groups.entry(id.clone()).or_default().merge(group);
        }
        merge_extra(&mut self.extra, &other.extra);
        self.version = self.version.max(other.version);
    }
}

/// A nonce and a tag: what sealing adds.
const SEAL_OVERHEAD: usize = 12 + 16;

/// Pads `plain` like an envelope (spec 6.1) and seals it (nonce || ChaCha20-Poly1305 || tag) to a size class of
/// `SEALED_SIZES`: the storage and a backup's sealed part (spec 6.9, 7). `Protocol("too large")` past the largest.
pub fn seal_padded(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    let sizes = yo_wire::limits::SEALED_SIZES.map(|size| size - SEAL_OVERHEAD);
    let padded = yo_wire::envelope::pad(plain, &sizes).map_err(|_| Error::Protocol("too large"))?;
    Ok(crate::aead::seal(key, aad, &padded))
}

/// Opens what `seal_padded` sealed, and unpadded JSON sealed before the padding (it starts with `{`, not 1).
pub fn open_padded(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let plain = crate::aead::open(key, aad, sealed)?;
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
    open_under(&key(account_secret), user_id, sealed)
}

/// Opens the storage sealed under `key` (`open` derives it from the account's secret).
pub fn open_under(key: &[u8; 32], user_id: &[u8], sealed: &[u8]) -> Result<Storage> {
    let plain =
        open_padded(key, &[&b"yo storage v1"[..], user_id].concat(), sealed).ok_or(Error::Protocol("storage"))?;
    serde_json::from_slice(&plain).map_err(|_| Error::Protocol("storage"))
}

fn decode_key(text: &str) -> Option<[u8; 32]> {
    yo_wire::unb64(text).ok()?.try_into().ok()
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

    /// Adds a friend, or changes its name or colour; written to the hub with the next `sync` or `maintenance`.
    pub fn set_friend(&mut self, user: &str, name: Option<String>, color: Option<u8>) -> Result<()> {
        crate::user_bytes(user)?;
        if name.as_ref().is_some_and(|n| n.len() > MAX_FRIEND_NAME) {
            return Err(Error::Protocol("friend's name too long"));
        }
        self.promote_contact(user);
        let now = crate::wall_ms();
        // Added (again): a contact as a yes makes one (spec 4.10), told this account's key and name.
        let adding = self.state.storage.friends.get(user).is_none_or(|f| f.removed.value);
        let named = self.state.name.is_some();
        if let Some(contact) = self.state.contacts.get_mut(user).filter(|c| adding && !c.accepted && !c.probe_service) {
            contact.accepted = true;
            contact.chat = true;
            contact.key_due = !contact.blocked && !contact.gone;
            contact.profile_due = named;
        }
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

    /// A contact said yes to (accepted, or written to) kept as a friend, so a restored phone has it as one and not as a
    /// request (spec 4.10, 6.9): the app's own name and colour stay as they are, a removal is taken back.
    pub(crate) fn keep_friend(&mut self, user: &str) {
        if user == self.state.user_id || self.probe_service(user) {
            return;
        }
        let friend = self.state.storage.friends.entry(user.to_owned()).or_default();
        if friend.removed.value || friend.removed.unset() {
            friend.removed.set(false, crate::wall_ms().max(friend.removed.at + 1));
            self.state.storage_dirty = true;
        }
    }

    /// Takes a friend off the list (a block stays as it is). No contact from then on, as after a no and as a phone
    /// restored from the storage has it (spec 4.10, 6.9): what comes from them later is a request again, and nothing
    /// (key, name, receipt) goes back until a yes, a Yo to them or adding them again.
    pub fn remove_friend(&mut self, user: &str) -> Result<()> {
        let now = crate::wall_ms();
        if let Some(contact) = self.state.contacts.get_mut(user) {
            contact.accepted = false;
            contact.chat = false;
            contact.key_due = false;
            contact.profile_due = false;
            contact.receipt_due = false;
        }
        if let Some(friend) = self.state.storage.friends.get_mut(user) {
            friend.removed.set(true, now);
            self.state.storage_dirty = true;
        }
        self.persist()
    }

    /// The app's icon as kept in the storage: none for the default.
    pub fn app_icon(&self) -> Option<String> {
        self.state.storage.icon.value.clone()
    }

    /// When the hub last held all of the storage (wall clock, ms); none while a change waits to go up.
    pub fn storage_saved_at(&self) -> Option<u64> {
        let held = !self.state.storage_dirty && !self.state.storage_locked;
        (self.state.storage_fetched && held && self.state.storage_saved_at > 0).then_some(self.state.storage_saved_at)
    }

    pub fn set_app_icon(&mut self, icon: Option<String>) -> Result<()> {
        if self.state.storage.icon.value != icon {
            self.state.storage.icon.set(icon, crate::wall_ms());
            return self.storage_changed();
        }
        Ok(())
    }

    /// Left-handed mode as kept in the storage: none until chosen.
    pub fn left_handed(&self) -> Option<bool> {
        self.state.storage.left_handed.value
    }

    pub fn set_left_handed(&mut self, left_handed: bool) -> Result<()> {
        if self.state.storage.left_handed.value != Some(left_handed) {
            self.state.storage.left_handed.set(Some(left_handed), crate::wall_ms());
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

    /// The profile key into the storage, timed after the one there whatever the clock says, so a later key wins.
    pub(crate) fn store_own_key(&mut self) {
        let value = self.state.profile_key.map(|key| yo_wire::b64(&key));
        let generation = self.state.profile_key_gen;
        let storage = &mut self.state.storage;
        if storage.profile_key.value != value
            || storage.profile_key.unset()
            || storage.profile_key_gen.value != generation
        {
            let at = crate::wall_ms().max(storage.profile_key.at + 1).max(storage.profile_key_gen.at + 1);
            storage.profile_key.set(value, at);
            storage.profile_key_gen.set(generation, at);
            self.state.storage_dirty = true;
        }
    }

    /// A contact's profile key as this device has it now (none: forgotten), into the storage.
    pub(crate) fn store_key(&mut self, user: &str, key: Option<[u8; 32]>) {
        if self.probe_service(user) {
            return;
        }
        let value = key.map(|key| yo_wire::b64(&key));
        let field = self.state.storage.keys.entry(user.to_owned()).or_default();
        if field.value != value || field.unset() {
            let at = crate::wall_ms().max(field.at + 1);
            field.set(value, at);
            self.state.storage_dirty = true;
        }
    }

    /// A contact's key forgotten (refused by the hub): not taken back from the storage.
    pub(crate) fn forget_key_stored(&mut self, user: &str) {
        if self.state.storage.keys.get(user).is_some_and(|f| f.value.is_some()) {
            self.store_key(user, None);
        }
    }

    /// Applies the profile keys the storage holds: this account's and the contacts' not known here (a new phone).
    fn apply_keys(&mut self) {
        let friends: Vec<_> =
            self.state.storage.friends.iter().filter(|(_, f)| !f.removed.value).map(|(user, _)| user.clone()).collect();
        for user in friends {
            self.promote_contact(&user);
        }

        let own = self.state.storage.profile_key.value.as_deref().and_then(decode_key);
        match own {
            Some(key) if self.state.profile_key != Some(key) => {
                let new = self.state.profile_key.is_some();
                self.state.profile_key = Some(key);
                self.state.profile_key_gen = self.state.storage.profile_key_gen.value;
                // The hub's check is told again; it may be this key's already.
                self.state.access_key_told = false;
                // A key made elsewhere while this device had its own: every contact gets it; a new phone's first key
                // not.
                if new {
                    self.key_due_to_all();
                }
            }
            Some(_) => {}
            None if self.state.profile_key.is_some() => self.store_own_key(),
            None => {}
        }
        let keys: Vec<(String, [u8; 32])> = self
            .state
            .storage
            .keys
            .iter()
            .filter_map(|(user, f)| Some((user.clone(), f.value.as_deref().and_then(decode_key)?)))
            .collect();
        for (user, key) in keys {
            if user == self.state.user_id {
                continue;
            }
            let contact = self.state.contacts.entry(user.clone()).or_default();
            if contact.profile_key.is_none() {
                self.adopt_key(&user, key);
            }
        }
    }

    /// A fetched group into the storage: its key while in it, and when it left. A group never kept and left stays out
    /// (a stranger's made-up key never goes up).
    pub(crate) fn store_group(&mut self, id: &str) {
        let Some(group) = self.state.groups.get(id).filter(|g| g.revision > 0) else { return };
        let Some(key) = crate::state::unhex(&group.key) else { return };
        let (left, accepted) = (group.left, group.accepted);
        let storage = &mut self.state.storage;
        if left && storage.groups.get(id).is_none_or(|g| g.removed.value) {
            return;
        }
        let entry = storage.groups.entry(id.to_owned()).or_default();
        let mut changed = false;
        if entry.key.is_empty() {
            entry.key = yo_wire::b64(&key);
            changed = true;
        }
        if entry.removed.value != left {
            entry.removed.set(left, crate::wall_ms().max(entry.removed.at + 1));
            changed = true;
        }
        if !left && entry.accepted.value != accepted {
            entry.accepted.set(accepted, crate::wall_ms().max(entry.accepted.at + 1));
            changed = true;
        }
        if changed {
            self.state.storage_dirty = true;
        }
    }

    /// The hub says this account is in the group no more: so in the storage too, if it is there.
    pub(crate) fn unstore_group(&mut self, id: &str) {
        if let Some(entry) = self.state.storage.groups.get_mut(id).filter(|g| !g.removed.value) {
            entry.removed.set(true, crate::wall_ms().max(entry.removed.at + 1));
            self.state.storage_dirty = true;
        }
    }

    /// Groups the storage holds that this device does not know (a restored phone), fetched with `refresh_groups`.
    fn apply_groups(&mut self) {
        let entries: Vec<(String, String, bool)> = self
            .state
            .storage
            .groups
            .iter()
            .filter(|(id, g)| !g.removed.value && !self.state.groups.contains_key(*id))
            .filter_map(|(id, g)| {
                let key: yo_wire::content::GroupKey = yo_wire::unb64(&g.key).ok()?.try_into().ok()?;
                // Its id is the key's: a damaged entry is passed over.
                (crate::groups::group_id_of(&key) == *id)
                    .then(|| (id.clone(), crate::state::hex(&key), g.accepted.value))
            })
            .collect();
        for (id, key, accepted) in entries {
            if self.state.groups.len() >= crate::groups::MAX_GROUPS {
                self.note("a group of the storage past the most kept".into());
                break;
            }
            self.state.groups.insert(id, crate::state::Group { key, accepted, ..Default::default() });
        }
    }

    fn storage_changed(&mut self) -> Result<()> {
        self.state.storage_dirty = true;
        self.persist()
    }

    /// The storage applied to the contacts: its blocks hold, and friends this device does not know (a restored
    /// phone) become accepted contacts due the profile key (4.10); a known one keeps its own say.
    fn apply_storage(&mut self) {
        let me = self.state.user_id.clone();
        let friends: Vec<(String, Field<bool>, bool)> = self
            .state
            .storage
            .friends
            .iter()
            .filter(|(user, _)| **user != me)
            .map(|(user, f)| (user.clone(), f.blocked.clone(), f.removed.value))
            .collect();
        for (user, blocked, removed) in friends {
            if blocked.at > 0 {
                self.apply_block(&user, blocked.value);
            }
            if !removed && !blocked.value {
                let new = crate::state::Contact { accepted: true, key_due: true, ..Default::default() };
                self.state.contacts.entry(user).or_insert(new);
            }
        }
        self.apply_keys();
        self.apply_groups();
    }

    /// Takes in a version from the hub: merged with this device's, applied.
    fn take_storage(&mut self, version: u64, sealed: &[u8]) -> Result<()> {
        let user = self.user_id_bytes();
        let theirs = match (open(&self.state.account_secret, &user, sealed), self.state.legacy_storage_key) {
            (Ok(theirs), _) => theirs,
            // Sealed under the migrated Ed25519 seed's key: taken in, and written under the current key next time.
            (Err(error), Some(legacy)) => {
                let theirs = open_under(&legacy, &user, sealed).map_err(|_| error)?;
                self.state.storage_dirty = true;
                theirs
            }
            (Err(error), None) => return Err(error),
        };
        // Sealed with the version it was written as: an older blob under a newer number is a hub replaying it.
        if theirs.version != version {
            return Err(Error::Protocol("storage version"));
        }
        self.state.storage.merge(&theirs);
        // The hub's version is the one to write over, also an older one (its database restored from a backup).
        self.state.storage_version = version;
        // What this device had and the hub's did not (or had older) goes back up.
        if self.state.storage != (Storage { version: self.state.storage.version, ..theirs }) {
            self.state.storage_dirty = true;
        }
        self.apply_storage();
        Ok(())
    }

    /// The hub's storage taken in. Replayed (an older one): written over. Not openable here (sealed under a key not
    /// carried over): left standing, never written over by this device.
    fn take_stored(&mut self, stored: api::Storage) {
        // None at the hub (version 0, no data: its database restored to before this account's first write): all of
        // this device's goes up.
        if stored.version == 0 && stored.data.as_slice().is_empty() {
            self.state.storage_version = 0;
            self.state.storage_dirty = true;
            return;
        }
        if let Err(error) = self.take_storage(stored.version, stored.data.as_slice()) {
            self.drop_one(format!("storage: {error}"));
            self.state.storage_version = stored.version;
            match error {
                Error::Protocol("storage version") => self.state.storage_dirty = true,
                _ => self.state.storage_locked = true,
            }
        }
    }

    /// Brings the storage in line with the hub's: fetched once before anything is received, then written on change.
    pub(crate) fn sync_storage(&mut self) -> Result<()> {
        if !self.state.storage_fetched {
            let response = self.signed("GET", api::STORAGE, &[])?;
            match response.status {
                status::OK => {
                    self.take_stored(decode(&response.body)?);
                }
                status::GONE => return Err(Error::Replaced),
                other if crate::retryable(&Error::Refused(other)) => return Err(Error::Refused(other)),
                _ => {}
            }
            // Blocks this device had and the storage has not go up with the first write.
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
        // The groups this device knows, also those the storage does not have yet.
        let groups: Vec<String> = self.state.groups.keys().cloned().collect();
        for id in groups {
            self.store_group(&id);
        }
        if self.state.storage_locked {
            return Ok(());
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
                    // Under this key now: the one of before opens nothing more.
                    self.state.legacy_storage_key = None;
                    self.persist()?;
                }
                // Another writer was first: its version, merged, goes up again.
                status::CONFLICT => {
                    self.take_stored(decode(&response.body)?);
                    // Left unsaved when locked: never shown as held by the hub.
                    self.state.storage_dirty = true;
                    self.persist()?;
                    if self.state.storage_locked {
                        return Ok(());
                    }
                }
                status::GONE => return Err(Error::Replaced),
                other => return Err(Error::Refused(other)),
            }
        }
        Ok(())
    }

    /// The storage sealed within the hub's limit: the contacts' profile keys go first (oldest first; they come again),
    /// then groups left, then removed friends, longest gone first.
    fn sealed_within_limit(&mut self, next: &mut Storage) -> Result<Vec<u8>> {
        loop {
            if let Ok(data) = seal(&self.state.account_secret, &self.user_id_bytes(), next) {
                self.state.storage.friends.clone_from(&next.friends);
                self.state.storage.keys.clone_from(&next.keys);
                self.state.storage.groups.clone_from(&next.groups);
                return Ok(data);
            }
            if !next.keys.is_empty() {
                let drop = (next.keys.len() / 8).max(1);
                let mut by_age: Vec<(u64, String)> = next.keys.iter().map(|(u, f)| (f.at, u.clone())).collect();
                by_age.sort_unstable();
                for (_, user) in by_age.into_iter().take(drop) {
                    next.keys.remove(&user);
                }
                continue;
            }
            let left = next.groups.iter().filter(|(_, g)| g.removed.value).min_by_key(|(_, g)| g.removed.at);
            if let Some(id) = left.map(|(id, _)| id.clone()) {
                next.groups.remove(&id);
                continue;
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
        a.left_handed = Field { value: Some(true), at: 4 };
        a.icon = Field { value: Some("Negative".into()), at: 9 };
        let mut b = Storage::default();
        b.friends.insert("aa".into(), Friend { blocked: Field { value: true, at: 7 }, ..friend("Leonard", 3) });
        b.friends.insert("bb".into(), friend("Mia", 1));
        b.left_handed = Field { value: Some(false), at: 9 };
        b.icon = Field { value: Some("Original".into()), at: 4 };
        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab, ba, "the same either way");
        assert_eq!(ab.friends.len(), 2);
        assert_eq!(ab.friends["aa"].name.value.as_deref(), Some("Leo"), "the later name");
        assert!(ab.friends["aa"].blocked.value, "the block from the other side");
        assert_eq!(ab.left_handed.value, Some(false));
        assert_eq!(ab.icon.value.as_deref(), Some("Negative"));
    }

    #[test]
    fn left_handed_round_trips_and_old_storage_opens() {
        let key = [1; 32];
        let old =
            seal_padded(&self::key(&key), b"yo storage v1user", br#"{"version":3,"icon":["Negative",9]}"#).unwrap();
        let old = open(&key, b"user", &old).unwrap();
        assert_eq!(old.left_handed.value, None);
        assert_eq!(old.icon.value.as_deref(), Some("Negative"));
        for value in [None, Some(false), Some(true)] {
            let storage = Storage { left_handed: Field { value, at: 10 }, ..Storage::default() };
            let sealed = seal(&key, b"user", &storage).unwrap();
            assert_eq!(open(&key, b"user", &sealed).unwrap(), storage);
        }
    }

    /// Unknown fields merge as the known ones do, by time, on every level.
    #[test]
    fn unknown_fields_merge_by_time_on_every_level() {
        let parse = |json: &str| -> Storage { serde_json::from_str(json).unwrap() };
        let a = parse(r#"{"later":["z",1],"deep":{"p":["old",1],"q":["mine",4]},"friends":{"aa":{"x":["a",9]}}}"#);
        let b = parse(r#"{"later":["a",2],"deep":{"p":["new",3],"r":["theirs",2]},"friends":{"aa":{"x":["b",8]}}}"#);
        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab, ba, "the same either way");
        let json = serde_json::to_value(&ab).unwrap();
        assert_eq!(json["later"], serde_json::json!(["a", 2]), "the later");
        assert_eq!(json["deep"], serde_json::json!({"p": ["new", 3], "q": ["mine", 4], "r": ["theirs", 2]}));
        assert_eq!(json["friends"]["aa"]["x"], serde_json::json!(["a", 9]));
        // Equal times: by the value, the same on both sides.
        let (mut c, d) = (parse(r#"{"t":["a",5]}"#), parse(r#"{"t":["b",5]}"#));
        c.merge(&d);
        assert_eq!(serde_json::to_value(&c).unwrap()["t"], serde_json::json!(["b", 5]));
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
}
