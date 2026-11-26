//! What a device keeps (spec 1.3): its keys, contacts, the outbox and the session store (written to `statedb`).

use crate::error::{Error, Result};
use crate::identity::Key;
use serde::{Deserialize, Serialize};

/// Bytes in the stored JSON as base64url text, not as an array of numbers (a third of the size).
mod compact {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};
    use std::collections::VecDeque;

    fn bytes<'de, D: Deserializer<'de>>(text: String) -> Result<Vec<u8>, D::Error> {
        yoip::unb64(&text).map_err(|_| D::Error::custom("base64"))
    }

    pub mod bytes {
        use super::*;
        pub fn serialize<S: Serializer>(value: &[u8], s: S) -> Result<S::Ok, S::Error> {
            s.serialize_str(&yoip::b64(value))
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
            super::bytes::<D>(String::deserialize(d)?)
        }
    }

    pub mod list {
        use super::*;
        pub fn serialize<S: Serializer>(value: &VecDeque<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
            s.collect_seq(value.iter().map(|v| yoip::b64(v)))
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<VecDeque<Vec<u8>>, D::Error> {
            Vec::<String>::deserialize(d)?.into_iter().map(super::bytes::<D>).collect()
        }
    }

    pub mod hashes {
        use super::*;
        pub fn serialize<S: Serializer>(value: &VecDeque<[u8; 32]>, s: S) -> Result<S::Ok, S::Error> {
            s.collect_seq(value.iter().map(|v| yoip::b64(v)))
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<VecDeque<[u8; 32]>, D::Error> {
            let hash = |text| super::bytes::<D>(text)?.try_into().map_err(|_| D::Error::custom("hash"));
            Vec::<String>::deserialize(d)?.into_iter().map(hash).collect()
        }
    }
}
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};
use yoip::DeviceId;
use zeroize::Zeroize;

/// The old blob's versions: 2 holds libsignal's keys, 1 Ed25519 ones.
const STATE_VERSION: u8 = 2;
const AAD: &[u8] = b"yo state v2";

/// Resend plaintexts, shared until changed: a transaction's rollback copy stays cheap.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SentCache(std::sync::Arc<VecDeque<Vec<u8>>>);
impl Clone for SentCache {
    fn clone(&self) -> Self {
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

/// Read both the current list and the optional single value saved by earlier installs.
fn roster_list<'de, D, T>(d: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored<T> {
        List(Vec<T>),
        Single(Option<T>),
    }
    Ok(match Stored::deserialize(d)? {
        Stored::List(list) => list,
        Stored::Single(one) => one.into_iter().collect(),
    })
}

/// A user's device: its key and its number from the hub, the session's address (spec 3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub key: Key,
    pub slot: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    /// PING routing state; hidden only until explicitly added or written to.
    pub probe_service: bool,
    pub probe_reply_tag: [u8; 32],
    pub probe_pending: Option<[u8; 32]>,
    /// The account key pinned on first contact, or the one confirmed after a change.
    pub account_key: Key,
    pub name: Option<String>,
    pub verified: bool,
    /// The key changed after the contact was verified: nothing is sent until the person confirms it (spec 2.3).
    pub unconfirmed: bool,
    pub blocked: bool,
    /// Usernames as their owner writes them, one per skeleton.
    pub handles: Vec<String>,
    /// The hub no longer knows the user; its handles are forgotten, so its old name finds the new owner.
    pub gone: bool,
    /// The user's device as the hub named it last (spec 3.2).
    #[serde(default, alias = "device", deserialize_with = "roster_list")]
    pub devices: Vec<Slot>,
    /// Something was sent to the user or came from them: a chat the app shows.
    pub chat: bool,
    /// The own name is still to be told to the user.
    pub profile_due: bool,
    /// The last plaintexts sent, resent when a `DecryptionErrorMessage` names their time (spec 3.2).
    pub sent: SentCache,
    /// Which devices the cached content was encrypted for, by its original timestamp.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sent_devices: BTreeMap<u64, Vec<Key>>,
    /// Something came from the user since the last receipt (spec 3.2).
    pub receipt_due: bool,
    /// `DecryptionErrorMessage`s due to its device (spec 3.2).
    pub decryption_errors: Vec<DecryptionError>,
    /// When the hub last named its device without prekeys (ms; 0: not): an app too old for sessions (spec 3.1).
    pub outdated_at: u64,
    /// Accepted (spec 3.3): until then what comes from them is a request, with no receipt and no name back.
    pub accepted: bool,
    /// Their profile key, whose access key deliveries to them show.
    pub profile_key: Option<[u8; 32]>,
    /// This account's profile key is still to go to them.
    pub key_due: bool,
    /// The current profile key was queued for them.
    pub key_given: bool,
    /// When the key last went back for an identified message of theirs: once an hour at most (spec 3.3).
    pub key_resent_at: u64,
    /// The latest generation of their profile key seen: an older carried key is not taken.
    pub key_gen: u32,
}

