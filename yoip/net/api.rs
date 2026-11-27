//! The hub's routes and their bodies (spec 4.1). Bodies are TLS-encoded, `application/octet-stream`.

use crate::net::handle::Handle;
use crate::{DeviceId, UserId, KEY_LEN};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};

pub const USERNAME: &str = "/v1/username";
pub const REGISTER: &str = "/v1/register";
pub const PREKEYS: &str = "/v1/prekeys";
pub const LOOKUP: &str = "/v1/lookup";
pub const PUSH: &str = "/v1/push";
pub const INBOX: &str = "/v1/inbox";
pub const INBOX_ACK: &str = "/v1/inbox/ack";
pub const BACKUP: &str = "/v1/backup";
/// `DELETE`: the signing device's account, with everything the hub holds about it.
pub const ACCOUNT: &str = "/v1/account";
/// Help (spec 4): everything a phone tells the operator about the app, by a random install id
/// (`HEADER_INSTALL`), never by the account, never signed. `GET`: the support thread; `POST`: a message to it.
pub const HELP_MESSAGES: &str = "/v1/help/messages";
/// `POST`, `SupportRead`: the operator's answers read, up to an id.
pub const HELP_READ: &str = "/v1/help/read";
/// `POST`, `Report`: a user reported to the operator.
pub const HELP_REPORT: &str = "/v1/help/report";
/// `POST`, a JSON object of strings: a crash report.
pub const HELP_CRASH: &str = "/v1/help/crash";
/// `POST`, a JSON object of strings: an error report.
pub const HELP_ERROR: &str = "/v1/help/error";
/// The header of every help request: the install id, 32 lowercase hex digits (128 random bits) made on the phone
/// and kept only there; sent on no other request, never with the account (spec 1, 4).
pub const HEADER_INSTALL: &str = "yo-install";

/// Whether a text is an install id: 32 lowercase hex digits.
pub fn install_valid(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}
/// The account's storage, sealed (spec 4.3).
pub const STORAGE: &str = "/v1/storage";
/// Photos and videos, encrypted (spec 4.3).
pub const BLOBS: &str = "/v1/blobs";
/// `PUT`, signed: the check of the account's access key and whether strangers may send requests (spec 3.3).
pub const ACCESS_KEY: &str = "/v1/access-key";
/// `PUT`, signed, one byte: whether the account is a bot (1) or not (0), for everyone who looks it up.
pub const BOT: &str = "/v1/bot";
/// Linking a device (spec 2.5). `POST`, anonymous: a link id (16 random bytes, base64url, the body), one use, open
/// `LINK_MS`. `PUT {LINK}/{id}`, signed: the sealed `LinkBundle` (`204`; `404` no such link open, `409` one there).
/// `GET {LINK}/{id}?wait=S` (≤ `MAX_WAIT_S`), anonymous: the bundle once, deleted as it goes (`200`), `204` none yet,
/// `404` no such link open. Only on a hub that adds devices.
pub const LINK: &str = "/v1/link";
/// How long a link stays open (spec 2.5): 10 minutes.
pub const LINK_MS: u64 = 10 * 60 * 1000;
/// The largest sealed bundle a link takes.
pub const MAX_LINK_BUNDLE: usize = 1024;

/// The path of link `id` (base64url).
pub fn link_path(id: &str) -> String {
    format!("{LINK}/{id}")
}

/// What the old device hands the new one (spec 2.5), sealed with HPKE to the new device's one-time key, the link
/// id (its 16 bytes) as aad: the username, the account's secret and the storage key.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct LinkBundle {
    pub username: VLBytes,
    pub account_secret: [u8; 32],
    pub storage_key: [u8; 32],
}

/// `GET`, signed: this account's devices (`OwnDevices`, multi-device). `DELETE {DEVICES}/{number}`, signed: removes
/// one of them; its own requests answer `410` from then on (`404`: no such number, `400`: the signing device itself).
pub const DEVICES: &str = "/v1/devices";
/// `GET`, signed: a sender certificate for the signing device (spec 4.1), libsignal's `SenderCertificate`.
pub const CERTIFICATE: &str = "/v1/certificate";
/// The public key of the hub's sender-certificate trust root, 32 bytes (spec 4.1): for tools; apps pin their own copy.
pub const TRUST_ROOT_PATH: &str = "/.well-known/yo-trust-root";

/// `GET /v1/blobs/{id}`: a blob, to whoever names its id.
pub fn blob_path(id: &crate::protocol::content::BlobId) -> String {
    format!("{BLOBS}/{}", crate::b64(id))
}

/// `PUT /v1/blobs/{id}?fetches={n}`: stores a blob for `n` fetches.
pub fn blob_put_path(id: &crate::protocol::content::BlobId, fetches: u32) -> String {
    format!("{BLOBS}/{}?fetches={fetches}", crate::b64(id))
}

/// `POST`: a request wrapped to the hub's OHTTP key (spec 4.1, RFC 9458), answered wrapped.
pub const OHTTP: &str = "/v1/ohttp";
/// What this hub offers, as JSON: `{"link": true}` when it adds devices (`YO_ADD_DEVICES`, spec 2.5).
pub const FEATURES_PATH: &str = "/.well-known/yo-hub";

