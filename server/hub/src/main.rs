//! yo-hub: reads its configuration from the environment and serves.
//!
//!   YO_LISTEN             address, default 127.0.0.1:8080 (behind a Cloudflare Tunnel); a socket from systemd
//!                         (LISTEN_FDS, `yo-hub.socket`) goes first. Relay default 127.0.0.1:8090
//!   YO_MESH_CONFIG        optional mesh.toml for the live hub (certs or CREDENTIALS_DIRECTORY)
//!   YO_MESH_PEERS         optional mesh.toml for Corrosion inbox reads from other hubs
//!   YO_PEER_TOKEN         shared peer inbox credential, required for those reads
//!   YO_RELAY_STATUS       relay status address, default 127.0.0.1:8091
//!   YO_DB                 SQLite file, default yo-hub.sqlite
//!   YO_CORROSION_API      local Corrosion HTTP origin; unset for a standalone hub
//!   YO_CORROSION_DB       Corrosion SQLite file, required with YO_CORROSION_API
//!   YO_HUB_NAME           unique hub name for counts, required with YO_CORROSION_API
//!   YO_HUB_INDEX          unique hub number (0..15), required with YO_CORROSION_API
//!   YO_EDGE_UPSTREAM      relay HTTP origin; unset on the central hub
//!   YO_TRUST_CLOUDFLARE   1: take the client address from CF-Connecting-IP
//!   YO_ROBOT_RANGES       ADDRESS/BITS, comma-separated: test robots, whose support messages, reports and
//!                         diagnostics are answered 204 and dropped; default Firebase Test Lab's (config::ROBOT_RANGES)
//!   YO_APPLE_APP_IDS      TEAMID.bundle.id, comma-separated: apps that may make passkeys for this hub's domain
//!   YO_ATTEST             1: measure device attestation; off by default
//!   YO_NAME_ATTEST        on: new names 4+ with attestation in Register, 12+ without; off (default): YO_NAME_MIN
//!   YO_PLAY_INTEGRITY_DECRYPT_KEY, YO_PLAY_INTEGRITY_VERIFY_KEY, YO_PLAY_PACKAGE (see README)
//!   YO_APNS_KEY_FILE, YO_APNS_KEY_ID, YO_APNS_TEAM_ID, YO_APNS_TOPIC
//!   YO_FCM_SERVICE_ACCOUNT_FILE, YO_ASSET_LINKS_FILE
//!   YO_RESERVED_FILE      deprecated: accepted but ignored; names live in the database.
//!   YO_BLOB_DIR           photos and videos (spec 4.3), one file each; default `blobs` beside YO_DB. Not for the
//!                         database's backup: blobs are fleeting.
//!   YO_BLOB_BUDGET_MB     all blobs together at most, default 16384 (16 GiB); uploads beyond are refused (507)
//!   YO_BLOB_MIN_FREE_MB   uploads are refused while the disk has less free, default 4096 (4 GiB)
//!   YO_HUB_KEY_FILE       required: the hub's Curve25519 private key (32 bytes, `yo-hub hub-key new`), which signs
//!                         answers handing out keys (spec 4.1, XEdDSA); its public half is pinned in the apps
//!   YO_SERVER_CERT_FILE   required: the server key and its certificate by the trust root (spec 4.1), which sign the
//!                         devices' sender certificates; the trust root's public key is pinned in the apps
//!   YO_OHTTP_KEY_FILE     optional: 32 bytes of key material for the OHTTP key (spec 4.1), key id
//!                         YO_OHTTP_KEY_ID (default 1); YO_OHTTP_SPARE_FILE and YO_OHTTP_SPARE_KEY_ID (default 2)
//!                         the spare. Without, /v1/ohttp is off. Its key config is pinned in the apps.
//!   YO_DIRECT_GONE        1: after the OHTTP cutoff the direct YoIP routes answer 426 (apps before it update)
//!   YO_ZK_PARAMS_FILE     required: the zkgroup ServerSecretParams of its groups (spec 3.4); their public half is
//!                         pinned in the apps
//!
//! Rollback: stop the central hub and run `yo-hub corrosion-back` with its environment to copy pending mail
//! into YO_DB before restarting in standalone mode.
//!
//! Sender-certificate keys (Signal's: a trust root kept offline, a server certificate on the hub):
//!   yo-hub trust-root new ROOT_FILE                     a new trust root (its private key, 32 bytes, 0600); prints
//!                                                       its public key, base64url, for the apps
//!   yo-hub trust-root server ROOT_FILE KEY_ID OUT_FILE  a new server key with its certificate by that root: the hub's
//!                                                       YO_SERVER_CERT_FILE (0600)
//!
//! The hub key (signed answers, spec 4.1):
//!   yo-hub hub-key new FILE                             a new hub key (its private key, 32 bytes, 0600): the hub's
//!                                                       YO_HUB_KEY_FILE; prints its public key, base64url, for the apps
//!   yo-hub hub-key public FILE                          prints it again
//!
//! Group keys (Signal's zkgroup server params, spec 3.4):
//!   yo-hub zk new FILE                                  new ServerSecretParams (0600), the hub's YO_ZK_PARAMS_FILE;
//!                                                       prints the public params, base64url, for the apps
//!   yo-hub zk public FILE                               prints them again
//!
//! OHTTP keys (spec 4.1):
//!   yo-hub ohttp new FILE KEY_ID                        new key material (32 bytes, 0600), YO_OHTTP_KEY_FILE or
//!                                                       YO_OHTTP_SPARE_FILE; prints its key config, base64url, to pin
//!   yo-hub ohttp public FILE KEY_ID                     prints it again

