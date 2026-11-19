//! The product rules only the Android app uses (`yo_product`'s Android variants).

use yo_product::list::Kind;
use yo_product::{self as product, Language};

/// Whether a friend may be on a widget: not blocked, not a request, not gone.
#[uniffi::export]
pub fn product_widget_eligible(blocked: bool, request: bool, gone: bool) -> bool {
    product::widget_eligible(blocked, request, gone)
}

/// The widget's rows: the picked ids that still exist, else the head of the list (indexes into `friends`).
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

/// A notification's line: "Yo from Mia in #Hey 📷", or the fallback when the sender is unknown.
#[uniffi::export]
pub fn product_android_notification_line(
    name: Option<String>,
    group: Option<String>,
    kind: Kind,
    lang: Language,
) -> String {
    product::android_notification_line(name.as_deref(), group.as_deref(), kind, lang)
}

/// A request row's title: "Yo Request from Mia", or "Yo Request: #Hey" for a group.
#[uniffi::export]
pub fn product_request_row(name: String, group: bool, lang: Language) -> String {
    product::request_row(&name, group, lang)
}