/// `GET`: the hub's OHTTP key configs (`application/ohttp-keys`), current and spare: for tools; apps pin their own.
pub const OHTTP_KEYS_PATH: &str = "/.well-known/yo-ohttp";
pub const OHTTP_REQUEST_TYPE: &str = "message/ohttp-req";
pub const OHTTP_RESPONSE_TYPE: &str = "message/ohttp-res";
pub const OHTTP_KEYS_TYPE: &str = "application/ohttp-keys";
/// Ticket transfers (spec 4.1): `PUT` or `GET /v1/blobs/t/{ticket}`, direct.
pub const TICKETS: &str = "/v1/blobs/t";
/// On a blob `PUT` or `GET` inside OHTTP: answer a ticket for it (base64url, 16 bytes), not the transfer.
pub const HEADER_TICKET: &str = "yo-ticket";
/// On a ticket `PUT` inside OHTTP: the blob's length, which the transfer must have.
pub const HEADER_BLOB_LENGTH: &str = "yo-blob-length";

/// `PUT` or `GET /v1/blobs/t/{ticket}`.
pub fn ticket_path(ticket: &str) -> String {
    format!("{TICKETS}/{ticket}")
}

/// The header of a blob upload naming the SHA-256 of its body (base64url): what the signature covers, so the hub
/// checks the request before it takes the body (spec 4.3).
pub const HEADER_BODY_HASH: &str = "yo-body-sha256";
/// The app making a request, `PLATFORM/BUILD` ("ios/202611140619"): kept per device, and a build older than the
/// hub's minimum for its platform is answered `426` (spec 4.1). Not signed; any request may carry it.
pub const HEADER_APP: &str = "yo-app";
/// The latest round trip bucket, inside an anonymous OHTTP request (spec 4.1 Round trip).
pub const HEADER_RTT: &str = "yo-rtt";

/// The requests that name no account (sealed deliveries, blobs, help by install id, groups by their zkgroup proof): the
/// only ones a client sends on its anonymous connections; every other request, signed or not (a registration, a
/// backup's fetch), names its device or account and goes on the other ones, so no connection carries both, and no
/// install id meets an account (spec 1). A group's day's auth credential (`GROUP_AUTH`) is signed: it names the account.
pub fn anonymous(method: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    (method == "POST" && path.starts_with(&format!("{INBOX}/")) && path != INBOX_ACK)
        || path.starts_with(&format!("{BLOBS}/"))
        || path.starts_with("/v1/help/")
        || path.starts_with(&format!("{GROUPS}/"))
        || (method == "POST" && path == GROUP_CREDENTIAL)
}

/// `POST /v1/inbox/{user_id}`: to every device of a user.
pub fn deliver_path(user: &UserId) -> String {
    format!("{INBOX}/{}", crate::b64(user))
}

/// Whether a delivery warrants a visible push (`true` by default for older clients).
pub const HEADER_URGENT: &str = "yo-urgent";
/// The header of a delivery naming the devices it was encrypted for (spec 3.2): base64url of `devices::hash`.
pub const HEADER_DEVICES: &str = "yo-devices";
/// On a delivery whose body is `Copies`, one envelope per device (spec 3.2); without it (an app before multi-device)
/// the body is one envelope for every device of the user.
pub const HEADER_COPIES: &str = "yo-copies";
/// On a delivery: the recipient's access key (base64url, 16 bytes: Signal's unidentified access key, derived from its
/// profile key), as its contacts have it; without it, or with another, the delivery is a request (spec 3.3). It
/// names no sender: every contact has the same key.
pub const HEADER_ACCESS: &str = "yo-access";

/// `GET /v1/backup/{id}`: by the user id or by the recovery code's locator (spec 2.4).
pub fn backup_path(id: &[u8]) -> String {
    format!("{BACKUP}/{}", crate::b64(id))
}

/// `GET /v1/username/{name}`: whether a username is free (spec 2.2). The name's UTF-8 percent-encoded, every byte
/// but the unreserved ones of RFC 3986.
pub fn username_path(name: &str) -> String {
    let mut path = format!("{USERNAME}/");
    for &b in name.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            path.push(b as char);
        } else {
            path.push_str(&format!("%{b:02X}"));
        }
    }
    path
}

/// `GET /v1/inbox?wait={s}`
pub fn inbox_path(wait_s: u64) -> String {
    format!("{INBOX}?wait={wait_s}")
}

/// The push services a device can name.
pub const PUSH_NONE: u8 = 0;
pub const PUSH_APNS: u8 = 1;
pub const PUSH_APNS_SANDBOX: u8 = 2;
pub const PUSH_FCM: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Push {
    pub platform: u8,
    pub token: VLBytes,
}

impl Push {
    pub fn none() -> Self {
        Push { platform: PUSH_NONE, token: Vec::new().into() }
    }
}

