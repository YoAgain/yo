//! The hub's routes and their bodies (spec 6.3). Bodies are TLS-encoded, `application/octet-stream`.

use crate::handle::Handle;
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
pub const REPORT: &str = "/v1/report";
/// `POST`: feedback on the app, for the operator (spec 6.3).
pub const FEEDBACK: &str = "/v1/feedback";
/// A crash report: no account, no signature (spec 6.4); the body is a `Feedback`.
pub const CRASH: &str = "/v1/crash";
/// The account's storage, sealed (spec 6.9).
pub const STORAGE: &str = "/v1/storage";
/// Photos and videos, encrypted (spec 6.8).
pub const BLOBS: &str = "/v1/blobs";
/// `PUT`, signed: the check of the account's access key and whether strangers may send requests (spec 4.10).
pub const ACCESS_KEY: &str = "/v1/access-key";
/// `GET`, signed: a sender certificate for the signing device (spec 6.1), libsignal's `SenderCertificate`.
pub const CERTIFICATE: &str = "/v1/certificate";
/// The public key of the hub's sender-certificate trust root, 32 bytes (spec 6.1): for tools; apps pin their own copy.
pub const TRUST_ROOT_PATH: &str = "/.well-known/yo-trust-root";

/// `GET /v1/blobs/{id}`: a blob, to whoever names its id.
pub fn blob_path(id: &crate::content::BlobId) -> String {
    format!("{BLOBS}/{}", crate::b64(id))
}

/// `PUT /v1/blobs/{id}?fetches={n}`: stores a blob for `n` fetches.
pub fn blob_put_path(id: &crate::content::BlobId, fetches: u32) -> String {
    format!("{BLOBS}/{}?fetches={fetches}", crate::b64(id))
}

/// The header of a blob upload naming the SHA-256 of its body (base64url): what the signature covers, so the hub
/// checks the request before it takes the body (spec 6.8).
pub const HEADER_BODY_HASH: &str = "yo-body-sha256";
/// The app making a request, `PLATFORM/BUILD` ("ios/202611140619"): kept per device, and a build older than the
/// hub's minimum for its platform is answered `426` (spec 6.3). Not signed; any request may carry it.
pub const HEADER_APP: &str = "yo-app";

/// `POST /v1/inbox/{user_id}`: to every device of a user.
/// The requests that name nobody (sealed deliveries, blobs, crash reports): the only ones a client sends on its
/// anonymous connections; every other request, signed or not (a registration, a backup's fetch), names its device or
/// account and goes on the other ones, so no connection carries both (spec 1).
pub fn anonymous(method: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    (method == "POST" && path.starts_with(&format!("{INBOX}/")) && path != INBOX_ACK)
        || path.starts_with(&format!("{BLOBS}/"))
        || path == CRASH
}

pub fn deliver_path(user: &UserId) -> String {
    format!("{INBOX}/{}", crate::b64(user))
}

/// The header of a delivery naming the devices it was encrypted for (spec 4.4): base64url of `devices::hash`.
pub const HEADER_DEVICES: &str = "yo-devices";
/// On a delivery: the recipient's access key (base64url, 16 bytes: Signal's unidentified access key, derived from its
/// profile key), as its contacts have it; without it, or with another, the delivery is a request (spec 4.10). It
/// names no sender: every contact has the same key.
pub const HEADER_ACCESS: &str = "yo-access";

/// `GET /v1/backup/{id}`: by the user id or by the recovery code's locator (spec 7).
pub fn backup_path(id: &[u8]) -> String {
    format!("{BACKUP}/{}", crate::b64(id))
}

/// `GET /v1/username/{name}`: whether a username is free (spec 6.3). The name's UTF-8 percent-encoded, every byte
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

/// Registers a device with a username (spec 6.4): a new user if the name is free; the user whose current account key
/// this is again, whatever the name (a new phone restored from the backup).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Register {
    /// The username as its owner writes it (NFC): the account's only handle.
    pub username: VLBytes,
    pub device: NewDevice,
}

/// A device as it registers.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct NewDevice {
    pub device_id: DeviceId,
    /// Its public name among the account's devices, what senders encrypt for (spec 3).
    pub device_key: [u8; KEY_LEN],
    /// The account key vouching for `device_key`.
    pub certificate: VLBytes,
    pub auth_key: [u8; KEY_LEN],
    pub push: Push,
    /// The device's registration id in its sessions (spec 4.1).
    pub registration_id: u32,
    /// Its first prekeys: the signed prekey and the last resort Kyber prekey are required.
    pub prekeys: Prekeys,
    /// Proof of possession: the account key's signature over `registration_to_be_signed` (spec 6.4).
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
/// again (spec 6.4), and its username as its owner writes it now (none: given up).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Registered {
    pub user_id: UserId,
    pub username: Option<VLBytes>,
    /// The device's number among the user's devices.
    pub device_id: u8,
}

