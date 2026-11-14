//! Photos and videos at the hub (spec 6.8): blobs uploaded signed, fetched by id, gone after their last fetch or their
//! time; quotas, the budget and the disk's free space; over HTTP against a real hub.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use yo_core::media::{self, Media};
use yo_core::{Client, Error};
use yo_hub::config::Config;
use yo_tests::{random_u64, register, username, Http, TestHub};
use yo_wire::api;
use yo_wire::limits::MAX_BLOB_LEN;

const DAY_MS: u64 = 24 * 3600 * 1000;

fn hub_with(change: impl FnOnce(&mut Config)) -> (TestHub, PathBuf) {
    let mut config = Config::for_tests();
    change(&mut config);
    let dir = config.blobs.dir.clone();
    (TestHub::start_with(config, "127.0.0.1:0"), dir)
}

fn someone(hub: &TestHub) -> (Client, Arc<Http>) {
    let http = Http::new(hub.url());
    (register(http.clone(), &username(random_u64() % 1_000_000_000), None), http)
}

fn noise(len: usize) -> Vec<u8> {
    let mut seed = random_u64() | 1;
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect()
}

/// Seals `len` bytes and uploads them for `fetches` fetches.
fn upload(client: &mut Client, len: usize, fetches: u32) -> (Media, Vec<u8>, Result<(), Error>) {
    let plain = noise(len);
    let (media, sealed) = media::seal(&plain, 1200, 1600).unwrap();
    let sent = client.upload(&media, fetches).unwrap().send(&sealed);
    (media, plain, sent)
}

/// Blob files on disk, and uploads under way.
fn files(dir: &Path) -> (usize, usize) {
    let mut stored = 0;
    for shard in std::fs::read_dir(dir).unwrap() {
        let shard = shard.unwrap();
        if shard.file_name() != "tmp" {
            stored += std::fs::read_dir(shard.path()).unwrap().count();
        }
    }
    (stored, std::fs::read_dir(dir.join("tmp")).unwrap().count())
}

