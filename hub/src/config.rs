//! The hub's configuration: everything main.rs reads from the environment, and what tests set directly.

use crate::db::{BlobQuota, Reserved};
use std::path::PathBuf;

/// Rate limits (spec 6.6). Each is a token bucket: this many per window, refilled evenly.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Lookups that hand out prekeys: starting sessions.
    pub lookup_per_device_day: u32,
    /// Lookups of an account key only: checking who started a chat.
    pub identity_lookup_per_device_day: u32,
    /// Signed requests of one device, all routes.
    pub signed_per_device_minute: u32,
    /// Deliveries per address: they are not signed (spec 6.6).
    pub deliver_per_ip_minute: u32,
    /// Envelopes a device holds before deliveries to its user are refused; no rate a stranger could spend.
    pub max_pending_per_device: u32,
    /// Envelope bytes sealed deliveries bring one recipient, from everyone, as Signal's inbound message bytes: a
    /// bucket of `recipient_bytes_burst`, `recipient_bytes_per_hour` back an hour (spec 6.6); beyond, `429` with
    /// `Retry-After` for that recipient only. It bounds the inflow, never what waits nor fetching: a phone off for a
    /// week still gets everything. Blobs count by their own upload limits.
    pub recipient_bytes_burst: u64,
    pub recipient_bytes_per_hour: u64,
    /// One push per device at most every this many ms (spec 6.7): what comes meanwhile goes in one push at the end
    /// of the window; a quiet device's first push goes at once. A contact's delivery (sealed) and a request's (signed,
    /// 4.10) each have their own window.
    pub push_window_contact_ms: u64,
    pub push_window_request_ms: u64,
    /// A device with more than this many envelopes waiting (unfetched) gets at most one push per
    /// `push_backlog_interval_s` until its backlog is down again (spec 6.7): it is offline or flooded.
    pub push_backlog: u32,
    pub push_backlog_interval_s: u64,
    /// Bytes stored without identity, by address: envelopes stay up to `envelope_ttl_days`.
    pub anonymous_bytes_per_ip_hour: u64,
    /// Deliveries are refused (`507`) while the database's disk has less than this free.
    pub db_min_free_bytes: u64,
    /// Registrations by username: first come, first served, this many per address (spec 6.4).
    pub register_per_ip_hour: u32,
    /// Whether a username is free, asked while it is typed (spec 6.3).
    pub name_check_per_ip_minute: u32,
    /// Requests one sender sends one recipient at once, one more an hour (spec 4.10).
    pub requests_per_pair: u64,
    /// Recipients new to one sender's requests at once, one more an hour (spec 4.10).
    pub request_recipients_per_sender: u64,
    /// Requests one recipient takes from everyone at once, as many again a day (spec 4.10).
    pub requests_per_recipient: u64,
    /// The hour those refill by, ms: an hour; a load test that runs days of use in minutes makes it shorter.
    pub request_hour_ms: u64,
    /// Failed backup fetches by address: unknown records or invalid door signatures (spec 7).
    pub backup_per_ip_hour: u32,
    /// Reports of users, by one install id (spec 6.4).
    pub report_per_install_day: u32,
    /// Messages to the operator, by one install id (spec 6.4).
    pub support_per_install_day: u32,
    /// Messages and reports from one address a day, whatever install ids it names (spec 6.4).
    pub help_per_ip_day: u32,
    /// Crash and error reports by one install id, and from one address, an hour (spec 6.4).
    pub diagnostics_per_install_hour: u32,
    pub diagnostics_per_ip_hour: u32,
    /// The operator's to-dos from one address a day.
    pub todo_per_ip_day: u32,
    pub storage_per_user_hour: u32,
    pub envelope_ttl_days: u64,
    /// A blob not fetched by all its devices is deleted after this long (spec 6.8).
    pub blob_ttl_days: u64,
    /// Photos and videos one user may upload per day (UTC), and their bytes.
    pub blobs_per_user_day: u32,
    pub blob_bytes_per_user_day: u64,
    /// Bytes of one user's blobs waiting for their fetches, at any time.
    pub blob_bytes_stored_per_user: u64,
    pub blob_put_per_ip_minute: u32,
    /// Fetches, which name no one: by address only.
    pub blob_get_per_ip_minute: u32,
}

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