/// Registers a device with a username (spec 4.2): a new user if the name is free; the user whose current account key
/// this is again, whatever the name (a new phone restored from the backup).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Register {
    /// The username as its owner writes it (NFC): the account's only handle.
    pub username: VLBytes,
    /// An attestation over an unsigned challenge (spec 2.2, 4.2): with it a new name may be shorter.
    pub attest: Option<Attest>,
    pub device: NewDevice,
}

/// `Register` as clients before attestation send it: no `attest`.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct RegisterLegacy {
    pub username: VLBytes,
    pub device: NewDevice,
}

/// `Register` naming one of the account's devices (its number) that goes for this one (multi-device, a hub that
/// adds devices): what a new device sends after `409 device_limit`, the owner having chosen.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct RegisterReplacing {
    pub username: VLBytes,
    pub attest: Option<Attest>,
    pub device: NewDevice,
    pub replace: u8,
}

/// The body of `409` for a registration past the account's devices: this, then `OwnDevices` (to choose one).
pub const DEVICE_LIMIT: &[u8] = b"device_limit";

impl Register {
    /// A `Register` of either form: the current one, else the legacy one without `attest`. Decoding is exact,
    /// so a body only ever fits one of them.
    pub fn decode_any(bytes: &[u8]) -> Result<Self, crate::Error> {
        Self::decode_replacing(bytes).map(|(register, _)| register)
    }

    /// `decode_any`, or a `RegisterReplacing` with the number of the device it replaces.
    pub fn decode_replacing(bytes: &[u8]) -> Result<(Self, Option<u8>), crate::Error> {
        if let Ok(register) = crate::decode::<Register>(bytes) {
            return Ok((register, None));
        }
        if let Ok(r) = crate::decode::<RegisterReplacing>(bytes) {
            return Ok((Register { username: r.username, attest: r.attest, device: r.device }, Some(r.replace)));
        }
        let old: RegisterLegacy = crate::decode(bytes)?;
        Ok((Register { username: old.username, attest: None, device: old.device }, None))
    }
}

/// A device as it registers.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct NewDevice {
    pub device_id: DeviceId,
    /// Its public name among the account's devices, what senders encrypt for (spec 2).
    pub device_key: [u8; KEY_LEN],
    /// The account key vouching for `device_key`.
    pub certificate: VLBytes,
    pub auth_key: [u8; KEY_LEN],
    pub push: Push,
    /// The device's registration id in its sessions (spec 3.1).
    pub registration_id: u32,
    /// Its first prekeys: the signed prekey and the last resort Kyber prekey are required.
    pub prekeys: Prekeys,
    /// Proof of possession: the account key's signature over `registration_to_be_signed` (spec 4.2).
    pub proof: [u8; crate::SIGNATURE_LEN],
}

pub const REGISTRATION_LABEL: &[u8] = b"yo register v1";

/// What the account key signs to register a device for a handle: a copied certificate registers no one.
pub fn registration_to_be_signed(
    handle: &Handle,
    device_id: &DeviceId,
    device_key: &[u8; KEY_LEN],
    auth_key: &[u8; KEY_LEN],
) -> Vec<u8> {
    let mut out = REGISTRATION_LABEL.to_vec();
    out.extend_from_slice(&crate::encode(handle));
    out.extend_from_slice(device_id);
    out.extend_from_slice(device_key);
    out.extend_from_slice(auth_key);
    out
}

/// The answer to a registration: the account's user id, the same as before for a user's account key registered
/// again (spec 4.2), and its username as its owner writes it now (none: given up).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Registered {
    pub user_id: UserId,
    pub username: Option<VLBytes>,
    /// The device's number among the user's devices.
    pub device_id: u8,
}

/// The account's access key as the hub keeps it: `access_check`, never the key; and whether deliveries without it
/// (requests from strangers) are taken at all (spec 3.3).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct AccessKey {
    pub check: [u8; 32],
    pub requests_open: u8,
    /// The profile key's commitment (zkgroup's `ProfileKeyCommitment`, bincode): what the hub issues profile key
    /// credentials against (spec 3.4), so a contact holding the key can add this account to a group.
    pub commitment: VLBytes,
}

/// The length of an access key (Signal's unidentified access key).
pub const ACCESS_KEY_LEN: usize = 16;

/// The check the hub keeps of an access key (`AccessKey::check`): `SHA-256("yo access key" || access key)`.
pub fn access_check(access_key: &[u8; ACCESS_KEY_LEN]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"yo access key");
    hash.update(access_key);
    hash.finalize().into()
}

/// Claims a username for the signing device's account, replacing its old one; `None` gives it up.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct SetUsername {
    pub username: Option<VLBytes>,
}

/// A prekey signed by the account's identity key (spec 3.1): an EC key or a Kyber key, as libsignal serializes it.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct SignedPrekey {
    pub id: u32,
    pub key: VLBytes,
    pub signature: VLBytes,
}

/// A one-time EC prekey (spec 3.1): unsigned, as in Signal.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Prekey {
    pub id: u32,
    pub key: VLBytes,
}

