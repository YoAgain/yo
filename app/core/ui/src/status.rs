//! Status (Settings › Help & About › Status): how the phone's connection, keys and backup are doing.

use crate::Language;

/// A row's mark: fine, not known yet, needs time or a step, wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
    Ok,
    Open,
    Waiting,
    Problem,
}

/// This device's keys at the hub (the core's security status).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keys {
    pub hub_signed: bool,
    pub prekeys_at_hub: u32,
    pub refill_pending: bool,
    pub signed_prekey_days: u64,
    pub pq_sessions: u32,
    pub devices: u32,
}

const ENOUGH_PREKEYS: u32 = 16;
const FRESH_SIGNED_DAYS: u64 = 8;
const SLOW_MS: u32 = 2000;

/// Signed by Yo, one-time keys, signed key, quantum-safe sessions.
pub fn key_checks(keys: Option<Keys>) -> [Check; 4] {
    let Some(k) = keys else { return [Check::Waiting; 4] };
    let ok = |good: bool| if good { Check::Ok } else { Check::Waiting };
    let prekeys = if k.prekeys_at_hub >= ENOUGH_PREKEYS {
        Check::Ok
    } else if k.prekeys_at_hub == 0 && !k.refill_pending {
        Check::Problem
    } else {
        Check::Waiting
    };
    [ok(k.hub_signed), prekeys, ok(k.signed_prekey_days <= FRESH_SIGNED_DAYS), ok(k.pq_sessions == k.devices)]
}

/// The keys as one mark (not known yet: open).
pub fn keys_check(keys: Option<Keys>) -> Check {
    if keys.is_none() {
        return Check::Open;
    }
    let checks = key_checks(keys);
    if checks.contains(&Check::Problem) {
        Check::Problem
    } else if checks.iter().all(|c| *c == Check::Ok) {
        Check::Ok
    } else {
        Check::Waiting
    }
}

/// The Identity row: the keys and the contacts' sessions (not known yet: waiting) as one mark.
pub fn identity_check(keys: Option<Keys>, encrypted: Option<u32>, total: u32) -> Check {
    let contacts = match contacts_check(encrypted, total) {
        Check::Open => Check::Waiting,
        check => check,
    };
    let checks: Vec<Check> = key_checks(keys).into_iter().chain([contacts]).collect();
    if checks.contains(&Check::Problem) {
        Check::Problem
    } else if checks.iter().all(|c| *c == Check::Ok) {
        Check::Ok
    } else {
        Check::Waiting
    }
}

/// The contacts' sessions: all encrypted, or not yet (unknown: open).
pub fn contacts_check(encrypted: Option<u32>, total: u32) -> Check {
    match encrypted {
        None => Check::Open,
        Some(n) if n == total => Check::Ok,
        Some(_) => Check::Waiting,
    }
}

pub fn encryption_check(keys: Check, contacts: Check) -> Check {
    if keys == Check::Problem {
        Check::Problem
    } else if keys == Check::Open || contacts == Check::Open {
        Check::Open
    } else if keys == Check::Ok && contacts == Check::Ok {
        Check::Ok
    } else {
        Check::Waiting
    }
}

/// The hub: reachable and answering within two seconds.
pub fn server_check(reachable: Option<bool>, milliseconds: Option<u32>) -> Check {
    match reachable {
        None => Check::Open,
        Some(true) if milliseconds.unwrap_or(0) < SLOW_MS => Check::Ok,
        Some(_) => Check::Waiting,
    }
}

/// Everything Status looks at, as far as known (None: still asking).
#[derive(Clone, Copy, Debug, Default)]
pub struct Health {
    pub reachable: Option<bool>,
    pub milliseconds: Option<u32>,
    pub accepted: Option<bool>,
    pub notifications_allowed: Option<bool>,
    pub push_registered: bool,
    pub keys: Option<Keys>,
    pub encrypted_contacts: Option<u32>,
    pub contacts: u32,
    pub backup_available: bool,
    pub backup_saved: bool,
    pub storage_known: bool,
    pub answered: bool,
}

