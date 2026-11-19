//! Pure presentation and product rules shared by the mobile apps.
use std::collections::HashSet;
use unicode_segmentation::UnicodeSegmentation;

pub mod line;
pub mod list;
pub mod status;
pub mod zoom;

pub const VIDEO_LIMIT_MS: u32 = 10_000;
pub const VIDEO_LIMIT_BYTES: u32 = 9_000_000;
pub const PHOTO_LONG_SIDE: u32 = 2048;
pub const SOUND_LIMIT: u32 = 3;
pub const MOST_BANGS: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    English,
    German,
}

impl Language {
    pub(crate) fn choose(self, en: &'static str, de: &'static str) -> &'static str {
        match self {
            Self::English => en,
            Self::German => de,
        }
    }
}

macro_rules! copy {
    ($($name:ident => ($en:literal, $de:literal)),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Text { $($name),* }

        impl Text {
            pub fn get(self, language: Language) -> &'static str {
                match self { $(Self::$name => language.choose($en, $de)),* }
            }
        }
    };
}

copy! {
    Yo => ("Yo", "Yo"),
    YoBang => ("Yo!", "Yo!"),
    Sent => ("Sent", "Gesendet"),
    SentYo => ("Sent Yo", "Yo gesendet"),
    SentPhoto => ("Sent 📷", "📷 gesendet"),
    SentVideo => ("Sent 🎥", "🎥 gesendet"),
    Sending => ("Sending", "Wird gesendet"),
    NotSent => ("Not Sent", "Nicht gesendet"),
    Canceled => ("Canceled", "Abgebrochen"),
    NoAnswer => ("No Yo to answer", "Kein Yo zum Antworten"),
    NoAnswerYet => ("No Yo to answer yet.", "Noch kein Yo zum Antworten."),
    NoYo => ("No Yo", "Kein Yo"),
    NoYoBack => ("No Yo back", "Kein Yo zurück"),
    YoBack => ("Yo Back", "Yo zurück"),
    YoBackActivity => ("Yo back", "Yo zurück"),
    StaleYo => ("Yo?", "Yo?"),
    TurnOnNotifications => ("Turn On Notifications", "Mitteilungen aktivieren"),
    NotificationsOff => ("Notifications Are Off", "Mitteilungen sind aus"),
    NotificationsHint => ("Yos can't reach you. Opens Settings to turn on notifications for Yo.", "Yos können dich nicht erreichen. Öffnet die Einstellungen für Mitteilungen von Yo."),
    NotificationFallback => ("You may have received a Yo", "Du hast möglicherweise ein Yo erhalten"),
    Group => ("Group", "Gruppe"),
    FailedToSend => ("Failed to Send", "Senden fehlgeschlagen"),
    WidgetEmpty => ("Open Yo to see your friends here.", "Öffne Yo, um hier deine Freunde zu sehen."),
    WidgetFriend => ("Yo a Friend", "Yo an einen Freund"),
    WidgetFriendIntent => ("Sends a Yo to a friend or group.", "Sendet ein Yo an einen Freund oder eine Gruppe."),
    WidgetFriendDescription => ("A tap sends a Yo. Your top friend, or the one you pick.", "Tippen sendet ein Yo an deinen ersten oder ausgewählten Freund."),
    WidgetListIntent => ("Your friends and groups, in your order. None: the top of your list.", "Deine Freunde und Gruppen in deiner Reihenfolge. Ohne Auswahl wird der Listenanfang angezeigt."),
    WidgetListDescription => ("Your friends and groups, as in Yo, in the order you pick. A tap on a bar sends a Yo.", "Deine Freunde und Gruppen wie in Yo, in deiner Reihenfolge. Tippe auf eine Zeile, um ein Yo zu senden."),
    WidgetBackDescription => ("Sends a Yo back to whoever sent you the last one.", "Sendet ein Yo an die Person zurück, die dir zuletzt eines gesendet hat."),
    ShareQueued => ("Sends when you open Yo", "Wird beim Öffnen von Yo gesendet"),
    ShareUnavailable => ("Sharing to Yo isn’t set up on this iPhone yet.", "Teilen mit Yo ist auf diesem iPhone noch nicht eingerichtet."),
    ShareAddFriend => ("Add a friend in Yo first.", "Füge zuerst einen Freund in Yo hinzu."),
    VideoTooLong => ("Video too long", "Video zu lang"),
    NotOnYo => ("Not on Yo", "Nicht auf Yo"),
    GroupEmpty => ("Group Is Empty", "Gruppe ist leer"),
    ConfirmNewKey => ("Confirm New Key", "Neuen Schlüssel bestätigen"),
    Blocked => ("Blocked", "Blockiert"),
    NeedsUpdate => ("Needs Update", "Update erforderlich"),
    AlreadyAdded => ("Already Added", "Bereits hinzugefügt"),
    Someone => ("Someone", "Jemand"),
    YoUser => ("Yo User", "Yo-Nutzer"),
    ThatSimple => ("It’s that simple.", "So einfach ist das."),
    SignUp => ("Sign Up", "Registrieren"),
    SignIn => ("Sign In", "Anmelden"),
    GotYo => ("Got Yo?", "Schon bei Yo?"),
    Username => ("Username", "Benutzername"),
    NameTaken => ("Name taken. Yours?", "Name vergeben. Deiner?"),
    NameTakenLabel => ("Name taken. Sign In if it’s yours", "Name vergeben. Melde dich an, wenn es deiner ist"),
    NameUnavailable => ("Not available", "Nicht verfügbar"),
    LongerName => ("Choose a longer name.", "Wähle einen längeren Namen."),
    LongerNameOffline => ("Choose a longer name, or try again when connected.", "Wähle einen längeren Namen oder versuche es mit Verbindung erneut."),
    NoSuchUser => ("No Such User", "Unbekannter Name"),
    ThatsYou => ("That’s You", "Das bist du"),
}

