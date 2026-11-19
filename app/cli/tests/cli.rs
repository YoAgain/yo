//! `yo` as the operator and the skills run it: against a hub in-process, people as core clients.

mod common;

use common::*;
use std::time::Duration;
use yo_core::{Client, Content, Push};

#[test]
fn register_add_send_and_receive_once() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let name = format!("Cli{}", random_u64() % 1_000_000);
    assert_eq!(yo.run(&["whoami"]).0, 4, "nothing registered yet");
    let me = yo.ok(&["register", &name]);
    assert_eq!(me["username"], name.as_str());
    for file in ["account", "key", "state.sqlite"] {
        let mode =
            std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(yo.state.join(file)).unwrap().permissions());
        assert_eq!(mode & 0o777, 0o600, "{file}");
    }
    let (code, error) = yo.run(&["register", "Other"]);
    assert_eq!(code, 1);
    assert!(error["error"].as_str().unwrap().contains("already registered"), "{error}");
    assert_eq!(yo.ok(&["whoami"])["user"], me["user"]);

    let mut alice = someone(&hub);
    let alice_name = alice.username().unwrap().to_owned();
    assert_eq!(yo.ok(&["whois", &alice_name.to_lowercase()])["user"], alice.user_id());
    assert_eq!(yo.ok(&["add", &alice_name])["user"], alice.user_id());
    let friends = yo.ok(&["friends"]);
    assert_eq!(friends[0]["user"], alice.user_id(), "{friends}");
    yo.ok(&["send", &alice_name]);
    assert_eq!(yos_until_quiet(&mut alice, Duration::from_millis(500)), [me["user"].as_str().unwrap()]);

    // A Yo and a photo to it: `sync` hands them out once, the photo with its reference.
    let photo_bytes = picture_of(3000);
    let me_user = me["user"].as_str().unwrap();
    alice.send_yo(me_user).unwrap();
    let fetches = alice.media_fetches(me_user).unwrap();
    let photo = uploaded(&mut alice, &photo_bytes, fetches);
    alice.send(me_user, Content::Photo(photo)).unwrap();
    let events = yo.ok(&["sync", "--wait", "1"]);
    let messages: Vec<_> = events.as_array().unwrap().iter().filter(|e| e["type"] == "message").collect();
    assert_eq!(messages.len(), 2, "{events}");
    assert_eq!((messages[0]["content"].as_str(), messages[0]["from"].as_str()), (Some("yo"), Some(alice.user_id())));
    assert_eq!(messages[1]["reply_to"], alice.user_id());
    let reference = messages[1]["media"]["ref"].as_str().unwrap().to_owned();
    assert!(yo.ok(&["sync"]).as_array().unwrap().iter().all(|e| e["type"] != "message"), "handed out once");
    assert_eq!(yo.ok(&["status"])["contacts"], 1);

    // The photo back as it came, sealed anew, its reference on stdin only (never an argument); no video from a file.
    assert_eq!(yo.run(&["send", alice.user_id(), "--media", &reference]).0, 2);
    assert_eq!(yo.run(&["send", alice.user_id(), "--video", "clip.mp4", "--duration-ms", "1000"]).0, 2);
    assert_eq!(yo.run_with(&["send", alice.user_id(), "--media", "-"], &reference).0, 0);
    let (_, pictures) = answers_until_quiet(&mut alice, Duration::from_millis(800));
    assert_eq!(pictures, std::slice::from_ref(&photo_bytes));
    let mut with_exif = photo_bytes[..2].to_vec();
    with_exif.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x08, b'E', b'x', b'i', b'f', 0, 0]);
    with_exif.extend_from_slice(&photo_bytes[2..]);
    let file = yo.state.join("photo.jpg");
    std::fs::write(&file, with_exif).unwrap();
    yo.ok(&["send", &alice_name, "--photo", file.to_str().unwrap()]);
    let (_, pictures) = answers_until_quiet(&mut alice, Duration::from_millis(800));
    assert_eq!(pictures, [photo_bytes]);
    std::fs::write(&file, b"not a jpeg").unwrap();
    assert_eq!(yo.run(&["send", &alice_name, "--photo", file.to_str().unwrap()]).0, 1);
    assert_eq!(yo.run(&["send", &alice_name, "--bogus", "x"]).0, 2);
    let _ = std::fs::remove_dir_all(&yo.state);
}