mod mesh;

use std::sync::Arc;
use yo_hub::config::{
    parse_min_app, parse_ranges, read_hub_key, ApnsConfig, BlobConfig, Config, FcmConfig, Limits, ROBOT_RANGES,
};
use yo_hub::{push, Hub};
use yoip::b64;

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn required(name: &str) -> Result<String, String> {
    var(name).ok_or(format!("{name} is not set"))
}

fn file(name: &str) -> Result<Option<String>, String> {
    var(name).map(|path| std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))).transpose()
}

fn bytes(name: &str) -> Result<Vec<u8>, String> {
    let path = required(name)?;
    std::fs::read(&path).map_err(|e| format!("{name}: {path}: {e}"))
}

/// A variable's value as `T`, if `valid`; `default` when unset.
fn parsed<T: std::str::FromStr>(name: &str, default: T, valid: fn(&T) -> bool, what: &str) -> Result<T, String> {
    var(name).map_or(Ok(default), |text| text.parse().ok().filter(valid).ok_or(format!("{name}: {what}")))
}

fn megabytes(name: &str, default: u64) -> Result<u64, String> {
    let bytes = |mb: String| mb.parse::<u64>().ok()?.checked_mul(1024 * 1024);
    var(name).map_or(Ok(default), |mb| bytes(mb).ok_or(format!("{name}: a number of MiB")))
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
    for old in ["YO_REGISTER_PER_IP_HOUR", "YO_BACKUP_PER_IP_HOUR"] {
        if var(old).is_some() {
            eprintln!("yo-hub: {old} is gone and ignored; YO_EXPENSIVE_PER_IP_HOUR limits both (spec 4.1)");
        }
    }
    let attest = var("YO_ATTEST").as_deref() == Some("1");
    let play_keys = match (var("YO_PLAY_INTEGRITY_DECRYPT_KEY"), var("YO_PLAY_INTEGRITY_VERIFY_KEY")) {
        (Some(decrypt), Some(verify)) if attest => {
            use base64::{engine::general_purpose::STANDARD, Engine};
            let decrypt = STANDARD
                .decode(decrypt)
                .ok()
                .and_then(|v| v.try_into().ok())
                .ok_or("invalid YO_PLAY_INTEGRITY_DECRYPT_KEY")?;
            let verify = STANDARD.decode(verify).map_err(|_| "invalid YO_PLAY_INTEGRITY_VERIFY_KEY")?;
            use aws_lc_rs::signature::{ParsedPublicKey, ECDSA_P256_SHA256_FIXED};
            if verify.first() != Some(&0x30) {
                return Err("YO_PLAY_INTEGRITY_VERIFY_KEY: expected DER SubjectPublicKeyInfo".into());
            }
            ParsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &verify)
                .map_err(|_| "invalid YO_PLAY_INTEGRITY_VERIFY_KEY")?;
            Some(yo_hub::config::PlayKeys { decrypt, verify })
        }
        _ => None,
    };
    if attest && play_keys.is_none() {
        eprintln!("yo-hub: Play Integrity keys not configured; Android attestation unjudged");
    }
    let defaults = Limits::default();
    let config = Config {
        add_devices: var("YO_ADD_DEVICES").as_deref() == Some("1"),
        admin_token: var("YO_ADMIN_TOKEN"),
        db_path,
        corrosion: if let Some(api) = var("YO_CORROSION_API") {
            Some(yo_hub::config::CorrosionConfig {
                api,
                db: required("YO_CORROSION_DB")?,
                hub: required("YO_HUB_NAME")?,
                index: required("YO_HUB_INDEX")?
                    .parse::<u8>()
                    .ok()
                    .filter(|i| *i < 16)
                    .ok_or("YO_HUB_INDEX must be 0..15")?,
                upstream: var("YO_EDGE_UPSTREAM"),
                mesh_peers: var("YO_MESH_PEERS").map(Into::into),
                peer_token: var("YO_PEER_TOKEN").filter(|t| !t.is_empty()),
            })
        } else {
            None
        },
        reserved: Vec::new(), // the names live in the database, added through the admin page
        trust_cloudflare: var("YO_TRUST_CLOUDFLARE").as_deref() == Some("1"),
        robot_ranges: parse_ranges(&var("YO_ROBOT_RANGES").unwrap_or_else(|| ROBOT_RANGES.to_owned()))?,
        attest,
        play_keys,
        play_package: var("YO_PLAY_PACKAGE").unwrap_or_else(|| "app.justyo.android".into()),
        apple_app_ids: var("YO_APPLE_APP_IDS")
            .map(|ids| ids.split(',').map(|id| id.trim().to_string()).filter(|id| !id.is_empty()).collect())
            .unwrap_or_default(),
        asset_links: file("YO_ASSET_LINKS_FILE")?
            .map(|file| serde_json::from_str(&file).map_err(|error| format!("YO_ASSET_LINKS_FILE: {error}")))
            .transpose()?,
        limits: Limits {
            // A test hub takes more sign-ups from one address (scripts making accounts); the default is production's.
            expensive_per_ip_hour: parsed(
                "YO_EXPENSIVE_PER_IP_HOUR",
                defaults.expensive_per_ip_hour,
                |_| true,
                "a number",
            )?,
            // Limits on a flood (spec 4.1, 4.2): the operator may set them without a new build.
            recipient_bytes_burst: megabytes("YO_RECIPIENT_BYTES_BURST_MB", defaults.recipient_bytes_burst)?,
            recipient_bytes_per_hour: megabytes("YO_RECIPIENT_BYTES_PER_HOUR_MB", defaults.recipient_bytes_per_hour)?,
            push_window_contact_ms: parsed(
                "YO_PUSH_WINDOW_CONTACT_MS",
                defaults.push_window_contact_ms,
                |_| true,
                "a number of ms",
            )?,
            push_window_request_ms: parsed(
                "YO_PUSH_WINDOW_REQUEST_MS",
                defaults.push_window_request_ms,
                |_| true,
                "a number of ms",
            )?,
            push_backlog: parsed("YO_PUSH_BACKLOG", defaults.push_backlog, |_| true, "a number")?,
            push_backlog_interval_s: parsed(
                "YO_PUSH_BACKLOG_INTERVAL_S",
                defaults.push_backlog_interval_s,
                |_| true,
                "a number of seconds",
            )?,
            ..defaults
        },
        apns,
        fcm,
        blobs,
        min_app: var("YO_MIN_APP").map(|text| parse_min_app(&text)).transpose()?.unwrap_or_default(),
        name_attest: var("YO_NAME_ATTEST").as_deref() == Some("on"),
        name_min: parsed("YO_NAME_MIN", yo_hub::config::NAME_MIN, |n| (1..=16).contains(n), "1 to 16")?,
        hub_key: read_hub_key(&required("YO_HUB_KEY_FILE")?)?,
        server_certificate: bytes("YO_SERVER_CERT_FILE")?,
        zk_params: bytes("YO_ZK_PARAMS_FILE")?,
        ohttp_keys: ohttp_keys()?,
        direct_gone: var("YO_DIRECT_GONE").is_some_and(|v| v.trim() == "1"),
        bots: var("YO_BOT_ACCOUNTS").map(|text| yo_hub::config::parse_bots(&text)).transpose()?.unwrap_or_default(),
    };
    Ok((config, var("YO_LISTEN").unwrap_or_else(|| "127.0.0.1:8080".into())))
}

