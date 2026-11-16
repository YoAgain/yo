//! Synthetic, anonymous large-state fixtures with real active sessions and a real in-process HTTP hub.
//! Run: dev/cargo test --release -p yo-tests --test core_cost -- --ignored --nocapture
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use yo_core::{state, Client, Content, Push, Store};
use yo_tests::{Http, TestHub};
#[derive(Default)]
struct Disk {
    elapsed: Mutex<Duration>,
}
impl Store for Disk {
    fn save(&self, blob: Vec<u8>) -> Result<(), String> {
        let start = Instant::now();
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../target/yo-cost-{}", std::process::id()));
        let tmp = path.with_extension("new");
        let result = (|| {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&blob)?;
            let durable = std::env::var_os("YO_COST_FSYNC").is_some();
            if durable {
                f.sync_all()?;
            }
            std::fs::rename(&tmp, &path)?;
            if durable {
                std::fs::File::open(path.parent().unwrap())?.sync_all()?;
            }
            Ok(())
        })();
        *self.elapsed.lock().unwrap() += start.elapsed();
        result.map_err(|e: std::io::Error| e.to_string())
    }
}
fn measure(label: &str, disk: &Disk, mut f: impl FnMut()) {
    let mut rows = Vec::new();
    for _ in 0..21 {
        yo_core::cost::take();
        *disk.elapsed.lock().unwrap() = Duration::ZERO;
        let start = Instant::now();
        f();
        let total = start.elapsed();
        let parts = yo_core::cost::take();
        let write = *disk.elapsed.lock().unwrap();
        let other = total.saturating_sub(parts.iter().sum::<Duration>() + write);
        rows.push([total, parts[0], parts[1], parts[2], write, other].map(|d| d.as_secs_f64() * 1000.));
    }
    print!("{label}");
    for col in 0..6 {
        let mut v: Vec<_> = rows.iter().map(|r| r[col]).collect();
        v.sort_by(f64::total_cmp);
        print!(",{:.3}/{:.3}", v[10], v[19]);
    }
    println!();
}
#[test]
#[ignore = "performance measurement"]
fn core_cost() {
    yo_core::cost::baseline_snapshots(std::env::var_os("YO_COST_DEEP").is_some());
    let hub = TestHub::start();
    let mut me = Client::register_username(Http::new(hub.url()), "COSTME", None, None, Push::none()).unwrap();
    let mut peers: Vec<_> = (0..20)
        .map(|i| {
            Client::register_username(Http::new(hub.url()), &format!("COSTPEER{i}"), None, None, Push::none()).unwrap()
        })
        .collect();
    for p in &mut peers {
        p.send_yo(me.user_id()).unwrap();
    }
    me.sync(0).unwrap();
    for p in &peers {
        me.send_yo(p.user_id()).unwrap();
    }
    let (g6, _) =
        me.create_group("SIX", &peers[..5].iter().map(|p| p.user_id().to_owned()).collect::<Vec<_>>()).unwrap();
    let (g20, _) =
        me.create_group("TWENTY", &peers[..19].iter().map(|p| p.user_id().to_owned()).collect::<Vec<_>>()).unwrap();
    me.send_group(&g6, Content::Yo).unwrap();
    me.send_group(&g20, Content::Yo).unwrap();
    let key = [7; 32];
    let mut base = me.export(&key);
    println!("fixture/op,total,session,json,seal+TLS,file,other (median/p95 ms)");
    for contacts in [100, 500, 2000] {
        for outbox in [0, 1000] {
            let (mut s, records) = state::open(&base, &key).unwrap();
            s.contacts.retain(|user, _| peers.iter().any(|p| p.user_id() == user));
            s.outbox.clear();
            let template = s.contacts.values().next().unwrap().clone();
            for i in 20..contacts {
                let mut contact = template.clone();
                contact.account_key = yo_core::identity::random();
                contact.name = Some(format!("COST{i}"));
                contact.handles = vec![format!("COST{i}")];
                s.contacts.insert(format!("{:032x}", i + 1_000_000), contact);
            }
            // Retained session plaintext is representative of the bounded sent cache, not arbitrary padding.
            for c in s.contacts.values_mut() {
                c.sent = (0..if contacts == 2000 { 2 } else { 8 }).map(|_| vec![3; 96]).collect();
            }
            if outbox > 0 {
                // Blocked queued recipients remain queued during sync/deliver, as when offline sends await unblock.
                let user = format!("{:032x}", 1_000_020);
                s.contacts.get_mut(&user).unwrap().blocked = true;
                for seq in 0..outbox {
                    s.outbox.push_back(state::Outgoing {
                        queued_at: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64,
                        seq: seq as u64,
                        user: user.clone(),
                        plain: vec![1; 64],
                        device: None,
                        payload: None,
                        made_for: vec![],
                        refused: false,
                    });
                }
            }
            let blob = state::seal(&s, records, &key);
            let mut c = Client::restore(Http::new(hub.url()), &blob, &key).unwrap();
            let disk = Arc::new(Disk::default());
            c.persist_to(disk.clone(), key).unwrap();
            let prefix = format!("{contacts}/{outbox}/{}KiB", blob.len() / 1024);
            measure(&format!("{prefix}/save"), &disk, || disk.save(c.export(&key)).unwrap());
            let transport = Http::new(hub.url());
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../target/yo-cost-{}", std::process::id()));
            measure(&format!("{prefix}/restore"), &disk, || {
                let saved = std::fs::read(&path).unwrap();
                std::hint::black_box(Client::restore(transport.clone(), &saved, &key).unwrap());
            });
            measure(&format!("{prefix}/send1"), &disk, || {
                c.send_yo(peers[1].user_id()).unwrap();
            });
            measure(&format!("{prefix}/group6"), &disk, || {
                c.send_group(&g6, Content::Yo).unwrap();
            });
            measure(&format!("{prefix}/group20"), &disk, || {
                c.send_group(&g20, Content::Yo).unwrap();
            });
            for burst in [1, 100] {
                let mut samples = Vec::new();
                // Queue outside the measured interval; measure() runs one prepared receive at a time below.
                for _ in 0..21 {
                    for _ in 0..burst {
                        peers[0].send_yo(c.user_id()).unwrap();
                    }
                    yo_core::cost::take();
                    *disk.elapsed.lock().unwrap() = Duration::ZERO;
                    let start = Instant::now();
                    let events = c.sync(0).unwrap();
                    let total = start.elapsed();
                    assert_eq!(
                        events
                            .iter()
                            .filter(|e| matches!(e, yo_core::Event::Message { content: Content::Yo, .. }))
                            .count(),
                        burst,
                        "drops: {:?}, events: {:?}",
                        c.drop_reasons(),
                        events
                    );
                    let p = yo_core::cost::take();
                    let w = *disk.elapsed.lock().unwrap();
                    samples.push(
                        [total, p[0], p[1], p[2], w, total.saturating_sub(p.iter().sum::<Duration>() + w)]
                            .map(|d| d.as_secs_f64() * 1000.),
                    );
                }
                print!("{prefix}/receive{burst}");
                for col in 0..6 {
                    let mut v: Vec<_> = samples.iter().map(|r| r[col]).collect();
                    v.sort_by(f64::total_cmp);
                    print!(",{:.3}/{:.3}", v[10], v[19]);
                }
                println!();
            }
            base = c.export(&key);
        }
    }
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../target/yo-cost-{}", std::process::id()));
    let _ = std::fs::remove_file(path);
}
