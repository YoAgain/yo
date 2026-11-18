// Compiled as a yo-tests binary in the pre-V0001 checkout by db_upgrade.rs.
use std::{env, fs, path::Path, sync::Arc};
use yo_core::{
    media,
    statedb::{DbStore, StateDb},
    Client, Content, Event, Push,
};
use yo_tests::Http;
use yo_wire::api;

const KEY: [u8; 32] = [42; 32];
const IOS_WORD: &str = "12345678-1234-4234-8234-123456789ABC";

fn exchange(sender: &mut Client, recipient: &mut Client, message: &str) {
    sender.send(recipient.user_id(), Content::Text(message.into())).unwrap();
    assert!(recipient
        .sync(0)
        .unwrap()
        .iter()
        .any(|event| matches!(event, Event::Message { content: Content::Text(text), .. } if text == message)));
    recipient.acknowledge_events();
    recipient.after_sync().unwrap();
}

fn main() {
    let mut args = env::args().skip(1);
    let url = args.next().unwrap();
    let folder = args.next().unwrap();
    let folder = Path::new(&folder);
    let register = |name| Client::register_username(Http::new(url.clone()), name, None, None, Push::none()).unwrap();
    let http = Http::new(url.clone());
    let mut app = Client::register_username(http.clone(), "Upgrade", None, None, Push::none()).unwrap();
    let db = Arc::new(StateDb::open(folder.join("ios-v0.sqlite")).unwrap());
    app.persist_to(Arc::new(DbStore::new(db.clone(), None, None)), KEY).unwrap();
    let mut peers = [register("UpgradePeerA"), register("UpgradePeerB"), register("UpgradePeerC")];
    for peer in &mut peers {
        exchange(&mut app, peer, "first");
        exchange(peer, &mut app, "reply");
        exchange(&mut app, peer, "ratchet forward");
        exchange(peer, &mut app, "ratchet back");
    }
    app.set_name("Upgrade Fixture").unwrap();
    app.set_requests_open(false).unwrap();
    app.set_verified(peers[0].user_id(), true).unwrap();
    app.set_left_handed(true).unwrap();
    app.set_app_icon(Some("amethyst".into())).unwrap();
    let members: Vec<_> = peers.iter().map(|peer| peer.user_id().to_owned()).collect();
    let (group, _) = app.create_group("Upgrade Group", &members).unwrap();
    for peer in &mut peers {
        peer.sync(0).unwrap();
        peer.acknowledge_events();
        peer.after_sync().unwrap();
        assert!(peer.groups().iter().any(|entry| entry.id == group));
    }

    // Leave one event handed to the app and one encrypted photo at the hub.
    app.acknowledge_explicitly();
    peers[0].send(app.user_id(), Content::Text("handed but unacked".into())).unwrap();
    http.sent.lock().unwrap().clear();
    assert!(app.sync(0).unwrap().iter().any(|event| matches!(
        event, Event::Message { content: Content::Text(text), .. } if text == "handed but unacked"
    )));
    assert!(!http.sent.lock().unwrap().iter().any(|(_, path, _)| path == api::INBOX_ACK));
    let (photo, ciphertext) = media::seal(b"upgrade photo", 4, 3).unwrap();
    peers[1].upload(&photo, 1).unwrap().send(&ciphertext).unwrap();
    peers[1].send(app.user_id(), Content::Photo(photo)).unwrap();

    // One send reached the hub but the old process never applied the answer. The other never left.
    for (index, peer) in peers.iter().take(2).enumerate() {
        let delivery = app.send_begin(peer.user_id(), Content::Text(format!("queued {index}"))).unwrap();
        assert!(!delivery.is_empty());
        if index == 0 {
            assert!(delivery
                .send()
                .into_iter()
                .flatten()
                .any(|reply| reply.is_ok_and(|response| response.status == api::status::ACCEPTED)));
        }
    }
    assert_eq!(app.waiting(), 2);
    let (_, blob) = db.read().unwrap().unwrap();
    fs::write(folder.join("android-state"), &blob).unwrap();
    fs::write(folder.join("ios-state"), [IOS_WORD.as_bytes(), &blob].concat()).unwrap();
    for (index, peer) in peers.iter().enumerate() {
        fs::write(folder.join(format!("peer-{index}")), peer.export(&KEY)).unwrap();
    }
}
