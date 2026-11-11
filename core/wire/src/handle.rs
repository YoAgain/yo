//! Handles (spec 3): how people find an account. A phone number, a username, or the user id itself. Handles point
//! to a user id at the hub; they are never part of a certificate, so they can change without touching any group.

use crate::{phone, Error, UserId, USER_ID_LEN};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};

pub const KIND_USER: u8 = 0;
pub const KIND_PHONE: u8 = 1;
pub const KIND_USERNAME: u8 = 2;
/// An account key: asks the hub whose it is (to learn who started a chat).
pub const KIND_ACCOUNT_KEY: u8 = 3;

/// Names nobody may claim.
const RESERVED: [&str; 12] =
    ["YO", "YOAPP", "JUSTYO", "YOAGAIN", "YOLS", "ADMIN", "SUPPORT", "HELP", "ROOT", "SYSTEM", "OFFICIAL", "STAFF"];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Handle {
    pub kind: u8,
    /// User id: 16 bytes. Phone: E.164 digits. Username: its canonical form (upper case).
    pub value: VLBytes,
}

impl Handle {
    pub fn user(id: &UserId) -> Self {
        Handle { kind: KIND_USER, value: id.to_vec().into() }
    }

    pub fn phone(digits: &str) -> Self {
        Handle { kind: KIND_PHONE, value: digits.as_bytes().to_vec().into() }
    }

    pub fn account_key(key: &[u8; 32]) -> Self {
        Handle { kind: KIND_ACCOUNT_KEY, value: key.to_vec().into() }
    }

    pub fn username(name: &str) -> Self {
        Handle { kind: KIND_USERNAME, value: name.as_bytes().to_vec().into() }
    }

    /// Reads what a person typed: "+49 170 …" or "0049 …" is a phone number, 32 hex digits a user id (as the
    /// apps hand them out), anything else a username.
    pub fn parse(input: &str) -> Result<Self, Error> {
        let trimmed = input.trim();
        if trimmed.len() == 2 * USER_ID_LEN && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
            let id: Vec<u8> =
                (0..USER_ID_LEN).map(|i| u8::from_str_radix(&trimmed[2 * i..2 * i + 2], 16).unwrap()).collect();
            return Ok(Handle { kind: KIND_USER, value: id.into() });
        }
        if trimmed.starts_with('+') || trimmed.starts_with("00") {
            return Ok(Handle::phone(&phone::normalize(trimmed)?));
        }
        let name = trimmed.trim_start_matches('@').to_ascii_uppercase();
        check_username(&name)?;
        Ok(Handle::username(&name))
    }

    /// A handle as it may travel: well-formed and canonical.
    pub fn check(&self) -> Result<(), Error> {
        let value = self.value.as_slice();
        match self.kind {
            KIND_USER if value.len() == USER_ID_LEN => Ok(()),
            KIND_ACCOUNT_KEY if value.len() == crate::KEY_LEN => Ok(()),
            KIND_PHONE => phone::check(value),
            KIND_USERNAME => check_username(std::str::from_utf8(value).map_err(|_| Error::Format)?),
            _ => Err(Error::Format),
        }
    }

    pub fn user_id(&self) -> Option<UserId> {
        (self.kind == KIND_USER).then(|| self.value.as_slice().try_into().ok()).flatten()
    }

    /// For people: "+491701234567", "CHRIS".
    pub fn display(&self) -> String {
        let value = String::from_utf8_lossy(self.value.as_slice());
        match self.kind {
            KIND_PHONE => format!("+{value}"),
            KIND_USERNAME => value.into_owned(),
            _ => crate::b64(self.value.as_slice()),
        }
    }
}

/// A username: 3 to 16 of A–Z, 0–9 and _, starting with a letter, in upper case (the canonical form), not reserved.
pub fn check_username(name: &str) -> Result<(), Error> {
    let ok = (3..=16).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && name.as_bytes()[0].is_ascii_uppercase()
        && !RESERVED.contains(&name);
    ok.then_some(()).ok_or(Error::Format)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_people_type() {
        assert_eq!(Handle::parse("+49 170 1234567").unwrap(), Handle::phone("491701234567"));
        assert_eq!(Handle::parse("@chris").unwrap(), Handle::username("CHRIS"));
        assert_eq!(Handle::parse("Yo_Fan99").unwrap(), Handle::username("YO_FAN99"));
        for bad in ["yo", "ab", "9lives", "this_is_far_too_long", "chr is", "admin", "+0123"] {
            assert!(Handle::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn checks_what_travels() {
        assert!(Handle::user(&[1; 16]).check().is_ok());
        assert!(Handle { kind: KIND_USER, value: vec![1; 15].into() }.check().is_err());
        assert!(Handle::username("chris").check().is_err(), "not canonical");
        assert!(Handle { kind: 9, value: vec![].into() }.check().is_err());
    }
}