/// A device's prekeys, as it uploads them (`PUT /v1/prekeys`, and in its registration).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Prekeys {
    /// Replaces the device's signed prekey.
    pub signed: Option<SignedPrekey>,
    /// Replaces its last resort Kyber prekey: handed out when no one-time one is left.
    pub last_resort: Option<SignedPrekey>,
    pub one_time: Vec<Prekey>,
    /// One-time Kyber prekeys.
    pub kyber: Vec<SignedPrekey>,
}

/// What a sender needs to start a session with one device (spec 3.1). Its one-time keys are handed out once.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Bundle {
    pub device_key: [u8; KEY_LEN],
    pub registration_id: u32,
    pub signed: SignedPrekey,
    /// A one-time Kyber prekey, or the last resort when none is left.
    pub kyber: SignedPrekey,
    /// A one-time EC prekey, if one is left.
    pub one_time: Option<Prekey>,
}

/// `Lookup::flags`, bit 0: hand out a bundle per device (consumes its one-time keys); without it only who it is.
pub const LOOKUP_PREKEYS: u8 = 1;
/// `Lookup::flags`, bit 1: answer `LookupNamed`, with the user's username, instead of `LookupResult`.
pub const LOOKUP_USERNAME: u8 = 2;
/// `Lookup::flags`, bit 2, with `LOOKUP_USERNAME`: answer `LookupBot`.
pub const LOOKUP_BOT: u8 = 4;

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Lookup {
    pub handle: Handle,
    /// `LOOKUP_PREKEYS` and `LOOKUP_USERNAME`; 0: only who it is.
    pub flags: u8,
    /// Devices whose bundles are not wanted: those the sender has sessions with.
    pub except: Vec<[u8; KEY_LEN]>,
}

/// A device of a user as the hub names it: its key, and its number among the user's devices (1 to 127), which both
/// ends of a session use for its address (spec 3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Device {
    pub key: [u8; KEY_LEN],
    pub id: u8,
}

/// One of the account's devices as `GET /v1/devices` lists it: its number, when it registered and was last seen
/// (ms, kept to the hour), and the app it named (`Yo-App`, empty if none).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct OwnDevice {
    pub number: u8,
    pub created_at: u64,
    pub last_seen_at: u64,
    pub app: VLBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct OwnDevices {
    pub devices: Vec<OwnDevice>,
}

/// The path that removes device `number` (`DELETE`, signed).
pub fn device_path(number: u8) -> String {
    format!("{DEVICES}/{number}")
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct LookupResult {
    pub user_id: UserId,
    pub account_key: [u8; KEY_LEN],
    pub devices: Vec<Device>,
    pub bundles: Vec<Bundle>,
}

/// The answer to a lookup with `LOOKUP_USERNAME`: the user and its username as its owner writes it ("YoGPT"),
/// whatever spelling found it; none for an account without one.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct LookupNamed {
    pub found: LookupResult,
    pub username: Option<VLBytes>,
}

/// The answer to a lookup with `LOOKUP_BOT` too: whether the account declares itself a bot.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct LookupBot {
    pub named: LookupNamed,
    pub bot: u8,
}

