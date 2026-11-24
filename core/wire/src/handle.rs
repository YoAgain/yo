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

/// Maximum username length in grapheme clusters.
pub const USERNAME_MAX: usize = 16;
/// Maximum username length in bytes (UTF-8): a character (a letter with marks, an emoji sequence) has no length of
/// its own, a name does.
pub const USERNAME_MAX_BYTES: usize = 64;

/// A username as it may be written (spec 3.3): 1 to 16 characters (grapheme clusters, NFC), at most 64 bytes, of Latin
/// letters with their accents, 0–9, `_` and emoji (RGI sequences), starting with a letter, an emoji or a digit, with at
/// least one letter or emoji ("1abc", not "1234"), in any case ("YoGPT"). What the hub refuses with `400` (spec 6.3).
/// Which names are reserved only the hub knows.
pub fn check_username_form(name: &str) -> Result<(), Error> {
    let graphemes: Vec<&str> = name.graphemes(true).collect();
    let ok = (1..=USERNAME_MAX).contains(&graphemes.len())
        && name.len() <= USERNAME_MAX_BYTES
        && name.nfc().eq(name.chars())
        && graphemes.iter().all(|g| allowed(g))
        && graphemes.first().is_some_and(|g| *g != "_")
        && graphemes.iter().any(|g| is_latin_letter(g) || is_emoji(g));
    ok.then_some(()).ok_or(Error::Format)
}

/// How many characters a username has, as people count them (grapheme clusters): what a hub's least is measured in.
pub fn username_len(name: &str) -> usize {
    name.graphemes(true).count()
}

/// Whether two usernames are one name: the same skeleton.
pub fn same_name(a: &str, b: &str) -> bool {
    skeleton(a) == skeleton(b)
}

/// What a person typed, as a username can be written: NFC, only the allowed characters, no `_` first, at most 16 and 64
/// bytes (whole characters), the case as typed (digits alone stay: the letter may come next). The apps keep their
/// field to this; the hub accepts only names already so.
pub fn canonical(typed: &str) -> String {
    let nfc: String = typed.nfc().collect();
    let mut name = String::new();
    for g in nfc.graphemes(true).filter(|g| allowed(g)).skip_while(|g| *g == "_").take(USERNAME_MAX) {
        if name.len() + g.len() > USERNAME_MAX_BYTES {
            break;
        }
        name.push_str(g);
    }
    name
}

/// What a username is compared by (spec 3.3): upper case, NFKD, without nonspacing marks, emoji variation selectors,
/// skin tones, the zero width joiner (U+200D) and tag characters (U+E0020–E007F), then 0 → O, 1 → I, VV → W. Names
/// that look alike have one skeleton, and the hub keeps one name per skeleton: Leo, LE0 and LÉO are one name, and so
/// are 👍 and 👍🏿, 👍👍 and 👍‍👍 (joined where a font has no such sequence), 🏴 and a flag of tags.
pub fn skeleton(name: &str) -> String {
    let stripped: String = name
        .to_uppercase()
        .nfkd()
        .filter(|&c| {
            let v = c as u32;
            c.general_category() != GeneralCategory::NonspacingMark
                && v != 0xFE0E
                && v != 0xFE0F
                && v != 0x200D
                && !(0xE0020..=0xE007F).contains(&v)
                && !(0x1F3FB..=0x1F3FF).contains(&v)
        })
        .collect();
    stripped.replace('0', "O").replace('1', "I").replace("VV", "W")
}