/// The onboarding's line for an invite: "Mia invited you".
pub fn invited_you(name: &str, lang: Language) -> String {
    match lang {
        Language::English => format!("{name} invited you"),
        Language::German => format!("{name} hat dich eingeladen"),
    }
}

pub fn counted(count: i64) -> String {
    if count <= 1 {
        "Yo".into()
    } else {
        format!("{count}× Yo")
    }
}

pub fn send_words(mark: &str, sending: bool, lang: Language) -> String {
    let verb = if sending { Text::Sending } else { Text::Sent }.get(lang);
    match lang {
        Language::English => format!("{verb} {mark}!"),
        Language::German if sending => format!("{mark} wird gesendet!"),
        Language::German => format!("{mark} gesendet!"),
    }
}

pub fn arrival_line(yos: &str, name: &str, lang: Language) -> String {
    let from = lang.choose("from", "von");
    format!("{yos} {from} {name}")
}

pub fn name_cuts(name: &str) -> Vec<String> {
    let boundaries: Vec<_> = name.grapheme_indices(true).map(|(offset, _)| offset).collect();
    (3..boundaries.len().saturating_sub(1)).rev().map(|length| format!("{}…", &name[..boundaries[length]])).collect()
}

pub fn arrival_lines(count: i64, name: &str, lang: Language) -> Vec<String> {
    let yos = counted(count);
    let mut lines = vec![arrival_line(&yos, name, lang)];
    lines.extend(name_cuts(name).into_iter().map(|cut| arrival_line(&yos, &cut, lang)));
    lines.push(yos);
    lines
}

pub fn failed_to_send(name: Option<&str>, lang: Language) -> String {
    match name.filter(|name| name.graphemes(true).count() <= 18) {
        Some(name) => match lang {
            Language::English => format!("Failed to Send to {name}"),
            Language::German => format!("Senden an {name} fehlgeschlagen"),
        },
        None => Text::FailedToSend.get(lang).into(),
    }
}

pub fn show_notification_row(denied: bool, asked_this_launch: bool) -> bool {
    denied && !asked_this_launch
}

pub fn is_real_friend(skeleton: &str, group: bool, request: bool, blocked: bool, gone: bool) -> bool {
    !(group || request || blocked || gone || matches!(skeleton, "YOGPT" | "PING" | "PINGTEST"))
}

pub fn sound_slot(playing: &[bool]) -> Option<u32> {
    playing.iter().take(SOUND_LIMIT as usize).position(|busy| !busy).map(|index| index as u32)
}

pub fn notification_line(name: &str, kind: list::Kind, request: bool, lang: Language) -> String {
    if request {
        let prefix = lang.choose("Yo Request from", "Yo-Anfrage von");
        return format!("{prefix} {name}");
    }
    let mut line = arrival_line("Yo", name, lang);
    if kind != list::Kind::Yo {
        line.push(' ');
        line.push_str(kind.sign());
    }
    line
}

