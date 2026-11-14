//! Restores to the extreme: accounts wiped and brought back again and again, by every way the app has (spec 7), the
//! friends writing all along. After every cycle: the same user id, messages both ways exactly once, the block holding,
//! the storage (6.9) intact and converged, nothing left behind at the hub.

use yo_core::{Client, Content, Error, Event, Push};
use yo_tests::backup::{self, Account};
use yo_tests::{random_u64, Http, Rng, TestHub};

fn register(hub: &TestHub, name: &str, secret: Option<[u8; 32]>) -> Result<Client, Error> {
    Client::register_username(Http::new(hub.url()), name, secret, None, Push::none())
}

fn texts_from(events: &[Event], from: &str) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Message { user, content: Content::Text(t), .. } if user == from => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// Receives until nothing more comes; the texts from `from`.
fn drain(client: &mut Client, from: &str) -> Vec<String> {
    let mut got = Vec::new();
    for _ in 0..3 {
        got.extend(texts_from(&client.sync(0).unwrap(), from));
    }
    got
}

fn random<const N: usize>(rng: &mut Rng) -> [u8; N] {
    std::array::from_fn(|_| rng.next_u64() as u8)
}

/// One account and what its owner keeps outside the phone: the words, the passkey's PRF output, the iCloud Keychain
/// item (name and secret).
struct Person {
    name: String,
    client: Client,
    user: String,
    secret: [u8; 32],
    k: [u8; 32],
    code: Vec<String>,
    prf: [u8; 32],
}

impl Person {
    fn account(&self) -> Account {
        Account {
            username: self.name.clone(),
            user_id: yo_core::state::unhex(&self.user).unwrap(),
            account_key: self.secret,
            by_passkey: true,
        }
    }

    fn put_backup(&mut self) {
        let record = backup::make(&self.account(), &self.code, &self.k, Some(&self.prf));
        self.client.put_backup(&record).unwrap();
    }
}