impl Limits {
    pub fn blob_quota(&self) -> BlobQuota {
        BlobQuota {
            per_day: self.blobs_per_user_day,
            bytes_per_day: self.blob_bytes_per_user_day,
            stored: self.blob_bytes_stored_per_user,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            lookup_per_device_day: 50,
            identity_lookup_per_device_day: 1000,
            signed_per_device_minute: 1200,
            deliver_per_ip_minute: 1000,
            max_pending_per_device: 5000,
            recipient_bytes_burst: 50 * MIB,
            recipient_bytes_per_hour: 50 * MIB,
            push_window_contact_ms: 1000,
            push_window_request_ms: 10_000,
            push_backlog: 50,
            push_backlog_interval_s: 600,
            anonymous_bytes_per_ip_hour: 64 * MIB,
            db_min_free_bytes: GIB,
            register_per_ip_hour: 10,
            name_check_per_ip_minute: 60,
            requests_per_pair: 10,
            request_recipients_per_sender: 20,
            requests_per_recipient: 1000,
            request_hour_ms: 3_600_000,
            backup_per_ip_hour: 30,
            report_per_install_day: 20,
            support_per_install_day: 10,
            help_per_ip_day: 200,
            diagnostics_per_install_hour: 300,
            diagnostics_per_ip_hour: 3000,
            todo_per_ip_day: 5,
            storage_per_user_hour: 120,
            envelope_ttl_days: 30,
            blob_ttl_days: 30,
            blobs_per_user_day: 2000,
            blob_bytes_per_user_day: 2 * GIB,
            blob_bytes_stored_per_user: GIB,
            blob_put_per_ip_minute: 120,
            blob_get_per_ip_minute: 600,
        }
    }
}

impl Limits {
    /// No rate limit worth the name: for load tests from one address.
    pub fn unlimited() -> Self {
        Limits {
            lookup_per_device_day: u32::MAX,
            identity_lookup_per_device_day: u32::MAX,
            signed_per_device_minute: u32::MAX,
            deliver_per_ip_minute: u32::MAX,
            recipient_bytes_burst: u64::MAX,
            // Every push at once: tests count them one by one (pacing has its own tests).
            push_window_contact_ms: 0,
            push_window_request_ms: 0,
            push_backlog: u32::MAX,
            anonymous_bytes_per_ip_hour: u64::MAX,
            db_min_free_bytes: 0,
            register_per_ip_hour: u32::MAX,
            name_check_per_ip_minute: u32::MAX,
            requests_per_pair: u64::MAX,
            request_recipients_per_sender: u64::MAX,
            requests_per_recipient: u64::MAX,
            backup_per_ip_hour: u32::MAX,
            report_per_install_day: u32::MAX,
            support_per_install_day: u32::MAX,
            help_per_ip_day: u32::MAX,
            diagnostics_per_install_hour: u32::MAX,
            diagnostics_per_ip_hour: u32::MAX,
            todo_per_ip_day: u32::MAX,
            storage_per_user_hour: u32::MAX,
            blob_put_per_ip_minute: u32::MAX,
            blob_get_per_ip_minute: u32::MAX,
            ..Limits::default()
        }
    }
}

/// Where blobs live and how much room they may take (spec 6.8).
#[derive(Debug, Clone)]
pub struct BlobConfig {
    /// One file per blob in 256 directories, `tmp/` for uploads; not backed up: blobs are fleeting.
    pub dir: PathBuf,
    /// All blobs together, at most; uploads beyond are refused (`507`) until fetches and time have made room.
    pub budget_bytes: u64,
    /// Uploads are refused while the disk has less than this free (`507`), whatever the budget says.
    pub min_free_bytes: u64,
}

