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
    /// Deliveries of one sender. A delivery is not signed (the hub does not learn who sends), so the sender is its
    /// address (spec 6.6).
    pub deliver_per_ip_minute: u32,
    /// Envelopes a device holds before deliveries to its user are refused. No rate per recipient: a stranger who
    /// knows a user id could spend it and shut out everyone else.
    pub max_pending_per_device: u32,
    /// Bytes stored without identity, by address: delivered envelopes. They stay up to `envelope_ttl_days`, so
    /// without it one address could fill the disk.
    pub anonymous_bytes_per_ip_hour: u64,
    /// Deliveries are refused (`507`) while the database's disk has less than this free:
    /// better a refusal than a full disk.
    pub db_min_free_bytes: u64,
    /// Registrations by username: first come, first served, this many per address (spec 6.4).
    pub register_per_ip_hour: u32,
    /// Whether a username is free, asked while it is typed (spec 6.3).
    pub name_check_per_ip_minute: u32,
    /// Requests (signed deliveries without the recipient's profile key) one sender sends one recipient at once,
    /// one more an hour (spec 4.10).
    pub requests_per_pair: u64,
    /// Recipients new to one sender's requests at once, one more an hour (spec 4.10).
    pub request_recipients_per_sender: u64,
    /// Requests one recipient takes from everyone at once, as many again a day (spec 4.10): against many accounts
    /// made to flood one.
    pub requests_per_recipient: u64,
    /// The hour those refill by, ms: an hour; a load test that runs days of use in minutes makes it shorter.
    pub request_hour_ms: u64,
    /// Failed backup fetches by address: unknown records or invalid door signatures (spec 7).
    pub backup_per_ip_hour: u32,
    /// Reports of users, by one user (spec 6.3).
    pub report_per_user_day: u32,
    /// Feedback on the app, by one user (spec 6.3).
    pub feedback_per_user_day: u32,
    /// Crash reports from one address a day (spec 6.4): no account to count them by.
    pub crash_per_ip_day: u32,
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
            anonymous_bytes_per_ip_hour: 64 * MIB,
            db_min_free_bytes: GIB,
            register_per_ip_hour: 10,
            name_check_per_ip_minute: 60,
            requests_per_pair: 10,
            request_recipients_per_sender: 20,
            requests_per_recipient: 1000,
            request_hour_ms: 3_600_000,
            backup_per_ip_hour: 30,
            report_per_user_day: 20,
            feedback_per_user_day: 10,
            crash_per_ip_day: 5,
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
            max_pending_per_device: 5000,
            anonymous_bytes_per_ip_hour: u64::MAX,
            db_min_free_bytes: 0,
            register_per_ip_hour: u32::MAX,
            name_check_per_ip_minute: u32::MAX,
            requests_per_pair: u64::MAX,
            request_recipients_per_sender: u64::MAX,
            requests_per_recipient: u64::MAX,
            request_hour_ms: 3_600_000,
            backup_per_ip_hour: u32::MAX,
            report_per_user_day: u32::MAX,
            feedback_per_user_day: u32::MAX,
            crash_per_ip_day: u32::MAX,
            storage_per_user_hour: u32::MAX,
            envelope_ttl_days: 30,
            blob_put_per_ip_minute: u32::MAX,
            blob_get_per_ip_minute: u32::MAX,
            ..Limits::default()
        }
    }
}

/// Where blobs live and how much room they may take (spec 6.8).
#[derive(Debug, Clone)]
pub struct BlobConfig {
    /// One file per blob, in 256 directories, and `tmp/` for uploads under way. Not in the database's backup:
    /// blobs are fleeting.
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

#[derive(Debug, Clone)]
pub struct Config {
    /// Shared admin credential; absent disables the admin route.
    pub admin_token: Option<String>,
    /// The SQLite file; ":memory:" for tests.
    pub db_path: String,
    /// Names to reserve when the hub starts, for tests and tools only: the operator's names live in the database
    /// (added through the admin page), never in a file.
    pub reserved: Vec<Reserved>,
    /// Take the client's address from `CF-Connecting-IP`: only behind Cloudflare.
    pub trust_cloudflare: bool,
    /// Apps allowed to make passkeys for this hub's domain (`TEAMID.bundle.id`), served as
    /// `/.well-known/apple-app-site-association`; none, no such file.
    pub apple_app_ids: Vec<String>,
    pub limits: Limits,
    pub apns: Option<ApnsConfig>,
    pub fcm: Option<FcmConfig>,
    pub blobs: BlobConfig,
    /// The oldest build each platform may still use ("ios" → 202611140619), from `YO_MIN_APP`
    /// ("ios/202611140619,android/12"): an older one gets `426` and asks to be updated. A request without `Yo-App`
    /// passes (apps from before it, tools).
    pub min_app: Vec<(String, u64)>,
    /// The fewest characters (grapheme clusters) a username may have on this hub, from `YO_NAME_MIN` (default 2): a
    /// switch the operator can lower to 1, or raise, without an app update (spec 3.3).
    pub name_min: usize,
    /// The hub's Curve25519 private key (`YO_HUB_KEY_FILE`, 32 bytes, the operator's; `yo-hub hub-key new`): it signs
    /// the answers that hand out keys for a client that asks so (spec 6.10), XEdDSA. Apps ship its public half.
    pub hub_key: [u8; 32],
    /// The server key and its certificate by the operator's trust root (`YO_SERVER_CERT_FILE`, made by
    /// `yo-hub trust-root server`): it signs the devices' sender certificates (spec 6.1). The hub does not start
    /// without a good one.
    pub server_certificate: Vec<u8>,
    /// The hub's zkgroup `ServerSecretParams` (`YO_ZK_PARAMS_FILE`, made by `yo-hub zk new`): groups' credentials and
    /// endorsements (spec 4.9). Apps pin the public half. The hub does not start without good ones.
    pub zk_params: Vec<u8>,
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
            apple_app_ids: Vec::new(),
            limits: Limits::unlimited(),
            apns: None,
            fcm: None,
            blobs: BlobConfig::for_tests(),
            min_app: Vec::new(),
            name_min: 2,
            hub_key: TEST_HUB_KEY,
            server_certificate: crate::certificate::test_file(),
            zk_params: {
                static PARAMS: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
                PARAMS.get_or_init(crate::groups::test_params).clone()
            },
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
}
