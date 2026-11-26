#[cfg(test)]
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
#[cfg(test)]
use chacha20poly1305::ChaCha20Poly1305;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// HKDF-SHA256 without salt (RFC 5869), up to 32 bytes: the derivation spec 2.4 and 4.3 name.
pub fn hkdf(ikm: &[u8], info: &[u8], out: &mut [u8]) {
    hkdf::Hkdf::<sha2::Sha256>::new(None, ikm).expand(info, out).expect("valid HKDF output length");
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
    pub fn unset(&self) -> bool {
        self.at == 0
    }
}

impl<T: Clone + Serialize> Field<T> {
    pub fn set(&mut self, value: T, at: u64) {
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

/// Fields of a later version, merged without knowing them (spec 4.3): a `[value, at]` field by its time, then its
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
    /// Joined (Signal's whitelisted): else a request (spec 3.3).
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

/// A service's connection (`Client::connections`): the codes (base64url) are of what it has not answered yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    // n name, c connected, w waiting link code, x refused, e ending code (Connect, content type 14).
    #[serde(rename = "n")]
    pub name: String,
    #[serde(default, rename = "c")]
    pub connected: bool,
    #[serde(default, rename = "w", skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
    #[serde(default, rename = "x")]
    pub refused: bool,
    #[serde(default, rename = "e", skip_serializing_if = "Option::is_none")]
    pub ending: Option<String>,
}

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
    /// "Requests from Strangers" (spec 3.3): one choice for the account, none until a device sets it.
    #[serde(default, rename = "ro", skip_serializing_if = "Field::unset")]
    pub requests_open: Field<Option<bool>>,
    /// This account's profile key (base64url), so a restored phone keeps its contacts' deliveries good (spec 3.3).
    #[serde(default, rename = "pk", skip_serializing_if = "Field::unset")]
    pub profile_key: Field<Option<String>>,
    /// Its generation (spec 3.3), set with it.
    #[serde(default, rename = "pg", skip_serializing_if = "Field::unset")]
    pub profile_key_gen: Field<u32>,
    /// Contacts' profile keys (base64url) by user id (hex), none once forgotten. The first to go when it would not fit.
    #[serde(default, rename = "k", skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, Field<Option<String>>>,
    /// The groups this account is or was in, by group id (hex), as Signal's storage keeps them.
    #[serde(default, rename = "g", skip_serializing_if = "BTreeMap::is_empty")]
    pub groups: BTreeMap<String, GroupEntry>,
    /// Services connected with another one, by user id (hex).
    #[serde(default, rename = "s", skip_serializing_if = "BTreeMap::is_empty")]
    pub connections: BTreeMap<String, Field<Connection>>,
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
        self.requests_open.merge(&other.requests_open);
        self.profile_key.merge(&other.profile_key);
        self.profile_key_gen.merge(&other.profile_key_gen);
        for (user, key) in &other.keys {
            self.keys.entry(user.clone()).or_default().merge(key);
        }
        for (id, group) in &other.groups {
            self.groups.entry(id.clone()).or_default().merge(group);
        }
        for (user, connection) in &other.connections {
            self.connections.entry(user.clone()).or_default().merge(connection);
        }
        merge_extra(&mut self.extra, &other.extra);
        self.version = self.version.max(other.version);
    }
}

/// A nonce and a tag: what sealing adds.
const SEAL_OVERHEAD: usize = 12 + 16;

/// Pads `plain` like an envelope (spec 4.1) and seals it (nonce || ChaCha20-Poly1305 || tag) to a size class of
/// `SEALED_SIZES`: the storage and a backup's sealed part (spec 4.3, 2.4). `Error::Size` past the largest.
pub fn seal_padded(
    key: &[u8; 32],
    aad: &[u8],
    plain: &[u8],
    nonce: impl FnOnce() -> [u8; 12],
) -> Result<Vec<u8>, crate::Error> {
    let sizes = crate::net::limits::SEALED_SIZES.map(|size| size - SEAL_OVERHEAD);
    let padded = crate::protocol::envelope::pad(plain, &sizes)?;
    Ok(crate::protocol::aead::seal(key, aad, &padded, nonce()))
}

