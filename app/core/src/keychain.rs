//! The account item in the iCloud Keychain (spec 2.4): its JSON, as the iOS app has always written it.
//! One item per account and hub (name, user id, account key, recovery key); Android keeps the same item in Google
//! Block Store under the same rules. A phone without an account registers with it at once (new device, new prekeys),
//! only when exactly one item exists that is neither marked signed out nor the account this phone left. Signing out
//! marks the item; account deletion deletes it. Key material leaves the phone only into a cloud end-to-end encrypted
//! for the person (iCloud Keychain; Block Store only while `isEndToEndEncryptionAvailable()`, checked at each write),
//! never Android Auto Backup or adb backup (`allowBackup=false`, data extraction rules exclude all), iCloud Drive or
//! plain Google Drive; without such a cloud the backup stays off.

use serde::{Deserialize, Serialize};

/// The item's format today: the libsignal account key and the recovery key.
pub const ITEM_VERSION: i64 = 3;

/// Its `Debug` leaves out the secrets (the account key, the recovery key, an old seed).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub username: String,
    #[serde(rename = "userID", with = "b64")]
    pub user_id: Vec<u8>,
    /// The account key's secret (v1: an Ed25519 seed).
    #[serde(with = "b64")]
    pub account_key: Vec<u8>,
    pub by_passkey: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_code: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_auth: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passkey_auth: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_out: Option<bool>,
    /// None: v1, the Ed25519 seed and the 12 words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    /// The seed a v1 item had: the hub's storage may still be sealed under its key (spec 4.3).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "b64_option")]
    pub legacy_seed: Option<Vec<u8>>,
}

impl std::fmt::Debug for Item {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Item")
            .field("username", &self.username)
            .field("by_passkey", &self.by_passkey)
            .field("by_code", &self.by_code)
            .field("signed_out", &self.signed_out)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

pub fn decode(json: &[u8]) -> Option<Item> {
    serde_json::from_slice(json).ok()
}

pub fn encode(item: &Item) -> Vec<u8> {
    serde_json::to_vec(item).expect("an item encodes")
}

/// The item in today's format: v3 as it is, v1 converted (remove with `legacy`), anything else none.
pub fn upgraded(item: &Item) -> Option<Item> {
    let seed: [u8; 32] = item.account_key.as_slice().try_into().ok()?;
    match item.version {
        Some(ITEM_VERSION) => Some(item.clone()),
        None => Some(Item {
            account_key: crate::identity::legacy_account_secret(&seed).to_vec(),
            by_passkey: false,
            by_code: Some(false),
            code_auth: None,
            passkey_auth: None,
            recovery_key: None,
            version: Some(ITEM_VERSION),
            legacy_seed: Some(seed.to_vec()),
            ..item.clone()
        }),
        Some(_) => None,
    }
}

mod b64 {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        STANDARD.decode(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

mod b64_option {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => super::b64::serialize(bytes, s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        Option::<String>::deserialize(d)?
            .map(|text| STANDARD.decode(text).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_v3_item_round_trips_and_a_v1_item_converts_keeping_its_seed() {
        let v3 = decode(include_bytes!("../../../tests/fixtures/keychain-v1/item-v3.json")).unwrap();
        assert_eq!(upgraded(&v3), Some(v3.clone()));
        assert_eq!(decode(&encode(&v3)), Some(v3.clone()));
        let v1 = decode(include_bytes!("../../../tests/fixtures/keychain-v1/item-v1.json")).unwrap();
        let up = upgraded(&v1).unwrap();
        assert_eq!(up.version, Some(ITEM_VERSION));
        assert_eq!(up.legacy_seed.as_deref(), Some(v1.account_key.as_slice()));
        assert_eq!(up.account_key, v3.account_key, "the same account key as the converted item");
        assert_eq!((up.signed_out, up.recovery_key, up.by_code), (Some(false), None, Some(false)));
    }

    #[test]
    fn short_keys_and_unknown_versions_are_no_item() {
        let mut item = decode(include_bytes!("../../../tests/fixtures/keychain-v1/item-v3.json")).unwrap();
        item.version = Some(4);
        assert_eq!(upgraded(&item), None);
        item.version = None;
        item.account_key.truncate(31);
        assert_eq!(upgraded(&item), None);
        assert_eq!(decode(b"{}"), None);
    }

    #[test]
    fn items_without_auth_flags_decode_and_both_flags_survive() {
        let old =
            br#"{"username":"TEST","userID":"AQ==","accountKey":"Ag==","byPasskey":true,"byCode":true,"version":3}"#;
        let mut item = decode(old).unwrap();
        assert_eq!((item.code_auth, item.passkey_auth), (None, None));
        for passkey in [true, false] {
            item.code_auth = Some(true);
            item.passkey_auth = Some(passkey);
            assert_eq!(decode(&encode(&item)), Some(item.clone()));
        }
    }
}
