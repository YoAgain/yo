//! The iCloud Keychain's account item and the one-time sign-in of an old install (Migrations/KeychainV1.swift).

use yo_core::keychain::Item;
use yo_core::migrations::keychain_v1::{self as legacy, Found, Shown, Source};

#[uniffi::remote(Record)]
#[uniffi(name = "SyncedItem")]
pub struct Item {
    pub username: String,
    pub user_id: Vec<u8>,
    pub account_key: Vec<u8>,
    pub by_passkey: bool,
    pub by_code: Option<bool>,
    pub code_auth: Option<bool>,
    pub passkey_auth: Option<bool>,
    pub recovery_key: Option<String>,
    pub signed_out: Option<bool>,
    pub version: Option<i64>,
    pub legacy_seed: Option<Vec<u8>>,
}

#[uniffi::export]
pub fn synced_item_version() -> i64 {
    yo_core::keychain::ITEM_VERSION
}

#[uniffi::export]
pub fn synced_item_decode(json: Vec<u8>) -> Option<Item> {
    yo_core::keychain::decode(&json)
}

#[uniffi::export]
pub fn synced_item_encode(item: Item) -> Vec<u8> {
    yo_core::keychain::encode(&item)
}

/// The item in today's format (a v1 item converted), none when it is no item of Yo's.
#[uniffi::export]
pub fn synced_item_upgraded(item: Item) -> Option<Item> {
    yo_core::keychain::upgraded(&item)
}

/// An old install's state file and the state keys read for its place (`keys_unavailable`: the keychain did not answer).
#[derive(uniffi::Record)]
pub struct LegacySource {
    pub path: String,
    pub format: u8,
    pub keys: Vec<Vec<u8>>,
    pub keys_unavailable: bool,
}

/// An old install's own key material (Android's one-time sign-in): the account secret (`ed25519`: a seed to convert)
/// and the key the hub's storage is still sealed under.
#[derive(uniffi::Record)]
pub struct LegacyIdentity {
    pub secret: Vec<u8>,
    pub ed25519: bool,
    pub storage_key: Option<Vec<u8>>,
}

fn sources_of(sources: Vec<LegacySource>) -> Vec<Source> {
    sources
        .into_iter()
        .map(|source| Source {
            path: source.path.into(),
            format: source.format,
            keys: source.keys.into_iter().filter_map(|key| key.try_into().ok()).collect(),
            keys_unavailable: source.keys_unavailable,
        })
        .collect()
}

/// The identity of the first old state file that opens; none when there is none, `Network` when one is not readable
/// now (asked again later, nothing is deleted meanwhile).
#[uniffi::export]
pub fn legacy_identity(sources: Vec<LegacySource>) -> Result<Option<LegacyIdentity>, crate::YoError> {
    let local = legacy::local_identity(None, &sources_of(sources));
    match local.identity {
        Some(found) => Ok(Some(LegacyIdentity {
            secret: found.secret.to_vec(),
            ed25519: found.ed25519,
            storage_key: found.storage_key.map(Vec::from),
        })),
        None if local.not_now => Err(crate::YoError::Network { message: "old state is unavailable now".into() }),
        None => Ok(None),
    }
}

#[derive(uniffi::Record)]
pub struct LegacyShown {
    pub username: String,
    pub user_id: Vec<u8>,
    pub by_passkey: bool,
    pub by_code: Option<bool>,
}

#[derive(uniffi::Enum)]
pub enum LegacyFound {
    SignIn { item: Item, storage_key: Option<Vec<u8>> },
    SignedOut,
    Missing,
    NotNow,
}

/// An old install not signed in again yet (`saved`: the app's preferences of the account).
#[uniffi::export]
pub fn legacy_pending(has_account: bool, saved: Option<Vec<u8>>, has_old_state: bool, signed_in: Option<bool>) -> bool {
    legacy::pending(has_account, saved.as_deref(), has_old_state, signed_in)
}

/// The one-time sign-in's decision: `stored` the iCloud Keychain's item (upgraded), else the old local state.
#[uniffi::export]
pub fn legacy_sign_in(
    stored: Option<Item>,
    saved: Option<Vec<u8>>,
    sources: Vec<LegacySource>,
    shown: LegacyShown,
    recovery_key: Option<String>,
    left: Option<Vec<u8>>,
) -> LegacyFound {
    let local = legacy::local_identity(saved.as_deref(), &sources_of(sources));
    let shown = Shown {
        username: shown.username,
        user_id: shown.user_id,
        by_passkey: shown.by_passkey,
        by_code: shown.by_code,
    };
    match legacy::sign_in(stored, local, &shown, recovery_key, left.as_deref()) {
        Found::SignIn { item, storage_key } => LegacyFound::SignIn { item, storage_key: storage_key.map(Vec::from) },
        Found::SignedOut => LegacyFound::SignedOut,
        Found::Missing => LegacyFound::Missing,
        Found::NotNow => LegacyFound::NotNow,
    }
}