pub fn notification_group_name(name: &str, group: &str, lang: Language) -> String {
    if group.is_empty() {
        return name.into();
    }
    let in_group = lang.choose("in", "in");
    format!("{name} {in_group} #{group}")
}

pub fn notification_thread(user: &str, group: Option<&str>) -> String {
    group.map_or_else(|| user.into(), |id| format!("group:{id}"))
}

pub fn notification_id(event_id: &str) -> String {
    format!("yo:{event_id}")
}

/// Supply the new threads in delivery order, earliest first.
pub fn notification_sounds(delivered: &[String], new: &[String]) -> Vec<bool> {
    let mut unread: HashSet<_> = delivered.iter().map(String::as_str).collect();
    new.iter().map(|thread| unread.insert(thread.as_str())).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Sending,
    Sent,
    Failed,
    Nobody,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityPlace {
    Expanded,
    Compact,
    LockScreen,
}

#[allow(clippy::too_many_arguments)]
pub fn activity_words(
    outcome: Outcome,
    place: ActivityPlace,
    stale: bool,
    count: i64,
    total: i64,
    has_others: bool,
    back: bool,
    lang: Language,
) -> String {
    use ActivityPlace::*;
    use Outcome::*;
    match (place, outcome) {
        (_, Sending) if stale => Text::StaleYo.get(lang).into(),
        (Expanded, Failed) => Text::NotSent.get(lang).into(),
        (Expanded, Nobody) => Text::NoAnswer.get(lang).into(),
        (Expanded, Sending | Sent) => {
            send_words(&counted(if has_others { total } else { count }), outcome == Sending, lang)
        }
        (Compact | LockScreen, Failed) | (Compact, Nobody) => Text::NoYo.get(lang).into(),
        (LockScreen, Nobody) => Text::NoYoBack.get(lang).into(),
        (LockScreen, _) if has_others => format!("{total} Yo"),
        (_, _) if count > 1 => counted(count),
        (LockScreen, _) if back => Text::YoBackActivity.get(lang).into(),
        _ => Text::Yo.get(lang).into(),
    }
}

pub fn activity_names(name: &str, others: &[String], all: bool, nobody: bool) -> String {
    if !all {
        return name.into();
    }
    if nobody {
        return String::new();
    }
    others.iter().rev().map(String::as_str).chain(std::iter::once(name)).collect::<Vec<_>>().join(" · ")
}

#[derive(Clone, Debug)]
pub struct WidgetFriend {
    pub id: String,
    pub name: String,
}

/// Returns indices into the supplied list so platform presentation data stays with its original row.
pub fn widget_selection(
    friends: &[WidgetFriend],
    picks: &[Option<WidgetFriend>],
    slots: usize,
    fallback: usize,
) -> Vec<u32> {
    let mut seen = HashSet::new();
    let picks: Vec<_> = picks
        .iter()
        .take(slots)
        .flatten()
        .filter_map(|pick| {
            friends
                .iter()
                .position(|friend| friend.id == pick.id)
                .or_else(|| friends.iter().position(|friend| friend.name == pick.name))
        })
        .filter(|&index| seen.insert(&friends[index].id))
        .map(|index| index as u32)
        .collect();
    if picks.is_empty() {
        (0..friends.len().min(fallback) as u32).collect()
    } else {
        picks
    }
}

pub fn share_subtitle(photos: u32, videos: u32, lang: Language) -> String {
    match (photos, videos) {
        (0, 1) => {
            lang.choose("Tap a name to send this video.", "Tippe auf einen Namen, um dieses Video zu senden.").into()
        }
        (1, 0) => {
            lang.choose("Tap a name to send this photo.", "Tippe auf einen Namen, um dieses Foto zu senden.").into()
        }
        (0, 0) => lang.choose("Nothing to send.", "Nichts zum Senden.").into(),
        _ => {
            let total = u64::from(photos) + u64::from(videos);
            match lang {
                Language::English => format!("Tap a name to send these {total}."),
                Language::German => format!("Tippe auf einen Namen, um diese {total} Dateien zu senden."),
            }
        }
    }
}

pub fn video_limit_label() -> String {
    format!("max {} s", VIDEO_LIMIT_MS / 1000)
}

pub const VIDEO_LONG_SIDE: u32 = 1280;
pub const VIDEO_SHORT_SIDE: u32 = 720;
pub const VIDEO_BIT_RATE: u32 = 2_500_000;
pub const AUDIO_BIT_RATE: u32 = 128_000;
pub const PHOTO_QUALITY: f64 = 0.75;

pub fn undo_seconds(seconds: f64) -> f64 {
    seconds.clamp(list::UNDO_MS as f64 / 1000.0, 10.0)
}

pub fn media_sign(kind: list::Kind) -> &'static str {
    kind.sign()
}