/// The one mark and word for the Status row: the first thing wrong, else checking, else online.
pub fn summary(h: &Health, lang: Language) -> (Check, String) {
    let word = |en: &'static str, de: &'static str| lang.choose(en, de).to_string();
    let encryption = encryption_check(keys_check(h.keys), contacts_check(h.encrypted_contacts, h.contacts));
    let (check, text) = if h.reachable == Some(false) {
        (Check::Waiting, word("Offline", "Offline"))
    } else if server_check(h.reachable, h.milliseconds) == Check::Waiting {
        (Check::Waiting, word("Slow", "Langsam"))
    } else if h.accepted == Some(false) {
        (Check::Waiting, word("Update", "Update"))
    } else if h.notifications_allowed == Some(false) || !h.push_registered {
        (Check::Waiting, word("No Push", "Kein Push"))
    } else if encryption == Check::Problem {
        (Check::Problem, word("Problem", "Problem"))
    } else if encryption == Check::Waiting {
        (Check::Waiting, word("Encryption", "Verschlüsselung"))
    } else if h.backup_available && !h.backup_saved {
        (Check::Waiting, word("Account Backup", "Kontobackup"))
    } else if h.reachable.is_none()
        || h.accepted.is_none()
        || h.notifications_allowed.is_none()
        || !h.storage_known
        || !h.answered
        || !h.backup_available
        || encryption != Check::Ok
    {
        (Check::Open, word("Checking", "Prüfe"))
    } else {
        (Check::Ok, word("Online", "Online"))
    };
    (check, text)
}

/// What is known of one contact (Settings › Account › Contacts).
#[derive(Clone, Copy, Debug, Default)]
pub struct Contact {
    pub blocked: bool,
    /// Gone as the list knows it (the account deleted).
    pub friend_gone: bool,
    /// Gone as the core knows it.
    pub contact_gone: bool,
    pub unconfirmed: bool,
    pub outdated: bool,
    pub key_changed: bool,
    pub verified: bool,
    /// A chat exists (a user id).
    pub has_user: bool,
    /// The core's state of the contact is read.
    pub loaded: bool,
    pub sessions: u32,
    /// A session retrying or not usable.
    pub sessions_unready: bool,
}

/// The mark a contact gets in the list: only the exceptions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactMark {
    Verified,
    Attention,
}

pub fn sessions_check(sessions: u32, unready: bool) -> Check {
    if sessions == 0 {
        Check::Open
    } else if unready {
        Check::Waiting
    } else {
        Check::Ok
    }
}

/// The worst of the checks that matter for a contact.
pub fn contact_check(c: &Contact) -> Check {
    if c.friend_gone || c.contact_gone || c.unconfirmed {
        Check::Problem
    } else if c.outdated || c.key_changed {
        Check::Waiting
    } else if !c.has_user {
        Check::Open
    } else {
        sessions_check(c.sessions, c.sessions_unready)
    }
}

pub fn contact_mark(c: &Contact) -> Option<ContactMark> {
    if c.friend_gone || c.key_changed || c.contact_gone || c.unconfirmed || c.outdated {
        Some(ContactMark::Attention)
    } else if c.verified {
        Some(ContactMark::Verified)
    } else {
        None
    }
}