/// No blob files left: the last fetch deletes its blob once its last byte is handed on, a moment after the client may
/// have it.
fn all_gone(dir: &Path) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while files(dir) != (0, 0) {
        assert!(std::time::Instant::now() < until, "{:?}", files(dir));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn status(request: reqwest::blocking::RequestBuilder) -> u16 {
    request.send().unwrap().status().as_u16()
}

#[test]
fn a_blob_is_fetched_by_each_device_and_gone_after_the_last() {
    let (hub, dir) = hub_with(|_| {});
    let (mut alice, _) = someone(&hub);
    let (media, plain, sent) = upload(&mut alice, 300_000, 2);
    sent.unwrap();
    assert_eq!(files(&dir), (1, 0));
    // Fetched without identity, by its id alone: twice, then gone, file and all.
    let stranger = Http::new(hub.url());
    assert_eq!(media::fetch_open(stranger.as_ref(), &media).unwrap().unwrap(), plain);
    assert_eq!(media::fetch_open(stranger.as_ref(), &media).unwrap().unwrap(), plain);
    assert_eq!(media::fetch(stranger.as_ref(), &media).unwrap(), None);
    all_gone(&dir);
    // A wrong key or hash opens nothing.
    let (other, _, sent) = upload(&mut alice, 1000, 2);
    sent.unwrap();
    let wrong = Media { key: [0; 32], ..other.clone() };
    assert_eq!(media::fetch_open(stranger.as_ref(), &wrong), Err(Error::Protocol("media")));
    let lied = Media { sha256: [0; 32], ..other };
    assert_eq!(media::fetch(stranger.as_ref(), &lied), Err(Error::Protocol("media hash")));
}

#[test]
fn the_largest_blob_goes_and_comes_and_a_larger_one_is_refused() {
    let (hub, dir) = hub_with(|_| {});
    let (mut alice, _) = someone(&hub);
    let (media, plain, sent) = upload(&mut alice, media::MAX_MEDIA_LEN, 1);
    sent.unwrap();
    assert_eq!(media::fetch_open(Http::new(hub.url()).as_ref(), &media).unwrap().unwrap(), plain);
    assert!(media::seal(&noise(media::MAX_MEDIA_LEN + 1), 1, 1).is_err());
    // Refused by its length, before the body is read or anyone is asked who sends it.
    use std::io::{Read, Write};
    let mut socket = std::net::TcpStream::connect(hub.addr).unwrap();
    let path = api::blob_put_path(&[9; 16], 1);
    write!(socket, "PUT {path} HTTP/1.1\r\nHost: hub\r\nContent-Length: {}\r\n\r\n", MAX_BLOB_LEN + 1).unwrap();
    let mut answer = [0u8; 12];
    socket.read_exact(&mut answer).unwrap();
    assert_eq!(&answer, b"HTTP/1.1 413");
    all_gone(&dir);
}

/// Fetches at once never pass a blob's count: each takes its fetch as it starts. One cut off gives it back.
#[test]
fn fetches_at_once_take_no_more_than_the_blob_has_and_one_cut_off_gives_its_fetch_back() {
    let (hub, dir) = hub_with(|_| {});
    let (mut alice, _) = someone(&hub);
    // The largest blob: more than the sockets hold, so a fetch not read is not complete.
    let (media, plain, sent) = upload(&mut alice, media::MAX_MEDIA_LEN, 1);
    sent.unwrap();
    let url = format!("{}{}", hub.url(), api::blob_path(&media.id));
    let started = reqwest::blocking::get(&url).unwrap();
    assert_eq!(started.status().as_u16(), 200);
    assert_eq!(status(reqwest::blocking::Client::new().get(&url)), 404, "its one fetch is under way");
    drop(started);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let stranger = Http::new(hub.url());
    let opened = loop {
        if let Some(opened) = media::fetch_open(stranger.as_ref(), &media).unwrap() {
            break opened;
        }
        assert!(std::time::Instant::now() < until, "the fetch cut off was given back");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(opened, plain);
    assert_eq!(media::fetch(stranger.as_ref(), &media).unwrap(), None);
    all_gone(&dir);
}

#[test]
fn an_upload_must_be_signed_whole_and_once() {
    let (hub, dir) = hub_with(|_| {});
    let (mut alice, http) = someone(&hub);
    let url = hub.url();
    let client = reqwest::blocking::Client::new();
    let (media, sealed) = media::seal(&noise(5000), 10, 10).unwrap();
    let path = api::blob_put_path(&media.id, 1);
    let hash = (api::HEADER_BODY_HASH, yo_wire::b64(&media.sha256));
    // Checked before anything else: a length (`411` without, sent chunked) of a tag at least (`400`).
    let chunked = reqwest::blocking::Body::new(std::io::Cursor::new(sealed.clone()));
    assert_eq!(status(client.put(format!("{url}{path}")).header(hash.0, &hash.1).body(chunked)), 411);
    assert_eq!(status(client.put(format!("{url}{path}")).header(hash.0, &hash.1).body(vec![0; 15])), 400);
    // Unsigned.
    assert_eq!(status(client.put(format!("{url}{path}")).header(hash.0, &hash.1).body(sealed.clone())), 401);
    // A body that is not the signed one: refused, nothing kept.
    let tampered = {
        let mut bytes = sealed.clone();
        bytes[100] ^= 1;
        bytes
    };
    assert_eq!(alice.upload(&media, 1).unwrap().send(&tampered), Err(Error::Refused(400)));
    assert_eq!(files(&dir), (0, 0));
    // Fetches 1 to 32, checked first.
    for (n, refused) in [(0, 400), (33, 400), (32, 401)] {
        let path = api::blob_put_path(&media.id, n);
        assert_eq!(status(client.put(format!("{url}{path}")).header(hash.0, &hash.1).body(sealed.clone())), refused);
    }
    // Stored once; the same signed request again is a replay.
    alice.upload(&media, 1).unwrap().send(&sealed).unwrap();
    let (method, path, headers, body) = http.recorded.lock().unwrap().last().cloned().unwrap();
    let mut replay = client.request(method.parse().unwrap(), format!("{url}{path}")).body(body);
    for (name, value) in headers {
        replay = replay.header(name, value);
    }
    assert_eq!(status(replay), 401);
    // The same id again, signed anew: taken (the core takes that for its own lost answer).
    let again = media::seal(&noise(10), 1, 1).unwrap();
    let same_id = Media { id: media.id, ..again.0 };
    assert_eq!(alice.upload(&same_id, 1).unwrap().send(&again.1), Ok(()));
    assert_eq!(files(&dir), (1, 0));
    // Not a blob id: no such route.
    assert_eq!(status(client.get(format!("{url}/v1/blobs/AAAA"))), 404);
}

#[test]
fn quotas_per_user_and_day_and_waiting_bytes() {
    let (hub, _) = hub_with(|config| {
        config.limits.blobs_per_user_day = 3;
        config.limits.blob_bytes_stored_per_user = 25_000;
    });
    let (mut alice, _) = someone(&hub);
    let (mut bob, _) = someone(&hub);
    for _ in 0..3 {
        upload(&mut alice, 100, 1).2.unwrap();
    }
    let refused = upload(&mut alice, 100, 1).2.unwrap_err();
    assert_eq!(refused, Error::Refused(429));
    assert!(media::retry_later(&refused), "the app tries again later");
    // Someone else's quota is their own.
    let (waiting, _, sent) = upload(&mut bob, 20_000, 1);
    sent.unwrap();
    // Bytes waiting for their fetches: more only once these are fetched.
    assert_eq!(upload(&mut bob, 10_000, 1).2, Err(Error::Refused(429)));
    media::fetch(Http::new(hub.url()).as_ref(), &waiting).unwrap().unwrap();
    upload(&mut bob, 10_000, 1).2.unwrap();
}

#[test]
fn the_budget_and_the_free_disk_hold_uploads_until_there_is_room() {
    let (hub, _) = hub_with(|config| config.blobs.budget_bytes = 30_000);
    let (mut alice, _) = someone(&hub);
    let (first, _, sent) = upload(&mut alice, 20_000, 1);
    sent.unwrap();
    let full = upload(&mut alice, 20_000, 1).2.unwrap_err();
    assert_eq!(full, Error::Refused(507));
    assert!(media::retry_later(&full));
    // Fetched, deleted: room again.
    media::fetch(Http::new(hub.url()).as_ref(), &first).unwrap().unwrap();
    upload(&mut alice, 20_000, 1).2.unwrap();

    let (hub, _) = hub_with(|config| config.blobs.min_free_bytes = u64::MAX / 4);
    let (mut alice, _) = someone(&hub);
    assert_eq!(upload(&mut alice, 100, 1).2, Err(Error::Refused(507)));
}

#[test]
fn a_blob_lives_thirty_days_at_most() {
    let (hub, dir) = hub_with(|_| {});
    let (mut alice, _) = someone(&hub);
    let (media, _, sent) = upload(&mut alice, 1000, 3);
    sent.unwrap();
    let http = Http::new(hub.url());
    media::fetch(http.as_ref(), &media).unwrap().unwrap();
    let now = yo_hub::now_ms();
    hub.hub.prune(now + 29 * DAY_MS).unwrap();
    assert!(media::fetch(http.as_ref(), &media).unwrap().is_some(), "within its time");
    hub.hub.prune(now + 31 * DAY_MS).unwrap();
    assert_eq!(media::fetch(http.as_ref(), &media).unwrap(), None);
    assert_eq!(files(&dir), (0, 0));
}

#[test]
fn a_deleted_account_takes_its_blobs_and_strays_are_swept() {
    let (hub, dir) = hub_with(|_| {});
    let (mut alice, _) = someone(&hub);
    let (mut bob, _) = someone(&hub);
    let (media, _, sent) = upload(&mut alice, 1000, 1);
    sent.unwrap();
    let (kept, _, sent) = upload(&mut bob, 1000, 1);
    sent.unwrap();
    alice.delete_account().unwrap();
    let http = Http::new(hub.url());
    assert_eq!(media::fetch(http.as_ref(), &media).unwrap(), None);
    assert_eq!(files(&dir), (1, 0));
    // A file no blob owns, an upload left behind long ago: gone with the hourly cleanup. Bob's stays.
    std::fs::create_dir_all(dir.join("ab")).unwrap();
    std::fs::write(dir.join("ab").join("ab".repeat(16)), b"stray").unwrap();
    let left = std::fs::File::create(dir.join("tmp").join("cut-off")).unwrap();
    left.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600)).unwrap();
    std::fs::write(dir.join("tmp").join("under-way"), b"").unwrap();
    assert_eq!(files(&dir), (2, 2));
    hub.hub.prune(yo_hub::now_ms()).unwrap();
    assert_eq!(files(&dir), (1, 1), "the recent upload stays");
    assert!(media::fetch(http.as_ref(), &kept).unwrap().is_some());
}

#[test]
fn fetches_and_uploads_are_limited_per_address() {
    let (hub, _) = hub_with(|config| {
        config.limits.blob_get_per_ip_minute = 2;
        config.limits.blob_put_per_ip_minute = 2;
    });
    let (mut alice, _) = someone(&hub);
    let (media, _, sent) = upload(&mut alice, 100, 5);
    sent.unwrap();
    upload(&mut alice, 100, 1).2.unwrap();
    assert_eq!(upload(&mut alice, 100, 1).2, Err(Error::Refused(429)));
    let http = Http::new(hub.url());
    media::fetch(http.as_ref(), &media).unwrap().unwrap();
    media::fetch(http.as_ref(), &media).unwrap().unwrap();
    assert_eq!(media::fetch(http.as_ref(), &media), Err(Error::Refused(429)));
}