pub fn media_kind(label: &str) -> list::Kind {
    [list::Kind::Video, list::Kind::Photo]
        .into_iter()
        .find(|kind| label.ends_with(kind.sign()))
        .unwrap_or(list::Kind::Yo)
}

pub const ACTIVITY_SHOWN_SECS: u32 = 3;
pub const ACTIVITY_STALE_SECS: u32 = 30;
pub const WIDGET_SENT_SECS: u32 = 3;
pub const SHARE_PHOTOS: u32 = 10;
pub const SHARE_VIDEOS: u32 = 1;

pub fn share_can_send(sending: bool, too_long: bool, has_media: bool, has_feedback: bool) -> bool {
    !sending && !too_long && has_media && !has_feedback
}

pub fn widget_eligible(blocked: bool, request: bool, gone: bool) -> bool {
    !(blocked || request || gone)
}

/// Android stores a list of IDs, allows duplicate choices, and falls back only for a signed-in account.
pub fn widget_id_selection(
    friends: &[String],
    picks: &[String],
    signed_in: bool,
    slots: usize,
    fallback: usize,
) -> Vec<u32> {
    if !signed_in {
        return Vec::new();
    }
    let choices: Vec<_> = picks
        .iter()
        .filter_map(|id| friends.iter().position(|friend| friend == id))
        .take(slots)
        .map(|index| index as u32)
        .collect();
    if choices.is_empty() {
        (0..friends.len().min(slots).min(fallback) as u32).collect()
    } else {
        choices
    }
}

pub fn android_notification_line(name: Option<&str>, group: Option<&str>, kind: list::Kind, lang: Language) -> String {
    let Some(name) = name else {
        return Text::NotificationFallback.get(lang).into();
    };
    let from =
        group.map_or_else(|| name.into(), |group| format!("{name} in #{}", group.strip_prefix('#').unwrap_or(group)));
    notification_line(&from, kind, false, lang)
}

/// A request row's title: "Yo Request from Mia", or "Yo Request: #Hey" for a group (one "#", whether the name
/// brings its own or not). Shown whole: where it does not fit, the apps take the bar's small size, never a cut.
pub fn request_row(name: &str, group: bool, lang: Language) -> String {
    if group {
        let prefix = lang.choose("Yo Request", "Yo-Anfrage");
        format!("{prefix}: #{}", name.strip_prefix('#').unwrap_or(name))
    } else {
        notification_line(name, list::Kind::Yo, true, lang)
    }
}

#[cfg(test)]
mod tests {
    use super::{list::Kind::*, ActivityPlace::*, Language::*, Outcome::*, *};

    #[test]
    fn count_and_send_copy() {
        for single in [i64::MIN, -1, 0, 1] {
            assert_eq!(counted(single), "Yo");
        }
        assert_eq!(counted(i64::MAX), "9223372036854775807× Yo");
        assert_eq!(send_words(&counted(2), false, English), "Sent 2× Yo!");
        assert_eq!(send_words(&counted(1), true, English), "Sending Yo!");
        assert_eq!(send_words(&counted(2), false, German), "2× Yo gesendet!");
    }

    #[test]
    fn onboarding_copy() {
        assert_eq!(Text::NameTaken.get(English), "Name taken. Yours?");
        assert_eq!(Text::NameTaken.get(German), "Name vergeben. Deiner?");
        assert_eq!(Text::LongerName.get(English), "Choose a longer name.");
        assert_eq!(Text::SignIn.get(German), "Anmelden");
        assert_eq!(Text::ThatsYou.get(English), "That’s You");
        assert_eq!(Text::NoSuchUser.get(English), "No Such User");
        assert_eq!(invited_you("Mia", English), "Mia invited you");
        assert_eq!(invited_you("Mia", German), "Mia hat dich eingeladen");
    }

