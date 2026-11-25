//! A picture the other side of a connection named: a JPEG from a public https address, and nothing from this machine or
//! its network. The address checked is the address asked: the name is looked up once, here.

use crate::gatekeeper::action::MAX_PICTURE;
use reqwest::{Client, Response, Url};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

/// The name's lookup, and all that follows.
const TIME: Duration = Duration::from_secs(10);

/// Or whether trying again may help.
pub fn fetch(url: &str) -> Result<Vec<u8>, bool> {
    fetch_within(url, TIME)
}

/// One deadline, from the name's lookup to the last byte.
fn fetch_within(url: &str, time: Duration) -> Result<Vec<u8>, bool> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_| true)?;
    let jpeg = runtime.block_on(async { tokio::time::timeout(time, get(url)).await });
    // A lookup still under way ends by itself.
    runtime.shutdown_background();
    jpeg.unwrap_or(Err(true))
}

async fn get(url: &str) -> Result<Vec<u8>, bool> {
    let url = Url::parse(url).ok().filter(|u| u.scheme() == "https" && u.username().is_empty() && u.port().is_none());
    let url = url.ok_or(false)?;
    // A name, not an address written out: those end in a digit or a bracket.
    let host = url.host_str().filter(|h| h.ends_with(|c: char| c.is_ascii_alphabetic())).ok_or(false)?.to_owned();
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 443)).await.map_err(|_| true)?.collect();
    if addresses.is_empty() || !addresses.iter().all(|a| public(a.ip())) {
        return Err(false);
    }
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&host, &addresses)
        .build()
        .map_err(|_| true)?;
    body(client.get(url).send().await.map_err(|_| true)?).await
}

async fn body(mut answer: Response) -> Result<Vec<u8>, bool> {
    let status = answer.status();
    if !status.is_success() {
        return Err(status.is_server_error() || matches!(status.as_u16(), 408 | 429));
    }
    let mut jpeg = Vec::new();
    while let Some(part) = answer.chunk().await.map_err(|_| true)? {
        jpeg.extend_from_slice(&part);
        if jpeg.len() > MAX_PICTURE {
            return Err(false);
        }
    }
    jpeg.starts_with(b"\xff\xd8\xff").then_some(jpeg).ok_or(false)
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

    /// A body that drips on and on ends at the deadline, not at the last byte.
    #[test]
    fn a_slow_picture_is_not_waited_for() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 1000\r\n\r\n\xff\xd8\xff");
            while stream.write_all(b"x").is_ok() {
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let began = std::time::Instant::now();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let done = runtime.block_on(async {
            let answer = Client::new().get(format!("http://{address}/")).send().await.unwrap();
            tokio::time::timeout(Duration::from_millis(400), body(answer)).await
        });
        assert!(done.is_err() && began.elapsed() < Duration::from_secs(5));
        assert_eq!(fetch_within("https://example.com/a.jpg", Duration::ZERO), Err(true));
    }

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
