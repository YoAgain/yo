//! The product rules (`yo_product`) for Swift, under the names the app has always used.

use yo_product::list::Kind;
use yo_product::{self as product, ActivityPlace, Language, Outcome, Text, WidgetFriend};

#[uniffi::remote(Enum)]
#[uniffi(name = "ProductLanguage")]
pub enum Language {
    English,
    German,
}

#[uniffi::remote(Enum)]
#[uniffi(name = "ProductText")]
pub enum Text {
    Yo,
    YoBang,
    Sent,
    SentYo,
    SentPhoto,
    SentVideo,
    Sending,
    NotSent,
    Canceled,
    NoAnswer,
    NoAnswerYet,
    NoYo,
    NoYoBack,
    YoBack,
    YoBackActivity,
    StaleYo,
    TurnOnNotifications,
    NotificationsOff,
    NotificationsHint,
    NotificationFallback,
    Group,
    FailedToSend,
    WidgetEmpty,
    WidgetFriend,
    WidgetFriendIntent,
    WidgetFriendDescription,
    WidgetListIntent,
    WidgetListDescription,
    WidgetBackDescription,
    ShareQueued,
    ShareUnavailable,
    ShareAddFriend,
    VideoTooLong,
    NotOnYo,
    GroupEmpty,
    ConfirmNewKey,
    Blocked,
    NeedsUpdate,
    AlreadyAdded,
    Someone,
    YoUser,
    ThatSimple,
    SignUp,
    SignIn,
    GotYo,
    Username,
    NameTaken,
    NameTakenLabel,
    NameUnavailable,
    LongerName,
    LongerNameOffline,
    NoSuchUser,
    NoConnection,
    ThatsYou,
    InviteAddYo,
    InviteYo,
    AddAndYo,
    Cancel,
}

#[uniffi::remote(Enum)]
#[uniffi(name = "ProductOutcome")]
pub enum Outcome {
    Sending,
    Sent,
    Failed,
    Nobody,
}

#[uniffi::remote(Enum)]
#[uniffi(name = "ProductActivityPlace")]
pub enum ActivityPlace {
    Expanded,
    Compact,
    LockScreen,
}

#[uniffi::remote(Record)]
#[uniffi(name = "ProductWidgetFriend")]
pub struct WidgetFriend {
    pub id: String,
    pub name: String,
}

#[uniffi::export]
pub fn product_text(text: Text, lang: Language) -> String {
    text.get(lang).into()
}

#[uniffi::export]
pub fn product_invited_you(name: String, lang: Language) -> String {
    product::invited_you(&name, lang)
}

#[uniffi::export]
pub fn product_counted(count: i64) -> String {
    product::counted(count)
}

#[uniffi::export]
pub fn product_send_words(mark: String, sending: bool, lang: Language) -> String {
    product::send_words(&mark, sending, lang)
}

#[uniffi::export]
pub fn product_arrival_lines(count: i64, name: String, lang: Language) -> Vec<String> {
    product::arrival_lines(count, &name, lang)
}

#[uniffi::export]
pub fn product_failed_to_send(name: Option<String>, lang: Language) -> String {
    product::failed_to_send(name.as_deref(), lang)
}

#[uniffi::export]
pub fn product_show_notification_row(denied: bool, asked_this_launch: bool) -> bool {
    product::show_notification_row(denied, asked_this_launch)
}

#[uniffi::export]
pub fn product_is_real_friend(skeleton: String, group: bool, request: bool, blocked: bool, gone: bool) -> bool {
    product::is_real_friend(&skeleton, group, request, blocked, gone)
}

#[uniffi::export]
pub fn product_sound_slot(playing: Vec<bool>) -> Option<u32> {
    product::sound_slot(&playing)
}

#[uniffi::export]
pub fn product_sound_limit() -> u32 {
    product::SOUND_LIMIT
}

#[uniffi::export]
pub fn product_notification_line(name: String, kind: Kind, request: bool, lang: Language) -> String {
    product::notification_line(&name, kind, request, lang)
}

#[uniffi::export]
pub fn product_notification_group_name(name: String, group: String, lang: Language) -> String {
    product::notification_group_name(&name, &group, lang)
}

#[uniffi::export]
pub fn product_notification_thread(user: String, group: Option<String>) -> String {
    product::notification_thread(&user, group.as_deref())
}

#[uniffi::export]
pub fn product_notification_id(event_id: String) -> String {
    product::notification_id(&event_id)
}

