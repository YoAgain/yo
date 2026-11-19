//! The account's state directory (the same as YoGPT's before: `account`, `key`, `state.sqlite`, all 0600, in a 0700
//! directory), the hub connection, and opening or registering the account in it.

use crate::Failure;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use yo_core::statedb::{DbStore, Loaded, StateDb};
use yo_core::{Client, Error, Push, Response, Transport};

/// An answer from the hub, at most: a full inbox fetch (100 envelopes of at most 65 KiB) fits.
const MAX_RESPONSE: u64 = 16 << 20;

/// Its `Yo-App`: "yogpt/" (what the admin page knows the bots by) and the build's UTC minute (YYYYMMDDHHMM, `YO_BUILD`).
const BUILD: &str = match option_env!("YO_BUILD") {
    Some(build) => build,
    None => "0",
};

/// Where the account lives and how its hub is reached: flags, else the environment (`YO_STATE`, `YO_HUB`,
/// `YO_HUB_KEYS`, `YO_TRUST_ROOTS`, `YO_ZK_PARAMS`; lists comma separated).
pub struct Config {
    pub state: PathBuf,
    pub hub: Option<String>,
    /// The hub's public keys, pinned (spec 6.10): the current one and a spare.
    pub hub_keys: Vec<[u8; 32]>,
    /// The hub's sender-certificate trust roots (spec 6.1): without one no sealed Yo opens.
    pub trust_roots: Vec<[u8; 32]>,
    /// The hub's zkgroup public params (spec 4.9): without them no group.
    pub zk_params: Option<Vec<u8>>,
}

fn key32(what: &str, text: &str) -> Result<[u8; 32], Failure> {
    yo_wire::unb64(text.trim())
        .ok()
        .and_then(|k| k.try_into().ok())
        .ok_or_else(|| Failure::usage(format!("{what}: 32 bytes, base64url")))
}

impl Config {
    /// The global flags before the command (`--state`, `--hub`, `--hub-key`, `--trust-root`, `--zk-params`), the
    /// environment for those not given; the rest of the arguments.
    pub fn parse(args: &[String]) -> Result<(Config, &[String]), Failure> {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let list = |name: &str| -> Vec<String> {
            env(name)
                .map(|v| v.split([',', ' ']).filter(|s| !s.is_empty()).map(str::to_owned).collect())
                .unwrap_or_default()
        };
        let (mut state, mut hub) = (env("YO_STATE"), env("YO_HUB"));
        let (mut hub_keys, mut trust_roots, mut zk) = (Vec::new(), Vec::new(), env("YO_ZK_PARAMS"));
        let (mut keys_given, mut roots_given) = (false, false);
        let mut i = 0;
        while let Some(flag) = args.get(i).filter(|a| a.starts_with("--")) {
            let value = args.get(i + 1).ok_or_else(|| Failure::usage(format!("{flag}: a value")))?.clone();
            match flag.as_str() {
                "--state" => state = Some(value),
                "--hub" => hub = Some(value),
                "--hub-key" => (keys_given, _) = (true, hub_keys.push(key32("--hub-key", &value)?)),
                "--trust-root" => (roots_given, _) = (true, trust_roots.push(key32("--trust-root", &value)?)),
                "--zk-params" => zk = Some(value),
                _ => return Err(Failure::usage(format!("unknown flag {flag}"))),
            }
            i += 2;
        }
        if !keys_given {
            hub_keys = list("YO_HUB_KEYS").iter().map(|k| key32("YO_HUB_KEYS", k)).collect::<Result<_, _>>()?;
        }
        if !roots_given {
            trust_roots =
                list("YO_TRUST_ROOTS").iter().map(|k| key32("YO_TRUST_ROOTS", k)).collect::<Result<_, _>>()?;
        }
        let zk_params = match zk {
            Some(p) => Some(yo_wire::unb64(p.trim()).map_err(|_| Failure::usage("--zk-params: base64url"))?),
            None => None,
        };
        let state = state.ok_or_else(|| Failure::usage("no state directory: --state DIR or YO_STATE"))?;
        let hub = hub.map(|h| h.trim_end_matches('/').to_owned());
        Ok((Config { state: state.into(), hub, hub_keys, trust_roots, zk_params }, &args[i..]))
    }

    pub fn control(&self) -> PathBuf {
        self.state.join("control.sock")
    }
}

/// Signed requests and anonymous ones (sealed deliveries, blob fetches) on separate clients: never one connection for
/// both (spec 1).
struct Http {
    client: reqwest::blocking::Client,
    anonymous: reqwest::blocking::Client,
    base: String,
}

