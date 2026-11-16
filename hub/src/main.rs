//! yo-hub: reads its configuration from the environment and serves.
//!
//!   YO_LISTEN             address, default 127.0.0.1:8080 (behind a Cloudflare Tunnel)
//!   YO_DB                 SQLite file, default yo-hub.sqlite
//!   YO_TRUST_CLOUDFLARE   1: take the client address from CF-Connecting-IP
//!   YO_APPLE_APP_IDS      TEAMID.bundle.id, comma-separated: apps that may make passkeys for this hub's domain
//!   YO_APNS_KEY_FILE, YO_APNS_KEY_ID, YO_APNS_TEAM_ID, YO_APNS_TOPIC
//!   YO_FCM_SERVICE_ACCOUNT_FILE
//!   YO_RESERVED_FILE      deprecated: accepted but ignored; names and switch live in the database.
//!   YO_BLOB_DIR           photos and videos (spec 6.8), one file each; default `blobs` beside YO_DB. Not for the
//!                         database's backup: blobs are fleeting.
//!   YO_BLOB_BUDGET_MB     all blobs together at most, default 16384 (16 GiB); uploads beyond are refused (507)
//!   YO_BLOB_MIN_FREE_MB   uploads are refused while the disk has less free, default 4096 (4 GiB)
//!   YO_HUB_KEY_FILE       required: the hub's Curve25519 private key (32 bytes, `yo-hub hub-key new`), which signs
//!                         answers handing out keys (spec 6.10, XEdDSA); its public half is pinned in the apps
//!   YO_SERVER_CERT_FILE   required: the server key and its certificate by the trust root (spec 6.1), which sign the
//!                         devices' sender certificates; the trust root's public key is pinned in the apps
//!   YO_ZK_PARAMS_FILE     required: the zkgroup ServerSecretParams of its groups (spec 4.9); their public half is
//!                         pinned in the apps
//!
//! Sender-certificate keys (Signal's: a trust root kept offline, a server certificate on the hub):
//!   yo-hub trust-root new ROOT_FILE                     a new trust root (its private key, 32 bytes, 0600); prints
//!                                                       its public key, base64url, for the apps
//!   yo-hub trust-root server ROOT_FILE KEY_ID OUT_FILE  a new server key with its certificate by that root: the hub's
//!                                                       YO_SERVER_CERT_FILE (0600)
//!
//! The hub key (signed answers, spec 6.10):
//!   yo-hub hub-key new FILE                             a new hub key (its private key, 32 bytes, 0600): the hub's
//!                                                       YO_HUB_KEY_FILE; prints its public key, base64url, for the apps
//!   yo-hub hub-key public FILE                          prints it again
//!
//! Group keys (Signal's zkgroup server params, spec 4.9):
//!   yo-hub zk new FILE                                  new ServerSecretParams (0600), the hub's YO_ZK_PARAMS_FILE;
//!                                                       prints the public params, base64url, for the apps
//!   yo-hub zk public FILE                               prints them again