impl LookupBot {
    /// Also of a hub that does not know bots yet.
    pub fn read(answer: &[u8]) -> Result<LookupBot, crate::Error> {
        crate::decode(answer).or_else(|_| Ok(LookupBot { named: crate::decode(answer)?, bot: 0 }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct InboxItem {
    pub id: u64,
    /// When the hub took it (ms since 1970): the time its sender certificate is checked at (spec 4.1).
    pub received_ms: u64,
    pub envelope: VLBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Inbox {
    /// One-time prekeys the device still has at the hub: the fewer of its EC and its Kyber ones.
    pub prekeys_left: u32,
    pub items: Vec<InboxItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Ack {
    pub ids: Vec<u64>,
}

/// The length of a backup's locator: HKDF(the recovery key's backup key, "yo backup v3 locator") (spec 2.4).
pub const LOCATOR_LEN: usize = 16;

/// The backup record's version: 3 is opened by the recovery key (libsignal's account entropy pool), its doors' auth
/// keys Curve25519. Records of before (12 words, Ed25519) are gone from the hub (spec 2.4).
pub const BACKUP_VERSION: u8 = 3;

/// The account's backup record (spec 2.4): `PUT /v1/backup` stores it, `GET /v1/backup/{id}` hands it out to whoever
/// names its user id or locator and signs the request with one of its auth keys, which only a door gives. The hub can
/// open none of it: K is wrapped by the passkey and by the recovery key, and K seals the rest.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Backup {
    pub version: u8,
    /// K under HKDF(the passkey's PRF output, "yo backup v3 passkey"); empty without a passkey.
    pub by_passkey: VLBytes,
    /// K under HKDF(the recovery key's backup key, "yo backup v3 code"); always there.
    pub by_code: VLBytes,
    /// ChaCha20-Poly1305 under K: the account key, the username, the user id, the recovery key.
    pub sealed: VLBytes,
    /// Where the recovery key alone finds the record: `LOCATOR_LEN` bytes, or empty (found by the user id only).
    pub locator: VLBytes,
    /// The Curve25519 key from HKDF(the passkey's PRF output, "yo backup v3 auth passkey"), which signs a fetch
    /// (XEdDSA): 32 bytes, or empty without a passkey.
    pub auth_passkey: VLBytes,
    /// The Curve25519 key from HKDF(the recovery key's backup key, "yo backup v3 auth code"): 32 bytes, always there.
    pub auth_code: VLBytes,
}

impl Backup {
    /// A record of this version.
    pub fn new(
        by_passkey: Vec<u8>,
        by_code: Vec<u8>,
        sealed: Vec<u8>,
        locator: Vec<u8>,
        auth_passkey: Option<[u8; KEY_LEN]>,
        auth_code: [u8; KEY_LEN],
    ) -> Self {
        Backup {
            version: BACKUP_VERSION,
            by_passkey: by_passkey.into(),
            by_code: by_code.into(),
            sealed: sealed.into(),
            locator: locator.into(),
            auth_passkey: auth_passkey.map(|k| k.to_vec()).unwrap_or_default().into(),
            auth_code: auth_code.to_vec().into(),
        }
    }

    /// A record as it may be stored: this version, the code's door and auth key there, every part within its limit.
    pub fn check(&self) -> Result<(), crate::Error> {
        if self.version != BACKUP_VERSION {
            return Err(crate::Error::Unsupported);
        }
        use crate::net::limits::{MAX_BACKUP_SEALED_LEN, MAX_BACKUP_WRAP_LEN};
        let (passkey, code, sealed) = (self.by_passkey.as_slice(), self.by_code.as_slice(), self.sealed.as_slice());
        if passkey.len() > MAX_BACKUP_WRAP_LEN
            || code.len() > MAX_BACKUP_WRAP_LEN
            || sealed.len() > MAX_BACKUP_SEALED_LEN
        {
            return Err(crate::Error::Size);
        }
        let (locator, auth_passkey) = (self.locator.as_slice().len(), self.auth_passkey.as_slice().len());
        if code.is_empty()
            || sealed.is_empty()
            || !(locator == 0 || locator == LOCATOR_LEN)
            || self.auth_code.as_slice().len() != KEY_LEN
            || !(auth_passkey == 0 || auth_passkey == KEY_LEN)
        {
            return Err(crate::Error::Format);
        }
        Ok(())
    }

    /// The auth keys that may sign this record's fetch: the passkey's (if there) and the code's.
    pub fn auth_keys(&self) -> (Option<[u8; KEY_LEN]>, Option<[u8; KEY_LEN]>) {
        (self.auth_passkey.as_slice().try_into().ok(), self.auth_code.as_slice().try_into().ok())
    }

    /// The locator, if the record has one.
    pub fn locator(&self) -> Option<[u8; LOCATOR_LEN]> {
        self.locator.as_slice().try_into().ok()
    }
}

/// Why a user is reported (spec 4).
pub const REPORT_SPAM: u8 = 1;
pub const REPORT_ABUSE: u8 = 2;
pub const REPORT_OTHER: u8 = 3;

/// `POST /v1/help/report`: a user reported to the operator. No content: the hub has none (spec 1).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Report {
    pub user_id: UserId,
    /// `REPORT_SPAM`, `REPORT_ABUSE` or `REPORT_OTHER`.
    pub reason: u8,
}

/// `POST /v1/help/messages`: what a user writes to the operator about the app, and the app's version. Plain text: the
/// hub reads it (spec 4).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Feedback {
    /// UTF-8, 1 to `MAX_FEEDBACK_LEN` bytes.
    pub text: VLBytes,
    /// UTF-8, at most `MAX_APP_VERSION_LEN` bytes: "1.0 (42)".
    pub app_version: VLBytes,
}

impl Feedback {
    /// The text and the version, as the hub takes them: UTF-8, within their lengths, the text not empty.
    pub fn check(&self) -> Result<(&str, &str), crate::Error> {
        use crate::net::limits::{MAX_APP_VERSION_LEN, MAX_FEEDBACK_LEN};
        let (text, version) = (self.text.as_slice(), self.app_version.as_slice());
        if text.len() > MAX_FEEDBACK_LEN || version.len() > MAX_APP_VERSION_LEN {
            return Err(crate::Error::Size);
        }
        match (std::str::from_utf8(text), std::str::from_utf8(version)) {
            (Ok(text), Ok(version)) if !text.is_empty() => Ok((text, version)),
            _ => Err(crate::Error::Format),
        }
    }
}

/// The account's storage (spec 4.3): `GET /v1/storage` answers it, `PUT /v1/storage` replaces it. Sealed by the
/// client under a key the hub never has. `version` is the hub's: in a `PUT` the version it replaces (0: none yet),
/// which must be the hub's current one, else `409` with the current one; in an answer, the one stored.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Storage {
    pub version: u64,
    /// At most `MAX_STORAGE_LEN` bytes.
    pub data: VLBytes,
}

/// A delivery's body (spec 3.2, `HEADER_COPIES`): an envelope for each device of the user, by its number, as Signal's
/// message list. To the sender's own user, every device but the sending one.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Copies {
    pub copies: Vec<CopyFor>,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct CopyFor {
    pub device: u8,
    pub envelope: VLBytes,
}

/// The body of a `409` to a delivery: the user's current account key and devices (spec 3.2).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Devices {
    pub account_key: [u8; KEY_LEN],
    pub devices: Vec<Device>,
}

impl Devices {
    pub fn keys(&self) -> Vec<[u8; KEY_LEN]> {
        self.devices.iter().map(|d| d.key).collect()
    }
}

// ---- groups (spec 3.4): Signal's Groups v2, zkgroup ----

/// `GET`, signed: today's auth credential (`GroupAuthCredential`), what a member presents to the hub for a group.
pub const GROUP_AUTH: &str = "/v1/group-auth";
/// `POST`, anonymous: a profile key credential for a user whose profile key the asker holds (`ProfileCredentialRequest`
/// → zkgroup's `ExpiringProfileKeyCredentialResponse`): with it a member adds that user.
pub const GROUP_CREDENTIAL: &str = "/v1/group-credential";
/// `PUT /v1/groups/{id}` makes a group (`NewGroup`), `POST` fetches or changes it (`GroupRequest`), anonymous (the
/// zkgroup proof in the body); both answer `GroupAnswer`. `{id}` is the group's identifier (32 bytes, base64url).
pub const GROUPS: &str = "/v1/groups";
/// The hub's `ServerPublicParams` (zkgroup, bincode): for tools; apps pin their own copy.
pub const ZK_PARAMS_PATH: &str = "/.well-known/yo-zk-params";
/// On a delivery: a group send token (zkgroup's `GroupSendFullToken`, base64url) for the recipient, from a group both
/// are in: a contact's delivery without the recipient's profile key (spec 3.4).
pub const HEADER_GROUP_SEND: &str = "yo-group-send";
/// The length of a group's identifier at the hub.
pub const GROUP_ID_LEN: usize = 32;

pub fn group_path(id: &[u8; GROUP_ID_LEN]) -> String {
    format!("{GROUPS}/{}", crate::b64(id))
}

/// A day's auth credential (zkgroup's `AuthCredentialWithPniZkcResponse`, bincode), for `redemption_s` (a UTC day's
/// start, seconds).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct GroupAuthCredential {
    pub redemption_s: u64,
    pub credential: VLBytes,
}

