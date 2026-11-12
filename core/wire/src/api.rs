//! The hub's routes and their bodies (spec 6.3). Bodies are TLS-encoded, `application/octet-stream`.

use crate::handle::Handle;
use crate::{DeviceId, GroupId, UserId, KEY_LEN, TOKEN_LEN};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};

pub const VERIFY_START: &str = "/v1/verify/start";
pub const VERIFY_CHECK: &str = "/v1/verify/check";
pub const USERNAME: &str = "/v1/username";
pub const REGISTER: &str = "/v1/register";
pub const KEY_PACKAGES: &str = "/v1/keypackages";
pub const LOOKUP: &str = "/v1/lookup";
pub const PUSH: &str = "/v1/push";
pub const INBOX: &str = "/v1/inbox";
pub const INBOX_ACK: &str = "/v1/inbox/ack";
pub const GROUPS: &str = "/v1/groups";

/// `POST /v1/inbox/{user_id}`: to every device of a user.
pub fn deliver_path(user: &UserId) -> String {
    format!("{INBOX}/{}", crate::b64(user))
}

/// The header of a delivery naming the devices it was encrypted for (spec 4.6): base64url of `devices::hash`.
pub const HEADER_DEVICES: &str = "yo-devices";

/// `POST /v1/groups/{id}/commit`
pub fn commit_path(group: &GroupId) -> String {
    format!("{GROUPS}/{}/commit", crate::b64(group))
}

/// `GET /v1/groups/{id}/commits/{epoch}`: the commit the hub accepted on that epoch.
pub fn commit_fetch_path(group: &GroupId, epoch: u64) -> String {
    format!("{GROUPS}/{}/commits/{epoch}", crate::b64(group))
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

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct VerifyStart {
    pub phone: VLBytes,
}

/// Registers a device with the code from the SMS: one request.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct VerifyCheck {
    pub phone: VLBytes,
    pub code: VLBytes,
    pub device: NewDevice,
}

/// Registers a device without a number (spec 6.4): only where the hub switches it on.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Register {
    /// The username, canonical: the account's only handle.
    pub username: VLBytes,
    pub device: NewDevice,
}

/// A device as it registers.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct NewDevice {
    pub device_id: DeviceId,
    /// Its MLS signature key: the leaf key of all its KeyPackages.
    pub device_key: [u8; KEY_LEN],
    /// The account key vouching for `device_key`.
    pub certificate: VLBytes,
    pub auth_key: [u8; KEY_LEN],
    /// Proof of possession: the account key's signature over `registration_to_be_signed` (spec 6.4).
    pub push: Push,
    pub key_packages: Vec<VLBytes>,
    pub last_resort: VLBytes,
    pub proof: [u8; crate::SIGNATURE_LEN],
}

pub const REGISTRATION_LABEL: &[u8] = b"yo register v1";

/// What the account key signs to register a device for a handle: copied certificates and KeyPackages register no one.
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

/// The answer to a registration: the account's user id. The same as before when the number had one: a new phone
/// keeps its user id, with a new account key (spec 7).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Registered {
    pub user_id: UserId,
}

/// Claims a username for the signing device's account, replacing its old one; `None` gives it up.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct SetUsername {
    pub username: Option<VLBytes>,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct KeyPackages {
    pub key_packages: Vec<VLBytes>,
    pub last_resort: Option<VLBytes>,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Lookup {
    pub handle: Handle,
    /// 1: hand out one KeyPackage per device (consumes them); 0: only who it is.
    pub key_packages: u8,
    /// Devices whose KeyPackages are not wanted: those already in the group being repaired.
    pub except: Vec<[u8; KEY_LEN]>,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct LookupResult {
    pub user_id: UserId,
    pub account_key: [u8; KEY_LEN],
    /// The user's devices (their MLS signature keys).
    pub device_keys: Vec<[u8; KEY_LEN]>,
    pub key_packages: Vec<VLBytes>,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct InboxItem {
    pub id: u64,
    pub envelope: VLBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Inbox {
    /// Single-use KeyPackages the device still has at the hub.
    pub key_packages_left: u32,
    pub items: Vec<InboxItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Ack {
    pub ids: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct GroupCreate {
    pub group_id: GroupId,
    pub epoch: u64,
    pub token_hash: [u8; TOKEN_LEN],
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Commit {
    /// The epoch the commit builds on.
    pub epoch: u64,
    /// That epoch's token.
    pub token: [u8; TOKEN_LEN],
    /// The hash of the next epoch's token.
    pub next_token_hash: [u8; TOKEN_LEN],
    /// The commit's envelope: the hub keeps it for whoever misses it (spec 4.3).
    pub envelope: VLBytes,
}

/// The body of a `409` to a delivery: the user's current account key and devices (spec 4.6).
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Devices {
    pub account_key: [u8; KEY_LEN],
    pub device_keys: Vec<[u8; KEY_LEN]>,
}

/// The body of a `409` to a commit: the hub's epoch of the group.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Epoch {
    pub epoch: u64,
}

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
    /// A commit built on an epoch the group has left.
    pub const CONFLICT: u16 = 409;
    /// The mailbox's device was replaced: look the person up again.
    pub const GONE: u16 = 410;
    pub const TOO_LARGE: u16 = 413;
    pub const RATE_LIMITED: u16 = 429;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let register = VerifyCheck {
            phone: b"491701234567".to_vec().into(),
            code: b"123456".to_vec().into(),
            device: NewDevice {
                device_id: [7; 16],
                device_key: [8; 32],
                certificate: vec![1; 96].into(),
                auth_key: [2; 32],
                push: Push { platform: PUSH_APNS, token: b"abc".to_vec().into() },
                key_packages: vec![vec![3; 300].into(), vec![4; 300].into()],
                last_resort: vec![5; 300].into(),
                proof: [6; 64],
            },
        };
        assert_eq!(crate::decode::<VerifyCheck>(&crate::encode(&register)).unwrap(), register);
        let lookup = Lookup { handle: Handle::username("CHRIS"), key_packages: 1, except: vec![[1; 32]] };
        assert_eq!(crate::decode::<Lookup>(&crate::encode(&lookup)).unwrap(), lookup);
        let inbox = Inbox { key_packages_left: 7, items: vec![InboxItem { id: 1, envelope: vec![0; 1024].into() }] };
        assert_eq!(crate::decode::<Inbox>(&crate::encode(&inbox)).unwrap(), inbox);
    }

    #[test]
    fn paths() {
        assert_eq!(inbox_path(30), "/v1/inbox?wait=30");
        assert!(deliver_path(&[0; 16]).starts_with("/v1/inbox/AAAA"));
        assert!(commit_path(&[0; 32]).ends_with("/commit"));
    }
}
