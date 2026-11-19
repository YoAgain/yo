use crate::ListKind;
use yo_product as product;

#[derive(Clone, Copy, uniffi::Enum)]
pub enum ProductLanguage {
    English,
    German,
}
impl From<ProductLanguage> for product::Language {
    fn from(value: ProductLanguage) -> Self {
        match value {
            ProductLanguage::English => Self::English,
            ProductLanguage::German => Self::German,
        }
    }
}

#[derive(Clone, Copy, uniffi::Enum)]
pub enum ProductText {
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
}
impl From<ProductText> for product::Text {
    fn from(value: ProductText) -> Self {
        match value {
            ProductText::Yo => Self::Yo,
            ProductText::YoBang => Self::YoBang,
            ProductText::Sent => Self::Sent,
            ProductText::SentYo => Self::SentYo,
            ProductText::SentPhoto => Self::SentPhoto,
            ProductText::SentVideo => Self::SentVideo,
            ProductText::Sending => Self::Sending,
            ProductText::NotSent => Self::NotSent,
            ProductText::Canceled => Self::Canceled,
            ProductText::NoAnswer => Self::NoAnswer,
            ProductText::NoAnswerYet => Self::NoAnswerYet,
            ProductText::NoYo => Self::NoYo,
            ProductText::NoYoBack => Self::NoYoBack,
            ProductText::YoBack => Self::YoBack,
            ProductText::YoBackActivity => Self::YoBackActivity,
            ProductText::StaleYo => Self::StaleYo,
            ProductText::TurnOnNotifications => Self::TurnOnNotifications,
            ProductText::NotificationsOff => Self::NotificationsOff,
            ProductText::NotificationsHint => Self::NotificationsHint,
            ProductText::NotificationFallback => Self::NotificationFallback,
            ProductText::Group => Self::Group,
            ProductText::FailedToSend => Self::FailedToSend,
            ProductText::WidgetEmpty => Self::WidgetEmpty,
            ProductText::WidgetFriend => Self::WidgetFriend,
            ProductText::WidgetFriendIntent => Self::WidgetFriendIntent,
            ProductText::WidgetFriendDescription => Self::WidgetFriendDescription,
            ProductText::WidgetListIntent => Self::WidgetListIntent,
            ProductText::WidgetListDescription => Self::WidgetListDescription,
            ProductText::WidgetBackDescription => Self::WidgetBackDescription,
            ProductText::ShareQueued => Self::ShareQueued,
            ProductText::ShareUnavailable => Self::ShareUnavailable,
            ProductText::ShareAddFriend => Self::ShareAddFriend,
            ProductText::VideoTooLong => Self::VideoTooLong,
            ProductText::NotOnYo => Self::NotOnYo,
            ProductText::GroupEmpty => Self::GroupEmpty,
            ProductText::ConfirmNewKey => Self::ConfirmNewKey,
            ProductText::Blocked => Self::Blocked,
            ProductText::NeedsUpdate => Self::NeedsUpdate,
            ProductText::AlreadyAdded => Self::AlreadyAdded,
        }
    }
}

#[derive(Clone, Copy, uniffi::Enum)]
pub enum ProductOutcome {
    Sending,
    Sent,
    Failed,
    Nobody,
}
impl From<ProductOutcome> for product::Outcome {
    fn from(value: ProductOutcome) -> Self {
        match value {
            ProductOutcome::Sending => Self::Sending,
            ProductOutcome::Sent => Self::Sent,
            ProductOutcome::Failed => Self::Failed,
            ProductOutcome::Nobody => Self::Nobody,
        }
    }
}

#[derive(Clone, Copy, uniffi::Enum)]
pub enum ProductActivityPlace {
    Expanded,
    Compact,
    LockScreen,
}
impl From<ProductActivityPlace> for product::ActivityPlace {
    fn from(value: ProductActivityPlace) -> Self {
        match value {
            ProductActivityPlace::Expanded => Self::Expanded,
            ProductActivityPlace::Compact => Self::Compact,
            ProductActivityPlace::LockScreen => Self::LockScreen,
        }
    }
}