/// The account's access key as the hub keeps it: `access_check`, never the key; and whether deliveries without it
/// (requests from strangers) are taken at all (spec 4.10).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct AccessKey {
    pub check: [u8; 32],
    pub requests_open: u8,
    /// The profile key's commitment (zkgroup's `ProfileKeyCommitment`, bincode): what the hub issues profile key
    /// credentials against (spec 4.9), so a contact holding the key can add this account to a group.
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

/// A prekey signed by the account's identity key (spec 4.1): an EC key or a Kyber key, as libsignal serializes it.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct SignedPrekey {
    pub id: u32,
    pub key: VLBytes,
    pub signature: VLBytes,
}

/// A one-time EC prekey (spec 4.1): unsigned, as in Signal.
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

/// What a sender needs to start a session with one device (spec 4.1). Its one-time keys are handed out once.
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

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Lookup {
    pub handle: Handle,
    /// `LOOKUP_PREKEYS` and `LOOKUP_USERNAME`; 0: only who it is.
    pub flags: u8,
    /// Devices whose bundles are not wanted: those the sender has sessions with.
    pub except: Vec<[u8; KEY_LEN]>,
}

/// A device of a user as the hub names it: its key, and its number among the user's devices (1 to 127), which both
/// ends of a session use for its address (spec 4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Device {
    pub key: [u8; KEY_LEN],
    pub id: u8,
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

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct InboxItem {
    pub id: u64,
    /// When the hub took it (ms since 1970): the time its sender certificate is checked at (spec 6.1).
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

/// The length of a backup's locator: HKDF(the recovery code, "yo backup v1 locator") (spec 7).
pub const LOCATOR_LEN: usize = 16;

/// The backup record's version: 2 carries the doors' auth keys. Version 1 had none; the hub keeps no such record
/// (spec 7).
pub const BACKUP_VERSION: u8 = 2;

/// The account's backup record (spec 7): `PUT /v1/backup` stores it, `GET /v1/backup/{id}` hands it out to whoever
/// names its user id or locator and signs the request with one of its auth keys, which only a door gives. The hub can
/// open none of it: K is wrapped by the passkey and by the recovery code, and K seals the rest.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Backup {
    pub version: u8,
    /// K under HKDF(the passkey's PRF output, "yo backup v1 passkey"); empty without a passkey.
    pub by_passkey: VLBytes,
    /// K under HKDF(the recovery code, "yo backup v1 code"); always there.
    pub by_code: VLBytes,
    /// ChaCha20-Poly1305 under K: the account key, the friends, the recovery code.
    pub sealed: VLBytes,
    /// Where the recovery code alone finds the record: `LOCATOR_LEN` bytes, or empty (found by the user id only).
    pub locator: VLBytes,
    /// The Ed25519 key from HKDF(the passkey's PRF output, "yo backup v1 auth passkey"), which signs a fetch: 32
    /// bytes, or empty without a passkey.
    pub auth_passkey: VLBytes,
    /// The Ed25519 key from HKDF(the recovery code, "yo backup v1 auth code"): 32 bytes, always there.
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
        use crate::limits::{MAX_BACKUP_SEALED_LEN, MAX_BACKUP_WRAP_LEN};
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

/// Why a user is reported (spec 6.3).
pub const REPORT_SPAM: u8 = 1;
pub const REPORT_ABUSE: u8 = 2;
pub const REPORT_OTHER: u8 = 3;

/// `POST /v1/report`: a user reported to the operator. No content: the hub has none (spec 1).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Report {
    pub user_id: UserId,
    /// `REPORT_SPAM`, `REPORT_ABUSE` or `REPORT_OTHER`.
    pub reason: u8,
}

/// `POST /v1/feedback`: what a user writes to the operator about the app, and the app's version. Plain text: the
/// hub reads it (spec 1).
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
        use crate::limits::{MAX_APP_VERSION_LEN, MAX_FEEDBACK_LEN};
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

/// The account's storage (spec 6.9): `GET /v1/storage` answers it, `PUT /v1/storage` replaces it. Sealed by the
/// client under a key the hub never has. `version` is the hub's: in a `PUT` the version it replaces (0: none yet),
/// which must be the hub's current one, else `409` with the current one; in an answer, the one stored.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Storage {
    pub version: u64,
    /// At most `MAX_STORAGE_LEN` bytes.
    pub data: VLBytes,
}