/// A profile key credential asked for `user_id` (zkgroup's `ProfileKeyCredentialRequest`, bincode).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct ProfileCredentialRequest {
    pub user_id: UserId,
    pub request: VLBytes,
}

/// A member added: with a profile key credential presentation (zkgroup's `ExpiringProfileKeyCredentialPresentation`)
/// a member whose key the adder holds; without one (empty), invited by its `UuidCiphertext` alone.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct NewMember {
    pub user: VLBytes,
    pub presentation: VLBytes,
}

/// A new group: its `GroupPublicParams`, the maker's auth presentation (`AnyAuthCredentialPresentation`), the
/// encrypted title, the members (the maker among them).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct NewGroup {
    pub public_params: VLBytes,
    pub auth: VLBytes,
    pub title: VLBytes,
    pub members: Vec<NewMember>,
}

pub const ACTION_ADD: u8 = 1;
pub const ACTION_REMOVE: u8 = 2;
pub const ACTION_TITLE: u8 = 3;
pub const ACTION_PROFILE_KEY: u8 = 4;

/// One change: add (`NewMember`), remove (a member's `UuidCiphertext`), title (encrypted), profile key (the asker's own
/// presentation: from invited to a member with its key).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct GroupAction {
    pub kind: u8,
    pub data: VLBytes,
}

/// A member's request for a group: no actions, the group as it is; else the changes as `revision` (the hub's + 1).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct GroupRequest {
    pub auth: VLBytes,
    pub revision: u32,
    pub actions: Vec<GroupAction>,
}

/// A member as the hub keeps it: its `UuidCiphertext`, its `ProfileKeyCiphertext` (empty: invited), who added it.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct GroupMember {
    pub user: VLBytes,
    pub profile_key: VLBytes,
    pub added_by: VLBytes,
}

/// A group as the hub keeps it: everything encrypted under the group's key but the revision.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct GroupState {
    pub revision: u32,
    pub title: VLBytes,
    pub members: Vec<GroupMember>,
}

/// The group, and group send endorsements for its members (`GroupSendEndorsementsResponse`, bincode; empty when the
/// asker is no member any more).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct GroupAnswer {
    pub state: GroupState,
    pub endorsements: VLBytes,
}

/// The seed of the test hubs' `ServerSecretParams` (`ServerSecretParams::generate`): public, never a real hub's.
pub const TEST_ZK_SEED: [u8; 32] = [0x5a; 32];

