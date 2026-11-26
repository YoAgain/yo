//! The backup record as the app makes and opens it (spec 2.4; `BackupRecord` in app/ios Account.swift), for the tests:
//! the crypto is the core's (`yo_core::backup`), the sealed part the app's JSON (`Data` as base64).

use base64::Engine as _;
use sha2::Digest as _;
use yo_core::backup::{self as core, AccountEntropyPool, Door, PasskeyDoor};
use yo_core::Backup;

/// A recovery key of 16 bytes of seed, the same each time: 64 characters a–z and 0–9 (libsignal's account entropy
/// pool, spec 2.4).
pub fn recovery_key(seed: [u8; 16]) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    (0..64u8)
        .map(|i| {
            let byte = sha2::Sha256::digest([&seed[..], &[i]].concat())[0];
            ALPHABET[byte as usize % ALPHABET.len()] as char
        })
        .collect()
}

/// The recovery key as the core takes it.
pub fn pool(key: &str) -> AccountEntropyPool {
    core::parse_recovery_key(key).expect("a recovery key")
}

/// Where the recovery key alone finds its record.
pub fn locator(key: &str) -> [u8; yoip::net::api::LOCATOR_LEN] {
    core::locator(&pool(key))
}

/// The salt every Yo passkey is asked with.
pub fn prf_salt() -> [u8; 32] {
    sha2::Sha256::digest(b"yo backup v1").into()
}

/// What K seals: the account and the recovery key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub username: String,
    pub user_id: Vec<u8>,
    pub account_key: [u8; 32],
    pub by_passkey: bool,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The app's JSON of the account and the recovery key.
pub fn sealed_json(account: &Account, code: &str) -> Vec<u8> {
    let sealed = serde_json::json!({
        "account": {
            "username": account.username,
            "userID": b64(&account.user_id),
            "accountKey": b64(&account.account_key),
            "byPasskey": account.by_passkey,
            "byCode": true,
        },
        "code": code,
    });
    serde_json::to_vec(&sealed).unwrap()
}

/// A record for the hub: K under the recovery key always, under the passkey's PRF output too when given.
pub fn make(account: &Account, code: &str, k: &[u8; 32], prf: Option<&[u8]>) -> Backup {
    let passkey = prf.map_or(PasskeyDoor::None, PasskeyDoor::New);
    core::make(k, &sealed_json(account, code), &pool(code), passkey).unwrap()
}

/// The account and the recovery key in the app's JSON.
pub fn parse_sealed(plain: &[u8]) -> Option<(Account, String)> {
    let json: serde_json::Value = serde_json::from_slice(plain).ok()?;
    let bytes = |v: &serde_json::Value| base64::engine::general_purpose::STANDARD.decode(v.as_str()?).ok();
    let account = Account {
        username: json["account"]["username"].as_str()?.to_owned(),
        user_id: bytes(&json["account"]["userID"])?,
        account_key: bytes(&json["account"]["accountKey"])?.try_into().ok()?,
        by_passkey: json["account"]["byPasskey"].as_bool()?,
    };
    let code = json["code"].as_str()?.to_owned();
    Some((account, code))
}

/// The account and the recovery key a record seals, by a door: `None` when that door does not open it.
pub fn open_sealed(backup: &Backup, door: &Door) -> Option<(Account, String)> {
    parse_sealed(&core::open(backup.clone(), door)?.plain)
}