/// The OHTTP keys (`Config::ohttp_keys`): current, then spare; each optional.
fn ohttp_keys() -> Result<Vec<(u8, [u8; 32])>, String> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let mut keys = Vec::new();
    for (file, id, default) in
        [("YO_OHTTP_KEY_FILE", "YO_OHTTP_KEY_ID", 1u8), ("YO_OHTTP_SPARE_FILE", "YO_OHTTP_SPARE_KEY_ID", 2)]
    {
        let Some(path) = var(file) else { continue };
        let id = match var(id) {
            Some(text) => text.trim().parse().map_err(|_| format!("{id}: 0 to 255"))?,
            None => default,
        };
        keys.push((id, read_hub_key(&path)?));
    }
    if keys.len() == 2 && keys[0].0 == keys[1].0 {
        return Err("YO_OHTTP_KEY_ID and YO_OHTTP_SPARE_KEY_ID must differ".into());
    }
    Ok(keys)
}

/// Writes a new file readable by its owner alone, never over one that is there; prints the public half, base64url.
fn write_new(path: &str, bytes: &[u8], public: Option<&[u8]>) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{path}: {e}"))?;
    file.write_all(bytes).and_then(|()| file.sync_all()).map_err(|e| format!("{path}: {e}"))?;
    if let Some(public) = public {
        println!("{}", b64(public));
    }
    Ok(())
}