impl BlobConfig {
    pub const DEFAULT_BUDGET_BYTES: u64 = 16 * GIB;
    pub const DEFAULT_MIN_FREE_BYTES: u64 = 4 * GIB;

    /// A new directory of its own under the system's temporary one: for tests.
    pub fn for_tests() -> Self {
        let mut n = [0u8; 8];
        getrandom::getrandom(&mut n).expect("random");
        BlobConfig {
            dir: std::env::temp_dir().join(format!("yo-hub-blobs-{}-{}", std::process::id(), u64::from_le_bytes(n))),
            budget_bytes: Self::DEFAULT_BUDGET_BYTES,
            min_free_bytes: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApnsConfig {
    /// The .p8 key, PEM.
    pub key_pem: String,
    pub key_id: String,
    pub team_id: String,
    /// The app's bundle id.
    pub topic: String,
}

#[derive(Debug, Clone)]
pub struct FcmConfig {
    /// The Firebase service account, JSON.
    pub service_account_json: String,
}

/// The fewest characters (grapheme clusters) of a new username when `YO_NAME_MIN` sets none (spec 3.3). Only the hub
/// has a least: the apps show its `400`.
pub const NAME_MIN: usize = 3;

#[derive(Debug, Clone)]
pub struct Config {
    /// Shared admin credential; absent disables the admin route.
    pub admin_token: Option<String>,
    /// The SQLite file; ":memory:" for tests.
    pub db_path: String,
    /// Names to reserve at start, for tests and tools: the operator's live in the database.
    pub reserved: Vec<Reserved>,
    /// Take the client's address from `CF-Connecting-IP`: only behind Cloudflare.
    pub trust_cloudflare: bool,
    /// Addresses of test robots (`YO_ROBOT_RANGES`, default `ROBOT_RANGES`): their support messages, reports and
    /// diagnostics are answered `204` and dropped.
    pub robot_ranges: Vec<(std::net::IpAddr, u8)>,
    /// Apps allowed passkeys for this domain (`TEAMID.bundle.id`), for `/.well-known/apple-app-site-association`.
    pub apple_app_ids: Vec<String>,
    /// Digital Asset Links statements served at /.well-known/assetlinks.json.
    pub asset_links: Option<Vec<serde_json::Value>>,
    pub limits: Limits,
    pub apns: Option<ApnsConfig>,
    pub fcm: Option<FcmConfig>,
    pub blobs: BlobConfig,
    /// The oldest build per platform (`YO_MIN_APP` "ios/202611140619,android/12"): older gets `426` (spec 6.3).
    pub min_app: Vec<(String, u64)>,
    /// The fewest grapheme clusters of a username (`YO_NAME_MIN`, default `NAME_MIN`, spec 3.3): new names only.
    pub name_min: usize,
    /// The hub's Curve25519 private key (`YO_HUB_KEY_FILE`): it signs answers handing out keys (spec 6.10).
    pub hub_key: [u8; 32],
    /// The server key and its certificate by the trust root (`YO_SERVER_CERT_FILE`, spec 6.1).
    pub server_certificate: Vec<u8>,
    /// The hub's zkgroup `ServerSecretParams` (`YO_ZK_PARAMS_FILE`, spec 4.9).
    pub zk_params: Vec<u8>,
    /// The operator's bot accounts (`YO_BOT_ACCOUNTS` "YoGPT:USERHEX,PING:USERHEX"), a stopgap: no rate limits per
    /// device or user, and what they send counted apart (`bot-yo`, `bot-media`, `bot-media:LABEL`), out of the Yo counts.
    pub bots: Vec<(String, yo_wire::UserId)>,
}

/// `YO_BOT_ACCOUNTS`: comma separated `LABEL:USER`, the label letters and digits (at most 16), the user id 32 hex.
pub fn parse_bots(text: &str) -> Result<Vec<(String, yo_wire::UserId)>, String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let bad = || format!("YO_BOT_ACCOUNTS: {s:?} is not LABEL:USER (32 hex)");
            let (label, user) = s.split_once(':').ok_or_else(bad)?;
            let label_ok = (1..=16).contains(&label.len()) && label.bytes().all(|b| b.is_ascii_alphanumeric());
            let hex_ok = user.len() == 32 && user.bytes().all(|b| b.is_ascii_hexdigit());
            if !label_ok || !hex_ok {
                return Err(bad());
            }
            let mut id = [0u8; 16];
            for (i, byte) in id.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&user[2 * i..2 * i + 2], 16).map_err(|_| bad())?;
            }
            Ok((label.to_owned(), id))
        })
        .collect()
}

