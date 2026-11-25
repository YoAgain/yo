//! A service's TOML, read before anything of its script runs. Every field is required, an unknown one is refused.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    /// The Yo names of its accounts; the first is its folder's.
    pub names: Vec<String>,
    pub about: String,
    pub triggers: Vec<Trigger>,
    pub allowed_urls: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    Yo,
    Cron,
    Webhook,
}

impl Trigger {
    pub fn word(self) -> &'static str {
        match self {
            Trigger::Yo => "yo",
            Trigger::Cron => "cron",
            Trigger::Webhook => "webhook",
        }
    }
}

impl Header {
    /// `folder`: the service's, its first name in lower case.
    pub fn parse(text: &str, folder: &str) -> Result<Header, String> {
        let header: Header = toml::from_str(text).map_err(|e| e.message().to_owned())?;
        header.check(folder)?;
        Ok(header)
    }

    fn check(&self, folder: &str) -> Result<(), String> {
        let name = |name: &String| {
            (1..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
        };
        let first = self.names.first().ok_or("names: none")?;
        let twice = |name: &String| self.names.iter().filter(|n| n.eq_ignore_ascii_case(name)).count() > 1;
        match () {
            () if !self.names.iter().all(name) => Err("names: letters, digits, . and _".into()),
            () if self.names.iter().any(twice) => Err("names: one twice".into()),
            () if first.to_ascii_lowercase() != folder => Err("the folder is not named after it".into()),
            () if self.triggers.is_empty() => Err("triggers: none".into()),
            () if self.has(Trigger::Webhook) || !self.allowed_urls.is_empty() => {
                Err("webhook and allowed_urls are not built yet".into())
            }
            () => Ok(()),
        }
    }

    pub fn has(&self, trigger: Trigger) -> bool {
        self.triggers.contains(&trigger)
    }

    /// Whether it may be reminded of a user later. Without it nothing is kept of anyone.
    pub fn remembers(&self) -> bool {
        self.has(Trigger::Cron)
    }

    /// The one service that gets the apps' round-trip probes.
    pub fn probed(&self) -> bool {
        self.names.first().is_some_and(|n| n == "PING")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "names = [\"YoGPT\"]\nabout = \"a\"\ntriggers = [\"yo\"]\nallowed_urls = []\n";

    #[test]
    fn a_toml_is_read_whole_or_refused() {
        let header = Header::parse(GOOD, "yogpt").unwrap();
        assert_eq!((header.names[0].as_str(), header.remembers(), header.probed()), ("YoGPT", false, false));
        assert!(Header::parse(GOOD, "ping").is_err());
        for (from, to) in [
            ("about = \"a\"\n", ""),
            ("about", "root = true\nabout"),
            ("about", "print(1)\nabout"),
            ("\"YoGPT\"", "\"../x\""),
            ("\"YoGPT\"", "\"PING\""),
            ("\"YoGPT\"", ""),
            ("\"YoGPT\"", "\"YoGPT\", \"yogpt\""),
            ("\"YoGPT\"", "\"YoGPT\", \"a b\""),
            ("[\"YoGPT\"]", "\"YoGPT\""),
            ("\"a\"", "1"),
            ("[\"yo\"]", "[]"),
            ("[\"yo\"]", "[\"root\"]"),
            ("[\"yo\"]", "[\"yo\", \"webhook\"]"),
            ("urls = []", "urls = [\"https://example.com/\"]"),
        ] {
            let bad = GOOD.replacen(from, to, 1);
            assert_ne!(bad, GOOD);
            assert!(Header::parse(&bad, "yogpt").is_err(), "{bad}");
        }
        assert!(Header::parse("", "").is_err());
        let two = Header::parse(&GOOD.replace("\"YoGPT\"", "\"PING\", \"PINGTEST\""), "ping").unwrap();
        assert!(two.probed() && two.names.len() == 2);
        assert!(Header::parse(&GOOD.replace("[\"yo\"]", "[\"yo\", \"cron\"]"), "yogpt").unwrap().remembers());
    }
}