/// While `yo listen` runs, the other commands run in it (its control socket); events come as JSON lines, a group's
/// with the group to answer into.
#[test]
fn commands_run_in_the_listener_while_it_listens() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let me = yo.ok(&["register", &format!("Lis{}", random_u64() % 1_000_000)]);
    let me = me["user"].as_str().unwrap().to_owned();
    let mut alice = someone(&hub);
    let bob = someone(&hub);
    let mut command = yo.command();
    command.arg("listen");
    let (listener, mut out) = Process::spawn_piped(command);
    let ready = next_line(&mut out);
    assert_eq!((ready["type"].as_str(), ready["user"].as_str()), (Some("ready"), Some(me.as_str())));

    alice.send_yo(&me).unwrap();
    let line = loop {
        let line = next_line(&mut out);
        if line["type"] == "message" {
            break line;
        }
    };
    assert_eq!((line["from"].as_str(), line["content"].as_str()), (Some(alice.user_id()), Some("yo")));
    let status = yo.ok(&["status"]);
    assert_eq!(status["listening"], true, "{status}");
    assert_eq!(yo.run(&["sync"]).0, 1, "the listener has the events");
    yo.ok(&["send", alice.user_id()]);
    assert_eq!(yos_until_quiet(&mut alice, Duration::from_millis(500)), std::slice::from_ref(&me));

    let group = yo.ok(&["groups", "create", "Trio", alice.username().unwrap(), bob.user_id()]);
    let group = group["group"].as_str().unwrap().to_owned();
    let groups = yo.ok(&["groups"]);
    assert_eq!(groups[0]["members"].as_array().unwrap().len(), 3, "{groups}");
    let _ = alice.sync(0).unwrap();
    alice.acknowledge_events();
    alice.send_group(&group, Content::Yo).unwrap();
    let line = loop {
        let line = next_line(&mut out);
        if line["type"] == "message" {
            break line;
        }
    };
    assert_eq!(line["group"], group.as_str());
    assert_eq!(line["reply_to"], format!("group:{group}"));
    yo.ok(&["send", &format!("group:{group}")]);
    let got = echoed(&mut alice, &me, Some(&group), Duration::from_millis(500));
    assert_eq!(got, [(Content::Yo, None)]);
    // A photo into the group: uploaded for its members' fetches, the sessions with them opened first.
    let photo = picture_of(2000);
    let file = yo.state.join("group.jpg");
    std::fs::write(&file, &photo).unwrap();
    yo.ok(&["send", &format!("group:{group}"), "--photo", file.to_str().unwrap()]);
    let got = echoed(&mut alice, &me, Some(&group), Duration::from_millis(500));
    assert!(matches!(got.as_slice(), [(Content::Photo(_), Some(bytes))] if *bytes == photo), "{got:?}");
    yo.ok(&["groups", "leave", &group]);
    assert!(yo.ok(&["groups"])[0]["left"].as_bool().unwrap());

    // Stopped hard: the directory is free again, the account as it was.
    drop(listener);
    assert_eq!(yo.ok(&["whoami"])["user"], me.as_str());
    let _ = std::fs::remove_dir_all(&yo.state);
}

/// `register` never replaces the device of a directory whose state it cannot open (its `key` lost or wrong) unless
/// `--force`.
#[test]
fn register_refuses_a_state_it_cannot_open() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let name = format!("Keep{}", random_u64() % 1_000_000);
    let me = yo.ok(&["register", &name]);
    let key = std::fs::read(yo.state.join("key")).unwrap();
    std::fs::remove_file(yo.state.join("key")).unwrap();
    let (code, error) = yo.run(&["register", &name]);
    assert_eq!(code, 1, "{error}");
    assert!(!yo.state.join("key").exists(), "no new key made");
    std::fs::write(yo.state.join("key"), [1u8; 32]).unwrap();
    assert_eq!(yo.run(&["register", &name]).0, 1, "a wrong key");
    assert!(yo.state.join("state.sqlite").exists() && !yo.state.join("state.old").exists());
    std::fs::write(yo.state.join("key"), &key).unwrap();
    assert_eq!(yo.ok(&["whoami"])["device"], me["device"]);
    std::fs::write(yo.state.join("key"), [1u8; 32]).unwrap();
    let again = yo.ok(&["register", &name, "--force"]);
    assert_eq!(again["user"], me["user"]);
    assert_ne!(again["device"], me["device"]);
    assert!(yo.state.join("state.old").exists());
    let _ = std::fs::remove_dir_all(&yo.state);
}

/// The device taken over by a new phone of the account: `listen` and the commands end with 3.
#[test]
fn a_device_the_hub_no_longer_knows_exits_3() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let name = format!("Gone{}", random_u64() % 1_000_000);
    yo.ok(&["register", &name]);
    let alice = someone(&hub);
    let secret: [u8; 32] = std::fs::read(yo.state.join("account")).unwrap().try_into().unwrap();
    Client::register_username(Http::new(hub.url()), &name, Some(secret), None, Push::none()).unwrap();
    assert_eq!(yo.run(&["send", alice.user_id()]).0, 3);
    let mut command = yo.command();
    command.arg("listen");
    let mut listener = Process::spawn(command);
    assert_eq!(listener.exit_within(Duration::from_secs(40)), Some(3));
    let _ = std::fs::remove_dir_all(&yo.state);
}

