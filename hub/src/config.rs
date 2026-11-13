//! The hub's configuration: everything main.rs reads from the environment, and what tests set directly.

/// Rate limits (spec 6.6). Each is a token bucket: this many per window, refilled evenly.
#[derive(Debug, Clone)]
pub struct Limits {
    pub verify_per_phone_hour: u32,
    pub verify_per_ip_hour: u32,
    /// Code attempts, against guessing.
    pub check_per_phone_hour: u32,
    /// Lookups that hand out KeyPackages: starting chats.
    pub lookup_per_device_day: u32,
    /// Lookups of an account key only: checking who started a chat.
    pub identity_lookup_per_device_day: u32,
    /// Signed requests of one device, all routes.
    pub signed_per_device_minute: u32,
    pub deliver_per_ip_minute: u32,
    /// Envelopes a device holds before deliveries to its user are refused. No rate per recipient: a stranger who
    /// knows a user id could spend it and shut out everyone else.
    pub max_pending_per_device: u32,
    pub groups_per_ip_hour: u32,
    pub commit_per_ip_minute: u32,
    /// Registrations without a number, where they are switched on.
    pub register_per_ip_hour: u32,
    pub envelope_ttl_days: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            verify_per_phone_hour: 3,
            verify_per_ip_hour: 10,
            check_per_phone_hour: 10,
            lookup_per_device_day: 50,
            identity_lookup_per_device_day: 1000,
            signed_per_device_minute: 1200,
            deliver_per_ip_minute: 1200,
            max_pending_per_device: 5000,
            groups_per_ip_hour: 60,
            commit_per_ip_minute: 600,
            register_per_ip_hour: 10,
            envelope_ttl_days: 30,
        }
    }
}

impl Limits {
    /// No rate limit worth the name: for load tests from one address.
    pub fn unlimited() -> Self {
        Limits {
            verify_per_phone_hour: u32::MAX,
            verify_per_ip_hour: u32::MAX,
            check_per_phone_hour: u32::MAX,
            lookup_per_device_day: u32::MAX,
            identity_lookup_per_device_day: u32::MAX,
            signed_per_device_minute: u32::MAX,
            deliver_per_ip_minute: u32::MAX,
            max_pending_per_device: 5000,
            groups_per_ip_hour: u32::MAX,
            commit_per_ip_minute: u32::MAX,
            register_per_ip_hour: u32::MAX,
            envelope_ttl_days: 30,
        }
    }
}

#[derive(Debug, Clone)]
pub enum SmsConfig {
    /// Accepts one fixed code and sends nothing: development and tests.
    Dev { code: String },
    /// Twilio Verify.
    Twilio { account_sid: String, auth_token: String, service_sid: String },
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
    /// The SQLite file; ":memory:" for tests.
    pub db_path: String,
    /// The key of the phone HMAC. Never in the database.
    pub phone_secret: [u8; 32],
    /// Take the client's address from `CF-Connecting-IP`: only behind Cloudflare.
    pub trust_cloudflare: bool,
    /// `POST /v1/register`: accounts without a number (spec 6.4). Off unless switched on.
    pub open_registration: bool,
    /// Apps allowed to make passkeys for this hub's domain (`TEAMID.bundle.id`), served as
    /// `/.well-known/apple-app-site-association`; none, no such file.
    pub apple_app_ids: Vec<String>,
    pub limits: Limits,
    pub sms: SmsConfig,
    pub apns: Option<ApnsConfig>,
    pub fcm: Option<FcmConfig>,
}

impl Config {
    /// A hub for tests: in memory, dev SMS code "000000", registration without a number on, no rate limits, no push
    /// services.
    pub fn for_tests() -> Self {
        Config {
            db_path: ":memory:".into(),
            phone_secret: [7; 32],
            trust_cloudflare: false,
            open_registration: true,
            apple_app_ids: Vec::new(),
            limits: Limits::unlimited(),
            sms: SmsConfig::Dev { code: "000000".into() },
            apns: None,
            fcm: None,
        }
    }
}
