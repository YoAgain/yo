//! yo-send: an account that sends Yos, for timing the apps against a hub (app/ios/TESTING.md, "Speed").
//!
//!   yo-send --url https://dev.justyo.app --state FILE --name NAME --to NAME [--count 1] [--every-ms 0] [--sync]
//!           [--photo FILE | --video FILE] [--group] [--delete]
//!
//!           [--make-group NAME --members A,B | --group --to GROUP --add A | --remove A | --rename NEW]
//!
//! With `--url https://dev.justyo.app` the dev hub is pinned as the app pins it (hub keys, trust roots, zk params).
//!
//! `--group`: `--to` names a group this account is in (after a sync); the Yos and the photo or video go there.
//!
//! The account is registered as NAME the first time and kept in FILE (sealed under a fixed key: a test account).
//! `--sync` first receives (Yos sent to it). Each Yo is a line: when it was sent (wall clock,
//! ms, to compare with the app's YO_PERF marks) and how long `send_yo` took.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use yo_core::{Client, Push, Store};
use yo_tests::Http;

struct File(String);

impl Store for File {
    fn save(&self, blob: Vec<u8>) -> Result<(), String> {
        std::fs::write(&self.0, blob).map_err(|e| e.to_string())
    }
}

const KEY: [u8; 32] = [9; 32];

const DEV_HUB_KEYS: [&str; 2] =
    ["LPiW1TOvv_ricbn7bXmS_fNdVbgtEw_3o7OdelvEdVo", "nFQuDMd-2Ueoec6oYG-Ty7lHQELq1OHs4TVODmPmryg"];
const DEV_TRUST_ROOTS: [&str; 2] =
    ["kKVskVCuVwESdvLeYWAEVAQmT9mdwvkQ2uhOlnxXO3Q", "Fj-t9EAOsTDPSHPT1Y39FdbcIvl2I7aEEdqlcRAQc0c"];