/// The body of a `409` to a delivery: the user's current account key and devices (spec 4.4).
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

// ---- groups (spec 4.9): Signal's Groups v2, zkgroup ----

/// `GET`, signed: today's auth credential (`GroupAuthCredential`), what a member presents to the hub for a group.
pub const GROUP_AUTH: &str = "/v1/group-auth";
/// `POST`, signed: a profile key credential for a user whose profile key the asker holds (`ProfileCredentialRequest`
/// → zkgroup's `ExpiringProfileKeyCredentialResponse`): with it a member adds that user.
pub const GROUP_CREDENTIAL: &str = "/v1/group-credential";
/// `PUT /v1/groups/{id}` makes a group (`NewGroup`), `POST` fetches or changes it (`GroupRequest`); both answer
/// `GroupAnswer`. `{id}` is the group's identifier (32 bytes, base64url).
pub const GROUPS: &str = "/v1/groups";
/// The hub's `ServerPublicParams` (zkgroup, bincode): for tools; apps pin their own copy.
pub const ZK_PARAMS_PATH: &str = "/.well-known/yo-zk-params";
/// On a delivery: a group send token (zkgroup's `GroupSendFullToken`, base64url) for the recipient, from a group both
/// are in: a contact's delivery without the recipient's profile key (spec 4.9).
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
        assert!(anonymous("GET", "/v1/blobs/abc") && anonymous("PUT", "/v1/blobs/abc?fetches=2"));
        assert!(!anonymous("POST", INBOX_ACK) && !anonymous("GET", &inbox_path(25)));
        assert!(!anonymous("POST", REGISTER) && !anonymous("GET", "/v1/backup/abc"));
    }

    #[test]
    fn round_trips() {
        let register = Register {
            username: b"Chris".to_vec().into(),
            device: NewDevice {
                device_id: [7; 16],
                device_key: [8; 32],
                certificate: vec![1; 96].into(),
                auth_key: [2; 32],
                push: Push { platform: PUSH_APNS, token: b"abc".to_vec().into() },
                registration_id: 77,
                prekeys: Prekeys {
                    signed: Some(SignedPrekey { id: 1, key: vec![3; 33].into(), signature: vec![4; 64].into() }),
                    last_resort: Some(SignedPrekey { id: 2, key: vec![5; 1569].into(), signature: vec![6; 64].into() }),
                    one_time: vec![Prekey { id: 3, key: vec![7; 33].into() }],
                    kyber: Vec::new(),
                },
                proof: [6; 64],
            },
        };
        assert_eq!(crate::decode::<Register>(&crate::encode(&register)).unwrap(), register);
        let lookup = Lookup { handle: Handle::username("CHRIS"), flags: LOOKUP_PREKEYS, except: vec![[1; 32]] };
        assert_eq!(crate::decode::<Lookup>(&crate::encode(&lookup)).unwrap(), lookup);
        let inbox =
            Inbox { prekeys_left: 7, items: vec![InboxItem { id: 1, received_ms: 5, envelope: vec![0; 1024].into() }] };
        assert_eq!(crate::decode::<Inbox>(&crate::encode(&inbox)).unwrap(), inbox);
        let backup = backup();
        assert_eq!(crate::decode::<Backup>(&crate::encode(&backup)).unwrap(), backup);
    }

    #[test]
    fn usernames_are_percent_encoded_in_paths() {
        assert_eq!(username_path("LEO_1"), "/v1/username/LEO_1");
        assert_eq!(username_path("JOSÉ"), "/v1/username/JOS%C3%89");
        assert_eq!(username_path("🔥/?"), "/v1/username/%F0%9F%94%A5%2F%3F");
    }

    fn backup() -> Backup {
        Backup::new(vec![1; 60], vec![2; 60], vec![3; 1000], vec![4; LOCATOR_LEN], Some([5; 32]), [6; 32])
    }

    #[test]
    fn backups_are_checked() {
        use crate::limits::{MAX_BACKUP_SEALED_LEN, MAX_BACKUP_WRAP_LEN, MAX_BODY};
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
        assert_eq!(Backup { version: 1, ..backup() }.check(), Err(Error::Unsupported));
        assert_eq!(Backup { version: 3, ..backup() }.check(), Err(Error::Unsupported));
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

    #[test]
    fn paths() {
        assert_eq!(inbox_path(30), "/v1/inbox?wait=30");
        assert!(deliver_path(&[0; 16]).starts_with("/v1/inbox/AAAA"));
        assert_eq!(backup_path(&[0; 16]), "/v1/backup/AAAAAAAAAAAAAAAAAAAAAA");
    }
}
