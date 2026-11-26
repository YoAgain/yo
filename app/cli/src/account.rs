//! The account's state directory (the same as YoGPT's before: `account`, `key`, `state.sqlite`, all 0600, in a 0700
//! directory), the hub connection, and opening or registering the account in it.

use crate::Failure;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
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
/// `YO_HUB_KEYS`, `YO_TRUST_ROOTS`, `YO_ZK_PARAMS`; lists comma separated). `YO_RECOVERY_LOOKUPS` only from the
/// environment.
#[derive(Clone)]
pub struct Config {
    pub state: PathBuf,
    pub hub: Option<String>,
    /// The hub's public keys, pinned (spec 4.1): the current one and a spare.
    pub hub_keys: Vec<[u8; 32]>,
    /// The hub's sender-certificate trust roots (spec 4.1): without one no sealed Yo opens.
    pub trust_roots: Vec<[u8; 32]>,
    /// The hub's zkgroup public params (spec 3.4): without them no group.
    pub zk_params: Option<Vec<u8>>,
    /// Roster lookups a day for messages from a replaced phone (`YO_RECOVERY_LOOKUPS`; else the core's 10): a bot the
    /// hub does not limit raises it, or messages from users with new phones wait for days.
    pub recovery_lookups: Option<u32>,
    /// `--direct`: no Oblivious HTTP, every request straight to the hub (spec 4.1); the default wraps them.
    pub direct: bool,
    /// The hub's OHTTP key configs (`--ohttp-key`, `YO_OHTTP_KEYS`): else those the core pins for the hub.
    pub ohttp_keys: Vec<Vec<u8>>,
    /// `YO_SMALL_ENVELOPES=1`: seal to the small first bucket (spec 4.1), once receivers take it.
    pub small_envelopes: bool,
}

/// `$XDG_DATA_HOME/yo`, else `~/.local/share/yo`.
pub fn default_state() -> Option<String> {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    env("XDG_DATA_HOME").map(|d| format!("{d}/yo")).or_else(|| env("HOME").map(|h| format!("{h}/.local/share/yo")))
}

fn key32(what: &str, text: &str) -> Result<[u8; 32], Failure> {
    yoip::unb64(text.trim())
        .ok()
        .and_then(|k| k.try_into().ok())
        .ok_or_else(|| Failure::usage(format!("{what}: 32 bytes, base64url")))
}

/// `--profile NAME` (YO_PROFILE): an account of its own under the default directory, `profiles/NAME`.
pub fn profile_state(name: &str) -> Result<String, Failure> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !ok {
        return Err(Failure::usage("--profile NAME: letters, digits, _ - ."));
    }
    let base = default_state().ok_or_else(|| Failure::usage("no home directory for --profile: --state DIR"))?;
    Ok(format!("{base}/profiles/{name}"))
}

/// The profile a state directory is, if it is one (`…/profiles/NAME`).
pub fn profile_of(state: &Path) -> Option<String> {
    let parent = state.parent()?;
    let name = state.file_name()?.to_string_lossy().into_owned();
    (parent.file_name()? == "profiles").then_some(name)
}

