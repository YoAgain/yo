//! The backup record as the app makes and opens it (spec 7; `BackupRecord` in app/ios Account.swift), for the tests:
//! the crypto is the core's (`yo_core::backup`), the sealed part the app's JSON (`Data` as base64).

use base64::Engine as _;
use sha2::Digest as _;
use yo_core::backup::{self as core, Door, PasskeyDoor};
use yo_core::Backup;

pub use yo_core::backup::locator;

const WORDS: &str = include_str!("../../app/ios/YoApp/Resources/bip39-english.txt");

/// The 12 words of 16 bytes of entropy (BIP-39: 128 bits and a 4-bit checksum).
pub fn words(entropy: [u8; 16]) -> Vec<String> {
    let list: Vec<&str> = WORDS.lines().collect();
    let check = sha2::Sha256::digest(entropy)[0] >> 4;
    let mut bits: Vec<u8> = entropy.iter().flat_map(|b| (0..8).map(move |i| (b >> (7 - i)) & 1)).collect();
    bits.extend((0..4).map(|i| (check >> (3 - i)) & 1));
    (0..12)
        .map(|i| list[bits[i * 11..i * 11 + 11].iter().fold(0usize, |n, b| n << 1 | *b as usize)].to_owned())
        .collect()
}

/// The salt every Yo passkey is asked with.
pub fn prf_salt() -> [u8; 32] {
    sha2::Sha256::digest(b"yo backup v1").into()
}

/// What K seals: the account and the words.
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

/// The app's JSON of the account and the words.
pub fn sealed_json(account: &Account, code: &[String]) -> Vec<u8> {
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

/// A record for the hub: K under the words always, under the passkey's PRF output too when given.
pub fn make(account: &Account, code: &[String], k: &[u8; 32], prf: Option<&[u8]>) -> Backup {
    let passkey = prf.map_or(PasskeyDoor::None, PasskeyDoor::New);
    core::make(k, &sealed_json(account, code), code, passkey).unwrap()
}

/// The account and the words in the app's JSON.
pub fn parse_sealed(plain: &[u8]) -> Option<(Account, Vec<String>)> {
    let json: serde_json::Value = serde_json::from_slice(plain).ok()?;
    let bytes = |v: &serde_json::Value| base64::engine::general_purpose::STANDARD.decode(v.as_str()?).ok();
    let account = Account {
        username: json["account"]["username"].as_str()?.to_owned(),
        user_id: bytes(&json["account"]["userID"])?,
        account_key: bytes(&json["account"]["accountKey"])?.try_into().ok()?,
        by_passkey: json["account"]["byPasskey"].as_bool()?,
    };
    let code = json["code"].as_array()?.iter().filter_map(|w| Some(w.as_str()?.to_owned())).collect();
    Some((account, code))
}

/// The account and the words a record seals, by a door: `None` when that door does not open it.
pub fn open_sealed(backup: &Backup, door: &Door) -> Option<(Account, Vec<String>)> {
    parse_sealed(&core::open(backup.clone(), door)?.plain)
}