/// A `DecryptionErrorMessage` (as libsignal's `PlaintextContent`) due to a contact's device (spec 3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecryptionError {
    pub device: Key,
    #[serde(with = "compact::bytes")]
    pub content: Vec<u8>,
}

/// This device's registration (spec 4.2), saved before it leaves so a crash can finish it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registering {
    #[serde(with = "compact::bytes")]
    pub request: Vec<u8>,
    /// Sent before: the hub may have taken it, so it is asked first.
    pub sent: bool,
}

/// A message for one user, waiting for the hub to take it (spec 3.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outgoing {
    /// When queued (wall clock, ms).
    pub queued_at: u64,
    /// Its place in the order of everything this device sent.
    pub seq: u64,
    /// The user it goes to (hex).
    pub user: String,
    /// The session plaintext (`envelope::Plain`); for `device`, a `PlaintextContent` sent as it is.
    #[serde(with = "compact::bytes")]
    pub plain: Vec<u8>,
    /// Only for this device of the user, outside the session: a `DecryptionErrorMessage` (spec 3.2).
    pub device: Option<Key>,
    /// A decryption-error resend: catch up silently even when its content was visible.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub non_urgent: bool,
    /// Its encrypted payloads, one per device (spec 3.2, 4.1), and the devices they were made for: made again if
    /// those changed. A save from before keeps one payload for all (`payload`): not read, made again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payloads: Vec<DevicePayload>,
    #[serde(default, deserialize_with = "roster_list")]
    pub made_for: Vec<Key>,
    /// Only these keys get a copy (targeted repair or a sibling delivery).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audience: Vec<Key>,
    /// Refused (`403`) with the recipient's key: their new key may wait in the inbox, so it goes again after the
    /// next receive (spec 3.3).
    pub refused: bool,
}

/// One device's payload of a message: its number and the payload (`envelope::Payload` with its one copy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePayload {
    pub device: u8,
    #[serde(with = "compact::bytes")]
    pub payload: Vec<u8>,
}

/// This device's prekeys (spec 3.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prekeys {
    pub next_id: u32,
    pub signed_at: u64,
    /// One-time prekeys of the scarcer kind the hub still has.
    pub left: u32,
    /// An upload not yet answered, sent again as it is, and how many one-time prekeys of each kind it brings.
    pub upload: Option<(Vec<u8>, u32)>,
    /// Replaced signed (`sp`) and last resort (`kl`) prekeys with when: kept a month, while sessions started with
    /// them may still wait at the hub.
    pub retired: Vec<(String, u32, u64)>,
}

/// A group as fetched from the hub (spec 3.4), kept also once left.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// Its master key (hex).
    pub key: String,
    pub name: String,
    /// Member user ids (hex), sorted.
    pub members: Vec<String>,
    pub revision: u32,
    pub left: bool,
    /// Joined; until then a group a stranger added this account to is a request (spec 3.3).
    pub accepted: bool,
    /// This account's entry at the hub has its profile key (else it is only invited).
    pub keyed: bool,
    /// The newest revision a message named (spec 3.4): above `revision`, the group is fetched again.
    pub named: u32,
    /// Group send endorsements (spec 3.4): until when (s), and each member's.
    pub endorsed_until: u64,
    pub endorsements: BTreeMap<String, String>,
    /// Entries at the hub no member can open (hex of their bytes), to be removed (spec 3.4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
}