impl Transport for Http {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        let url = format!("{}{}", self.base, path);
        // Only what names nobody goes anonymously; a registration or a backup's fetch names the account (spec 1).
        let signed = headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("yo-device"));
        let anonymous = !signed && yo_wire::api::anonymous(method, path);
        let client = if anonymous { &self.anonymous } else { &self.client };
        let mut request = client.request(method.parse().map_err(|_| "method")?, url).body(body.to_vec());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        // Its build, as an app names its own (spec 6.3), on what names the account anyway: never on an anonymous one.
        if signed {
            request = request.header(yo_wire::api::HEADER_APP, format!("yogpt/{BUILD}"));
        }
        // Without the URL: a blob's path is its capability (spec 6.8), never for an error or a log.
        let response = request.send().map_err(|e| e.without_url().to_string())?;
        let status = response.status().as_u16();
        let mut body = Vec::new();
        response.take(MAX_RESPONSE + 1).read_to_end(&mut body).map_err(|_| "the answer broke off")?;
        if body.len() as u64 > MAX_RESPONSE {
            return Err("answer too large".into());
        }
        Ok(Response { status, body })
    }
}

fn transport(config: &Config) -> Result<Arc<dyn Transport>, Failure> {
    let base = config.hub.clone().ok_or_else(|| Failure::usage("no hub: --hub URL or YO_HUB"))?;
    let client = || {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| Failure::new(format!("http: {e}")))
    };
    Ok(Arc::new(Http { client: client()?, anonymous: client()?, base }))
}

/// Writes beside the file, then renames, then syncs the directory: a crash leaves the old one or the new one.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(tmp, path)?;
    std::fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()
}