#[derive(uniffi::Record)]
pub struct ProductWidgetFriend {
    pub id: String,
    pub name: String,
}
impl From<ProductWidgetFriend> for product::WidgetFriend {
    fn from(friend: ProductWidgetFriend) -> Self {
        Self { id: friend.id, name: friend.name }
    }
}

#[uniffi::export]
pub fn product_text(text: ProductText, lang: ProductLanguage) -> String {
    product::Text::from(text).get(lang.into()).into()
}

#[uniffi::export]
pub fn product_counted(count: i64) -> String {
    product::counted(count)
}

#[uniffi::export]
pub fn product_send_words(mark: String, sending: bool, lang: ProductLanguage) -> String {
    product::send_words(&mark, sending, lang.into())
}

#[uniffi::export]
pub fn product_arrival_lines(count: i64, name: String, lang: ProductLanguage) -> Vec<String> {
    product::arrival_lines(count, &name, lang.into())
}

#[uniffi::export]
pub fn product_failed_to_send(name: Option<String>, lang: ProductLanguage) -> String {
    product::failed_to_send(name.as_deref(), lang.into())
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
pub fn product_notification_line(name: String, kind: ListKind, request: bool, lang: ProductLanguage) -> String {
    product::notification_line(&name, kind.into(), request, lang.into())
}

#[uniffi::export]
pub fn product_notification_group_name(name: String, group: String, lang: ProductLanguage) -> String {
    product::notification_group_name(&name, &group, lang.into())
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
    outcome: ProductOutcome,
    place: ProductActivityPlace,
    stale: bool,
    count: i64,
    total: i64,
    has_others: bool,
    back: bool,
    lang: ProductLanguage,
) -> String {
    product::activity_words(outcome.into(), place.into(), stale, count, total, has_others, back, lang.into())
}

#[uniffi::export]
pub fn product_activity_names(name: String, others: Vec<String>, all: bool, nobody: bool) -> String {
    product::activity_names(&name, &others, all, nobody)
}

#[uniffi::export]
pub fn product_widget_selection(
    friends: Vec<ProductWidgetFriend>,
    picks: Vec<Option<ProductWidgetFriend>>,
    slots: u32,
    fallback: u32,
) -> Vec<u32> {
    let friends: Vec<_> = friends.into_iter().map(Into::into).collect();
    let picks: Vec<_> = picks.into_iter().map(|pick| pick.map(Into::into)).collect();
    product::widget_selection(&friends, &picks, slots as usize, fallback as usize)
}

#[uniffi::export]
pub fn product_share_subtitle(photos: u32, videos: u32, lang: ProductLanguage) -> String {
    product::share_subtitle(photos, videos, lang.into())
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
pub fn product_media_sign(kind: ListKind) -> String {
    product::media_sign(kind.into()).into()
}

#[uniffi::export]
pub fn product_media_kind(label: String) -> ListKind {
    match product::media_kind(&label) {
        product::list::Kind::Yo => ListKind::Yo,
        product::list::Kind::Photo => ListKind::Photo,
        product::list::Kind::Video => ListKind::Video,
    }
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

#[uniffi::export]
pub fn product_widget_eligible(blocked: bool, request: bool, gone: bool) -> bool {
    product::widget_eligible(blocked, request, gone)
}

#[uniffi::export]
pub fn product_widget_id_selection(
    friends: Vec<String>,
    picks: Vec<String>,
    signed_in: bool,
    slots: u32,
    fallback: u32,
) -> Vec<u32> {
    product::widget_id_selection(&friends, &picks, signed_in, slots as usize, fallback as usize)
}

#[uniffi::export]
pub fn product_android_notification_line(
    name: Option<String>,
    group: Option<String>,
    kind: ListKind,
    lang: ProductLanguage,
) -> String {
    product::android_notification_line(name.as_deref(), group.as_deref(), kind.into(), lang.into())
}

#[uniffi::export]
pub fn product_request_row(name: String, group: bool, lang: ProductLanguage) -> String {
    product::request_row(&name, group, lang.into())
}
