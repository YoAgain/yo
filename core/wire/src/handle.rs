//! Handles (spec 3): how people find an account. A username, or the user id itself. Handles point
//! to a user id at the hub; they are never part of a certificate, so they can change without touching any group.

use crate::{Error, UserId, USER_ID_LEN};
use tls_codec::{TlsDeserializeBytes, TlsSerialize, TlsSize, VLBytes};
use unicode_normalization::UnicodeNormalization;
use unicode_properties::{EmojiStatus, GeneralCategory, UnicodeEmoji, UnicodeGeneralCategory};
use unicode_segmentation::UnicodeSegmentation;

pub const KIND_USER: u8 = 0;
// Kind 1 was a phone number: no longer made, never to be reused (a hub may still hold such rows).
pub const KIND_USERNAME: u8 = 2;
/// An account key: asks the hub whose it is (to learn who started a chat).
pub const KIND_ACCOUNT_KEY: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, TlsSerialize, TlsDeserializeBytes, TlsSize)]
pub struct Handle {
    pub kind: u8,
    /// User id: 16 bytes. Username: as written (NFC), "YoGPT"; it names the account whose name
    /// has the same skeleton ("yogpt", "Y0GPT" find "YoGPT").
    pub value: VLBytes,
}

impl Handle {
    pub fn user(id: &UserId) -> Self {
        Handle { kind: KIND_USER, value: id.to_vec().into() }
    }

    pub fn account_key(key: &[u8; 32]) -> Self {
        Handle { kind: KIND_ACCOUNT_KEY, value: key.to_vec().into() }
    }

    pub fn username(name: &str) -> Self {
        Handle { kind: KIND_USERNAME, value: name.as_bytes().to_vec().into() }
    }

    /// Reads what a person typed: 32 hex digits a user id (as the apps hand them out), anything else a username.
    pub fn parse(input: &str) -> Result<Self, Error> {
        let trimmed = input.trim();
        if trimmed.len() == 2 * USER_ID_LEN && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
            let id: Vec<u8> =
                (0..USER_ID_LEN).map(|i| u8::from_str_radix(&trimmed[2 * i..2 * i + 2], 16).unwrap()).collect();
            return Ok(Handle { kind: KIND_USER, value: id.into() });
        }
        // NFC only: what is typed must be a name as it is, never one trimmed into another.
        let name: String = trimmed.trim_start_matches('@').nfc().collect();
        check_username_form(&name)?;
        Ok(Handle::username(&name))
    }

    /// A handle as it may travel: well-formed, a username in NFC.
    pub fn check(&self) -> Result<(), Error> {
        let value = self.value.as_slice();
        match self.kind {
            KIND_USER if value.len() == USER_ID_LEN => Ok(()),
            KIND_ACCOUNT_KEY if value.len() == crate::KEY_LEN => Ok(()),
            KIND_USERNAME => check_username_form(std::str::from_utf8(value).map_err(|_| Error::Format)?),
            _ => Err(Error::Format),
        }
    }

    pub fn user_id(&self) -> Option<UserId> {
        (self.kind == KIND_USER).then(|| self.value.as_slice().try_into().ok()).flatten()
    }

    /// For people: "Chris".
    pub fn display(&self) -> String {
        let value = String::from_utf8_lossy(self.value.as_slice());
        match self.kind {
            KIND_USERNAME => value.into_owned(),
            _ => crate::b64(self.value.as_slice()),
        }
    }
}

/// A username as it may be written (spec 3.3): 3 to 16 characters (grapheme clusters, NFC) of Latin letters with their
/// accents, 0–9, `_` and emoji, starting with a letter or an emoji, in any case ("YoGPT"). What the hub refuses with
/// `400` (spec 6.3). Which names are reserved only the hub knows.
pub fn check_username_form(name: &str) -> Result<(), Error> {
    let graphemes: Vec<&str> = name.graphemes(true).collect();
    let ok = (3..=16).contains(&graphemes.len())
        && name.nfc().eq(name.chars())
        && graphemes.iter().all(|g| allowed(g))
        && graphemes.first().is_some_and(|g| is_latin_letter(g) || is_emoji(g));
    ok.then_some(()).ok_or(Error::Format)
}

