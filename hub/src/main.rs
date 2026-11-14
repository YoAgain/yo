//! yo-hub: reads its configuration from the environment and serves.
//!
//!   YO_LISTEN             address, default 127.0.0.1:8080 (behind a Cloudflare Tunnel)
//!   YO_DB                 SQLite file, default yo-hub.sqlite
//!   YO_TRUST_CLOUDFLARE   1: take the client address from CF-Connecting-IP
//!   YO_APPLE_APP_IDS      TEAMID.bundle.id, comma-separated: apps that may make passkeys for this hub's domain
//!   YO_APNS_KEY_FILE, YO_APNS_KEY_ID, YO_APNS_TEAM_ID, YO_APNS_TOPIC
//!   YO_FCM_SERVICE_ACCOUNT_FILE
//!   YO_RESERVED_FILE      usernames nobody may claim (spec 3.3), kept by the operator outside this repository: one
//!                         per line in its preferred spelling, an optional `# note`; blank lines and `#` lines are
//!                         skipped. Read at every start; names the database does not hold yet are added (by
//!                         skeleton, INSERT OR IGNORE). A name taken off the list stays reserved until deleted by hand:
//!                         `DELETE FROM reserved WHERE skeleton = 'ADMIN';`. Unset, or no such file: nothing added.
//!                         Reserving a name takes it from no one who has it already.
//!   YO_BLOB_DIR           photos and videos (spec 6.8), one file each; default `blobs` beside YO_DB. Not for the
//!                         database's backup: blobs are fleeting.
//!   YO_BLOB_BUDGET_MB     all blobs together at most, default 16384 (16 GiB); uploads beyond are refused (507)
//!   YO_BLOB_MIN_FREE_MB   uploads are refused while the disk has less free, default 4096 (4 GiB)

use std::sync::Arc;
use yo_hub::config::{read_reserved, ApnsConfig, BlobConfig, Config, FcmConfig, Limits};
use yo_hub::{push, Hub};

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn required(name: &str) -> Result<String, String> {
    var(name).ok_or(format!("{name} is not set"))
}

fn file(name: &str) -> Result<Option<String>, String> {
    var(name).map(|path| std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))).transpose()
}

fn megabytes(name: &str, default: u64) -> Result<u64, String> {
    match var(name) {
        Some(value) => value
            .parse::<u64>()
            .ok()
            .and_then(|mb| mb.checked_mul(1024 * 1024))
            .ok_or_else(|| format!("{name}: a number of MiB")),
        None => Ok(default),
    }
}

fn config() -> Result<(Config, String), String> {
    let apns = match file("YO_APNS_KEY_FILE")? {
        Some(key_pem) => Some(ApnsConfig {
            key_pem,
            key_id: required("YO_APNS_KEY_ID")?,
            team_id: required("YO_APNS_TEAM_ID")?,
            topic: required("YO_APNS_TOPIC")?,
        }),
        None => None,
    };
    let fcm = file("YO_FCM_SERVICE_ACCOUNT_FILE")?.map(|service_account_json| FcmConfig { service_account_json });
    let db_path = var("YO_DB").unwrap_or_else(|| "yo-hub.sqlite".into());
    let blobs = BlobConfig {
        dir: var("YO_BLOB_DIR").map(Into::into).unwrap_or_else(|| {
            let db = std::path::Path::new(&db_path);
            db.parent().unwrap_or(std::path::Path::new(".")).join("blobs")
        }),
        budget_bytes: megabytes("YO_BLOB_BUDGET_MB", BlobConfig::DEFAULT_BUDGET_BYTES)?,
        min_free_bytes: megabytes("YO_BLOB_MIN_FREE_MB", BlobConfig::DEFAULT_MIN_FREE_BYTES)?,
    };
    let config = Config {
        db_path,
        trust_cloudflare: var("YO_TRUST_CLOUDFLARE").as_deref() == Some("1"),
        apple_app_ids: var("YO_APPLE_APP_IDS")
            .map(|ids| ids.split(',').map(|id| id.trim().to_string()).filter(|id| !id.is_empty()).collect())
            .unwrap_or_default(),
        limits: Limits::default(),
        apns,
        fcm,
        reserved: read_reserved(var("YO_RESERVED_FILE").as_deref())?,
        blobs,
    };
    Ok((config, var("YO_LISTEN").unwrap_or_else(|| "127.0.0.1:8080".into())))
}

#[tokio::main]
async fn main() {
    let (config, listen) = config().unwrap_or_else(|error| {
        eprintln!("yo-hub: {error}");
        std::process::exit(2);
    });
    let trust_cloudflare = config.trust_cloudflare;
    let hub = Hub::new(config, Arc::new(push::Https::default())).unwrap_or_else(|error| {
        eprintln!("yo-hub: {error}");
        std::process::exit(2);
    });
    let listener = tokio::net::TcpListener::bind(&listen).await.unwrap_or_else(|error| {
        eprintln!("yo-hub: {listen}: {error}");
        std::process::exit(2);
    });
    eprintln!("yo-hub: listening on {listen}");
    if trust_cloudflare && !listener.local_addr().is_ok_and(|a| a.ip().is_loopback()) {
        // CF-Connecting-IP is believed: whoever reaches this address without Cloudflare names any address it likes
        // and passes every per-address limit. Fine only where nothing but cloudflared can reach it (a container
        // whose port is published on 127.0.0.1 alone).
        eprintln!("yo-hub: YO_TRUST_CLOUDFLARE with {listen}, not a loopback address: only cloudflared may reach it");
    }
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    if let Err(error) = hub.serve(listener, shutdown).await {
        eprintln!("yo-hub: {error}");
        std::process::exit(1);
    }
}
