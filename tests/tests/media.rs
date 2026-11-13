//! Photos and videos end to end (spec 5, 6.8): a Yo that carries one, its blob uploaded first, fetched and opened by
//! the receiver once, then gone at the hub; a hub that says "not now" holds the upload until there is room.

use yo_core::media::{self, Media};
use yo_core::{Client, Content, Error, Event};
use yo_hub::config::Config;
use yo_tests::{phone, random_u64, register, Http, TestHub};

fn pair(hub: &TestHub) -> (Client, Client, String, String) {
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (phone(n), phone(n + 1));
    let a = register(Http::new(hub.url()), &pa, Some("ALICE"));
    let b = register(Http::new(hub.url()), &pb, Some("BOB"));
    (a, b, pa, pb)
}

/// The photos and videos among events, with who sent them.
fn media_in(events: &[Event]) -> Vec<(String, Content)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Message { user, content: c @ (Content::Photo(_) | Content::Video { .. }), .. } => {
                Some((user.clone(), c.clone()))
            }
            _ => None,
        })
        .collect()
}

/// What the app does: seal, upload for the chat's other devices, then send the Yo that carries it.
fn send_media(from: &mut Client, to: &str, plain: &[u8], video: Option<u32>) -> Result<Media, Error> {
    let (media, sealed) = media::seal(plain, 1080, 1920)?;
    let fetches = from.media_fetches(to).expect("a chat");
    from.upload(&media, fetches).send(&sealed)?;
    let content = match video {
        None => Content::Photo(media.clone()),
        Some(duration_ms) => Content::Video { media: media.clone(), duration_ms },
    };
    from.send(to, content)?;
    Ok(media)
}

#[test]
fn a_photo_and_a_video_go_with_their_yos_and_are_seen_once() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    // A chat first (the app sends the first Yo of a new friend plain, or opens the chat before the upload).
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    assert_eq!(a.media_fetches(&pb), Some(1), "one device on the other side, none other of A's");

    let photo = b"a JPEG, as the app makes it".repeat(1000);
    let video = b"an MP4".repeat(5000);
    send_media(&mut a, &pb, &photo, None).unwrap();
    send_media(&mut a, &pb, &video, Some(9_800)).unwrap();
    let events = b.sync(0).unwrap();
    let got = media_in(&events);
    assert_eq!(got.len(), 2, "{events:?}");
    let from_a = yo_tests::user_of(&pa);
    assert!(got.iter().all(|(user, _)| *user == from_a));
    let transport = b.transport();
    let (Content::Photo(p), Content::Video { media: v, duration_ms: 9_800 }) = (&got[0].1, &got[1].1) else {
        panic!("{got:?}")
    };
    assert_eq!((p.width, p.height), (1080, 1920));
    assert_eq!(media::fetch_open(transport.as_ref(), p).unwrap().unwrap(), photo);
    assert_eq!(media::fetch_open(transport.as_ref(), v).unwrap().unwrap(), video);
    // Fetched by every device it was for: gone at the hub. The Yo stays a Yo for whoever comes late.
    assert_eq!(media::fetch(transport.as_ref(), p).unwrap(), None);
    assert_eq!(media::fetch(transport.as_ref(), v).unwrap(), None);
    assert_eq!(a.dropped() + b.dropped(), 0);
}

#[test]
fn a_blocked_sender_s_photos_are_dropped_unseen() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    b.set_blocked(&pa, true).unwrap();
    send_media(&mut a, &pb, b"unwanted", None).unwrap();
    assert!(media_in(&b.sync(0).unwrap()).is_empty());
}

#[test]
fn a_full_hub_holds_the_upload_and_the_yo_until_there_is_room() {
    let mut config = Config::for_tests();
    config.limits.blobs_per_user_day = 2;
    config.limits.blob_bytes_stored_per_user = 50_000;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let (mut a, mut b, _, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    let first = send_media(&mut a, &pb, &[1; 40_000], None).unwrap();
    // Waiting for its fetch, the first takes A's room: the second is "later", and its Yo is not sent.
    let refused = send_media(&mut a, &pb, &[2; 20_000], None).unwrap_err();
    assert_eq!(refused, Error::Refused(429));
    assert!(media::retry_later(&refused));
    let events = b.sync(0).unwrap();
    assert_eq!(media_in(&events).len(), 1, "only the first came");
    // B fetches it: room again, the second goes, signed anew.
    media::fetch(b.transport().as_ref(), &first).unwrap().unwrap();
    send_media(&mut a, &pb, &[2; 20_000], None).unwrap();
    assert_eq!(media_in(&b.sync(0).unwrap()).len(), 1);
    // The day's count is used up: later again (the next day it goes).
    assert_eq!(send_media(&mut a, &pb, &[3; 100], None).unwrap_err(), Error::Refused(429));
}

#[test]
fn a_client_that_forgets_media_keys_keeps_only_the_id() {
    let hub = TestHub::start();
    let (mut a, mut b, _, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    b.forget_media_keys();
    let sent = send_media(&mut a, &pb, b"for a bot", None).unwrap();
    let got = media_in(&b.sync(0).unwrap());
    let [(_, Content::Photo(media))] = got.as_slice() else { panic!("{got:?}") };
    assert_eq!((media.id, media.key), (sent.id, [0; 32]));
    // The blob can still be fetched into nothing, so the hub deletes it.
    assert!(media::fetch(b.transport().as_ref(), media).unwrap().is_some());
}