/// 32 bytes kept in `path`, made at random the first time when `make`.
fn secret(path: &Path, make: bool) -> Result<Option<[u8; 32]>, Failure> {
    let io = |e: std::io::Error| Failure::new(format!("{}: {e}", path.display()));
    match std::fs::read(path) {
        Ok(bytes) => bytes.try_into().map(Some).map_err(|_| Failure::new(format!("{}: not 32 bytes", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && make => {
            let mut bytes = [0u8; 32];
            getrandom::getrandom(&mut bytes).map_err(|e| Failure::new(e.to_string()))?;
            write_private(path, &bytes).map_err(io)?;
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io(error)),
    }
}

/// The account, open, and the directory's lock: one process per directory, held as long as the client lives.
pub struct Opened {
    pub client: Client,
    pub _lock: std::fs::File,
}

/// What opening found.
pub enum Open {
    Opened(Box<Opened>),
    /// Another process (a `yo listen`, or another command) has the directory.
    Busy,
}

/// The directory's lock, its account secret if it has one, and its state key.
type Locked = (std::fs::File, Option<[u8; 32]>, [u8; 32]);

/// The directory, locked, and its two secrets (made only when `make`); `None` when another process has it.
fn lock(config: &Config, make: bool) -> Result<Option<Locked>, Failure> {
    let dir = &config.state;
    let io = |e: std::io::Error| Failure::new(format!("{}: {e}", dir.display()));
    std::fs::create_dir_all(dir).map_err(io)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(io)?;
    let lock = std::fs::File::create(dir.join("lock")).map_err(io)?;
    if lock.try_lock().is_err() {
        return Ok(None);
    }
    let account = secret(&dir.join("account"), make)?;
    let Some(key) = secret(&dir.join("key"), make || account.is_some())? else { return Err(Failure::not_registered()) };
    Ok(Some((lock, account, key)))
}

/// The database, with the state file of the build before it taken in.
fn database(config: &Config, key: [u8; 32]) -> Result<Arc<StateDb>, Failure> {
    let path = config.state.join("state.sqlite");
    let db = StateDb::open_fresh(&path, key, Arc::new(|_| {})).map_err(|e| Failure::new(format!("state: {e}")))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| Failure::new(format!("{}: {e}", path.display())))?;
    let db = Arc::new(db);
    take_old_state(&db, &config.state.join("state"), key).map_err(|e| Failure::new(format!("the state file: {e}")))?;
    Ok(db)
}

/// The state file of the build before the database, taken in offline as it is (same device, sessions, outbox) and
/// kept beside as `state.v2`, never read again (rollback: README).
fn take_old_state(db: &Arc<StateDb>, old: &Path, key: [u8; 32]) -> Result<(), String> {
    if !old.exists() {
        return Ok(());
    }
    if db.word().map_err(|e| e.to_string())?.is_none() {
        let blob = std::fs::read(old).map_err(|e| e.to_string())?;
        let (state, records) = yo_core::legacy::read_state(&blob, &key).map_err(|e| e.to_string())?;
        yo_core::Store::save_state(&DbStore::new(db.clone(), None, None), &state, records)?;
    }
    std::fs::rename(old, old.with_extension("v2")).map_err(|e| e.to_string())
}

/// The pins, and for `listen --drain-media` no media keys kept.
fn configure(client: &mut Client, config: &Config) -> Result<(), Failure> {
    if !config.hub_keys.is_empty() {
        client.set_hub_keys(config.hub_keys.clone());
    }
    client.set_trust_roots(config.trust_roots.clone());
    if let Some(params) = &config.zk_params {
        client.set_zk_params(params).map_err(|e| Failure::new(format!("--zk-params: {e}")))?;
    }
    Ok(())
}

/// Opens the registered account; a registration under way when it stopped is finished first.
pub fn open(config: &Config) -> Result<Open, Failure> {
    let Some((lock, _, key)) = lock(config, false)? else { return Ok(Open::Busy) };
    let db = database(config, key)?;
    let transport = transport(config)?;
    match db.load(None, &key).map_err(Failure::from)? {
        Loaded::Newer { word, state, records } => {
            let (mut client, _) = Client::from_snapshot(transport, *state, records, false)?;
            client.persist_to(Arc::new(DbStore::new(db, Some(word), None)), key)?;
            configure(&mut client, config)?;
            client.finish_registration()?;
            Ok(Open::Opened(Box::new(Opened { client, _lock: lock })))
        }
        _ => Err(Failure::not_registered()),
    }
}

/// Registers `username` in the directory: with its account secret if it has one (the same account, a new device).
/// Saved before it is sent (spec 6.4): a lost answer loses neither key nor prekeys, and `register` again finishes it.
/// A directory with a state it cannot open (no `key`, a wrong one, damaged) is refused without `force`: registering
/// there replaces the account's device and loses its sessions.
pub fn register(config: &Config, username: &str, force: bool) -> Result<Open, Failure> {
    let dir = &config.state;
    let has_state = ["state.sqlite", "state", "state.v2"].iter().any(|file| dir.join(file).exists());
    if has_state && !dir.join("key").exists() && !force {
        return Err(Failure::new(
            "the directory has a state but no key: put its key back (registering replaces the device; --force)",
        ));
    }
    let Some((lock, account, key)) = lock(config, true)? else { return Ok(Open::Busy) };
    let path = config.state.join("state.sqlite");
    let refused = || {
        Failure::new(
            "the state cannot be opened (a wrong key?): registering replaces the device; --force sets it aside",
        )
    };
    // With `--force`, a state it cannot open is set aside as `state.old`: the account secret registers anew.
    let set_aside = || -> Result<Arc<StateDb>, Failure> {
        std::fs::rename(&path, config.state.join("state.old")).map_err(|e| Failure::new(e.to_string()))?;
        eprintln!("yo: the state could not be opened; set aside as state.old, registering again");
        database(config, key)
    };
    let transport = transport(config)?;
    let db = match database(config, key) {
        Err(_) if path.exists() && !force => return Err(refused()),
        Err(_) if path.exists() => {
            return register_new(config, lock, account, key, username, set_aside()?, transport);
        }
        db => db?,
    };
    let loaded = match db.load(None, &key) {
        Err(Error::State) if path.exists() && !force => return Err(refused()),
        Err(Error::State) if path.exists() => {
            drop(db);
            return register_new(config, lock, account, key, username, set_aside()?, transport);
        }
        loaded => loaded?,
    };
    match loaded {
        Loaded::Newer { word, state, records } => {
            let (mut client, _) = Client::from_snapshot(transport, *state, records, false)?;
            client.persist_to(Arc::new(DbStore::new(db, Some(word), None)), key)?;
            configure(&mut client, config)?;
            if !client.is_registering() {
                let name = client.username().unwrap_or("?").to_owned();
                return Err(Failure::new(format!("already registered as {name}")));
            }
            client.finish_registration()?;
            Ok(Open::Opened(Box::new(Opened { client, _lock: lock })))
        }
        _ => register_new(config, lock, account, key, username, db, transport),
    }
}

fn register_new(
    config: &Config,
    lock: std::fs::File,
    account: Option<[u8; 32]>,
    key: [u8; 32],
    username: &str,
    db: Arc<StateDb>,
    transport: Arc<dyn Transport>,
) -> Result<Open, Failure> {
    let store = Arc::new(DbStore::new(db, None, None));
    let mut client =
        Client::register_username_saved(transport, store, key, username, account, None, None, Push::none())?;
    configure(&mut client, config)?;
    Ok(Open::Opened(Box::new(Opened { client, _lock: lock })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transport error names neither the hub nor the path: a blob's id lets anyone fetch and acknowledge it.
    #[test]
    fn a_transport_error_carries_no_url() {
        let config = Config {
            state: PathBuf::new(),
            hub: Some("http://127.0.0.1:1".into()),
            hub_keys: Vec::new(),
            trust_roots: Vec::new(),
            zk_params: None,
        };
        let Err(error) = transport(&config).unwrap().request("GET", "/v1/blobs/SECRETID", &[], &[]) else {
            panic!("port 1 answered")
        };
        assert!(!error.contains("SECRETID") && !error.contains("127.0.0.1"), "{error}");
    }
}
