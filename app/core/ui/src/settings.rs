//! Settings menu shared by platform. The mobile apps still have their own menus. Consuming this data is a follow-up.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    Ios,
    Android,
    Cli,
}

#[derive(PartialEq, Eq)]
pub struct Item {
    pub id: &'static str,
    pub title: &'static str,
    pub platforms: &'static [Platform],
}

pub struct Section {
    pub id: &'static str,
    pub title: &'static str,
    pub items: &'static [Item],
}

pub const TERMS_URL: &str = "https://justyo.co/p/terms";
pub const PRIVACY_URL: &str = "https://justyo.co/p/privacy";

const MOBILE: &[Platform] = &[Platform::Ios, Platform::Android];
const ALL: &[Platform] = &[Platform::Ios, Platform::Android, Platform::Cli];

/// In order. The account's own rows first and Sign Out last in it; devices are listed under "devices" by the
/// platform (each device a row), "link_device" after them.
pub const SECTIONS: &[Section] = &[
    Section {
        id: "account",
        title: "Account",
        items: &[
            Item { id: "profile", title: "Your Code", platforms: ALL },
            Item { id: "recovery_key", title: "Recovery Key", platforms: ALL },
            Item { id: "passkey", title: "Passkey", platforms: MOBILE },
            Item { id: "delete_account", title: "Delete Account", platforms: MOBILE },
            Item { id: "sign_out", title: "Sign Out", platforms: ALL },
        ],
    },
    Section {
        id: "friends",
        title: "Friends",
        items: &[
            Item { id: "add_friend", title: "Add Friend", platforms: MOBILE },
            Item { id: "requests", title: "Requests from Strangers", platforms: ALL },
            Item { id: "blocked", title: "Blocked", platforms: ALL },
        ],
    },
    Section {
        id: "devices",
        title: "Devices",
        items: &[
            Item { id: "devices", title: "Devices", platforms: ALL },
            Item { id: "link_device", title: "Link a New Device", platforms: ALL },
        ],
    },
    Section { id: "look", title: "Look", items: &[Item { id: "look", title: "Look", platforms: MOBILE }] },
    Section {
        id: "about",
        title: "About",
        items: &[
            Item { id: "status", title: "Status", platforms: ALL },
            Item { id: "about", title: "About & Legal", platforms: ALL },
            Item { id: "feedback", title: "Feedback", platforms: ALL },
        ],
    },
];

pub fn sections(platform: Platform) -> Vec<(&'static Section, Vec<&'static Item>)> {
    SECTIONS
        .iter()
        .filter_map(|section| {
            let items: Vec<_> = section.items.iter().filter(|item| item.platforms.contains(&platform)).collect();
            (!items.is_empty()).then_some((section, items))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(platform: Platform) -> (Vec<&'static str>, Vec<&'static str>) {
        let menu = sections(platform);
        (
            menu.iter().map(|(s, _)| s.id).collect(),
            menu.iter().flat_map(|(_, items)| items.iter().map(|i| i.id)).collect(),
        )
    }

    #[test]
    fn menu_order_and_visibility() {
        assert_eq!(
            ids(Platform::Cli),
            (
                vec!["account", "friends", "devices", "about"],
                vec![
                    "profile",
                    "recovery_key",
                    "sign_out",
                    "requests",
                    "blocked",
                    "devices",
                    "link_device",
                    "status",
                    "about",
                    "feedback"
                ]
            )
        );
        for platform in [Platform::Ios, Platform::Android] {
            assert_eq!(
                ids(platform),
                (
                    vec!["account", "friends", "devices", "look", "about"],
                    vec![
                        "profile",
                        "recovery_key",
                        "passkey",
                        "delete_account",
                        "sign_out",
                        "add_friend",
                        "requests",
                        "blocked",
                        "devices",
                        "link_device",
                        "look",
                        "status",
                        "about",
                        "feedback"
                    ]
                )
            );
        }
    }
}