use std::sync::Arc;
use yo_hub::config::{parse_min_app, read_hub_key, ApnsConfig, BlobConfig, Config, FcmConfig, Limits};
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
    if var("YO_RESERVED_FILE").is_some() {
        eprintln!("yo-hub: YO_RESERVED_FILE is deprecated and ignored; manage reserved names through admin");
    }
    let config = Config {
        admin_token: var("YO_ADMIN_TOKEN").filter(|token| !token.is_empty()),
        db_path,
        reserved: Vec::new(), // the names live in the database, added through the admin page
        trust_cloudflare: var("YO_TRUST_CLOUDFLARE").as_deref() == Some("1"),
        apple_app_ids: var("YO_APPLE_APP_IDS")
            .map(|ids| ids.split(',').map(|id| id.trim().to_string()).filter(|id| !id.is_empty()).collect())
            .unwrap_or_default(),
        limits: Limits {
            // A test hub takes more sign-ups from one address (scripts making accounts); the default is production's.
            register_per_ip_hour: var("YO_REGISTER_PER_IP_HOUR")
                .map(|text| text.parse::<u32>().map_err(|_| "YO_REGISTER_PER_IP_HOUR: a number"))
                .transpose()?
                .unwrap_or(Limits::default().register_per_ip_hour),
            ..Limits::default()
        },
        apns,
        fcm,

        blobs,
        min_app: var("YO_MIN_APP").map(|text| parse_min_app(&text)).transpose()?.unwrap_or_default(),
        name_min: var("YO_NAME_MIN")
            .map(|text| text.parse::<usize>().ok().filter(|n| (1..=16).contains(n)).ok_or("YO_NAME_MIN: 1 to 16"))
            .transpose()?
            .unwrap_or(2),
        hub_key: read_hub_key(&required("YO_HUB_KEY_FILE")?)?,
        server_certificate: {
            let path = required("YO_SERVER_CERT_FILE")?;
            std::fs::read(&path).map_err(|e| format!("YO_SERVER_CERT_FILE: {path}: {e}"))?
        },
        zk_params: {
            let path = required("YO_ZK_PARAMS_FILE")?;
            std::fs::read(&path).map_err(|e| format!("YO_ZK_PARAMS_FILE: {path}: {e}"))?
        },
    };
    Ok((config, var("YO_LISTEN").unwrap_or_else(|| "127.0.0.1:8080".into())))
}

/// Writes a new file readable by its owner alone; never over one that is there.
fn write_new(path: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{path}: {e}"))?;
    file.write_all(bytes).and_then(|()| file.sync_all()).map_err(|e| format!("{path}: {e}"))
}

/// `yo-hub trust-root …` (see above).
fn trust_root(args: &[String]) -> Result<(), String> {
    match args {
        [new, root] if new == "new" => {
            let (private, public) = yo_hub::certificate::new_root();
            write_new(root, &private)?;
            println!("{}", yo_wire::b64(&public));
            Ok(())
        }
        [server, root, key_id, out] if server == "server" => {
            let private = std::fs::read(root).map_err(|e| format!("{root}: {e}"))?;
            let key_id = key_id.parse::<u32>().map_err(|_| "KEY_ID: a number".to_string())?;
            write_new(out, &yo_hub::certificate::server_file(&private, key_id)?)
        }
        _ => Err("usage: yo-hub trust-root new ROOT_FILE | yo-hub trust-root server ROOT_FILE KEY_ID OUT_FILE".into()),
    }
}

/// `yo-hub hub-key …` (see above).
fn hub_key(args: &[String]) -> Result<(), String> {
    match args {
        [new, file] if new == "new" => {
            let (private, public) = yo_hub::config::new_hub_key();
            write_new(file, &private)?;
            println!("{}", yo_wire::b64(&public));
            Ok(())
        }
        [public, file] if public == "public" => {
            println!("{}", yo_wire::b64(&yo_hub::config::hub_public_key(&read_hub_key(file)?)));
            Ok(())
        }
        _ => Err("usage: yo-hub hub-key new FILE | yo-hub hub-key public FILE".into()),
    }
}

/// `yo-hub zk …` (see above).
fn zk(args: &[String]) -> Result<(), String> {
    match args {
        [new, file] if new == "new" => {
            let (secret, public) = yo_hub::groups::new_params();
            write_new(file, &secret)?;
            println!("{}", yo_wire::b64(&public));
            Ok(())
        }
        [public, file] if public == "public" => {
            let secret = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            println!("{}", yo_wire::b64(yo_hub::groups::Zk::from_bytes(&secret)?.public()));
            Ok(())
        }
        _ => Err("usage: yo-hub zk new FILE | yo-hub zk public FILE".into()),
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("hub-key") {
        if let Err(error) = hub_key(&args[1..]) {
            eprintln!("yo-hub: {error}");
            std::process::exit(2);
        }
        return;
    }
    if args.first().map(String::as_str) == Some("zk") {
        if let Err(error) = zk(&args[1..]) {
            eprintln!("yo-hub: {error}");
            std::process::exit(2);
        }
        return;
    }
    if args.first().map(String::as_str) == Some("trust-root") {
        if let Err(error) = trust_root(&args[1..]) {
            eprintln!("yo-hub: {error}");
            std::process::exit(2);
        }
        return;
    }
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
