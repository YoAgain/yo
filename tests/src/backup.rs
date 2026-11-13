//! The backup record as the app makes and opens it (spec 7; `BackupRecord` in app/ios Account.swift), in Rust for the
//! tests: K wrapped by the passkey's PRF output and by the 12 words, the account sealed under K, the words' locator.
//! The formats are the app's: HKDF-SHA256 without salt, ChaCha20-Poly1305 as nonce || ciphertext || tag (CryptoKit's
//! `combined`), the sealed part the app's JSON (`Data` as base64).

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::ChaCha20Poly1305;
use sha2::Digest as _;
use yo_core::storage::hkdf;
use yo_core::Backup;

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

fn derive<const N: usize>(ikm: &[u8], info: &str) -> [u8; N] {
    let mut out = [0; N];
    hkdf(ikm, info.as_bytes(), &mut out);
    out
}

/// The salt every Yo passkey is asked with.
pub fn prf_salt() -> [u8; 32] {
    sha2::Sha256::digest(b"yo backup v1").into()
}

pub fn key_from_prf(prf: &[u8]) -> [u8; 32] {
    derive(prf, "yo backup v1 passkey")
}

pub fn key_from_code(code: &[String]) -> [u8; 32] {
    derive(code.join(" ").as_bytes(), "yo backup v1 code")
}

pub fn locator(code: &[String]) -> [u8; 16] {
    derive(code.join(" ").as_bytes(), "yo backup v1 locator")
}

fn seal(key: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    getrandom::getrandom(&mut nonce).unwrap();
    [nonce.to_vec(), ChaCha20Poly1305::new(key.into()).encrypt(&nonce.into(), plain).unwrap()].concat()
}

fn open(key: &[u8; 32], sealed: &[u8]) -> Option<Vec<u8>> {
    let (nonce, rest) = sealed.split_at_checked(12)?;
    ChaCha20Poly1305::new(key.into()).decrypt(nonce.into(), rest).ok()
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

/// A record for the hub: K under the words always, under the passkey's PRF output too when given.
pub fn make(account: &Account, code: &[String], k: &[u8; 32], prf: Option<&[u8]>) -> Backup {
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
    Backup::new(
        prf.map(|prf| seal(&key_from_prf(prf), k)).unwrap_or_default(),
        seal(&key_from_code(code), k),
        seal(k, &serde_json::to_vec(&sealed).unwrap()),
        locator(code).to_vec(),
    )
}

/// K, by the passkey's PRF output or by the words: `None` when that door does not open it.
pub fn unwrap(wrapped: &[u8], key: &[u8; 32]) -> Option<[u8; 32]> {
    open(key, wrapped)?.try_into().ok()
}

/// The account and the words a record seals under K.
pub fn open_sealed(backup: &Backup, k: &[u8; 32]) -> Option<(Account, Vec<String>)> {
    let json: serde_json::Value = serde_json::from_slice(&open(k, backup.sealed.as_slice())?).ok()?;
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