#[test]
fn accounts_restored_forty_times_by_every_way_lose_nothing() {
    let hub = TestHub::start();
    let mut rng = Rng(random_u64() | 1);
    let seed = rng.0;
    let run = random_u64() % 100_000;
    let mut friend = register(&hub, &format!("Friend{run}"), None).unwrap();
    let mut pest = register(&hub, &format!("Pest{run}"), None).unwrap();
    let (friend_id, pest_id) = (friend.user_id().to_owned(), pest.user_id().to_owned());
    let mut people: Vec<Person> = (0..6)
        .map(|i| {
            let name = format!("Cycle{run}x{i}");
            let mut client = register(&hub, &name, None).unwrap();
            let (user, secret) = (client.user_id().to_owned(), client.account_secret());
            friend.send(&user, Content::Text("welcome".into())).unwrap();
            pest.send(&user, Content::Text("spam".into())).unwrap();
            assert_eq!(drain(&mut client, &friend_id), ["welcome"]);
            client.set_friend(&friend_id, Some("My Friend".into()), Some(2)).unwrap();
            client.set_blocked(&pest_id, true).unwrap();
            client.set_app_icon(Some("Negative".into()));
            client.maintenance().unwrap();
            let mut person = Person {
                name,
                client,
                user,
                secret,
                k: random(&mut rng),
                code: backup::words(random(&mut rng)),
                prf: random(&mut rng),
            };
            person.put_backup();
            person
        })
        .collect();
    drain(&mut friend, "");
    let mut sent = 0u64;
    for cycle in 0..40 {
        for (i, p) in people.iter_mut().enumerate() {
            let way = (cycle + i) % 6;
            let ctx = format!("seed {seed}, cycle {cycle}, {} by way {way}", p.name);
            // Every cycle changes the storage a little, so each restore has something new to bring back.
            p.client.set_friend(&friend_id, Some(format!("Friend {cycle}")), Some((cycle % 7) as u8)).unwrap();
            p.client.set_app_icon(if cycle % 2 == 0 { Some("Original".into()) } else { None });
            p.client.maintenance().unwrap();
            // (d): the passkey replaced and the words renewed first; the old ones must open nothing any more.
            if way == 3 {
                let (old_code, old_prf) = (p.code.clone(), p.prf);
                p.code = backup::words(random(&mut rng));
                p.prf = random(&mut rng);
                p.put_backup();
                let record = Client::fetch_backup(&*Http::new(hub.url()), &backup::locator(&old_code)).unwrap();
                assert!(record.is_none(), "{ctx}: the old words still find a record");
                let current = Client::fetch_backup(&*Http::new(hub.url()), &backup::locator(&p.code)).unwrap().unwrap();
                assert!(
                    backup::unwrap(current.by_passkey.as_slice(), &backup::key_from_prf(&old_prf)).is_none(),
                    "{ctx}: the old passkey still opens it"
                );
            }
            // (e): the old phone is still running and sends meanwhile.
            sent += 1;
            let before = format!("before {cycle}");
            p.client.send(&friend_id, Content::Text(before.clone())).unwrap();
            // The phone is wiped: what the person has left opens the account again.
            let secret = match way {
                // (a) The passkey: the record by the user id (its user handle), K by the PRF output.
                0 => {
                    let id = yo_core::state::unhex(&p.user).unwrap();
                    let record = Client::fetch_backup(&*Http::new(hub.url()), &id).unwrap().expect(&ctx);
                    let k = backup::unwrap(record.by_passkey.as_slice(), &backup::key_from_prf(&p.prf)).expect(&ctx);
                    let (account, code) = backup::open_sealed(&record, &k).expect(&ctx);
                    assert_eq!((account.username.as_str(), &code), (p.name.as_str(), &p.code), "{ctx}");
                    account.account_key
                }
                // (b), (d) The 12 words: the record by their locator, K by them.
                1 | 3 => {
                    let record =
                        Client::fetch_backup(&*Http::new(hub.url()), &backup::locator(&p.code)).unwrap().expect(&ctx);
                    let k = backup::unwrap(record.by_code.as_slice(), &backup::key_from_code(&p.code)).expect(&ctx);
                    backup::open_sealed(&record, &k).expect(&ctx).0.account_key
                }
                // (c), (e) The iCloud Keychain: the name and the secret on a fresh machine, nothing else.
                _ => p.secret,
            };
            assert_eq!(secret, p.secret, "{ctx}");
            let new = register(&hub, &p.name, Some(secret)).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            assert_eq!(new.user_id(), p.user, "{ctx}: the same user");
            let mut old = std::mem::replace(&mut p.client, new);
            // The old phone learns it was replaced, and stops.
            assert_eq!(old.send(&friend_id, Content::Text("after".into())), Err(Error::Replaced), "{ctx}");
            assert_eq!(old.sync(0).map(|_| ()), Err(Error::Replaced), "{ctx}");
            // (g) The storage came back, merged: the friend as last named, the block, the icon.
            let friends = p.client.friends();
            let f = friends.iter().find(|f| f.user == friend_id).expect(&ctx);
            assert_eq!((f.name.clone(), f.color), (Some(format!("Friend {cycle}")), Some((cycle % 7) as u8)), "{ctx}");
            assert!(friends.iter().any(|f| f.user == pest_id && f.blocked), "{ctx}");
            let icon = if cycle % 2 == 0 { Some("Original".to_owned()) } else { None };
            assert_eq!(p.client.app_icon(), icon, "{ctx}");
            // Both ways, exactly once; the pest not at all.
            pest.send(&p.user, Content::Text(format!("spam {cycle}"))).unwrap();
            // What the friend receives on the way (a send receives first) counts too.
            let mut got = texts_from(&friend.send(&p.user, Content::Text(format!("to new {cycle}"))).unwrap(), &p.user);
            assert_eq!(drain(&mut p.client, &friend_id), [format!("to new {cycle}")], "{ctx}");
            assert!(drain(&mut p.client, &pest_id).is_empty(), "{ctx}: the pest got through");
            p.client.send(&friend_id, Content::Text(format!("from new {cycle}"))).unwrap();
            got.extend(texts_from(&friend.send(&p.user, Content::Yo).unwrap(), &p.user));
            got.extend(drain(&mut friend, &p.user));
            assert_eq!(got, [before, format!("from new {cycle}")], "{ctx}");
        }
        assert_eq!(hub.hub.audit().unwrap(), [], "cycle {cycle}: orphan rows");
    }
    // (f) Deleted: the backup goes with it; the name is anyone's, and the old secret takes nothing back.
    let p = &mut people[0];
    p.client.delete_account().unwrap();
    let record = Client::fetch_backup(&*Http::new(hub.url()), &backup::locator(&p.code)).unwrap();
    assert!(record.is_none(), "the deleted account's backup is gone");
    let stranger = register(&hub, &p.name, None).unwrap();
    assert_ne!(stranger.user_id(), p.user);
    assert_eq!(register(&hub, &p.name, Some(p.secret)).map(|_| ()), Err(Error::Refused(409)));
    assert_eq!(friend.send(&p.user, Content::Yo).map(|_| ()), Err(Error::NotOnYo));
    assert_eq!(hub.hub.audit().unwrap(), []);
    assert!(sent >= 240);
}