/// `YO_RECOVERY_LOOKUPS`: a number of lookups a day.
fn lookups(text: &str) -> Result<u32, Failure> {
    text.trim().parse().map_err(|_| Failure::usage("YO_RECOVERY_LOOKUPS: a number"))
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
        let mut profile = env("YO_PROFILE");
        let (mut hub_keys, mut trust_roots, mut zk) = (Vec::new(), Vec::new(), env("YO_ZK_PARAMS"));
        let (mut keys_given, mut roots_given) = (false, false);
        let mut i = 0;
        let (mut direct, mut ohttp_keys) = (false, Vec::new());
        let config = |text: &str| yoip::unb64(text.trim()).map_err(|_| Failure::usage("OHTTP key config: base64url"));
        while let Some(flag) = args.get(i).filter(|a| a.starts_with("--")) {
            if flag == "--direct" {
                (direct, i) = (true, i + 1);
                continue;
            }
            let value = args.get(i + 1).ok_or_else(|| Failure::usage(format!("{flag}: a value")))?.clone();
            match flag.as_str() {
                "--state" => state = Some(value),
                "--profile" => profile = Some(value),
                "--hub" => hub = Some(value),
                "--hub-key" => (keys_given, _) = (true, hub_keys.push(key32("--hub-key", &value)?)),
                "--trust-root" => (roots_given, _) = (true, trust_roots.push(key32("--trust-root", &value)?)),
                "--zk-params" => zk = Some(value),
                "--ohttp-key" => ohttp_keys.push(config(&value)?),
                _ => return Err(Failure::usage(format!("unknown flag {flag}"))),
            }
            i += 2;
        }
        let hub = hub.unwrap_or_else(|| crate::pins::DEFAULT_HUB.to_owned());
        let pins = crate::pins::for_hub(&hub);
        if !keys_given {
            hub_keys = list("YO_HUB_KEYS").iter().map(|k| key32("YO_HUB_KEYS", k)).collect::<Result<_, _>>()?;
        }
        if !roots_given {
            trust_roots =
                list("YO_TRUST_ROOTS").iter().map(|k| key32("YO_TRUST_ROOTS", k)).collect::<Result<_, _>>()?;
        }
        // A known hub (live or dev) without pins given: those the apps pin.
        if let Some(pins) = pins {
            if hub_keys.is_empty() {
                hub_keys = pins.hub_keys.iter().map(|k| key32("pin", k)).collect::<Result<_, _>>()?;
            }
            if trust_roots.is_empty() {
                trust_roots = pins.trust_roots.iter().map(|k| key32("pin", k)).collect::<Result<_, _>>()?;
            }
            if zk.is_none() {
                zk = Some(pins.zk_params.to_owned());
            }
        }
        let zk_params = match zk {
            Some(p) => Some(yoip::unb64(p.trim()).map_err(|_| Failure::usage("--zk-params: base64url"))?),
            None => None,
        };
        if ohttp_keys.is_empty() {
            ohttp_keys = list("YO_OHTTP_KEYS").iter().map(|k| config(k)).collect::<Result<_, _>>()?;
        }
        let small_envelopes = env("YO_SMALL_ENVELOPES").is_some_and(|v| v.trim() == "1");
        let recovery_lookups = env("YO_RECOVERY_LOOKUPS").as_deref().map(lookups).transpose()?;
        // --state (YO_STATE) wins over --profile (YO_PROFILE).
        let state = match (state, profile) {
            (Some(state), _) => Some(state),
            (None, Some(name)) => Some(profile_state(&name)?),
            (None, None) => None,
        };
        let state = state
            .or_else(default_state)
            .ok_or_else(|| Failure::usage("no state directory: --state DIR or YO_STATE"))?;
        let hub = Some(hub.trim_end_matches('/').to_owned());
        let config = Config {
            state: state.into(),
            hub,
            hub_keys,
            trust_roots,
            zk_params,
            recovery_lookups,
            direct,
            ohttp_keys,
            small_envelopes,
        };
        Ok((config, &args[i..]))
    }

    pub fn control(&self) -> PathBuf {
        control_path(&self.state, std::env::var_os("XDG_RUNTIME_DIR").as_deref().map(Path::new))
    }
}

/// Long state paths share a short socket name across processes.
fn control_path(state: &Path, runtime: Option<&Path>) -> PathBuf {
    let default = state.join("control.sock");
    let Ok(state) = state.canonicalize() else { return default };
    let canonical = state.join("control.sock");
    if canonical.as_os_str().len() <= 100 {
        return canonical;
    }
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: POSIX geteuid has no arguments or preconditions.
    let uid = unsafe { geteuid() };
    let dir = match runtime.filter(|p| !p.as_os_str().is_empty()) {
        Some(runtime) => runtime.join("yo"),
        None => {
            let Ok(meta) = state.metadata() else { return default };
            std::env::temp_dir().join(format!("yo-{}", meta.uid()))
        }
    };
    if let Err(e) = std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return default;
        }
    }
    let private = dir.symlink_metadata().is_ok_and(|m| m.is_dir() && m.uid() == uid && m.mode() & 0o077 == 0);
    if private {
        let hash = state
            .as_os_str()
            .as_encoded_bytes()
            .iter()
            .fold(0xcbf29ce484222325u64, |hash, b| (hash ^ u64::from(*b)).wrapping_mul(0x100000001b3));
        dir.join(format!("{hash:016x}.sock"))
    } else {
        default
    }
}