    #[test]
    fn cut_only_names_at_grapheme_boundaries() {
        assert_eq!(name_cuts("Chris"), ["Chr…"]);
        assert_eq!(name_cuts("abcd"), Vec::<String>::new());
        assert_eq!(name_cuts("👨‍👩‍👧‍👦🇩🇪e\u{301}✋🏾X"), ["👨‍👩‍👧‍👦🇩🇪e\u{301}…"]);
        assert_eq!(arrival_lines(2, "مرحبا", English), ["2× Yo from مرحبا", "2× Yo from مرح…", "2× Yo"]);
        assert_eq!(arrival_lines(1, "", English), ["Yo from ", "Yo"]);
        let name = "あ".repeat(1000);
        let candidates = arrival_lines(123, &name, English);
        assert_eq!(candidates.len(), 998);
        assert!(candidates.iter().all(|line| line.starts_with("123× Yo")));
        assert_eq!(candidates.last().unwrap(), "123× Yo");
        assert_eq!(arrival_lines(1, "Mia", German), ["Yo von Mia", "Yo"]);
    }

    #[test]
    fn notification_copy_and_first_unread_sound() {
        assert_eq!(notification_line("Mia", Yo, false, English), "Yo from Mia");
        assert_eq!(notification_line("Mia", Photo, true, English), "Yo Request from Mia");
        assert_eq!(notification_line("Mia", Photo, false, English), "Yo from Mia 📷");
        assert_eq!(notification_line("Mia", Video, false, German), "Yo von Mia 🎥");
        assert_eq!(notification_group_name("Mia", "Hey", English), "Mia in #Hey");
        assert_eq!(notification_group_name("Mia", "", English), "Mia");
        let user = notification_thread("user", None);
        let group = notification_thread("user", Some("id"));
        assert_eq!(user, "user");
        assert_eq!(group, "group:id");
        assert_eq!(notification_id("id/user/123"), "yo:id/user/123");
        let new = [user.clone(), group.clone(), group];
        assert_eq!(notification_sounds(&[], &new), [true, true, false]);
        assert_eq!(notification_sounds(&[user], &new), [false, true, false]);
        assert!(notification_sounds(&[], &[]).is_empty());
    }

    #[test]
    fn notification_row_and_add_eligibility() {
        for denied in [false, true] {
            for asked in [false, true] {
                assert_eq!(show_notification_row(denied, asked), denied && !asked);
            }
        }
        for bot in ["PING", "PINGTEST", "YOGPT"] {
            assert!(!is_real_friend(bot, false, false, false, false));
        }
        for flags in 0..16 {
            assert_eq!(
                is_real_friend("MIA", flags & 1 != 0, flags & 2 != 0, flags & 4 != 0, flags & 8 != 0),
                flags == 0
            );
        }
    }

    #[test]
    fn foreground_sound_uses_first_free_slot_and_never_a_fourth() {
        assert_eq!(sound_slot(&[]), None);
        assert_eq!(sound_slot(&[true, false, false]), Some(1));
        assert_eq!(sound_slot(&[true, true, false]), Some(2));
        assert_eq!(sound_slot(&[true, true, true, false]), None);
    }

    #[test]
    fn live_activity_outcomes_and_recipient_order() {
        let words = |outcome, place, stale, count, others, back| {
            activity_words(outcome, place, stale, count, 4, others, back, English)
        };
        for place in [Compact, Expanded, LockScreen] {
            assert_eq!(words(Sending, place, true, 2, false, false), "Yo?");
        }
        assert_eq!(words(Sending, Expanded, false, 1, false, false), "Sending Yo!");
        assert_eq!(words(Sent, Expanded, false, 2, true, false), "Sent 4× Yo!");
        assert_eq!(words(Failed, Expanded, false, 1, false, false), "Not Sent");
        assert_eq!(words(Nobody, Expanded, false, 1, false, false), "No Yo to answer");
        assert_eq!(words(Failed, LockScreen, false, 2, true, true), "No Yo");
        assert_eq!(words(Nobody, Compact, false, 1, false, false), "No Yo");
        assert_eq!(words(Nobody, LockScreen, false, 1, false, false), "No Yo back");
        assert_eq!(words(Sent, LockScreen, false, 1, false, true), "Yo back");
        assert_eq!(words(Sent, LockScreen, false, 2, false, true), "2× Yo");
        assert_eq!(words(Sent, LockScreen, false, 2, true, true), "4 Yo");
        assert_eq!(words(Sending, Compact, false, 1, false, false), "Yo");
        let others = ["B".into(), "A".into()];
        assert_eq!(activity_names("C", &others, true, false), "A · B · C");
        assert_eq!(activity_names("C", &others, false, false), "C");
        assert_eq!(activity_names("C", &others, true, true), "");
    }

