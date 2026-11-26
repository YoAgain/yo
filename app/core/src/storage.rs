//! The account's storage at the hub (spec 4.3): friends with their name and colour, blocks, the app's icon,
//! left-handed mode, profile keys and groups' master keys, sealed under a key from the account key so a restored phone
//! gets them back and the hub reads none of it.
//! Every field carries the time it was set; versions merge field by field, the later winning, friends as the union.
//! The hub takes a write only on top of its current version, so the loser fetches, merges and writes again.

use crate::error::{Error, Result};
use crate::Client;
use yoip::net::api::{self, status};
use yoip::protocol::storage::{self, decode_key};
pub use yoip::protocol::storage::{
    hkdf, key, open_padded, Connection, Field, Friend, GroupEntry, Storage, MAX_FRIEND_NAME,
};
use yoip::{decode, encode};

/// Pads `plain` like an envelope (spec 4.1) and seals it (nonce || ChaCha20-Poly1305 || tag) to a size class of
/// `SEALED_SIZES`: the storage and a backup's sealed part (spec 4.3, 2.4). `Protocol("too large")` past the largest.
pub fn seal_padded(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    storage::seal_padded(key, aad, plain, crate::identity::random).map_err(|_| Error::Protocol("too large"))
}

/// Seals the storage for a user (associated data: "yo storage v1" || user id), padded (`seal_padded`).
pub fn seal(account_secret: &[u8; 32], user_id: &[u8], storage: &Storage) -> Result<Vec<u8>> {
    storage::seal(account_secret, user_id, storage, crate::identity::random).map_err(|_| Error::Protocol("too large"))
}

pub fn open(account_secret: &[u8; 32], user_id: &[u8], sealed: &[u8]) -> Result<Storage> {
    storage::open(account_secret, user_id, sealed).map_err(|_| Error::Protocol("storage"))
}

/// Opens the storage sealed under `key` (`open` derives it from the account's secret).
pub fn open_under(key: &[u8; 32], user_id: &[u8], sealed: &[u8]) -> Result<Storage> {
    storage::open_under(key, user_id, sealed).map_err(|_| Error::Protocol("storage"))
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
        // Added (again): a contact as a yes makes one (spec 3.3), told this account's key.
        let adding = self.state.storage.friends.get(user).is_none_or(|f| f.removed.value);
        if let Some(contact) = self.state.contacts.get_mut(user).filter(|c| adding && !c.accepted && !c.probe_service) {
            contact.accepted = true;
            contact.chat = true;
            contact.key_due = !contact.blocked && !contact.gone;
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
    /// request (spec 3.3, 4.3): the app's own name and colour stay as they are, a removal is taken back.
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
    /// restored from the storage has it (spec 3.3, 4.3): what comes from them later is a request again, and nothing
    /// (key, name, receipt) goes back until a yes, a Yo to them or adding them again.
    pub fn remove_friend(&mut self, user: &str) -> Result<()> {
        let now = crate::wall_ms();
        if let Some(contact) = self.state.contacts.get_mut(user) {
            contact.accepted = false;
            contact.chat = false;
            contact.key_due = false;
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

    /// "Requests from Strangers", kept in the storage too (`set_requests_open`): one choice for the account.
    pub(crate) fn storage_requests_open(&mut self, open: bool) {
        if self.state.storage.requests_open.value != Some(open) {
            self.state.storage.requests_open.set(Some(open), crate::wall_ms());
            self.state.storage_dirty = true;
        }
    }

    /// A block or unblock, kept in the storage too (`set_blocked`).
    pub(crate) fn storage_block(&mut self, user: &str, blocked: bool) {
        // Blocking someone never on the list (a stranger's request) keeps only the block: the entry is marked
        // removed, so no device takes them for a friend, now or after the unblock (spec 3.3, 4.3).
        let stranger = blocked
            && !self.state.storage.friends.contains_key(user)
            && self.state.contacts.get(user).is_none_or(|c| !c.accepted);
        let friend = self.state.storage.friends.entry(user.to_owned()).or_default();
        if stranger {
            friend.removed.set(true, crate::wall_ms());
            self.state.storage_dirty = true;
        }
        if friend.blocked.value != blocked || friend.blocked.at == 0 {
            friend.blocked.set(blocked, crate::wall_ms());
            self.state.storage_dirty = true;
        }
    }

    /// A service's connection, changed; saved by the caller.
    pub(crate) fn set_connection(&mut self, user: &str, change: impl FnOnce(&mut Connection)) {
        let field = self.state.storage.connections.entry(user.to_owned()).or_default();
        let mut connection = field.value.clone();
        change(&mut connection);
        field.set(connection, crate::wall_ms().max(field.at + 1));
        self.state.storage_dirty = true;
    }

    /// The profile key into the storage, timed after the one there whatever the clock says, so a later key wins.
    pub(crate) fn store_own_key(&mut self) {
        let value = self.state.profile_key.map(|key| yoip::b64(&key));
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
        let value = key.map(|key| yoip::b64(&key));
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
                if self.state.block_access_due {
                    self.rotate_profile_key();
                } else if new {
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
        let Some(group) = self.state.groups.get(id).filter(|g| g.revision > 0 && (g.accepted || g.left)) else {
            return;
        };
        let Some(key) = crate::state::unhex(&group.key) else { return };
        let (left, accepted) = (group.left, group.accepted);
        let storage = &mut self.state.storage;
        if left && storage.groups.get(id).is_none_or(|g| g.removed.value) {
            return;
        }
        let entry = storage.groups.entry(id.to_owned()).or_default();
        let mut changed = false;
        if entry.key.is_empty() {
            entry.key = yoip::b64(&key);
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
            .filter(|(id, g)| g.accepted.value && !g.removed.value && !self.state.groups.contains_key(*id))
            .filter_map(|(id, g)| {
                let key: yoip::protocol::content::GroupKey = yoip::unb64(&g.key).ok()?.try_into().ok()?;
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
    /// phone) become accepted contacts due the profile key (3.3); a known one keeps its own say.
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
        // "Requests from Strangers" as another device of this account set it: the hub is told again (3.3).
        if let Some(open) = self.state.storage.requests_open.value {
            if open != self.state.requests_open {
                self.state.requests_open = open;
                self.state.access_key_told = false;
            }
        }
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

    /// A first read or a sibling's StorageChanged hint. The caller saves the merged state.
    pub(super) fn read_storage(&mut self) -> Result<()> {
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
        Ok(())
    }

    /// Brings the storage in line with the hub's: fetched once before anything is received, then written on change.
    pub(crate) fn sync_storage(&mut self) -> Result<()> {
        if !self.state.storage_fetched {
            self.read_storage()?;
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
            next.groups.retain(|_, group| group.accepted.value || group.removed.value);
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
                    self.queue_sync(yoip::protocol::content::SyncMessage::StorageChanged)?;
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
    /// then groups left, then removed friends, longest gone first. Blocked entries are never dropped.
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
