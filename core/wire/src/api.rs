//! The hub's routes and their bodies (spec 6.3). Bodies are TLS-encoded, `application/octet-stream`.

use crate::{GroupId, KEY_LEN, TOKEN_LEN};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};

pub const VERIFY_START: &str = "/v1/verify/start";
pub const VERIFY_CHECK: &str = "/v1/verify/check";
pub const KEY_PACKAGES: &str = "/v1/keypackages";
pub const LOOKUP: &str = "/v1/lookup";
pub const PUSH: &str = "/v1/push";
pub const INBOX: &str = "/v1/inbox";
pub const INBOX_ACK: &str = "/v1/inbox/ack";
pub const GROUPS: &str = "/v1/groups";

/// `POST /v1/inbox/{mailbox}`
pub fn deliver_path(mailbox: &crate::Mailbox) -> String {
    format!("{INBOX}/{}", crate::b64(mailbox))
}

/// `POST /v1/groups/{id}/commit`
pub fn commit_path(group: &GroupId) -> String {
    format!("{GROUPS}/{}/commit", crate::b64(group))
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

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct VerifyCheck {
    pub phone: VLBytes,
    pub code: VLBytes,
    /// The device's certificate, signed by the account key it names.
    pub certificate: VLBytes,
    pub auth_key: [u8; KEY_LEN],
    pub push: Push,
    pub key_packages: Vec<VLBytes>,
    pub last_resort: VLBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct KeyPackages {
    pub key_packages: Vec<VLBytes>,
    pub last_resort: Option<VLBytes>,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Lookup {
    pub phone: VLBytes,
    /// 1: hand out one KeyPackage per device (consumes them); 0: only the account key.
    pub key_packages: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct LookupResult {
    pub account_key: [u8; KEY_LEN],
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
}

/// The body of a `409` to a commit: the hub's epoch of the group.
#[derive(Debug, Clone, PartialEq, Eq, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Epoch {
    pub epoch: u64,
}

/// The statuses the hub answers with, and what they mean to a client.
pub mod status {
    pub const OK: u16 = 200;
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
        let check = VerifyCheck {
            phone: b"491701234567".to_vec().into(),
            code: b"123456".to_vec().into(),
            certificate: vec![1; 200].into(),
            auth_key: [2; 32],
            push: Push { platform: PUSH_APNS, token: b"abc".to_vec().into() },
            key_packages: vec![vec![3; 300].into(), vec![4; 300].into()],
            last_resort: vec![5; 300].into(),
        };
        assert_eq!(crate::decode::<VerifyCheck>(&crate::encode(&check)).unwrap(), check);
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