#[uniffi::export]
pub fn product_notification_sounds(delivered: Vec<String>, new: Vec<String>) -> Vec<bool> {
    product::notification_sounds(&delivered, &new)
}

#[allow(clippy::too_many_arguments)]
#[uniffi::export]
pub fn product_activity_words(
    outcome: Outcome,
    place: ActivityPlace,
    stale: bool,
    count: i64,
    total: i64,
    has_others: bool,
    back: bool,
    lang: Language,
) -> String {
    product::activity_words(outcome, place, stale, count, total, has_others, back, lang)
}

#[uniffi::export]
pub fn product_activity_names(name: String, others: Vec<String>, all: bool, nobody: bool) -> String {
    product::activity_names(&name, &others, all, nobody)
}

#[uniffi::export]
pub fn product_widget_selection(
    friends: Vec<WidgetFriend>,
    picks: Vec<Option<WidgetFriend>>,
    slots: u32,
    fallback: u32,
) -> Vec<u32> {
    product::widget_selection(&friends, &picks, slots as usize, fallback as usize)
}

#[uniffi::export]
pub fn product_share_subtitle(photos: u32, videos: u32, lang: Language) -> String {
    product::share_subtitle(photos, videos, lang)
}

#[uniffi::export]
pub fn product_video_limit_label() -> String {
    product::video_limit_label()
}

#[uniffi::export]
pub fn product_video_limit_ms() -> u32 {
    product::VIDEO_LIMIT_MS
}

#[uniffi::export]
pub fn product_video_limit_bytes() -> u32 {
    product::VIDEO_LIMIT_BYTES
}

#[uniffi::export]
pub fn product_photo_long_side() -> u32 {
    product::PHOTO_LONG_SIDE
}

#[uniffi::export]
pub fn product_undo_seconds(seconds: f64) -> f64 {
    product::undo_seconds(seconds)
}

#[uniffi::export]
pub fn product_media_sign(kind: Kind) -> String {
    product::media_sign(kind).into()
}

#[uniffi::export]
pub fn product_media_kind(label: String) -> Kind {
    product::media_kind(&label)
}

#[derive(uniffi::Record)]
pub struct ProductEncoding {
    pub video_long_side: u32,
    pub video_short_side: u32,
    pub video_bit_rate: u32,
    pub audio_bit_rate: u32,
    pub photo_quality: f64,
}

#[uniffi::export]
pub fn product_encoding() -> ProductEncoding {
    ProductEncoding {
        video_long_side: product::VIDEO_LONG_SIDE,
        video_short_side: product::VIDEO_SHORT_SIDE,
        video_bit_rate: product::VIDEO_BIT_RATE,
        audio_bit_rate: product::AUDIO_BIT_RATE,
        photo_quality: product::PHOTO_QUALITY,
    }
}

#[derive(uniffi::Record)]
pub struct ProductDisplayTiming {
    pub activity_shown_secs: u32,
    pub activity_stale_secs: u32,
    pub widget_sent_secs: u32,
}

#[uniffi::export]
pub fn product_display_timing() -> ProductDisplayTiming {
    ProductDisplayTiming {
        activity_shown_secs: product::ACTIVITY_SHOWN_SECS,
        activity_stale_secs: product::ACTIVITY_STALE_SECS,
        widget_sent_secs: product::WIDGET_SENT_SECS,
    }
}

#[derive(uniffi::Record)]
pub struct ProductShareLimits {
    pub photos: u32,
    pub videos: u32,
}

#[uniffi::export]
pub fn product_share_limits() -> ProductShareLimits {
    ProductShareLimits { photos: product::SHARE_PHOTOS, videos: product::SHARE_VIDEOS }
}

#[uniffi::export]
pub fn product_share_can_send(sending: bool, too_long: bool, has_media: bool, has_feedback: bool) -> bool {
    product::share_can_send(sending, too_long, has_media, has_feedback)
}

use yo_core::security::SecurityStatus;
use yo_product::status::{self, Check, Contact, ContactMark, Health, Keys};

#[uniffi::remote(Enum)]
#[uniffi(name = "ProductCheck")]
pub enum Check {
    Ok,
    Open,
    Waiting,
    Problem,
}

fn keys(status: Option<SecurityStatus>) -> Option<Keys> {
    status.map(|s| Keys {
        hub_signed: s.hub_signed,
        prekeys_at_hub: s.prekeys_at_hub,
        refill_pending: s.refill_pending,
        signed_prekey_days: s.signed_prekey_days,
        pq_sessions: s.pq_sessions,
        devices: s.devices,
    })
}

