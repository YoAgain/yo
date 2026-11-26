use std::{net::SocketAddr, sync::Arc};
use yo_core::{media, Client, Content, Event, Response, Transport};
use yo_hub::{config::Config, Hub};
use yo_tests::{Http, TestHub};
use yoip::net::api::Push;

fn config() -> Config {
    Config { add_devices: true, ..Config::for_tests() }
}

fn exercise(http: Arc<dyn Transport>) {
    let register = |name, secret| {
        let mut c = Client::register_username(http.clone(), name, secret, None, Push::none()).unwrap();
        c.set_hub_keys(vec![yo_tests::hub_key()]);
        c
    };
    let mut a2 = register("First", None);
    let mut b = register("Second", None);
    let yos =
        |events: &[Event]| events.iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count();
    let added = |events: &[Event]| events.iter().filter(|e| matches!(e, Event::DeviceAdded { .. })).count();
    b.send_yo(a2.user_id()).unwrap();
    assert_eq!(yos(&a2.sync(0).unwrap()), 1);
    a2.accept(b.user_id()).unwrap();
    a2.maintenance().unwrap();

    // A1 joins the account: A2, there before it, is trusted at once; A1's first word (a storage hint) tells A2 of
    // the new device.
    let mut a1 = register("First", Some(a2.account_secret()));
    assert_eq!(a1.user_id(), a2.user_id());
    assert_eq!(a1.saved().state.siblings.len(), 1);
    assert!(a1.saved().state.siblings_retired.is_empty());
    a1.maintenance().unwrap();
    let events = a2.sync(0).unwrap();
    assert_eq!(added(&events), 1, "{events:?}");
    assert_eq!(a2.saved().state.siblings.len(), 1);
    assert!(a2.saved().state.siblings_retired.is_empty());

    // B has only the first slot. Its next delivery sees a signed 409 and opens the added session; each device gets
    // its own copy.
    b.send_yo(a2.user_id()).unwrap();
    assert_eq!(b.sessions_with(a2.user_id()).len(), 2);
    for a in [&mut a1, &mut a2] {
        assert_eq!(yos(&a.sync(0).unwrap()), 1);
        assert_eq!(yos(&a.sync(0).unwrap()), 0);
    }
    // What A2 sends goes to B, and to A1 as a sent transcript: A1's thread shows it as sent.
    a2.send_yo(b.user_id()).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap()), 1);
    let sent = a1.sync(0).unwrap();
    assert!(matches!(&sent[..], [Event::Sent { user, content: Content::Yo, .. }] if user == b.user_id()), "{sent:?}");
    assert_eq!(yos(&sent), 0);

    // A2 shows B's thread: A1 lets its notifications of it go, through B's Yo (its sent time, not a clock).
    let through = a2.saved().state.last_in[b.user_id()];
    a2.mark_seen(&[b.user_id().to_owned()]).unwrap();
    a2.mark_seen(&[b.user_id().to_owned()]).unwrap();
    assert!(a2.sync(0).unwrap().is_empty(), "never back to the sending device");
    assert!(a1.sync(0).unwrap().is_empty());
    assert_eq!(a1.take_seen().unwrap(), [(b.user_id().to_owned(), through)]);
    assert!(a1.take_seen().unwrap().is_empty());

    // A storage change: read on A1 once its inbox is.
    a2.set_app_icon(Some("test".into())).unwrap();
    a2.write_storage().unwrap();
    a2.maintenance().unwrap();
    assert!(a1.sync(0).unwrap().is_empty());
    a1.maintenance().unwrap();
    assert_eq!(a1.app_icon().as_deref(), Some("test"));

    let mut c = register("Third", None);
    let (group, _) = b.create_group("Group", &[a1.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    for peer in [&mut a1, &mut a2, &mut c] {
        peer.sync(0).unwrap();
    }
    b.send_group(&group, Content::Yo).unwrap();
    let (photo, ciphertext) = media::seal(b"photo", 1, 1).unwrap();
    let (fetches, _) = b.open_group(&group).unwrap();
    assert_eq!(fetches, 3);
    b.upload(&photo, fetches).unwrap().send(&ciphertext).unwrap();
    b.send_group(&group, Content::Photo(photo.clone())).unwrap();
    for peer in [&mut a1, &mut a2, &mut c] {
        let events = peer.sync(0).unwrap();
        let contents: Vec<_> = events
            .into_iter()
            .filter_map(|e| match e {
                Event::GroupMessage { group: id, content, .. } if id == group => Some(content),
                _ => None,
            })
            .collect();
        assert_eq!(contents, [Content::Yo, Content::Photo(photo.clone())]);
        assert_eq!(media::fetch_open(&*http, &photo).unwrap().unwrap(), b"photo");
        media::ack(&*http, &photo.id).unwrap();
    }
    assert!(media::fetch(&*http, &photo).unwrap().is_none());
    a1.accept_group(&group).unwrap();
    a1.send_group(&group, Content::Yo).unwrap();
    assert!(!a2.sync(0).unwrap().iter().any(|e| matches!(e, Event::GroupMessage { .. })));
    assert_eq!(yos(&b.sync(0).unwrap()), 0);

    // A3 joins: told on A1 and A2 once it writes. (A roster is asked at most
    // once a minute a user, and A2 asked for A1's: a minute on.)
    let mut a3 = register("First", Some(a1.account_secret()));
    a3.maintenance().unwrap();
    for a in [&mut a1, &mut a2] {
        a.set_clock(Arc::new(|| yo_hub::now_ms() + 2 * 60 * 1000));
        let events = a.sync(0).unwrap();
        assert_eq!(added(&events), 1, "{events:?}");
        assert_eq!(a.saved().state.siblings.len(), 2);
        assert!(a.saved().state.siblings_retired.is_empty());
    }
    // A3 restored the account's storage (its groups); no message of A1's reached it.
    let left = a3.sync(0).unwrap();
    assert!(!left.iter().any(|e| matches!(e, Event::Message { .. } | Event::GroupMessage { .. })), "{left:?}");

    // A1 lists the account's devices and removes A3: A3 gets 410 (Replaced) and forgets.
    let devices = a1.own_devices().unwrap();
    assert_eq!(devices.len(), 3);
    let number = a3.saved().state.device_number;
    assert!(devices.iter().any(|d| d.number == number && d.last_seen_at > 0));
    assert_eq!(a1.remove_device(a1.saved().state.device_number), Err(yo_core::Error::Refused(400)));
    a1.remove_device(number).unwrap();
    assert_eq!(a1.remove_device(number), Err(yo_core::Error::Refused(404)));
    assert_eq!(a1.own_devices().unwrap().len(), 2);
    assert_eq!(a3.sync(0), Err(yo_core::Error::Replaced));
}

#[test]
fn siblings_over_http() {
    let hub = TestHub::start_with(config(), "127.0.0.1:0");
    exercise(Http::new(hub.url()));
}

/// The same HTTP requests against the hub's service directly, including its real database writer.
#[test]
fn siblings_at_the_hub_router() {
    use axum::{
        body::{to_bytes, Body},
        extract::ConnectInfo,
        http::Request,
    };
    use tower::ServiceExt;
    struct Local(axum::Router, tokio::runtime::Runtime);
    impl Transport for Local {
        fn request(
            &self,
            method: &str,
            path: &str,
            headers: &[(String, String)],
            bytes: &[u8],
        ) -> Result<Response, String> {
            let mut request = Request::builder()
                .method(method)
                .uri(path)
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
            for (name, value) in headers {
                request = request.header(name, value);
            }
            let request = request.header("content-length", bytes.len()).body(Body::from(bytes.to_vec())).unwrap();
            self.1.block_on(async {
                let response = self.0.clone().oneshot(request).await.unwrap();
                Ok(Response {
                    status: response.status().as_u16(),
                    body: to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec(),
                })
            })
        }
    }
    let hub = Hub::new(config(), Arc::new(yo_hub::push::Recorder::default())).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    exercise(Arc::new(Local(hub.router(), runtime)));
}

/// Sent transcripts reach every own device the roster lists, also one linked after the sender and one that has not
/// written to it yet (spec 2.5): a device linked later is told to the app, not held.
#[test]
fn transcripts_reach_every_own_device() {
    let hub = TestHub::start_with(config(), "127.0.0.1:0");
    let http = Http::new(hub.url());
    let register = |name, secret| {
        let mut c = Client::register_username(http.clone(), name, secret, None, Push::none()).unwrap();
        c.set_hub_keys(vec![yo_tests::hub_key()]);
        c
    };
    let sent_to = |events: &[Event], to: &str| {
        events.iter().filter(|e| matches!(e, Event::Sent { user, content: Content::Yo, .. } if user == to)).count()
    };
    let mut a = register("First", None);
    let mut c = register("Second", None);
    c.send_yo(a.user_id()).unwrap();
    a.sync(0).unwrap();
    a.accept(c.user_id()).unwrap();
    a.send_yo(c.user_id()).unwrap();
    c.sync(0).unwrap();
    let to = c.user_id().to_owned();

    // (a) B is linked: its first send shows on A as sent.
    let mut b = register("First", Some(a.account_secret()));
    b.sync(0).unwrap();
    b.send_yo(&to).unwrap();
    let events = a.sync(0).unwrap();
    assert_eq!(sent_to(&events, &to), 1, "{events:?}");

    // (b) D is linked and its first word (the storage hint) is lost: once A's roster lists it, A's send reaches it.
    let mut d = register("First", Some(a.account_secret()));
    let mut saved = d.saved();
    saved.state.outbox.clear();
    d.reload(&saved).unwrap();
    d.sync(0).unwrap();
    a.set_clock(Arc::new(|| yo_hub::now_ms() + 25 * 3600 * 1000));
    a.sync(0).unwrap();
    assert_eq!(a.saved().state.siblings.len(), 2);
    a.send_yo(&to).unwrap();
    let events = d.sync(0).unwrap();
    assert_eq!(sent_to(&events, &to), 1, "{events:?}");

    // (c) B and D both joined after A: transcripts go between them both ways.
    for x in [&mut b, &mut d] {
        x.set_clock(Arc::new(|| yo_hub::now_ms() + 25 * 3600 * 1000));
        x.sync(0).unwrap();
        assert_eq!(x.saved().state.siblings.len(), 2);
    }
    b.send_yo(&to).unwrap();
    let events = d.sync(0).unwrap();
    assert_eq!(sent_to(&events, &to), 1, "{events:?}");
    d.send_yo(&to).unwrap();
    let events = b.sync(0).unwrap();
    assert_eq!(sent_to(&events, &to), 1, "{events:?}");
    for x in [&mut a, &mut b, &mut d] {
        x.sync(0).unwrap();
    }
    a.send_yo(&to).unwrap();
    for x in [&mut b, &mut d] {
        let events = x.sync(0).unwrap();
        assert_eq!(sent_to(&events, &to), 1, "{events:?}");
    }
}