/// `yo-hub trust-root …`, `yo-hub hub-key …`, `yo-hub zk …` (see above).
fn tool(args: &[String]) -> Result<(), String> {
    let read = |file: &str| std::fs::read(file).map_err(|e| format!("{file}: {e}"));
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["trust-root", "new", root] => {
            let (private, public) = yo_hub::certificate::new_root();
            write_new(root, &private, Some(&public))
        }
        ["trust-root", "server", root, key_id, out] => {
            let key_id = key_id.parse::<u32>().map_err(|_| "KEY_ID: a number".to_string())?;
            write_new(out, &yo_hub::certificate::server_file(&read(root)?, key_id)?, None)
        }
        ["hub-key", "new", file] => {
            let (private, public) = yo_hub::config::new_hub_key();
            write_new(file, &private, Some(&public))
        }
        ["hub-key", "public", file] => {
            println!("{}", b64(&yo_hub::config::hub_public_key(&read_hub_key(file)?)));
            Ok(())
        }
        ["zk", "new", file] => {
            let (secret, public) = yo_hub::groups::new_params();
            write_new(file, &secret, Some(&public))
        }
        ["zk", "public", file] => {
            println!("{}", b64(yo_hub::groups::Zk::from_bytes(&read(file)?)?.public()));
            Ok(())
        }
        ["ohttp", "new", file, id] => {
            let id = id.parse().map_err(|_| "KEY_ID: 0 to 255")?;
            let mut ikm = [0u8; 32];
            getrandom::fill(&mut ikm).map_err(|e| e.to_string())?;
            let public = yo_hub::ohttp::key_config(id, &ikm)?;
            write_new(file, &ikm, Some(&public))
        }
        ["ohttp", "public", file, id] => {
            let id = id.parse().map_err(|_| "KEY_ID: 0 to 255")?;
            println!("{}", b64(&yo_hub::ohttp::key_config(id, &read_hub_key(file)?)?));
            Ok(())
        }
        ["ohttp", ..] => Err("usage: yo-hub ohttp new FILE KEY_ID | yo-hub ohttp public FILE KEY_ID".into()),
        ["trust-root", ..] => {
            Err("usage: yo-hub trust-root new ROOT_FILE | yo-hub trust-root server ROOT_FILE KEY_ID OUT_FILE".into())
        }
        ["hub-key", ..] => Err("usage: yo-hub hub-key new FILE | yo-hub hub-key public FILE".into()),
        _ => Err("usage: yo-hub zk new FILE | yo-hub zk public FILE".into()),
    }
}