/// The statuses the hub answers with, and what they mean to a client.
pub mod status {
    pub const OK: u16 = 200;
    pub const CREATED: u16 = 201;
    pub const ACCEPTED: u16 = 202;
    pub const NO_CONTENT: u16 = 204;
    pub const BAD_REQUEST: u16 = 400;
    pub const UNAUTHORIZED: u16 = 401;
    pub const FORBIDDEN: u16 = 403;
    pub const NOT_FOUND: u16 = 404;
    /// Devices changed (a delivery), a name or id taken.
    pub const CONFLICT: u16 = 409;
    /// The mailbox's device was replaced: look the person up again.
    pub const GONE: u16 = 410;
    pub const LENGTH_REQUIRED: u16 = 411;
    pub const TOO_LARGE: u16 = 413;
    /// The app is older than the hub's minimum for its platform (`Yo-App`): it must be updated.
    pub const UPGRADE_REQUIRED: u16 = 426;
    pub const RATE_LIMITED: u16 = 429;
    /// The hub has no room for blobs now: later.
    pub const INSUFFICIENT_STORAGE: u16 = 507;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_requests_naming_nobody_are_anonymous() {
        assert!(anonymous("POST", &deliver_path(&[1; 16])));
        assert!(anonymous("DELETE", "/v1/blobs/abc"));
        assert!(anonymous("GET", "/v1/blobs/abc") && anonymous("PUT", "/v1/blobs/abc?fetches=2"));
        assert!(!anonymous("POST", INBOX_ACK) && !anonymous("GET", &inbox_path(25)));
        assert!(!anonymous("POST", REGISTER) && !anonymous("GET", "/v1/backup/abc"));
        for path in [HELP_MESSAGES, HELP_READ, HELP_REPORT, HELP_CRASH, HELP_ERROR] {
            assert!(anonymous("POST", path), "{path}");
        }
        assert!(anonymous("GET", HELP_MESSAGES));
        // Groups by their zkgroup proof (spec 3.4); the day's auth credential names the account.
        assert!(
            anonymous("PUT", &group_path(&[1; GROUP_ID_LEN])) && anonymous("POST", &group_path(&[1; GROUP_ID_LEN]))
        );
        assert!(anonymous("POST", GROUP_CREDENTIAL) && !anonymous("GET", GROUP_AUTH));
    }

    #[test]
    fn install_ids_are_32_lowercase_hex_digits() {
        assert!(install_valid(&"0a".repeat(16)));
        assert!(!install_valid(&"0A".repeat(16)) && !install_valid(&"0".repeat(31)) && !install_valid(&"g".repeat(32)));
    }

    #[test]
    fn usernames_are_percent_encoded_in_paths() {
        assert_eq!(username_path("LEO_1"), "/v1/username/LEO_1");
        assert_eq!(username_path("JOSÉ"), "/v1/username/JOS%C3%89");
        assert_eq!(username_path("🔥/?"), "/v1/username/%F0%9F%94%A5%2F%3F");
        assert_eq!(inbox_path(30), "/v1/inbox?wait=30");
        assert!(deliver_path(&[0; 16]).starts_with("/v1/inbox/AAAA"));
        assert_eq!(backup_path(&[0; 16]), "/v1/backup/AAAAAAAAAAAAAAAAAAAAAA");
    }

    fn backup() -> Backup {
        Backup::new(vec![1; 60], vec![2; 60], vec![3; 1000], vec![4; LOCATOR_LEN], Some([5; 32]), [6; 32])
    }

    #[test]
    fn backups_are_checked() {
        use crate::net::limits::{MAX_BACKUP_SEALED_LEN, MAX_BACKUP_WRAP_LEN, MAX_BODY};
        use crate::Error;
        assert_eq!(backup().check(), Ok(()));
        assert_eq!(backup().locator(), Some([4; LOCATOR_LEN]));
        assert_eq!(backup().auth_keys(), (Some([5; 32]), Some([6; 32])));
        let without = Backup {
            by_passkey: Vec::new().into(),
            locator: Vec::new().into(),
            auth_passkey: Vec::new().into(),
            ..backup()
        };
        assert_eq!(without.check(), Ok(()));
        assert_eq!(without.locator(), None);
        assert_eq!(Backup { version: 2, ..backup() }.check(), Err(Error::Unsupported));
        assert_eq!(Backup { version: 4, ..backup() }.check(), Err(Error::Unsupported));
        assert_eq!(Backup { auth_code: Vec::new().into(), ..backup() }.check(), Err(Error::Format));
        assert_eq!(Backup { auth_passkey: vec![5; 31].into(), ..backup() }.check(), Err(Error::Format));
        assert_eq!(Backup { by_code: Vec::new().into(), ..backup() }.check(), Err(Error::Format));
        assert_eq!(Backup { sealed: Vec::new().into(), ..backup() }.check(), Err(Error::Format));
        assert_eq!(Backup { locator: vec![4; 15].into(), ..backup() }.check(), Err(Error::Format));
        let wrap = vec![1; MAX_BACKUP_WRAP_LEN + 1];
        assert_eq!(Backup { by_passkey: wrap.clone().into(), ..backup() }.check(), Err(Error::Size));
        assert_eq!(Backup { by_code: wrap.into(), ..backup() }.check(), Err(Error::Size));
        let sealed = vec![3; MAX_BACKUP_SEALED_LEN + 1];
        assert_eq!(Backup { sealed: sealed.into(), ..backup() }.check(), Err(Error::Size));
        // The largest record still fits one request.
        let largest = Backup {
            by_passkey: vec![1; MAX_BACKUP_WRAP_LEN].into(),
            by_code: vec![2; MAX_BACKUP_WRAP_LEN].into(),
            sealed: vec![3; MAX_BACKUP_SEALED_LEN].into(),
            ..backup()
        };
        assert!(largest.check().is_ok() && crate::encode(&largest).len() <= MAX_BODY);
    }
}