/// Signed requests and anonymous ones (sealed deliveries, blob fetches) on separate clients: never one connection for
/// both (spec 1.1). Over OHTTP each line is an `Http` of its own (`yo_core::ohttp::Ohttp`).
struct Http {
    client: reqwest::blocking::Client,
    anonymous: reqwest::blocking::Client,
    base: String,
}

/// One line of OHTTP: its own client, so its connections are its own.
struct Line(Http);

impl Transport for Line {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        self.0.send(&self.0.client, false, method, path, headers, body)
    }
}

impl Transport for Http {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        // Only what names nobody goes anonymously; a registration or a backup's fetch names the account (spec 1).
        let signed = headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("yo-device"));
        let anonymous = !signed && yoip::net::api::anonymous(method, path);
        let client = if anonymous { &self.anonymous } else { &self.client };
        self.send(client, signed, method, path, headers, body)
    }
}

impl Http {
    fn send(
        &self,
        client: &reqwest::blocking::Client,
        signed: bool,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<Response, String> {
        let url = format!("{}{}", self.base, path);
        let mut request = client.request(method.parse().map_err(|_| "method")?, url).body(body.to_vec());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        // Its build, as an app names its own (spec 4.1), on what names the account anyway: never on an anonymous one.
        if signed {
            request = request.header(yoip::net::api::HEADER_APP, format!("yogpt/{BUILD}"));
        }
        // Without the URL: a blob's path is its capability (spec 4.3), never for an error or a log.
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

pub(crate) fn transport(config: &Config) -> Result<Arc<dyn Transport>, Failure> {
    let base = config.hub.clone().ok_or_else(|| Failure::usage("no hub: --hub URL or YO_HUB"))?;
    let client = || {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| Failure::new(format!("http: {e}")))
    };
    if config.direct {
        return Ok(Arc::new(Http { client: client()?, anonymous: client()?, base }));
    }
    // Oblivious HTTP by default (spec 4.1), never falling back to direct requests.
    let keys = match config.ohttp_keys.is_empty() {
        true => yo_core::ohttp::pinned(&base),
        false => config.ohttp_keys.clone(),
    };
    // None pinned for this hub: every request fails (never direct), until `--ohttp-key` or `--direct` is given.
    let line = |http: Http| -> Arc<dyn Transport> { Arc::new(Line(http)) };
    let (signed, anonymous) = (
        line(Http { client: client()?, anonymous: client()?, base: base.clone() }),
        line(Http { client: client()?, anonymous: client()?, base }),
    );
    Ok(Arc::new(Named(yo_core::ohttp::Ohttp::new(signed, anonymous, keys))))
}

/// Its build on a signed request, inside the wrapping, as `Http` does directly.
struct Named(yo_core::ohttp::Ohttp);

impl Transport for Named {
    fn authenticated(&self) -> bool {
        self.0.authenticated()
    }

    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        self.request_headers(method, path, headers, body).map(|(response, _)| response)
    }

    fn request_headers(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(Response, Vec<(String, String)>), String> {
        let mut headers = headers.to_vec();
        if headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("yo-device")) {
            headers.push((yoip::net::api::HEADER_APP.to_owned(), format!("yogpt/{BUILD}")));
        }
        self.0.request_headers(method, path, &headers, body)
    }
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
            getrandom::fill(&mut bytes).map_err(|e| Failure::new(e.to_string()))?;
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
    private_dir(dir).map_err(io)?;
    let Some(lock) = directory_lock(dir)? else { return Ok(None) };
    let account = secret(&dir.join("account"), make)?;
    let Some(key) = secret(&dir.join("key"), make || account.is_some())? else { return Err(Failure::not_registered()) };
    Ok(Some((lock, account, key)))
}

fn directory_lock(dir: &Path) -> Result<Option<std::fs::File>, Failure> {
    let path = dir.join("lock");
    let io = |e| Failure::new(format!("{}: {e}", path.display()));
    let lock =
        std::fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(&path).map_err(io)?;
    match lock.try_lock() {
        // Signout can unlink the lock between open and try_lock.
        Ok(()) if lock.metadata().map_err(io)?.nlink() > 0 => Ok(Some(lock)),
        Ok(()) | Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(io(e)),
    }
}

pub const SIGNOUT_PROMPT: &str = "Sign out on this device? Nothing stays here: the account's files in the state directory are deleted. Have your recovery key.";

pub fn signout(config: &Config) -> Result<(), Failure> {
    if !config.state.try_exists().map_err(|e| Failure::new(e.to_string()))? {
        return Ok(());
    }
    let _lock = directory_lock(&config.state)?.ok_or_else(|| Failure::new("stop yo listen first"))?;
    let remove = |path: &Path| match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Failure::new(format!("{}: {e}", path.display()))),
    };
    let socket = config.control();
    if socket.symlink_metadata().is_ok_and(|m| m.file_type().is_socket()) {
        if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            return Err(Failure::new("stop yo listen first"));
        }
        remove(&socket)?;
    }
    for name in [
        "state.sqlite-wal",
        "state.sqlite-shm",
        "state.sqlite-journal",
        "state.sqlite",
        "state",
        "state.v2",
        "state.old",
        "removed.tmp",
        "removed",
        "recovery.tmp",
        "recovery",
        "key.tmp",
        "key",
        "account.tmp",
        "account",
        "lock",
    ] {
        remove(&config.state.join(name))?;
    }
    Ok(())
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
        let (state, records) = yo_core::migrations::keychain_v1::read_state(&blob, &key).map_err(|e| e.to_string())?;
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
    if let Some(lookups) = config.recovery_lookups {
        client.set_recovery_lookups_per_day(lookups);
    }
    client.set_small_envelopes(config.small_envelopes);
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
/// Saved before it is sent (spec 4.2): a lost answer loses neither key nor prekeys, and `register` again finishes it.
/// A directory with a state it cannot open (no `key`, a wrong one, damaged) is refused without `force`: registering
/// there replaces the account's device and loses its sessions.
pub fn register(config: &Config, username: &str, force: bool) -> Result<Open, Failure> {
    register_with(config, username, force, None)
}