/// When to attest the device next (spec 4.2).
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Attestation {
    pub attested: bool,
    /// This device registered with attestation in place (asked right away); a device from before has it false and
    /// is asked once by `migrations::attest_old_devices`.
    #[serde(default)]
    pub at_registration: bool,
    pub attempts: u32,
    pub next_try_ms: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub attestation: Attestation,
    pub user_id: String,
    pub username: Option<String>,
    pub account_secret: [u8; 32],
    pub auth_secret: [u8; 32],
    /// The hub's name for this device's inbox, never given to other users.
    pub device_id: DeviceId,
    /// What senders encrypt for and the certificate vouches for (spec 2).
    pub device_key: Key,
    /// Its number from the hub: its sessions' address.
    pub device_number: u8,
    #[serde(with = "compact::bytes")]
    pub certificate: Vec<u8>,
    pub registration_id: u32,
    pub name: Option<String>,
    pub contacts: BTreeMap<String, Contact>,
    /// This account's other devices, as the hub lists them (spec 3.2).
    #[serde(default)]
    pub siblings: Vec<Slot>,
    /// Siblings that joined after this device and have not written to it yet: no sync goes while one is (a device
    /// the hub lists again after it was replaced gets nothing from here).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub siblings_held: Vec<Key>,
    /// When the own roster was last asked for, and whether it ever was: the first answer is the account as it was
    /// when this device joined, its devices trusted.
    #[serde(default)]
    pub own_roster_at: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub own_roster_known: bool,
    /// The newest message received per direct or group thread (sent at, ms): what a read names (spec 3.2).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub last_in: BTreeMap<String, u64>,
    /// Read through (sent at, ms) per thread, here or on a sibling.
    #[serde(default)]
    pub seen_through: BTreeMap<String, u64>,
    /// Threads a sibling showed, not yet handed to the app (`take_seen`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub seen_due: BTreeMap<String, u64>,
    pub outbox: VecDeque<Outgoing>,
    pub next_seq: u64,
    /// Events not yet handed to the app: saved, so a crash after the hub's ack loses nothing.
    pub events: Vec<crate::Event>,
    /// Handed out and not yet acknowledged: they come again after a crash.
    pub handed: Vec<crate::Event>,
    /// Hashes of session messages processed, against redelivery.
    #[serde(with = "compact::hashes")]
    pub seen: VecDeque<[u8; 32]>,
    pub prekeys: Prekeys,
    /// The time of the last signed request: the next is later, whichever process signs it (spec 4.1).
    pub signed_until: u64,
    /// The account's storage (spec 4.3), merged with the hub's.
    pub storage: crate::storage::Storage,
    pub storage_version: u64,
    pub storage_dirty: bool,
    /// When the hub last held all of it (wall clock, ms; 0: not yet).
    pub storage_saved_at: u64,
    /// Fetched once, before the first message is received.
    pub storage_fetched: bool,
    /// The storage key from a migrated Ed25519 seed (spec 4.3): opens the old storage until it is written anew.
    pub legacy_storage_key: Option<[u8; 32]>,
    /// The hub's storage did not open here: kept as it is at the hub, never written over.
    #[serde(default)]
    pub storage_locked: bool,
    /// Each message sent is strictly later than this.
    pub last_sent_ms: u64,
    /// Hashes of the last messages received (sender and plaintext): a copy sent again (4.6) is skipped.
    #[serde(with = "compact::hashes")]
    pub received: VecDeque<[u8; 32]>,
    /// This account's profile key (spec 3.3); none before the first sync.
    pub profile_key: Option<[u8; 32]>,
    /// The key's generation (1 the first), so a contact knows which of two is later (spec 3.3).
    pub profile_key_gen: u32,
    pub requests_open: bool,
    /// The hub has the current access key check and `requests_open`.
    pub access_key_told: bool,
    /// The sender certificate and when it came by this clock (spec 4.1).
    pub sender_certificate: Option<(Vec<u8>, u64)>,
    /// The latest this clock showed: a clock set back fetches a new certificate instead of stretching the day.
    pub certificate_clock: u64,
    /// The registration until the hub answered it (the user id is empty until then).
    pub registering: Option<Registering>,
    #[serde(default, rename = "hub_groups")]
    pub groups: BTreeMap<String, Group>,
    /// When a payload last made this device look a user up, and the day's recovery lookups (day, count): saved, so
    /// a restart or an extension does not reset the limits (spec 3.2).
    pub roster_asked: BTreeMap<String, u64>,
    pub recovery_lookups: (u64, u32),
    /// Group Yos waiting for their group to be fetched with their sender in it (at most `groups::MAX_HELD`).
    pub group_held: Vec<(String, crate::Event)>,
    /// Payloads from an unknown device that could not be looked up now: received again later (at most `DEFERRED`).
    #[serde(with = "compact::list")]
    pub deferred: VecDeque<Vec<u8>>,
    /// Yos received (by the app or the notification service) since the app last showed them: the badge.
    #[serde(default)]
    pub unread: u32,
    /// Users (hex) that declare themselves bots, as the hub said when they were last looked up.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub bots: BTreeSet<String>,
    /// This account declared itself one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bot: bool,
}

