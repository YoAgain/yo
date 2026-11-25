//! A service's TOML, read before anything of its script runs. Every field is required, an unknown one is refused.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    /// The Yo names of its accounts; the first is its folder's.
    pub names: Vec<String>,
    pub about: String,
    pub triggers: Vec<Trigger>,
    /// Only of a service that users connect with another one.
    pub bridge: Option<Bridge>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bridge {
    /// Where a connected user goes back to.
    pub return_url: String,
    /// Where a user who is not connected reads how to.
    pub help_url: String,
    /// What it may call, anything below.
    pub call_urls: Vec<String>,
    /// The header with the other side's key.
    pub key_header: String,
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
            // The list's place at the API's address.
            () if folder == "services" => Err("names: not services".into()),
            () if self.triggers.is_empty() => Err("triggers: none".into()),
            () if self.has(Trigger::Webhook) => Err("triggers: a webhook comes with a bridge".into()),
            () if self.bridge.as_ref().is_some_and(|c| !c.good()) => Err("bridge: plain https addresses".into()),
            () => Ok(()),
        }
    }

    /// Whether it may call `url`: a plain path below one of its `call_urls`.
    pub fn may_call(&self, url: &str) -> bool {
        self.bridge.iter().flat_map(|c| &c.call_urls).any(|allowed| below(allowed, url).is_some())
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

impl Bridge {
    fn good(&self) -> bool {
        let header = |b: u8| b.is_ascii_alphanumeric() || b == b'-';
        let host =
            |url: &str| below("https://", url).filter(|rest| !rest.starts_with('/')).map(|rest| rest.contains('/'));
        host(&self.return_url) == Some(true)
            && host(&self.help_url) == Some(true)
            && self.help_url.len() <= 128
            && self.call_urls.iter().all(|url| url.ends_with('/') && host(url) == Some(true))
            && (1..=64).contains(&self.key_header.len())
            && self.key_header.bytes().all(header)
    }
}

/// The rest of `url` after `start`: letters, digits, `.`, `_`, `-` and `/` only, and no way up.
fn below<'a>(start: &str, url: &'a str) -> Option<&'a str> {
    let rest = url.strip_prefix(start)?;
    let plain = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/');
    (rest.bytes().all(plain) && !rest.contains("..") && !rest.contains("//")).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "names = [\"YoGPT\"]\nabout = \"a\"\ntriggers = [\"yo\"]\n";

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
            ("triggers", "allowed_urls = []\ntriggers"),
        ] {
            let bad = GOOD.replacen(from, to, 1);
            assert_ne!(bad, GOOD);
            assert!(Header::parse(&bad, "yogpt").is_err(), "{bad}");
        }
        assert!(Header::parse("", "").is_err());
        let with = |back: &str, call: &str| {
            let connect =
                format!("[bridge]\nreturn_url = \"{back}\"\ncall_urls = [\"{call}\"]\nkey_header = \"X-Key\"\nhelp_url = \"{back}\"\n");
            Header::parse(&(GOOD.to_owned() + &connect), "yogpt")
        };
        assert!(Header::parse(&(GOOD.to_owned() + "[bridge]\nkey_header = \"X\"\n"), "yogpt").is_err());
        let header = with("https://example.com/back", "https://example.com/a/").unwrap();
        assert!(header.may_call("https://example.com/a/b") && header.may_call("https://example.com/a/"));
        for bad in
            ["https://example.com/b", "https://example.com/a/../b", "https://example.com/a/?x", "https://x.com/a/"]
        {
            assert!(!header.may_call(bad), "{bad}");
        }
        for connect in ["http://example.com/a", "https://example.com", "https://example.com/a?b", "https://a@b.com/x"] {
            assert!(with(connect, "https://example.com/").is_err(), "{connect}");
        }
        for url in ["http://example.com/", "https://example.com", "https:///"] {
            assert!(with("https://example.com/back", url).is_err(), "{url}");
        }
        let two = Header::parse(&GOOD.replace("\"YoGPT\"", "\"PING\", \"PINGTEST\""), "ping").unwrap();
        assert!(two.probed() && two.names.len() == 2);
        assert!(Header::parse(&GOOD.replace("[\"yo\"]", "[\"yo\", \"cron\"]"), "yogpt").unwrap().remembers());
    }
}
