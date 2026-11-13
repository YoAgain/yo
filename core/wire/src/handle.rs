//! Handles (spec 3): how people find an account. A phone number, a username, or the user id itself. Handles point
//! to a user id at the hub; they are never part of a certificate, so they can change without touching any group.

use crate::{phone, Error, UserId, USER_ID_LEN};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};
use unicode_normalization::UnicodeNormalization;
use unicode_properties::{EmojiStatus, GeneralCategory, UnicodeEmoji, UnicodeGeneralCategory};
use unicode_segmentation::UnicodeSegmentation;

pub const KIND_USER: u8 = 0;
pub const KIND_PHONE: u8 = 1;
pub const KIND_USERNAME: u8 = 2;
/// An account key: asks the hub whose it is (to learn who started a chat).
pub const KIND_ACCOUNT_KEY: u8 = 3;

/// Names nobody may claim, compared by skeleton.
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
        // Upper case and NFC only: what is typed must be a name as it is, never one trimmed into another.
        let name: String = trimmed.trim_start_matches('@').to_uppercase().nfc().collect();
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

/// A username (spec 3.3): 3 to 16 characters (grapheme clusters, NFC) of Latin letters with their accents, 0–9, `_`
/// and emoji, starting with a letter or an emoji, in upper case (the canonical form), not reserved.
pub fn check_username(name: &str) -> Result<(), Error> {
    let graphemes: Vec<&str> = name.graphemes(true).collect();
    let ok = (3..=16).contains(&graphemes.len())
        && name.nfc().eq(name.chars())
        && name.to_uppercase() == name
        && graphemes.iter().all(|g| allowed(g))
        && graphemes.first().is_some_and(|g| is_latin_letter(g) || is_emoji(g))
        && !RESERVED.iter().any(|r| skeleton(r) == skeleton(name));
    ok.then_some(()).ok_or(Error::Format)
}

/// What a person typed, as a username can be: upper case, NFC, only the allowed characters, a letter or an emoji
/// first, at most 16. The apps keep their field to this; the hub accepts only names that are already canonical.
pub fn canonical(typed: &str) -> String {
    let upper: String = typed.to_uppercase().nfc().collect();
    upper.graphemes(true).filter(|g| allowed(g)).skip_while(|g| !is_latin_letter(g) && !is_emoji(g)).take(16).collect()
}

/// What a username is compared by (spec 3.3): upper case, NFKD, without nonspacing marks, emoji variation selectors
/// and skin tones, then 0 → O, 1 → I, VV → W. Names that look alike have one skeleton, and the hub keeps one name
/// per skeleton: LEO, LE0 and LÉO are one name, and so are 👍 and 👍🏿.
pub fn skeleton(name: &str) -> String {
    let stripped: String = name
        .to_uppercase()
        .nfkd()
        .filter(|&c| {
            let v = c as u32;
            c.general_category() != GeneralCategory::NonspacingMark
                && v != 0xFE0E
                && v != 0xFE0F
                && !(0x1F3FB..=0x1F3FF).contains(&v)
        })
        .collect();
    stripped.replace('0', "O").replace('1', "I").replace("VV", "W")
}

fn allowed(g: &str) -> bool {
    is_latin_letter(g) || g == "_" || (g.len() == 1 && g.as_bytes()[0].is_ascii_digit()) || is_emoji(g)
}

/// A letter of the Latin script with its accents (Basic Latin, Latin-1, Extended-A/B, Extended Additional).
fn is_latin_letter(g: &str) -> bool {
    g.chars().next().is_some_and(char::is_alphabetic)
        && g.chars().all(|c| {
            let v = c as u32;
            c.is_ascii_alphabetic()
                || (0x300..=0x36F).contains(&v)
                || ((0xC0..=0x24F).contains(&v) && v != 0xD7 && v != 0xF7)
                || (0x1E00..=0x1EFF).contains(&v)
        })
}

/// An emoji as people type it: shown as emoji by default, or a sequence (keycaps, ZWJ, flags, skin tones).
fn is_emoji(g: &str) -> bool {
    let Some(first) = g.chars().next() else { return false };
    let presentation = matches!(
        first.emoji_status(),
        EmojiStatus::EmojiPresentation
            | EmojiStatus::EmojiPresentationAndModifierBase
            | EmojiStatus::EmojiPresentationAndEmojiComponent
            | EmojiStatus::EmojiPresentationAndModifierAndEmojiComponent
    );
    presentation || (first.is_emoji_char() && g.chars().count() > 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_people_type() {
        assert_eq!(Handle::parse("+49 170 1234567").unwrap(), Handle::phone("491701234567"));
        assert_eq!(Handle::parse("@chris").unwrap(), Handle::username("CHRIS"));
        assert_eq!(Handle::parse("Yo_Fan99").unwrap(), Handle::username("YO_FAN99"));
        assert_eq!(Handle::parse("léo").unwrap(), Handle::username("LÉO"));
        for bad in ["yo", "ab", "this_is_far_too_long_x", "admin", "+0123", "Adm1n"] {
            assert!(Handle::parse(bad).is_err(), "{bad}");
        }
    }

    /// The same vectors as the app's (Username in Account.swift): what typing makes, and what names collide.
    #[test]
    fn canonical_and_skeleton() {
        for (typed, name, skel) in [
            ("leo", "LEO", "LEO"),
            ("LE0", "LE0", "LEO"),
            ("Léo", "LÉO", "LEO"),
            ("ärger", "ÄRGER", "ARGER"),
            ("👍", "👍", "👍"),
            ("👍🏿", "👍🏿", "👍"),
            ("VVALDO", "VVALDO", "WALDO"),
            ("Straße", "STRASSE", "STRASSE"),
        ] {
            assert_eq!(canonical(typed), name, "{typed}");
            assert_eq!(skeleton(name), skel, "{typed}");
        }
        for (typed, name) in [("1abc", "ABC"), ("_x🔥", "X🔥"), ("abcабв", "ABC"), ("x×y", "XY"), ("🧙‍♂️🔥yo", "🧙‍♂️🔥YO")]
        {
            assert_eq!(canonical(typed), name, "{typed}");
        }
        assert!(check_username("🔥🔥🔥").is_ok());
        assert!(check_username("ÄRGER").is_ok());
        assert!(check_username("LÉO").is_ok());
        assert!(check_username("abc").is_err(), "not upper case");
        assert!(check_username("ABCабв").is_err(), "another script");
        assert!(check_username("1ABC").is_err(), "starts with a digit");
    }

    #[test]
    fn checks_what_travels() {
        assert!(Handle::user(&[1; 16]).check().is_ok());
        assert!(Handle { kind: KIND_USER, value: vec![1; 15].into() }.check().is_err());
        assert!(Handle::username("chris").check().is_err(), "not canonical");
        assert!(Handle { kind: 9, value: vec![].into() }.check().is_err());
    }
}