/// Support messages are plaintext for the Yo team. Zero read_at means unread.
#[derive(Debug, Clone, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct SupportMessage {
    pub id: u64,
    pub from_team: u8,
    pub text: VLBytes,
    pub created_at: u64,
    pub read_at: u64,
}
#[derive(Debug, Clone, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct SupportThread {
    pub messages: Vec<SupportMessage>,
    pub unread: u32,
}
/// Mark only the displayed messages through this id read.
#[derive(Debug, Clone, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct SupportRead {
    pub through: u64,
}

// ---- device attestation (spec 4.2): measured, never enforced ----

/// `GET`, signed: a fresh challenge for the signing device, single-use, valid `ATTEST_CHALLENGE_TTL_MS`.
pub const ATTEST_CHALLENGE: &str = "/v1/attest/challenge";
/// `POST`, signed, `Attest`: the platform's attestation over the device's last challenge. Answered `AttestResult`;
/// nothing is ever refused for a missing or failed attestation.
pub const ATTEST: &str = "/v1/attest";
/// Header on `GET /v1/username/{name}` and `POST /v1/register`: the unsigned challenge (base64url) the client
/// attests over. The name check answers `HEADER_NAME_MIN` 4 while that challenge is open, and the registration
/// verifies `Register::attest` against it.
pub const HEADER_ATTEST_CHALLENGE: &str = "Yo-Attest-Challenge";
/// Header on name checks: the fewest characters of a new name for this caller (spec 2.2).
pub const HEADER_NAME_MIN: &str = "Yo-Name-Min";
/// How long a challenge may be used.
pub const ATTEST_CHALLENGE_TTL_MS: u64 = 5 * 60 * 1000;
/// The length of a challenge.
pub const ATTEST_CHALLENGE_LEN: usize = 32;
/// The largest attestation payload.
pub const MAX_ATTEST_PAYLOAD: usize = 16 * 1024;
/// `Attest::platform`: an iOS App Attest attestation object (CBOR), made over SHA-256 of the challenge.
pub const ATTEST_IOS: u8 = 1;
/// `Attest::platform`: an Android Play Integrity token of a classic request (JWE, UTF-8), its nonce base64url
/// (unpadded) SHA-256 of the challenge.
pub const ATTEST_ANDROID: u8 = 2;
/// `AttestResult::verdict`: the attestation holds.
pub const ATTEST_ATTESTED: u8 = 1;
/// `AttestResult::verdict`: it does not (the reason stays at the hub).
pub const ATTEST_FAILED: u8 = 2;
/// `AttestResult::verdict`: the hub cannot judge it (no keys for the platform, or no challenge): try again later.
pub const ATTEST_UNJUDGED: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct AttestChallenge {
    /// `ATTEST_CHALLENGE_LEN` random bytes.
    pub challenge: VLBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Attest {
    /// `ATTEST_IOS` or `ATTEST_ANDROID`.
    pub platform: u8,
    /// At most `MAX_ATTEST_PAYLOAD` bytes.
    pub payload: VLBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct AttestResult {
    /// `ATTEST_ATTESTED`, `ATTEST_FAILED` or `ATTEST_UNJUDGED`.
    pub verdict: u8,
}

#[cfg(test)]
mod register_tests {
    use super::*;

    /// An old client's `Register` (no `attest`) still decodes; the current one round-trips with or without it.
    #[test]
    fn register_of_both_forms_decodes() {
        let device = NewDevice {
            device_id: [7; 16],
            device_key: [1; KEY_LEN],
            certificate: vec![2; 3].into(),
            auth_key: [3; KEY_LEN],
            push: Push { platform: 0, token: vec![].into() },
            registration_id: 9,
            prekeys: Prekeys { signed: None, last_resort: None, one_time: vec![], kyber: vec![] },
            proof: [4; 64],
        };
        let old = RegisterLegacy { username: b"Abcde".to_vec().into(), device: device.clone() };
        let decoded = Register::decode_any(&crate::encode(&old)).unwrap();
        assert_eq!((decoded.attest, decoded.device), (None, device.clone()));
        for attest in [None, Some(Attest { platform: ATTEST_IOS, payload: vec![5].into() })] {
            let new = Register { username: b"Abcde".to_vec().into(), attest, device: device.clone() };
            assert_eq!(Register::decode_any(&crate::encode(&new)).unwrap(), new);
        }
    }
}