fn or_exit<T>(result: Result<T, String>) -> T {
    result.unwrap_or_else(|error| {
        eprintln!("yo-hub: {error}");
        std::process::exit(2)
    })
}

/// The listening socket systemd hands over (socket activation, `yo-hub.socket`): fd 3 when LISTEN_FDS is 1 and
/// LISTEN_PID names this process or its parent (Litestream runs the live hub with `-exec` and passes the fd on).
/// None without one: the hub binds YO_LISTEN itself.
fn from_systemd() -> Result<Option<tokio::net::TcpListener>, String> {
    use std::os::fd::FromRawFd;
    let pid = std::env::var("LISTEN_PID").ok().and_then(|pid| pid.parse::<u32>().ok());
    let fds = std::env::var("LISTEN_FDS").ok().and_then(|fds| fds.parse::<u32>().ok());
    let ours = pid.is_some_and(|pid| pid == std::process::id() || pid == std::os::unix::process::parent_id());
    if !ours || fds.is_none_or(|fds| fds == 0) {
        return Ok(None);
    }
    if fds != Some(1) {
        return Err(format!("systemd passed {} sockets, the hub takes one", fds.unwrap_or(0)));
    }
    // SAFETY: under the socket activation protocol fd 3 is a listening socket systemd opened for this process, owned
    // by nobody else here; it is taken once, at start.
    let listener = unsafe { std::net::TcpListener::from_raw_fd(3) };
    listener.set_nonblocking(true).map_err(|error| format!("socket from systemd: {error}"))?;
    tokio::net::TcpListener::from_std(listener).map(Some).map_err(|error| format!("socket from systemd: {error}"))
}

async fn shutdown_signal() {
    let mut term = or_exit(
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|error| error.to_string()),
    );
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    eprintln!("yo-hub: stopping");
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| matches!(s.as_str(), "--relay" | "relay-status")) {
        match mesh::command(&args).await {
            Ok(true) => return,
            Ok(false) => std::process::exit(1),
            Err(e) => {
                eprintln!("yo-hub: {e}");
                std::process::exit(1);
            }
        }
    }
    if args.first().is_some_and(|command| ["hub-key", "zk", "trust-root", "ohttp"].contains(&command.as_str())) {
        return or_exit(tool(&args));
    }
    let (config, listen) = or_exit(config());
    if args.as_slice() == ["corrosion-back"] {
        return or_exit(yo_hub::db::Db::corrosion_back(&config).map_err(|e| e.to_string()));
    }
    let trust_cloudflare = config.trust_cloudflare;
    let hub = or_exit(Hub::new(config, Arc::new(push::Https::default())));
    let listener = match or_exit(from_systemd()) {
        Some(listener) => {
            eprintln!("yo-hub: listening on the socket from systemd");
            listener
        }
        None => {
            let listener =
                or_exit(tokio::net::TcpListener::bind(&listen).await.map_err(|error| format!("{listen}: {error}")));
            eprintln!("yo-hub: listening on {listen}");
            listener
        }
    };
    if trust_cloudflare && !listener.local_addr().is_ok_and(|a| a.ip().is_loopback()) {
        // CF-Connecting-IP is believed: whoever reaches this address without Cloudflare names any address it likes
        // and passes every per-address limit.
        eprintln!("yo-hub: YO_TRUST_CLOUDFLARE with {listen}, not a loopback address: only cloudflared may reach it");
    }
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let mesh = var("YO_MESH_CONFIG").map(|path| tokio::spawn(mesh::serve(path, stopped)));
    let shutdown = async {
        shutdown_signal().await;
        let _ = stop.send(());
    };
    let result = hub.serve(listener, shutdown).await;
    if let Some(task) = mesh {
        let _ = task.await;
    }
    if let Err(error) = result {
        eprintln!("yo-hub: {error}");
        std::process::exit(1);
    }
}
