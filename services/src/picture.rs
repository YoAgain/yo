//! A picture the other side of a connection named: a JPEG from a public https address, and nothing from this machine or
//! its network. The address checked is the address asked: the name is looked up once, here.

use crate::gatekeeper::action::MAX_PICTURE;
use reqwest::blocking::Client;
use reqwest::Url;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

const TIME: Duration = Duration::from_secs(10);

/// Or whether trying again may help.
pub fn fetch(url: &str) -> Result<Vec<u8>, bool> {
    let url = Url::parse(url).ok().filter(|u| u.scheme() == "https" && u.username().is_empty() && u.port().is_none());
    let url = url.ok_or(false)?;
    // A name, not an address written out: those end in a digit or a bracket.
    let host = url.host_str().filter(|h| h.ends_with(|c: char| c.is_ascii_alphabetic())).ok_or(false)?.to_owned();
    let addresses: Vec<SocketAddr> = (host.as_str(), 443).to_socket_addrs().map_err(|_| true)?.collect();
    if addresses.is_empty() || !addresses.iter().all(|a| public(a.ip())) {
        return Err(false);
    }
    let client = Client::builder()
        .timeout(TIME)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&host, &addresses)
        .build()
        .map_err(|_| true)?;
    let answer = client.get(url).send().map_err(|_| true)?;
    let status = answer.status();
    if !status.is_success() {
        return Err(status.is_server_error() || matches!(status.as_u16(), 408 | 429));
    }
    let mut jpeg = Vec::new();
    answer.take(MAX_PICTURE as u64 + 1).read_to_end(&mut jpeg).map_err(|_| true)?;
    (jpeg.len() <= MAX_PICTURE && jpeg.starts_with(b"\xff\xd8\xff")).then_some(jpeg).ok_or(false)
}

fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        // Global unicast alone (2000::/3), without what carries an IPv4 address (Teredo, 6to4) or is documentation.
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => public_v4(v4),
            None => {
                let s = ip.segments();
                (s[0] & 0xe000) == 0x2000 && s[0] != 0x2002 && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
            }
        },
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_documentation()
        || a == 0
        || a >= 224
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (18..20).contains(&b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_https_names() {
        for ip in ["127.0.0.1", "10.1.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fe80::1", "fd00::1"] {
            assert!(!public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["::ffff:127.0.0.1", "2002:7f00:1::", "64:ff9b::7f00:1", "255.255.255.255"] {
            assert!(!public(ip.parse().unwrap()), "{ip}");
        }
        assert!(public("1.1.1.1".parse().unwrap()) && public("2606:4700::1111".parse().unwrap()));
        for url in
            ["http://a.com/x", "https://127.0.0.1/x", "https://[::1]/x", "https://localhost/x", "https://u@a.com/x"]
        {
            assert_eq!(fetch(url), Err(false), "{url}");
        }
    }
}