/// Firebase Test Lab's devices (Google Play's pre-launch reports), as its documentation lists them
/// (firebase.google.com/docs/test-lab/android/get-started, "IP address blocks", 2026-11).
pub const ROBOT_RANGES: &str = "70.32.128.0/19,108.177.6.0/23,108.177.18.192/26,108.177.29.64/27,108.177.31.160/27,\
    199.36.156.8/29,199.36.156.16/28,209.85.131.0/27,2001:4860:1008::/48,2001:4860:1018::/48,2001:4860:1019::/48,\
    2001:4860:1020::/48,2001:4860:1022::/48,2001:4860:101d::/48,2001:4860:101e::/48,2001:4860:1031::/48,\
    70.32.128.48/28,70.32.150.192/27,108.177.6.0/27,108.177.24.160/27,108.177.29.0/27,108.177.25.128/27,\
    108.177.26.128/26,108.177.29.192/26,172.217.120.64/26,172.217.120.128/26,2001:4860:1037::/48,\
    2001:4860:1038::/48,2001:4860:103a::/48,34.68.194.64/29,34.69.234.64/29,34.73.34.72/29,34.73.178.72/29,\
    34.74.10.72/29,34.136.2.136/29,34.136.50.136/29,34.145.234.144/29,35.192.160.56/29,35.196.166.80/29,\
    35.196.169.240/29,35.203.128.0/28,35.234.176.160/28,35.243.2.0/27,35.245.243.240/29,199.192.115.0/30,\
    199.192.115.8/30,199.192.115.16/29";

/// `YO_ROBOT_RANGES`: comma separated `ADDRESS/BITS`.
pub fn parse_ranges(text: &str) -> Result<Vec<(std::net::IpAddr, u8)>, String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let bad = || format!("YO_ROBOT_RANGES: {s:?} is not ADDRESS/BITS");
            let (ip, bits) = s.split_once('/').ok_or_else(bad)?;
            let ip: std::net::IpAddr = ip.parse().map_err(|_| bad())?;
            let bits: u8 = bits.parse().map_err(|_| bad())?;
            match bits <= if ip.is_ipv4() { 32 } else { 128 } {
                true => Ok((ip, bits)),
                false => Err(bad()),
            }
        })
        .collect()
}

/// The address lies in one of the ranges.
pub fn in_ranges(ranges: &[(std::net::IpAddr, u8)], ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr::{V4, V6};
    let prefix = |a: u128, b: u128, bits: u8, width: u32| bits == 0 || (a ^ b) >> (width - bits as u32) == 0;
    ranges.iter().any(|&(net, bits)| match (net, ip) {
        (V4(net), V4(ip)) => prefix(u32::from(net).into(), u32::from(ip).into(), bits, 32),
        (V6(net), V6(ip)) => prefix(net.into(), ip.into(), bits, 128),
        _ => false,
    })
}

/// `YO_MIN_APP`: comma separated `PLATFORM/BUILD`.
pub fn parse_min_app(text: &str) -> Result<Vec<(String, u64)>, String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| app(s).ok_or_else(|| format!("YO_MIN_APP: {s:?} is not PLATFORM/BUILD")))
        .collect()
}

/// `PLATFORM/BUILD` (a `Yo-App` header): the platform in lower case letters (at most 16), the build a number.
pub fn app(text: &str) -> Option<(String, u64)> {
    let (platform, build) = text.split_once('/')?;
    let platform_ok = (1..=16).contains(&platform.len()) && platform.bytes().all(|b| b.is_ascii_lowercase());
    if !platform_ok || build.is_empty() || !build.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((platform.to_owned(), build.parse().ok()?))
}