/// Opens what `seal_padded` sealed, and unpadded JSON sealed before the padding (it starts with `{`, not 1).
pub fn open_padded(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let plain = crate::protocol::aead::open(key, aad, sealed)?;
    if plain.first() != Some(&crate::VERSION) {
        return Some(plain);
    }
    crate::protocol::envelope::unpad(&plain, &[plain.len()]).ok().map(<[u8]>::to_vec)
}

/// Seals the storage for a user (associated data: "yo storage v1" || user id), padded (`seal_padded`).
pub fn seal(
    account_secret: &[u8; 32],
    user_id: &[u8],
    storage: &Storage,
    nonce: impl FnOnce() -> [u8; 12],
) -> Result<Vec<u8>, crate::Error> {
    let plain = serde_json::to_vec(storage).expect("storage encodes");
    seal_padded(&key(account_secret), &[&b"yo storage v1"[..], user_id].concat(), &plain, nonce)
}

pub fn open(account_secret: &[u8; 32], user_id: &[u8], sealed: &[u8]) -> Result<Storage, crate::Error> {
    open_under(&key(account_secret), user_id, sealed)
}

/// Opens the storage sealed under `key` (`open` derives it from the account's secret).
pub fn open_under(key: &[u8; 32], user_id: &[u8], sealed: &[u8]) -> Result<Storage, crate::Error> {
    let plain = open_padded(key, &[&b"yo storage v1"[..], user_id].concat(), sealed).ok_or(crate::Error::Format)?;
    serde_json::from_slice(&plain).map_err(|_| crate::Error::Format)
}

pub fn decode_key(text: &str) -> Option<[u8; 32]> {
    crate::unb64(text).ok()?.try_into().ok()
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
        a.requests_open = Field { value: Some(false), at: 12 };
        a.icon = Field { value: Some("Negative".into()), at: 9 };
        let mut b = Storage::default();
        b.friends.insert("aa".into(), Friend { blocked: Field { value: true, at: 7 }, ..friend("Leonard", 3) });
        b.friends.insert("bb".into(), friend("Mia", 1));
        b.left_handed = Field { value: Some(false), at: 9 };
        b.requests_open = Field { value: Some(true), at: 3 };
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
        // "Requests from Strangers": the later choice of any device of the account.
        assert_eq!(ab.requests_open.value, Some(false));
        assert_eq!(ab.icon.value.as_deref(), Some("Negative"));
    }

    #[test]
    fn left_handed_round_trips_and_old_storage_opens() {
        let key = [1; 32];
        let old = seal_padded(
            &self::key(&key),
            b"yo storage v1user",
            br#"{"version":3,"icon":["Negative",9]}"#,
            rand::random,
        )
        .unwrap();
        let old = open(&key, b"user", &old).unwrap();
        assert_eq!(old.left_handed.value, None);
        assert_eq!(old.icon.value.as_deref(), Some("Negative"));
        for value in [None, Some(false), Some(true)] {
            let storage = Storage { left_handed: Field { value, at: 10 }, ..Storage::default() };
            let sealed = seal(&key, b"user", &storage, rand::random).unwrap();
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
        let sealed = seal(&[1; 32], b"user", &s, rand::random).unwrap();
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
            let sealed = seal_padded(&key, b"ad", &vec![b'x'; len], rand::random).unwrap();
            assert_eq!(sealed.len(), size, "{len} bytes");
            assert_eq!(open_padded(&key, b"ad", &sealed).unwrap(), vec![b'x'; len]);
        }
        assert!(seal_padded(&key, b"ad", &[b'x'; 61408], rand::random).is_err(), "past the largest");
        assert!(open_padded(&key, b"other", &seal_padded(&key, b"ad", b"x", rand::random).unwrap()).is_none());
        // Sealed before the padding: JSON with spaces after it.
        let nonce = [7; 12];
        let old = ChaCha20Poly1305::new(&key.into())
            .encrypt(&nonce.into(), Payload { msg: b"{\"version\":3}   ", aad: b"ad" })
            .unwrap();
        let old = [nonce.to_vec(), old].concat();
        assert_eq!(open_padded(&key, b"ad", &old).unwrap(), b"{\"version\":3}   ");
    }
}