/// Whether the directory holds an account secret already (registered, or under way).
pub fn has_account(config: &Config) -> bool {
    config.state.join("account").exists()
}

/// Puts an existing account's secret (from a backup or a linked device) into a directory that has none yet, before
/// `register_with`: the same account, a new device.
pub fn put_account_secret(config: &Config, secret: &[u8; 32]) -> Result<(), Failure> {
    let dir = &config.state;
    let io = |e: std::io::Error| Failure::new(format!("{}: {e}", dir.display()));
    if has_account(config) {
        return Err(Failure::new("this directory has an account already: another --state DIR"));
    }
    private_dir(dir).map_err(io)?;
    write_private(&dir.join("account"), secret).map_err(io)
}

/// A file of the directory, private (the recovery key).
pub fn write_file(config: &Config, name: &str, bytes: &[u8]) -> Result<(), Failure> {
    write_private(&config.state.join(name), bytes).map_err(|e| Failure::new(format!("{name}: {e}")))
}

pub fn read_file(config: &Config, name: &str) -> Option<Vec<u8>> {
    std::fs::read(config.state.join(name)).ok()
}

/// `register`, with the storage key of an account migrated from the old keys (a linked device's bundle names it).
pub fn register_with(config: &Config, username: &str, force: bool, legacy: Option<[u8; 32]>) -> Result<Open, Failure> {
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
            return register_new(config, lock, account, key, username, set_aside()?, transport, legacy);
        }
        db => db?,
    };
    let loaded = match db.load(None, &key) {
        Err(Error::State) if path.exists() && !force => return Err(refused()),
        Err(Error::State) if path.exists() => {
            drop(db);
            return register_new(config, lock, account, key, username, set_aside()?, transport, legacy);
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
        _ => register_new(config, lock, account, key, username, db, transport, legacy),
    }
}