/// The hub key of `Config::for_tests`: tests pin its public half.
pub const TEST_HUB_KEY: [u8; 32] = [0x48; 32];

/// The public half of a hub key (32 bytes, as `/.well-known/yo-hub-key` serves it and the apps pin it).
pub fn hub_public_key(private: &[u8; 32]) -> [u8; 32] {
    let key = libsignal_protocol::PrivateKey::deserialize(private).expect("32 bytes are a Curve25519 private key");
    key.public_key().expect("a public key").public_key_bytes().try_into().expect("32 bytes")
}

/// A new hub key: its private key (32 bytes) and its public key.
pub fn new_hub_key() -> ([u8; 32], [u8; 32]) {
    let pair = libsignal_protocol::KeyPair::generate(&mut rand::rng());
    let private: [u8; 32] = pair.private_key.serialize().try_into().expect("32 bytes");
    (private, hub_public_key(&private))
}

/// The hub key in a file: exactly 32 bytes (the Curve25519 private key).
pub fn read_hub_key(path: &str) -> Result<[u8; 32], String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    bytes.try_into().map_err(|_| format!("{path}: not 32 bytes"))
}

impl Config {
    /// A hub for tests: in memory, no rate limits, no push services.
    pub fn for_tests() -> Self {
        Config {
            db_path: ":memory:".into(),
            reserved: Vec::new(),
            admin_token: None,
            trust_cloudflare: false,
            robot_ranges: parse_ranges(ROBOT_RANGES).expect("the robot ranges"),
            apple_app_ids: Vec::new(),
            asset_links: None,
            limits: Limits::unlimited(),
            apns: None,
            fcm: None,
            blobs: BlobConfig::for_tests(),
            min_app: Vec::new(),
            name_min: NAME_MIN,
            hub_key: TEST_HUB_KEY,
            server_certificate: crate::certificate::test_file(),
            zk_params: {
                static PARAMS: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
                PARAMS.get_or_init(crate::groups::test_params).clone()
            },
            bots: Vec::new(),
        }
    }
}

/// A list of names, one per line, a note after `#` if any; blank lines and `#` lines skipped (tests and tools).
pub fn parse_reserved(text: &str) -> Vec<Reserved> {
    text.lines()
        .filter_map(|line| {
            let (name, note) = match line.split_once('#') {
                Some((name, note)) => (name, Some(note.trim()).filter(|n| !n.is_empty())),
                None => (line, None),
            };
            let name = name.trim().to_owned();
            (!name.is_empty()).then(|| Reserved { spelling: name, note: note.map(str::to_owned) })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_are_platform_and_build() {
        assert_eq!(app("ios/202611140619"), Some(("ios".into(), 202611140619)));
        for bad in ["", "ios", "ios/", "/1", "iOS/1", "ios/1a", "ios/-1", "a b/1", "ios/99999999999999999999"] {
            assert_eq!(app(bad), None, "{bad:?}");
        }
        assert_eq!(parse_min_app(" ios/5, android/7 ,").unwrap(), [("ios".into(), 5), ("android".into(), 7)]);
        assert!(parse_min_app("ios").is_err());
    }

    #[test]
    fn bot_accounts_are_labels_and_user_ids() {
        let bots =
            parse_bots(" YoGPT:000102030405060708090a0b0c0d0e0f, PING:FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF ,").unwrap();
        assert_eq!(bots, [("YoGPT".into(), std::array::from_fn(|i| i as u8)), ("PING".into(), [255; 16])]);
        for bad in
            ["YoGPT", "YoGPT:0001", ":000102030405060708090a0b0c0d0e0f", "Yo GPT:000102030405060708090a0b0c0d0e0f"]
        {
            assert!(parse_bots(bad).is_err(), "{bad:?}");
        }
        assert!(parse_bots("").unwrap().is_empty());
    }
}