    #[test]
    fn widget_picks_use_id_then_name_then_list_head() {
        let friend = |id: &str, name: &str| WidgetFriend { id: id.into(), name: name.into() };
        let friends = [friend("a", "Mia"), friend("b", "Leo")];
        let picks = [None, Some(friend("old", "Leo")), Some(friend("b", "")), Some(friend("a", "Leo"))];
        assert_eq!(widget_selection(&friends, &picks, 4, 2), [1, 0]);
        assert_eq!(widget_selection(&friends, &picks, 1, 1), [0]);
        assert!(widget_selection(&[], &picks, 4, 2).is_empty());
    }

    #[test]
    fn error_and_share_copy() {
        assert_eq!(failed_to_send(None, English), "Failed to Send");
        assert_eq!(failed_to_send(Some(""), English), "Failed to Send to ");
        assert_eq!(failed_to_send(Some(&"🇩🇪".repeat(19)), English), "Failed to Send");
        assert_eq!(failed_to_send(Some(&"🇩🇪".repeat(18)), English), format!("Failed to Send to {}", "🇩🇪".repeat(18)));
        assert_eq!(share_subtitle(0, 0, English), "Nothing to send.");
        assert_eq!(share_subtitle(0, 1, English), "Tap a name to send this video.");
        assert_eq!(share_subtitle(1, 0, English), "Tap a name to send this photo.");
        assert_eq!(share_subtitle(1, 1, English), "Tap a name to send these 2.");
        assert_eq!(Text::NotSent.get(German), "Nicht gesendet");
    }

    #[test]
    fn media_limits_and_undo_bounds() {
        assert_eq!(video_limit_label(), "max 10 s");
        assert_eq!(VIDEO_LIMIT_BYTES, 9_000_000);
        assert_eq!(PHOTO_LONG_SIDE, 2048);
        assert_eq!(media_sign(Video), "🎥");
        assert_eq!(media_kind("Sent 📷"), Photo);
        assert_eq!(media_kind("Sent 🎥"), Video);
        assert_eq!(media_kind("Sent 🎥!"), Yo);
        assert_eq!(undo_seconds(-1.0), 2.0);
        assert_eq!(undo_seconds(2.5), 2.5);
        assert_eq!(undo_seconds(11.0), 10.0);
        assert!(undo_seconds(f64::NAN).is_nan());
    }
    #[test]
    fn android_presentation_variants() {
        let ids = ["a".into(), "b".into()];
        let picks = ["missing".into(), "b".into(), "b".into()];
        assert_eq!(widget_id_selection(&ids, &picks, true, 4, 1), [1, 1]);
        assert_eq!(widget_id_selection(&ids, &picks, true, 1, 2), [1]);
        assert_eq!(widget_id_selection(&ids, &[], true, 4, 1), [0]);
        assert!(widget_id_selection(&ids, &picks, false, 4, 1).is_empty());
        for flags in 0..8 {
            assert_eq!(widget_eligible(flags & 1 != 0, flags & 2 != 0, flags & 4 != 0), flags == 0);
        }
        assert_eq!(android_notification_line(None, None, Yo, English), "You may have received a Yo");
        assert_eq!(android_notification_line(Some("Mia"), Some("#Hey"), Video, English), "Yo from Mia in #Hey 🎥");
        assert_eq!(request_row("Hey", true, English), "Yo Request: #Hey");
        assert_eq!(request_row("#FAMILIE", true, English), "Yo Request: #FAMILIE");
    }

    #[test]
    fn share_eligibility_and_display_durations() {
        for flags in 0..16 {
            assert_eq!(share_can_send(flags & 1 != 0, flags & 2 != 0, flags & 4 != 0, flags & 8 != 0), flags == 4);
        }
        assert_eq!((SHARE_PHOTOS, SHARE_VIDEOS), (10, 1));
        assert_eq!((ACTIVITY_SHOWN_SECS, ACTIVITY_STALE_SECS, WIDGET_SENT_SECS), (3, 30, 3));
    }
}
