use std::{net::SocketAddr, sync::Arc};
use yo_core::{media, Client, Content, Event, Response, Transport};
use yo_hub::{config::Config, Hub};
use yo_tests::{Http, TestHub};
use yo_wire::api::Push;

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
    let yos = |events: Vec<Event>| {
        events.into_iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count()
    };
    b.send_yo(a2.user_id()).unwrap();
    assert_eq!(yos(a2.sync(0).unwrap()), 1);
    a2.accept(b.user_id()).unwrap();
    a2.maintenance().unwrap();
    let mut a1 = register("First", Some(a2.account_secret()));
    assert_eq!(a1.user_id(), a2.user_id());

    // B has only the first slot. Its next delivery sees a signed 409 and opens the added session.
    b.send_yo(a2.user_id()).unwrap();
    assert_eq!(b.sessions_with(a2.user_id()).len(), 2);
    for a in [&mut a1, &mut a2] {
        assert_eq!(yos(a.sync(0).unwrap()), 1);
        assert_eq!(yos(a.sync(0).unwrap()), 0);
    }
    a1.send_yo(b.user_id()).unwrap();
    assert_eq!(yos(b.sync(0).unwrap()), 1);
    assert!(a2.sync(0).unwrap().is_empty(), "a transcript is silent");
    assert!(a2.saved().state.sibling_sent_at[b.user_id()] > 0);
    assert!(a2.saved().state.contacts[b.user_id()].chat);
    assert_eq!(a2.saved().state.siblings.len(), 1);

    a2.send_yo(b.user_id()).unwrap();
    assert_eq!(yos(b.sync(0).unwrap()), 1);
    assert!(a1.sync(0).unwrap().is_empty());
    assert!(a1.saved().state.sibling_sent_at[b.user_id()] > 0);
    for at in [91, 77] {
        a2.mark_seen(b.user_id(), at).unwrap();
    }
    assert!(a1.sync(0).unwrap().is_empty());
    assert_eq!(a1.saved().state.seen_through[b.user_id()], 91);

    a2.set_app_icon(Some("test".into())).unwrap();
    a2.write_storage().unwrap();
    a2.maintenance().unwrap();
    assert!(a1.sync(0).unwrap().is_empty());
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

    // Two accepted group legs release a single transcript to A's sibling.
    a1.accept_group(&group).unwrap();
    a1.send_group(&group, Content::Yo).unwrap();
    let events = a2.sync(0).unwrap();
    assert!(!events.iter().any(|e| matches!(e, Event::GroupMessage { .. })));
    assert!(a2.saved().state.sibling_sent_at[&group] > 0);
    assert_eq!(a1.open_group(&group).unwrap().0, 3);

    // A1 still knows only A2. Its own-account POST gets 409 after another installation
    // joins. The roster hash changes, but the transcript's original audience stays A2.
    let mut a3 = register("First", Some(a1.account_secret()));
    let before = a2.saved().state.sibling_sent_at[b.user_id()];
    a1.send_yo(b.user_id()).unwrap();
    assert_eq!(a1.saved().state.siblings.len(), 2);
    assert!(a2.sync(0).unwrap().is_empty());
    assert!(a2.saved().state.sibling_sent_at[b.user_id()] > before);
    // A3 restored the account's storage (its groups); no message of A1's reached it.
    let left = a3.sync(0).unwrap();
    assert!(!left.iter().any(|e| matches!(e, Event::Message { .. } | Event::GroupMessage { .. })), "{left:?}");
    assert!(!a3.saved().state.sibling_sent_at.contains_key(b.user_id()));
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