/// Whether two usernames are one name: the same skeleton.
pub fn same_name(a: &str, b: &str) -> bool {
    skeleton(a) == skeleton(b)
}

/// What a person typed, as a username can be written: NFC, only the allowed characters, a letter or an emoji first,
/// at most 16, the case as typed. The apps keep their field to this; the hub accepts only names already so.
pub fn canonical(typed: &str) -> String {
    let nfc: String = typed.nfc().collect();
    nfc.graphemes(true).filter(|g| allowed(g)).skip_while(|g| !is_latin_letter(g) && !is_emoji(g)).take(16).collect()
}

/// What a username is compared by (spec 3.3): upper case, NFKD, without nonspacing marks, emoji variation selectors
/// and skin tones, then 0 → O, 1 → I, VV → W. Names that look alike have one skeleton, and the hub keeps one name
/// per skeleton: Leo, LE0 and LÉO are one name, and so are 👍 and 👍🏿.
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
        assert_eq!(Handle::parse("@chris").unwrap(), Handle::username("chris"));
        assert_eq!(Handle::parse("Yo_Fan99").unwrap(), Handle::username("Yo_Fan99"));
        assert_eq!(Handle::parse("le\u{301}o").unwrap(), Handle::username("léo"), "NFC");
        // Reserved names are the hub's to refuse: here they are names like any.
        assert_eq!(Handle::parse("admin").unwrap(), Handle::username("admin"));
        for bad in ["yo", "ab", "this_is_far_too_long_x", "+49 170 1234567", "1abc"] {
            assert!(Handle::parse(bad).is_err(), "{bad}");
        }
    }

    /// The same vectors as the app's (Username in Account.swift): what typing makes, and what names collide.
    #[test]
    fn canonical_and_skeleton() {
        for (typed, name, skel) in [
            ("leo", "leo", "LEO"),
            ("LE0", "LE0", "LEO"),
            ("Léo", "Léo", "LEO"),
            ("ärger", "ärger", "ARGER"),
            ("👍", "👍", "👍"),
            ("👍🏿", "👍🏿", "👍"),
            ("VVALDO", "VVALDO", "WALDO"),
            ("vvaldo", "vvaldo", "WALDO"),
            ("Straße", "Straße", "STRASSE"),
            ("YoGPT", "YoGPT", "YOGPT"),
            ("y0gpt", "y0gpt", "YOGPT"),
        ] {
            assert_eq!(canonical(typed), name, "{typed}");
            assert_eq!(skeleton(name), skel, "{typed}");
        }
        for (typed, name) in [("1abc", "abc"), ("_x🔥", "x🔥"), ("abcабв", "abc"), ("x×y", "xy"), ("🧙‍♂️🔥yo", "🧙‍♂️🔥yo")]
        {
            assert_eq!(canonical(typed), name, "{typed}");
        }
        assert!(check_username_form("🔥🔥🔥").is_ok());
        assert!(check_username_form("ÄRGER").is_ok());
        assert!(check_username_form("Léo").is_ok());
        assert!(check_username_form("abc").is_ok(), "lower case is a name");
        assert!(check_username_form("YoGPT").is_ok());
        assert!(check_username_form("le\u{301}o").is_err(), "not NFC");
        assert!(check_username_form("ABCабв").is_err(), "another script");
        assert!(check_username_form("1ABC").is_err(), "starts with a digit");
        for (a, b) in [("YoGPT", "yogpt"), ("YoGPT", "YOGPT"), ("YoGPT", "Y0GPT"), ("Christopher", "CHRISTOPHER")] {
            assert!(same_name(a, b), "{a} {b}");
        }
        assert!(!same_name("YoGPT", "YoGPS"));
    }

    #[test]
    fn checks_what_travels() {
        assert!(Handle::user(&[1; 16]).check().is_ok());
        assert!(Handle { kind: KIND_USER, value: vec![1; 15].into() }.check().is_err());
        assert!(Handle::username("chris").check().is_ok(), "any case");
        assert!(Handle::username("le\u{301}o").check().is_err(), "not NFC");
        assert!(Handle { kind: 9, value: vec![].into() }.check().is_err());
    }
}
