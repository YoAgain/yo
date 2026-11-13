//! The hub's configuration: everything main.rs reads from the environment, and what tests set directly.

use crate::db::Reserved;

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
    /// Registrations by username: first come, first served, this many per address (spec 6.4).
    pub register_per_ip_hour: u32,
    /// Whether a username is free, asked while it is typed (spec 6.3).
    pub name_check_per_ip_minute: u32,
    /// Backup records fetched: by the user id or a locator, neither of which can be guessed (spec 7).
    pub backup_per_ip_hour: u32,
    /// Reports of users, by one user (spec 6.3).
    pub report_per_user_day: u32,
    pub envelope_ttl_days: u64,
    /// A user without a backup record whose devices made no request for this long is removed with its name
    /// (spec 6.4).
    pub unsecured_idle_days: u64,
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
            name_check_per_ip_minute: 60,
            backup_per_ip_hour: 10,
            report_per_user_day: 20,
            envelope_ttl_days: 30,
            unsecured_idle_days: 365,
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
            name_check_per_ip_minute: u32::MAX,
            backup_per_ip_hour: u32::MAX,
            report_per_user_day: u32::MAX,
            envelope_ttl_days: 30,
            unsecured_idle_days: 365,
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
    /// Apps allowed to make passkeys for this hub's domain (`TEAMID.bundle.id`), served as
    /// `/.well-known/apple-app-site-association`; none, no such file.
    pub apple_app_ids: Vec<String>,
    pub limits: Limits,
    pub sms: SmsConfig,
    pub apns: Option<ApnsConfig>,
    pub fcm: Option<FcmConfig>,
    /// Usernames nobody may claim (spec 3.3), from the operator's list (`YO_RESERVED_FILE`): added to the database
    /// at every start where it does not hold them yet.
    pub reserved: Vec<Reserved>,
}

/// The operator's list of reserved usernames: one per line in its preferred spelling, a note after `#` if any.
/// Blank lines and lines starting with `#` are skipped; so is nothing else: a name is what stands before the `#`,
/// trimmed, even one no app could register.
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

/// The list in a file, if there is one: none named, or none there, is an empty list (the hub reserves nothing new).
pub fn read_reserved(path: Option<&str>) -> Result<Vec<Reserved>, String> {
    let Some(path) = path else { return Ok(Vec::new()) };
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(parse_reserved(&text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("yo-hub: {path}: not there, no reserved names added");
            Ok(Vec::new())
        }
        Err(error) => Err(format!("{path}: {error}")),
    }
}

impl Config {
    /// A hub for tests: in memory, dev SMS code "000000", no rate limits, no push services.
    pub fn for_tests() -> Self {
        Config {
            db_path: ":memory:".into(),
            phone_secret: [7; 32],
            trust_cloudflare: false,
            apple_app_ids: Vec::new(),
            limits: Limits::unlimited(),
            sms: SmsConfig::Dev { code: "000000".into() },
            apns: None,
            fcm: None,
            reserved: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserved_list_is_read_tolerantly() {
        let path = std::env::temp_dir().join(format!("yo-hub-reserved-{}.txt", std::process::id()));
        std::fs::write(&path, "# Reserved\n\nYo\n  Admin   # the operator\nJust Yo#\n#Gone\nLéo # accents too\n\r\n")
            .unwrap();
        let read = read_reserved(path.to_str()).unwrap();
        std::fs::remove_file(&path).unwrap();
        let spelt: Vec<(&str, Option<&str>)> = read.iter().map(|r| (r.spelling.as_str(), r.note.as_deref())).collect();
        assert_eq!(
            spelt,
            [("Yo", None), ("Admin", Some("the operator")), ("Just Yo", None), ("Léo", Some("accents too"))]
        );
        assert!(read_reserved(None).unwrap().is_empty());
        assert!(read_reserved(path.to_str()).unwrap().is_empty(), "a file not there: nothing");
    }
}