/// Signed by Yo, one-time keys, signed key, quantum-safe sessions.
#[uniffi::export]
pub fn product_key_checks(status: Option<SecurityStatus>) -> Vec<Check> {
    status::key_checks(keys(status)).to_vec()
}

#[uniffi::export]
pub fn product_identity_check(status: Option<SecurityStatus>, encrypted: Option<u32>, total: u32) -> Check {
    status::identity_check(keys(status), encrypted, total)
}

#[uniffi::export]
pub fn product_contacts_check(encrypted: Option<u32>, total: u32) -> Check {
    status::contacts_check(encrypted, total)
}

#[uniffi::export]
pub fn product_server_check(reachable: Option<bool>, milliseconds: Option<u32>) -> Check {
    status::server_check(reachable, milliseconds)
}

/// What Status knows so far (None: still asking).
#[derive(uniffi::Record)]
pub struct ProductHealth {
    pub reachable: Option<bool>,
    pub milliseconds: Option<u32>,
    pub accepted: Option<bool>,
    pub notifications_allowed: Option<bool>,
    pub push_registered: bool,
    pub status: Option<SecurityStatus>,
    pub encrypted_contacts: Option<u32>,
    pub contacts: u32,
    pub backup_available: bool,
    pub backup_saved: bool,
    pub storage_known: bool,
    pub answered: bool,
}

#[derive(uniffi::Record)]
pub struct ProductStatus {
    pub check: Check,
    pub word: String,
}

#[uniffi::export]
pub fn product_status_summary(health: ProductHealth, lang: Language) -> ProductStatus {
    let h = Health {
        reachable: health.reachable,
        milliseconds: health.milliseconds,
        accepted: health.accepted,
        notifications_allowed: health.notifications_allowed,
        push_registered: health.push_registered,
        keys: keys(health.status),
        encrypted_contacts: health.encrypted_contacts,
        contacts: health.contacts,
        backup_available: health.backup_available,
        backup_saved: health.backup_saved,
        storage_known: health.storage_known,
        answered: health.answered,
    };
    let (check, word) = status::summary(&h, lang);
    ProductStatus { check, word }
}

#[uniffi::remote(Record)]
#[uniffi(name = "ProductContact")]
pub struct Contact {
    pub blocked: bool,
    pub friend_gone: bool,
    pub contact_gone: bool,
    pub unconfirmed: bool,
    pub outdated: bool,
    pub key_changed: bool,
    pub verified: bool,
    pub has_user: bool,
    pub loaded: bool,
    pub sessions: u32,
    pub sessions_unready: bool,
}

#[uniffi::remote(Enum)]
#[uniffi(name = "ProductContactMark")]
pub enum ContactMark {
    Verified,
    Attention,
}

#[uniffi::export]
pub fn product_sessions_check(sessions: u32, unready: bool) -> Check {
    status::sessions_check(sessions, unready)
}

#[uniffi::export]
pub fn product_contact_check(contact: Contact) -> Check {
    status::contact_check(&contact)
}

#[uniffi::export]
pub fn product_contact_mark(contact: Contact) -> Option<ContactMark> {
    status::contact_mark(&contact)
}

#[uniffi::export]
pub fn product_contact_summary(contact: Contact, lang: Language) -> String {
    status::contact_summary(&contact, lang)
}

/// The held viewer's zoom (`yo_product::zoom`): scale around the center, then the offset.
#[derive(uniffi::Record)]
pub struct ProductHoldZoom {
    pub scale: f64,
    pub x: f64,
    pub y: f64,
}

#[uniffi::export]
pub fn product_hold_zoom(finger_x: f64, finger_y: f64, bar_bottom: f64, width: f64, height: f64) -> ProductHoldZoom {
    let zoom = yo_product::zoom::hold_zoom(finger_x, finger_y, bar_bottom, width, height);
    ProductHoldZoom { scale: zoom.scale, x: zoom.x, y: zoom.y }
}

/// The recording's zoom (`yo_product::zoom::record_zoom`): the camera's zoom for a filming finger drawn out of its bar.
#[uniffi::export]
pub fn product_record_zoom(
    finger_y: f64,
    bar_top: f64,
    bar_bottom: f64,
    height: f64,
    start: f64,
    camera_most: f64,
) -> f64 {
    yo_product::zoom::record_zoom(finger_y, bar_top, bar_bottom, height, start, camera_most)
}