#[allow(clippy::too_many_arguments)]
fn register_new(
    config: &Config,
    lock: std::fs::File,
    account: Option<[u8; 32]>,
    key: [u8; 32],
    username: &str,
    db: Arc<StateDb>,
    transport: Arc<dyn Transport>,
    legacy: Option<[u8; 32]>,
) -> Result<Open, Failure> {
    let store = Arc::new(DbStore::new(db, None, None));
    let mut client =
        Client::register_username_saved(transport, store, key, username, account, legacy, None, Push::none(), None)?;
    configure(&mut client, config)?;
    Ok(Open::Opened(Box::new(Opened { client, _lock: lock })))
}

/// The state directory: made private (0700) when this makes it; one that exists keeps its mode (a directory made
/// read-only stays so, and writing into it fails).
fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signout_requires_the_lock_and_preserves_unknown_files() {
        let mut config = Config::parse(&[]).unwrap().0;
        config.state = std::env::temp_dir().join(format!("yo-signout-{}", yo_tests::random_u64()));
        std::fs::create_dir(&config.state).unwrap();
        let names = ["account", "key", "state.sqlite", "state.sqlite-wal", "recovery.tmp", "removed", "notes"];
        for name in names {
            std::fs::write(config.state.join(name), b"example").unwrap();
        }
        let lock = directory_lock(&config.state).unwrap().unwrap();
        assert_eq!(signout(&config).unwrap_err().error, "stop yo listen first");
        assert!(names.iter().all(|n| config.state.join(n).exists()));
        drop(lock);
        signout(&config).unwrap();
        signout(&config).unwrap();
        assert_eq!(std::fs::read_dir(&config.state).unwrap().count(), 1);
        assert_eq!(std::fs::read(config.state.join("notes")).unwrap(), b"example");
        std::fs::remove_dir_all(config.state).unwrap();
    }

    #[test]
    fn long_state_uses_a_private_socket_directory() {
        let root = std::env::temp_dir().join(format!("yo-path-{}", yo_tests::random_u64()));
        let state = root.join("long".repeat(30));
        std::fs::create_dir_all(&state).unwrap();
        let dir = root.join("yo");
        let socket = control_path(&state, Some(&root));
        assert_eq!(socket.parent(), Some(dir.as_path()));
        assert_eq!(socket.file_name().unwrap().len(), 21);
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&state, &alias).unwrap();
        assert_eq!(socket, control_path(&alias, Some(&root)));
        assert_eq!(dir.metadata().unwrap().mode() & 0o777, 0o700);
        assert_eq!(socket, control_path(&state.join("../").join(state.file_name().unwrap()), Some(&root)));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert_eq!(control_path(&state, Some(&root)), state.join("control.sock"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_lookups_are_a_number() {
        assert_eq!(lookups(" 2000 ").unwrap(), 2000);
        assert_eq!(lookups("many").unwrap_err().code, 2);
    }

    /// A transport error names neither the hub nor the path: a blob's id lets anyone fetch and acknowledge it.
    #[test]
    fn a_transport_error_carries_no_url() {
        let config = Config {
            state: PathBuf::new(),
            hub: Some("http://127.0.0.1:1".into()),
            hub_keys: Vec::new(),
            trust_roots: Vec::new(),
            zk_params: None,
            recovery_lookups: None,
            direct: true,
            ohttp_keys: Vec::new(),
            small_envelopes: false,
        };
        let Err(error) = transport(&config).unwrap().request("GET", "/v1/blobs/SECRETID", &[], &[]) else {
            panic!("port 1 answered")
        };
        assert!(!error.contains("SECRETID") && !error.contains("127.0.0.1"), "{error}");
    }
}