impl Drop for State {
    fn drop(&mut self) {
        self.account_secret.zeroize();
        self.auth_secret.zeroize();
        self.legacy_storage_key.zeroize();
    }
}

#[derive(Debug, TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Entry {
    key: VLBytes,
    value: VLBytes,
}

/// An old install's sealed state blob, read only for its account secret (`legacy_identity`).
#[derive(Debug, TlsSerialize, TlsDeserializeBytes, TlsSize)]
struct Plain {
    version: u8,
    state: VLBytes,
    records: Vec<Entry>,
}

fn open_plain(blob: &[u8], key: &[u8; 32]) -> Result<Plain> {
    let mut bytes = crate::aead::open(key, AAD, blob).ok_or(Error::State)?;
    let decoded: std::result::Result<Plain, _> = yoip::decode(&bytes);
    bytes.zeroize();
    decoded.map_err(|_| Error::State)
}

/// An old install's sealed state blob opened: its version, its JSON and libsignal's records (`legacy`).
pub(crate) fn open_legacy(blob: &[u8], key: &[u8; 32]) -> Result<(u8, serde_json::Value, crate::signal::Entries)> {
    let plain = open_plain(blob, key)?;
    if !matches!(plain.version, 1 | STATE_VERSION) {
        return Err(Error::State);
    }
    let json = zeroize::Zeroizing::new(Vec::from(plain.state));
    let value = serde_json::from_slice(&json).map_err(|_| Error::State)?;
    let records = (plain.records.into_iter())
        .map(|e| Ok((String::from_utf8(e.key.into()).map_err(|_| Error::State)?, e.value.into())))
        .collect::<Result<_>>()?;
    Ok((plain.version, value, records))
}

pub(crate) fn legacy_identity(blob: &[u8], key: &[u8; 32]) -> Result<crate::migrations::keychain_v1::Identity> {
    #[derive(Deserialize)]
    struct Secret {
        account_secret: [u8; 32],
        #[serde(default)]
        legacy_storage_key: Option<[u8; 32]>,
    }
    let (version, json, _) = open_legacy(blob, key)?;
    let secret = Secret::deserialize(&json).map_err(|_| Error::State)?;
    let ed25519 = version == 1;
    // A state of the seed: the storage is sealed under the seed's key; a converted one kept that key.
    let storage_key =
        if ed25519 { Some(crate::storage::key(&secret.account_secret)) } else { secret.legacy_storage_key };
    Ok(crate::migrations::keychain_v1::Identity { secret: secret.account_secret, ed25519, storage_key })
}

/// A state blob as the build before the database sealed it (`migrations::keychain_v1::read_state`).
#[cfg(any(test, feature = "test-support"))]
pub fn seal_legacy(json: &[u8], records: &crate::signal::Entries, key: &[u8; 32]) -> Vec<u8> {
    let records = records.iter().map(|(k, v)| Entry { key: k.as_bytes().into(), value: v.as_slice().into() });
    let plain = Plain { version: STATE_VERSION, state: json.into(), records: records.collect() };
    crate::aead::seal(key, AAD, &yoip::encode(&plain))
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
        assert_eq!(serde_json::from_str::<super::SentCache>("[\"AQID\"]").unwrap(), snapshot);
    }
}