fn allowed(g: &str) -> bool {
    // Never '#' or '@' in any form (the keycap emoji "#️⃣" too): a group is shown as "#name", a bot may be "@name".
    if g.contains('#') || g.contains('@') {
        return false;
    }
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

/// An emoji as people type it (UTS #51), checked as a whole sequence: a keycap (`1️⃣`), a flag (two regional
/// indicators), a tag sequence (🏴 and tags), or emoji joined by U+200D (`🧙‍♂️`), each one shown as emoji by default
/// or made one by U+FE0F, with a skin tone on those that take one; and one of the RGI emoji (Unicode's list, with
/// its less qualified spellings), so no made-up chain or flag of any length. Never a letter or digit with marks.
fn is_emoji(g: &str) -> bool {
    is_emoji_form(g) && emojis::get(g).is_some()
}

fn is_emoji_form(g: &str) -> bool {
    let chars: Vec<char> = g.chars().collect();
    let regional = |c: &char| ('\u{1F1E6}'..='\u{1F1FF}').contains(c);
    let tag = |c: &char| ('\u{E0020}'..='\u{E007E}').contains(c);
    match chars.as_slice() {
        [a, b] if regional(a) && regional(b) => true,
        [k, '\u{FE0F}', '\u{20E3}'] | [k, '\u{20E3}'] => matches!(k, '0'..='9' | '#' | '*'),
        ['\u{1F3F4}', tags @ .., '\u{E007F}'] if !tags.is_empty() => tags.iter().all(tag),
        _ => {
            let parts: Vec<&[char]> = chars.split(|&c| c == '\u{200D}').collect();
            parts.iter().all(|part| is_emoji_element(part, parts.len() > 1))
        }
    }
}

/// One emoji of a sequence: an emoji character (not ASCII: digits, `#` and `*` are emoji only in a keycap), then
/// U+FE0F, or a skin tone if it takes one, or nothing if it is shown as emoji by default or stands in a ZWJ sequence.
fn is_emoji_element(part: &[char], joined: bool) -> bool {
    let Some((first, rest)) = part.split_first() else { return false };
    if first.is_ascii() || !first.is_emoji_char() {
        return false;
    }
    let status = first.emoji_status();
    let presentation = matches!(
        status,
        EmojiStatus::EmojiPresentation
            | EmojiStatus::EmojiPresentationAndModifierBase
            | EmojiStatus::EmojiPresentationAndEmojiComponent
            | EmojiStatus::EmojiPresentationAndModifierAndEmojiComponent
    );
    let takes_tone = matches!(status, EmojiStatus::EmojiModifierBase | EmojiStatus::EmojiPresentationAndModifierBase);
    match rest {
        [] => presentation || joined,
        ['\u{FE0F}'] => true,
        [tone] => takes_tone && ('\u{1F3FB}'..='\u{1F3FF}').contains(tone),
        _ => false,
    }
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
        assert_eq!(Handle::parse("Or").unwrap(), Handle::username("Or"), "two letters are a name");
        assert_eq!(Handle::parse("Y").unwrap(), Handle::username("Y"), "one is a name by form; the hub sets the least");
        assert_eq!(Handle::parse("1abc").unwrap(), Handle::username("1abc"), "a digit first");
        for bad in ["", "this_is_far_too_long_x", "+49 170 1234567", "1234"] {
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
        for (typed, name) in [
            ("1abc", "1abc"),
            ("_1abc", "1abc"),
            ("1234", "1234"),
            ("_x🔥", "x🔥"),
            ("abcабв", "abc"),
            ("x×y", "xy"),
            ("🧙‍♂️🔥yo", "🧙‍♂️🔥yo"),
        ] {
            assert_eq!(canonical(typed), name, "{typed}");
        }
        assert!(check_username_form("🔥🔥🔥").is_ok());
        assert!(check_username_form("ÄRGER").is_ok());
        assert!(check_username_form("Léo").is_ok());
        assert!(check_username_form("abc").is_ok(), "lower case is a name");
        assert!(check_username_form("YoGPT").is_ok());
        assert!(check_username_form("le\u{301}o").is_err(), "not NFC");
        assert!(check_username_form("ABCабв").is_err(), "another script");
        assert!(check_username_form("1abc").is_ok(), "starts with a digit");
        assert!(check_username_form("007bond").is_ok());
        assert!(check_username_form("1🔥").is_ok(), "a digit, then an emoji");
        assert!(check_username_form("1234").is_err(), "no letter or emoji");
        assert!(check_username_form("12_3").is_err(), "no letter or emoji");
        assert!(check_username_form("_abc").is_err(), "starts with _");
        for (a, b) in [("YoGPT", "yogpt"), ("YoGPT", "YOGPT"), ("YoGPT", "Y0GPT"), ("Christopher", "CHRISTOPHER")] {
            assert!(same_name(a, b), "{a} {b}");
        }
        assert!(!same_name("YoGPT", "YoGPS"));
    }

    /// Any grapheme that began with a character of the Emoji property passed, and digits have it (for keycaps): a
    /// digit with an accent was a name's first "emoji". Now the whole sequence is checked.
    #[test]
    fn emoji_are_whole_sequences() {
        for good in [
            "🔥🔥🔥",
            "👍🏿yo",
            "🧙‍♂️Yo",
            "❤️abc",
            "❤️‍🔥ab",
            "🏳️‍🌈ab",
            "🧑🏽‍💻ab",
            "👨‍👩‍👧ab",
            "🇩🇪ab",
            "1️⃣ab",
            "🏴\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}ab",
            "👁️‍🗨️ab",
            "⌚ab",
            "☝🏻ab",
        ] {
            assert!(check_username_form(good).is_ok(), "{good:?}");
            assert_eq!(canonical(good), good, "{good:?}");
        }
        for bad in [
            "#️⃣ab",
            "a@b",
            "1\u{301}ab",
            "#\u{301}ab",
            "1\u{FE0F}ab",
            "a\u{20E3}bc",
            "☺ab",
            "☺\u{301}ab",
            "🔥\u{301}ab",
            "🔥\u{200D}ab",
            "👍\u{1F3FF}\u{1F3FF}ab",
            "🔥\u{1F3FF}ab",
            "\u{1F3F4}\u{E0067}ab",
            "\u{200D}🔥ab",
        ] {
            assert!(check_username_form(bad).is_err(), "{bad:?}");
        }
        assert_eq!(canonical("1\u{301}ab"), "ab", "skipped as typed");
    }

    /// U1: a character has no length of its own (a letter with marks, a chain of joined emoji, a tag sequence), a name
    /// does: 64 bytes. Only RGI emoji: no made-up chain or tag flag of any length.
    #[test]
    fn a_username_has_a_byte_bound_and_only_rgi_emoji() {
        let zalgo = format!("a{}", "\u{301}".repeat(2000));
        let chain = vec!["\u{1F44D}"; 1000].join("\u{200D}");
        let tags = format!("\u{1F3F4}{}\u{E007F}", "\u{E0061}".repeat(2000));
        for name in [&zalgo, &chain, &tags] {
            assert!(
                check_username_form(name).is_err() && Handle::username(name).check().is_err(),
                "{} bytes",
                name.len()
            );
            assert!(canonical(name).len() <= USERNAME_MAX_BYTES);
        }
        assert!(check_username_form(&"a".repeat(16)).is_ok());
        assert!(check_username_form(&"🔥".repeat(16)).is_ok(), "16 emoji of 4 bytes: 64 bytes");
        let toned = "👍🏿".repeat(16);
        assert_eq!(toned.len(), 128);
        assert!(check_username_form(&toned).is_err(), "16 characters, 128 bytes");
        assert_eq!(canonical(&toned), "👍🏿".repeat(8), "cut at whole characters within 64 bytes");
        // Made-up sequences that pass the form are no RGI emoji.
        for bad in ["👍\u{200D}👍ab", "🔥\u{200D}🔥ab", "\u{1F3F4}\u{E0061}\u{E007F}ab", "🇦🇦ab", "\u{1F3FF}ab", "🏿ab"]
        {
            assert!(check_username_form(bad).is_err(), "{bad:?}");
        }
        // The RGI ones stay.
        for good in
            ["👨‍👩‍👧ab", "🏴\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}ab", "🇩🇪ab", "🧑🏽‍💻ab", "❤️‍🔥ab", "🧙‍♂️Yo"]
        {
            assert!(check_username_form(good).is_ok(), "{good:?}");
        }
    }

    /// U1: the joiner (U+200D) and tag characters render as nothing where a font has no such sequence: the skeleton
    /// drops them, so look-alike names are one name (spec 3.3), whichever of them the form admits.
    #[test]
    fn invisible_joiners_and_tags_are_no_part_of_the_skeleton() {
        for (a, b) in [
            ("Leo\u{1F44D}\u{1F44D}", "Leo\u{1F44D}\u{200D}\u{1F44D}"),
            ("Leo\u{1F3F4}", "Leo\u{1F3F4}\u{E0061}\u{E007F}"),
            ("Leo\u{1F3F4}", "Leo🏴\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}"),
            ("👨👩👧ab", "👨‍👩‍👧ab"),
            ("Leo", "L\u{200D}eo"),
        ] {
            assert!(same_name(a, b), "{a:?} {b:?}");
        }
        assert_eq!(skeleton("🧙‍♂️Yo"), skeleton("🧙♂Yo"));
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