const DEV_ZK_PARAMS: &str = "AABte-FyyNG7G0rm_Swi9dLOf6khh32C0-TmfHELCW5J4k4m_beuhn-LDZ6GFT19xzzza_-DQfEKUPdsKy99Knl4FgkgVTu8j8yZP5JoagLdul10PyCTaUcx649Z-JJuUWidD_T_ZYOgvSS61z9QixJQB6LAB_k0AYFY563qzWItEncY_vD68l2OUM1aTDNuXO6QeT0MgeRwrlsqLhF10C2SMQJotdkaEOGAMdr4E261qG0CNuVEua6ojEXs7BXCOpTIPvZhkxQf6N2-WWXVXyrm65kQSDkE32SYFSTxhP9dNNdVop_SDJE24rMYPJy1SH5DD1gs7Y67w0RNQIooUHmaUn3WlOjHXFMgwHpDDMGlDuMqgEMkz1D-ArhgAoYsXG6LduuWJgSRWi3KED6jZRm2QCUpd_LIkZKR2vmQp_Y68IvRf_myayJE33VuqTbfT0jp9KBeiJuIOdqubZBWb2vG_jI0ihgyFRJPTrFtpW0iv1tpngyN0m7_V3I-jUlzIGQSJRJDvDvlEvKPMDeA9Sc2Nh5tRY3JiMTqcFwCVMsbCujL_mhC4VBfXUFy-urGdLiqdTBZF53mInuT_yR8VFoSPLeTi7mHnRSlACAnh_GFnE80FlbXVEbW8EdMXzwycwLy3GajU4z0wOLmeI58jYlwZw8A-dNcB2CHG4zBwpkTiD2ZHZcdAL4kSljBUqE2Tml9cfKtGzqcvH7zrxOhjCL6Sn9IyV36TVf6lKf-oRjDHsPHP9pSp3apVZfG1dEdRQr2UEvHRHla3C8jQrte8e02zrqQwG0KxsUTueOVgiVsWtaUV3YohIHtxbSUOljwoaA-iRmPQsvUHgcpL2UoOghIMYvKSbwRIde_-NYhyQVSCBpcgz5bhRbHcc-uS_j7Qg";

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut url, mut state, mut name, mut to) = (String::new(), String::new(), String::new(), String::new());
    let (mut count, mut every_ms, mut sync) = (1u32, 0u64, false);
    let (mut photo, mut video, mut group, mut delete) = (None::<String>, None::<String>, false, false);
    let (mut make_group, mut members, mut add, mut remove, mut rename) =
        (None::<String>, Vec::<String>::new(), None::<String>, None::<String>, None::<String>);
    let mut trust_roots: Vec<[u8; 32]> = Vec::new();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--sync" => sync = true,
            "--photo" => photo = Some(args.next().expect("--photo")),
            "--video" => video = Some(args.next().expect("--video")),
            "--group" => group = true,
            "--delete" => delete = true,
            "--url" => url = args.next().expect("--url"),
            "--state" => state = args.next().expect("--state"),
            "--name" => name = args.next().expect("--name"),
            "--to" => to = args.next().expect("--to"),
            "--count" => count = args.next().and_then(|v| v.parse().ok()).expect("--count"),
            "--every-ms" => every_ms = args.next().and_then(|v| v.parse().ok()).expect("--every-ms"),
            "--make-group" => make_group = args.next(),
            "--members" => members = args.next().expect("--members").split(',').map(str::to_owned).collect(),
            "--add" => add = args.next(),
            "--remove" => remove = args.next(),
            "--rename" => rename = args.next(),
            "--trust-root" => trust_roots.push(
                yo_wire::unb64(&args.next().expect("--trust-root"))
                    .ok()
                    .and_then(|k| k.try_into().ok())
                    .expect("--trust-root: 32 bytes, base64url"),
            ),
            other => panic!("unknown flag {other}"),
        }
    }
    let dev = url.contains("dev.justyo.app");
    let http = Http::new(url);
    let mut client = match std::fs::read(&state) {
        Ok(blob) => Client::restore(http, &blob, &KEY).expect("restore"),
        Err(_) => Client::register_username(http, &name, None, None, Push::none()).expect("register"),
    };
    // The dev hub's pins, as the app has them (app/ios/YoApp/Sources/YoApp/Hub.swift): hub keys, trust roots, zk params.
    if dev {
        let key = |text: &str| -> [u8; 32] { yo_wire::unb64(text).ok().and_then(|k| k.try_into().ok()).expect("pin") };
        client.set_hub_keys(DEV_HUB_KEYS.iter().map(|k| key(k)).collect());
        if trust_roots.is_empty() {
            trust_roots = DEV_TRUST_ROOTS.iter().map(|k| key(k)).collect();
        }
        client.set_zk_params(&yo_wire::unb64(DEV_ZK_PARAMS).expect("zk params")).expect("zk params");
    }
    // The hub's trust roots (`--trust-root`, base64url), else the test hubs'.
    if !trust_roots.is_empty() {
        client.set_trust_roots(trust_roots);
    }
    client.persist_to(Arc::new(File(state)), KEY).expect("state file");
    if sync {
        let events = client.sync(0).expect("sync");
        println!("synced: {} events", events.len());
    }
    if delete {
        client.delete_account().expect("delete");
        println!("deleted");
        return;
    }
    // A user id by name: a Yo first makes them a contact, then the contact names them.
    let user_of = |client: &mut Client, name: &str| -> String {
        client.send(name, yo_core::Content::Yo).expect("a Yo to them first");
        let handle = yo_core::handle::skeleton(name);
        client
            .contacts()
            .into_iter()
            .find(|c| c.handles.iter().any(|h| yo_core::handle::skeleton(h) == handle))
            .expect("the contact")
            .user
    };
    if let Some(group_name) = make_group {
        let ids: Vec<String> = members.iter().map(|m| user_of(&mut client, m)).collect();
        let (id, _) = client.create_group(&group_name, &ids).expect("create group");
        println!("group {group_name} {id} with {}", members.join(","));
        return;
    }
    if group {
        let found = client.groups().into_iter().find(|g| !g.left && g.name.eq_ignore_ascii_case(&to));
        to = found.expect("a group of that name (--sync first)").id;
    }
    if let Some(who) = add {
        let id = user_of(&mut client, &who);
        client.add_to_group(&to, &[id]).expect("add");
        println!("added {who}");
        return;
    }
    if let Some(who) = remove {
        let id = user_of(&mut client, &who);
        client.remove_from_group(&to, &[id]).expect("remove");
        println!("removed {who}");
        return;
    }
    if let Some(name) = rename {
        client.rename_group(&to, &name).expect("rename");
        println!("renamed to {name}");
        return;
    }
    // A known contact by its user id, as the app sends: a name would be looked up at the hub for every Yo (its limit
    // per device and day).
    if !group {
        let handle = yo_core::handle::skeleton(&to);
        if let Some(contact) =
            client.contacts().into_iter().find(|c| c.handles.iter().any(|h| yo_core::handle::skeleton(h) == handle))
        {
            to = contact.user;
        }
    }
    let send =
        |client: &mut Client, content| if group { client.send_group(&to, content) } else { client.send(&to, content) };
    if let Some(file) = photo.or(video.clone()) {
        // What the app does: seal, upload for the chat's other devices, then the Yo that carries it.
        let plain = std::fs::read(&file).expect("photo or video");
        let (media, sealed) = yo_core::media::seal(&plain, 1080, 1920).expect("seal");
        let fetches = if group { client.group_media_fetches(&to) } else { client.media_fetches(&to) };
        let fetches = fetches.expect("a chat (send a Yo first)");
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1000.0;
        client.upload(&media, fetches).expect("upload").send(&sealed).expect("upload send");
        let content = match video {
            Some(_) => yo_core::Content::Video { media, duration_ms: 5000 },
            None => yo_core::Content::Photo(media),
        };
        let result = send(&mut client, content);
        println!("media at {wall:.3} fetches {fetches} {} bytes {}", plain.len(), result.is_ok());
        return;
    }
    let start = Instant::now();
    for i in 0..count {
        let at = Instant::now();
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1000.0;
        let result = send(&mut client, yo_core::Content::Yo);
        println!("yo {i} at {wall:.3} took {:.1} ms {}", at.elapsed().as_secs_f64() * 1000.0, result.is_ok());
        if let Err(e) = &result { eprintln!("yo {i}: {e:?}"); }
        let next = start + Duration::from_millis(every_ms * u64::from(i + 1));
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
}