/// YoGPT's state file of the build before the database (`state`): taken in offline at the first start, the account
/// goes on as the same device with its sessions; the file is kept as `state.v2`.
#[test]
fn the_bots_state_file_of_before_goes_on_as_the_same_device() {
    let hub = TestHub::start();
    let mut bot = someone(&hub);
    let mut pal = someone(&hub);
    pal.send_yo(bot.user_id()).unwrap();
    assert_eq!(yos_until_quiet(&mut bot, Duration::from_millis(300)).len(), 1);
    // One more for it waits at the hub.
    pal.send_yo(bot.user_id()).unwrap();
    // The file as that build wrote it: a list of devices for each contact and outbox item.
    let saved = bot.saved();
    let mut json = serde_json::to_value(&saved.state).unwrap();
    let listed =
        |value: serde_json::Value| serde_json::Value::Array(value.is_null().then(Vec::new).unwrap_or(vec![value]));
    for contact in json["contacts"].as_object_mut().unwrap().values_mut() {
        contact["devices"] = listed(contact.as_object_mut().unwrap().remove("device").unwrap());
    }
    for item in json["outbox"].as_array_mut().unwrap() {
        item["made_for"] = listed(item["made_for"].take());
    }
    let key = [9u8; 32];
    let yo = Yo::new(&hub);
    std::fs::create_dir_all(&yo.state).unwrap();
    std::fs::write(yo.state.join("key"), key).unwrap();
    std::fs::write(yo.state.join("account"), bot.account_secret()).unwrap();
    let blob = yo_core::state::seal_legacy(&serde_json::to_vec(&json).unwrap(), &saved.records, &key);
    std::fs::write(yo.state.join("state"), blob).unwrap();
    let user = bot.user_id().to_owned();
    let device = bot.device_id();
    drop(bot);

    // The dry run (app/cli/dev/dry-run.sh) on a copy: the same user and device, offline; the directory untouched.
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dev/dry-run.sh");
    let dry = std::process::Command::new(script)
        .arg(&yo.state)
        .arg(YO)
        .env("TMPDIR", env!("CARGO_TARGET_TMPDIR"))
        .output()
        .unwrap();
    assert!(dry.status.success(), "{}", String::from_utf8_lossy(&dry.stderr));
    let seen: serde_json::Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!((seen["user"].as_str(), &seen["device"]), (Some(user.as_str()), &serde_json::json!(device)), "{seen}");
    assert!(String::from_utf8_lossy(&dry.stderr).contains("taken in (state.v2)"));
    assert!(
        yo.state.join("state").exists()
            && !yo.state.join("state.v2").exists()
            && !yo.state.join("state.sqlite").exists()
    );

    assert_eq!(yo.ok(&["whoami"])["user"], user.as_str());
    assert!(!yo.state.join("state").exists() && yo.state.join("state.v2").exists());
    let events = yo.ok(&["sync", "--wait", "1"]);
    let yos = events.as_array().unwrap().iter().filter(|e| e["content"] == "yo").count();
    assert_eq!(yos, 1, "the Yo waiting at the hub opens in the sessions of before: {events}");
    yo.ok(&["send", pal.user_id()]);
    assert_eq!(yos_until_quiet(&mut pal, Duration::from_millis(500)).len(), 1);
    // A second start takes nothing again.
    assert_eq!(yo.ok(&["whoami"])["user"], user.as_str());
    let _ = std::fs::remove_dir_all(&yo.state);
}

/// `listen --manual-ack`: what was handed out comes again after a restart until `yo ack`.
#[test]
fn unacknowledged_events_come_again_until_acked() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let me = yo.ok(&["register", &format!("Ack{}", random_u64() % 1_000_000)]);
    let me = me["user"].as_str().unwrap().to_owned();
    let mut alice = someone(&hub);
    let listen = || {
        let mut command = yo.command();
        command.args(["listen", "--manual-ack"]);
        let (process, mut out) = Process::spawn_piped(command);
        assert_eq!(next_line(&mut out)["type"], "ready");
        (process, out)
    };
    let message = |out: &mut _| loop {
        let line = next_line(out);
        if line["type"] == "message" {
            return line["sent_at_ms"].as_u64().unwrap();
        }
    };
    alice.send_yo(&me).unwrap();
    let (listener, mut out) = listen();
    let first = message(&mut out);
    drop(listener);
    let (_listener, mut out) = listen();
    assert_eq!(message(&mut out), first, "not acknowledged: again");
    yo.ok(&["ack"]);
    drop(_listener);
    std::thread::sleep(std::time::Duration::from_millis(5));
    alice.send_yo(&me).unwrap();
    let (_listener, mut out) = listen();
    assert_ne!(message(&mut out), first, "acknowledged: not again");
    let _ = std::fs::remove_dir_all(&yo.state);
}