/// The contact's word for VoiceOver and the row.
pub fn contact_summary(c: &Contact, lang: Language) -> String {
    let check = contact_check(c);
    let (en, de) = if c.blocked {
        ("Blocked", "Blockiert")
    } else if c.friend_gone {
        ("Not on Yo", "Nicht auf Yo")
    } else if !c.has_user {
        ("No Chat Yet", "Noch kein Chat")
    } else if !c.loaded {
        ("Updating…", "Wird aktualisiert …")
    } else if c.outdated {
        ("Needs Update", "Update erforderlich")
    } else if c.key_changed && check == Check::Waiting {
        ("New Key", "Neuer Schlüssel")
    } else if c.verified && check != Check::Problem {
        ("Verified", "Verifiziert")
    } else {
        match check {
            Check::Ok => ("Encrypted", "Verschlüsselt"),
            Check::Open => ("No Chat Yet", "Noch kein Chat"),
            Check::Waiting => ("Updating", "Wird aktualisiert"),
            Check::Problem => ("Check", "Prüfen"),
        }
    };
    lang.choose(en, de).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Keys {
        Keys {
            hub_signed: true,
            prekeys_at_hub: 20,
            refill_pending: false,
            signed_prekey_days: 1,
            pq_sessions: 2,
            devices: 2,
        }
    }

    fn healthy() -> Health {
        Health {
            reachable: Some(true),
            milliseconds: Some(80),
            accepted: Some(true),
            notifications_allowed: Some(true),
            push_registered: true,
            keys: Some(keys()),
            encrypted_contacts: Some(3),
            contacts: 3,
            backup_available: true,
            backup_saved: true,
            storage_known: true,
            answered: true,
        }
    }

    #[test]
    fn key_rules() {
        assert_eq!(key_checks(Some(keys())), [Check::Ok; 4]);
        let empty = Keys { prekeys_at_hub: 0, ..keys() };
        assert_eq!(key_checks(Some(empty))[1], Check::Problem);
        assert_eq!(keys_check(Some(empty)), Check::Problem);
        assert_eq!(key_checks(Some(Keys { refill_pending: true, ..empty }))[1], Check::Waiting);
        assert_eq!(key_checks(Some(Keys { signed_prekey_days: 9, ..keys() }))[2], Check::Waiting);
        assert_eq!(keys_check(None), Check::Open);
        assert_eq!(key_checks(None), [Check::Waiting; 4]);
        assert_eq!(identity_check(Some(keys()), Some(2), 2), Check::Ok);
        assert_eq!(identity_check(Some(keys()), None, 2), Check::Waiting);
        assert_eq!(identity_check(Some(empty), Some(2), 2), Check::Problem);
    }

    #[test]
    fn the_first_thing_wrong_names_the_status() {
        let en = Language::English;
        assert_eq!(summary(&healthy(), en), (Check::Ok, "Online".into()));
        assert_eq!(summary(&Health { reachable: Some(false), ..healthy() }, en).1, "Offline");
        assert_eq!(summary(&Health { milliseconds: Some(2500), ..healthy() }, en).1, "Slow");
        assert_eq!(summary(&Health { accepted: Some(false), ..healthy() }, en).1, "Update");
        assert_eq!(summary(&Health { push_registered: false, ..healthy() }, en).1, "No Push");
        assert_eq!(summary(&Health { encrypted_contacts: Some(1), ..healthy() }, en).1, "Encryption");
        let broken = Keys { prekeys_at_hub: 0, ..keys() };
        assert_eq!(summary(&Health { keys: Some(broken), ..healthy() }, en), (Check::Problem, "Problem".into()));
        assert_eq!(summary(&Health { backup_saved: false, ..healthy() }, en).1, "Account Backup");
        assert_eq!(summary(&Health { answered: false, ..healthy() }, en), (Check::Open, "Checking".into()));
        assert_eq!(summary(&Health::default(), en).1, "No Push");
        assert_eq!(summary(&Health { push_registered: true, ..Health::default() }, en).0, Check::Open);
    }

    #[test]
    fn contacts_show_only_their_exceptions() {
        let en = Language::English;
        let fine = Contact { has_user: true, loaded: true, sessions: 1, ..Contact::default() };
        assert_eq!((contact_check(&fine), contact_mark(&fine)), (Check::Ok, None));
        assert_eq!(contact_summary(&fine, en), "Encrypted");
        let verified = Contact { verified: true, ..fine };
        assert_eq!(
            (contact_mark(&verified), contact_summary(&verified, en).as_str()),
            (Some(ContactMark::Verified), "Verified")
        );
        let changed = Contact { key_changed: true, ..verified };
        assert_eq!(
            (contact_mark(&changed), contact_summary(&changed, en).as_str()),
            (Some(ContactMark::Attention), "New Key")
        );
        let unconfirmed = Contact { unconfirmed: true, ..verified };
        assert_eq!(
            (contact_check(&unconfirmed), contact_summary(&unconfirmed, en).as_str()),
            (Check::Problem, "Check")
        );
        assert_eq!(contact_summary(&Contact { has_user: true, ..Contact::default() }, en), "Updating…");
        assert_eq!(contact_summary(&Contact { blocked: true, friend_gone: true, ..fine }, en), "Blocked");
        assert_eq!(contact_summary(&Contact::default(), en), "No Chat Yet");
        assert_eq!(contact_check(&Contact { sessions_unready: true, ..fine }), Check::Waiting);
    }
}
