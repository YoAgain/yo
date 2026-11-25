//! The executor against a hub in-process, with the real `yo`: people Yo a bot as core clients. The scripts are started
//! per connection, as systemd does.

use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use yo_core::{media, Client, Content, Event};
use yo_tests::{random_u64, register, username, Http, TestHub};

const TOML: &str = "names = [\"NAME\"]\nabout = \"a\"\ntriggers = TRIGGERS\n";
const HEAD: &str = "import json, sys, time\ne = json.load(sys.stdin)\n";
const PING: &str = "print(json.dumps([{'send': e['kind'], 'media': e['media']}]))\n";
const REMIND: &str = "print(json.dumps([{'send': 'yo'}] if e['trigger'] == 'cron' else [{'remind': e['now'] + 60}]))\n";

/// `yo`, built beside this crate's binary (`cargo test --workspace`, or `cargo build -p yo-cli` before).
fn yo() -> PathBuf {
    let mut dir = Path::new(env!("CARGO_BIN_EXE_yo-services")).to_owned();
    while dir.pop() {
        let yo = dir.join("yo");
        if yo.is_file() {
            return yo;
        }
    }
    panic!("no yo: cargo build -p yo-cli");
}

struct Bot {
    dir: PathBuf,
    name: String,
    child: Option<Child>,
    hub: String,
    env: Vec<(&'static str, String)>,
    /// Its folder, if not the one of its name.
    service: Option<PathBuf>,
}

impl Bot {
    fn new(hub: &TestHub, code: &str, keeps: bool) -> Bot {
        let dir = std::env::temp_dir().join(format!("yo-services-{:x}", random_u64()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("BOT{}", random_u64() % 1_000_000_000);
        let triggers = if keeps { "[\"yo\", \"cron\"]" } else { "[\"yo\"]" };
        let folder = dir.join(name.to_lowercase());
        std::fs::create_dir(&folder).unwrap();
        let toml = TOML.replace("NAME", &name).replace("TRIGGERS", triggers);
        std::fs::write(folder.join(name.to_lowercase()).with_extension("toml"), toml).unwrap();
        let script = folder.join(name.to_lowercase()).with_extension("py");
        std::fs::write(&script, HEAD.to_owned() + code).unwrap();
        let listener = UnixListener::bind(dir.join("script.sock")).unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                let script = script.clone();
                std::thread::spawn(move || {
                    let out: OwnedFd = stream.try_clone().unwrap().into();
                    let _ = Command::new("python3")
                        .args(["-I", "-B"])
                        .arg(script)
                        .stdin(OwnedFd::from(stream))
                        .stdout(out)
                        .stderr(Stdio::null())
                        .status();
                });
            }
        });
        let bot = Bot { dir, name, child: None, hub: hub.url(), env: Vec::new(), service: None };
        let registered = bot.command(&yo()).args(["register", &bot.name]).output().unwrap();
        assert!(registered.status.success(), "{}", String::from_utf8_lossy(&registered.stderr));
        bot
    }

    fn folder(&self) -> PathBuf {
        self.dir.join(self.name.to_lowercase())
    }

    /// Its file with this extension.
    fn file(&self, extension: &str) -> PathBuf {
        self.folder().join(self.name.to_lowercase()).with_extension(extension)
    }

    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env("YO_STATE", self.dir.join("state"))
            .env("YO_HUB", &self.hub)
            .env("YO_HUB_KEYS", yo_wire::b64(&yo_tests::hub_key()))
            .env("YO_TRUST_ROOTS", yo_wire::b64(&yo_tests::trust_root()))
            .env("YO_ZK_PARAMS", yo_wire::b64(&yo_tests::zk_params()))
            .env("YO_BIN", yo())
            .env("SCRIPT_SOCKET", self.dir.join("script.sock"))
            .env("SERVICES_DB", self.dir.join("book.sqlite"))
            .envs(self.env.iter().cloned());
        command
    }

    fn start(mut self) -> Bot {
        let mut command = self.command(Path::new(env!("CARGO_BIN_EXE_yo-services")));
        self.child = Some(command.arg(self.service.clone().unwrap_or_else(|| self.folder())).spawn().unwrap());
        std::thread::sleep(Duration::from_secs(2));
        self
    }

    fn book(&self, sql: &str) -> String {
        let out = Command::new("python3")
            .args(["-c", "import sqlite3,sys;print(sqlite3.connect(sys.argv[1],isolation_level=None).execute(sys.argv[2]).fetchall())"])
            .arg(self.dir.join("book.sqlite"))
            .arg(sql)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }
}

impl Drop for Bot {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            // The listener, its child, ends with its pipe.
            let _ = Command::new("pkill").args(["-9", "-P", &child.id().to_string()]).status();
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn someone(hub: &TestHub) -> Client {
    register(Http::new(hub.url()), &username(random_u64() % 1_000_000_000), None)
}

/// What comes until `quiet` passes without anything.
fn comes(client: &mut Client, quiet: Duration) -> Vec<Content> {
    let (mut got, mut last) = (Vec::new(), Instant::now());
    while last.elapsed() < quiet {
        for event in client.sync(1).unwrap() {
            if let Event::Message { content, .. } = event {
                got.push(content);
                last = Instant::now();
            }
        }
        client.acknowledge_events();
    }
    got
}

const QUIET: Duration = Duration::from_secs(4);

#[test]
fn a_yo_gets_a_yo_and_a_photo_comes_back() {
    let hub = TestHub::start();
    let bot = Bot::new(&hub, PING, false).start();
    let mut alice = someone(&hub);
    alice.send_yo(&bot.name).unwrap();
    assert_eq!(comes(&mut alice, QUIET), [Content::Yo]);
    let (photo, sealed) = media::seal(b"unseen", 4, 3).unwrap();
    alice.upload(&photo, 1).unwrap().send(&sealed).unwrap();
    alice.send(&bot.name, Content::Photo(photo.clone())).unwrap();
    let got = comes(&mut alice, QUIET);
    let [Content::Photo(back)] = got.as_slice() else { panic!("{got:?}") };
    assert_ne!(back.key, photo.key, "sealed anew");
    assert_eq!(media::fetch_open(alice.transport().as_ref(), back).unwrap().unwrap(), b"unseen");
    assert_eq!(
        bot.book("SELECT way, user, cron_due FROM services_yos ORDER BY id"),
        "[('received', None, None), ('sent', None, None), ('received', None, None), ('sent', None, None)]"
    );
    let spool: Vec<_> = std::fs::read_dir(bot.dir.join("state")).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(!spool.iter().any(|n| n.to_string_lossy().starts_with("picture-")));
}

#[test]
fn a_reminder_comes_once_also_after_a_restart() {
    let hub = TestHub::start();
    let mut bot = Bot::new(&hub, REMIND, true).start();
    let mut alice = someone(&hub);
    alice.send_yo(&bot.name).unwrap();
    std::thread::sleep(Duration::from_millis(700));
    assert!(bot.book("SELECT user FROM services_yos WHERE cron_due IS NOT NULL").contains(alice.user_id()));
    let child = bot.child.as_mut().unwrap();
    let _ = Command::new("pkill").args(["-9", "-P", &child.id().to_string()]).status();
    let _ = child.kill();
    let _ = child.wait();
    bot.book("UPDATE services_yos SET cron_due = at WHERE cron_due IS NOT NULL");
    let bot = bot.start();
    let got = comes(&mut alice, Duration::from_secs(10));
    assert_eq!(got, [Content::Yo]);
    assert_eq!(
        bot.book("SELECT count(*) FROM services_yos WHERE cron_due IS NOT NULL AND cron_done >= cron_due"),
        "[(1,)]"
    );
}

#[test]
fn a_hostile_script_gets_nothing_through_and_stops_nobody() {
    let hub = TestHub::start();
    let code = "if e['kind'] == 'picture': print(json.dumps([{'send': 'yo'}]))\n\
                elif not e['group']: print(json.dumps([{'send': 'yo', 'to': 'x'}]), 'x' * 3000000)\n";
    let bot = Bot::new(&hub, code, false).start();
    let mut alice = someone(&hub);
    alice.send_yo(&bot.name).unwrap();
    assert_eq!(comes(&mut alice, QUIET), []);
    let (photo, sealed) = media::seal(b"x", 4, 3).unwrap();
    alice.upload(&photo, 1).unwrap().send(&sealed).unwrap();
    alice.send(&bot.name, Content::Photo(photo)).unwrap();
    assert_eq!(comes(&mut alice, QUIET), [Content::Yo], "the next one is answered");
}

#[test]
fn a_hanging_script_holds_up_nobody() {
    let hub = TestHub::start();
    let code = "if e['kind'] == 'yo': time.sleep(60)\nprint(json.dumps([{'send': 'yo'}]))\n";
    let bot = Bot::new(&hub, code, false).start();
    let (mut alice, mut bob) = (someone(&hub), someone(&hub));
    alice.send_yo(&bot.name).unwrap();
    let (photo, sealed) = media::seal(b"x", 4, 3).unwrap();
    bob.upload(&photo, 1).unwrap().send(&sealed).unwrap();
    bob.send(&bot.name, Content::Photo(photo)).unwrap();
    assert_eq!(comes(&mut bob, QUIET), [Content::Yo]);
    assert_eq!(comes(&mut alice, QUIET), []);
}

#[test]
fn only_the_one_user_is_served() {
    let hub = TestHub::start();
    let (mut alice, mut bob) = (someone(&hub), someone(&hub));
    let mut bot = Bot::new(&hub, PING, false);
    bot.env.push(("SERVICES_ONLY_USER", bob.user_id().to_owned()));
    let bot = bot.start();
    alice.send_yo(&bot.name).unwrap();
    bob.send_yo(&bot.name).unwrap();
    assert_eq!(comes(&mut bob, QUIET), [Content::Yo]);
    assert_eq!(comes(&mut alice, Duration::from_secs(1)), []);
}

#[test]
fn another_name_is_another_account_and_the_script_knows_which() {
    let hub = TestHub::start();
    let code = "print(json.dumps([{'send': 'yo'}] if e['name'] == 'NAME' else []))\n";
    let mut bot = Bot::new(&hub, "", false);
    std::fs::write(bot.file("py"), HEAD.to_owned() + &code.replace("NAME", &bot.name)).unwrap();
    // The service is FIRST, this account its second name.
    let service = bot.dir.join("first");
    std::fs::create_dir(&service).unwrap();
    let toml = std::fs::read_to_string(bot.file("toml")).unwrap().replace("[\"BOT", "[\"FIRST\", \"BOT");
    std::fs::write(service.join("first.toml"), toml).unwrap();
    bot.env.push(("SERVICES_NAME", bot.name.clone()));
    bot.service = Some(service);
    let bot = bot.start();
    let mut alice = someone(&hub);
    alice.send_yo(&bot.name).unwrap();
    assert_eq!(comes(&mut alice, QUIET), [Content::Yo]);
}

#[test]
fn it_does_not_start_with_another_account_or_a_bad_toml() {
    let hub = TestHub::start();
    let bot = Bot::new(&hub, PING, false);
    let toml = bot.file("toml");
    let good = std::fs::read_to_string(&toml).unwrap();
    for bad in [good.replace(&bot.name, "OTHER"), good.replace("about", "abou"), String::new()] {
        std::fs::File::create(&toml).unwrap().write_all(bad.as_bytes()).unwrap();
        let mut child = bot
            .command(Path::new(env!("CARGO_BIN_EXE_yo-services")))
            .arg(bot.folder())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let code = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status.code();
            }
            assert!(start.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(code, Some(2));
    }
}

/// Anna connects the service; the other side Yos her, is told of her Yo and asks for it.
#[test]
fn a_connection_both_ways() {
    let hub = TestHub::start();
    // The other side: takes one call, which the script makes for a Yo of a connected user.
    let code =
        "if e['trigger'] == 'webhook' and e['path'] == 'yos': a = [{'reply': {'yos': e['yos'], 'out': e['out']}}]\n\
                elif e['trigger'] == 'webhook' and e['out']: a = [{'send': 'yo'}, {'reply': {'id': e['id']}}]\n\
                elif e['trigger'] == 'webhook': a = [{'reply': {'test': e['test']}}]\n\
                else: a = [{'call': 'https://localhost.invalid/x', 'body': {'user': e['out']}}] if e['out'] else []\n\
                print(json.dumps(a))\n";
    let mut bot = Bot::new(&hub, code, false);
    let toml = std::fs::read_to_string(bot.file("toml")).unwrap();
    let connect =
        "[bridge]\nreturn_url = \"https://other.invalid/back\"\ncall_urls = [\"https://localhost.invalid/\"]\n\
                   key_header = \"X-Partner-Key\"\n";
    std::fs::write(bot.file("toml"), toml + connect).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    for (key, value) in [
        ("SERVICES_SOCKET", bot.dir.join(format!("{}.sock", bot.name.to_lowercase())).display().to_string()),
        ("SERVICES_KEY", "the-partners-own-key".into()),
    ] {
        bot.env.push((key, value));
    }
    let bot = bot.start();
    let mut entrance = Command::new(env!("CARGO_BIN_EXE_yo-services"))
        .arg("entrance")
        .env("ENTRANCE_LISTEN", format!("127.0.0.1:{port}"))
        .env("ENTRANCE_SOCKETS", &bot.dir)
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(1));
    let base = format!("http://127.0.0.1:{port}/{}", bot.name.to_lowercase());
    let http = reqwest::blocking::Client::new();
    let exchange = |code: &str, secret: &str| {
        let form = format!(
            "grant_type=authorization_code&client_id={}&client_secret={secret}&code={code}&\
             redirect_uri=https%3A%2F%2Fother.invalid%2Fback",
            bot.name.to_lowercase()
        );
        let request = http.post(format!("{base}/token")).header("content-type", "application/x-www-form-urlencoded");
        let answer = request.body(form).send().unwrap();
        (answer.status().as_u16(), answer.text().unwrap())
    };
    let call = |path: &str, key: &str| {
        let answer = http.post(format!("{base}/{path}")).bearer_auth(key).body("{}").send().unwrap();
        (answer.status().as_u16(), answer.text().unwrap())
    };
    let (mut alice, code) = (someone(&hub), "the-state-of-the-other-side");
    alice
        .connect_service(
            &bot.name,
            sha2::Digest::finalize(sha2::Digest::chain_update(sha2::Sha256::default(), code)).into(),
        )
        .unwrap();
    assert_eq!(exchange(code, "wrong").0, 401);
    let (status, token) = exchange(code, "the-partners-own-key");
    assert_eq!(status, 200, "{token}");
    let key = token.split('"').find(|part| part.len() == 43).unwrap().to_owned();
    assert_eq!(exchange(code, "the-partners-own-key").0, 400, "once");

    let (status, reply) = call("yo", &key);
    assert!(status == 200 && reply.contains("id"), "{reply}");
    assert_eq!(comes(&mut alice, QUIET), [Content::Yo]);
    assert_eq!((call("yo", "wrong").0, http.post(format!("{base}/yo")).send().unwrap().status().as_u16()), (401, 401));
    let keyed = http.get(format!("{base}/x")).header("x-partner-key", "the-partners-own-key").send().unwrap();
    assert_eq!((keyed.status().as_u16(), keyed.text().unwrap()), (200, r#"{"test":null}"#.into()));

    alice.send_yo(&bot.name).unwrap();
    someone(&hub).send_yo(&bot.name).unwrap();
    std::thread::sleep(Duration::from_secs(3));
    let (_, yos) = call("yos", &key);
    assert_eq!(yos.matches(r#""kind":"yo""#).count(), 1, "hers alone: {yos}");
    assert!(!yos.contains(alice.user_id()));
    assert_eq!(bot.book("SELECT count(*) FROM services_yos WHERE user IS NOT NULL"), "[(1,)]");
    let secrets = bot.book("SELECT hex(key), out FROM services_secrets");
    assert!(!secrets.contains(&key), "only its hash");
    let _ = entrance.kill();
    let _ = entrance.wait();
}

/// The real ones, from a checkout of yo-services (`YO_SERVICES`, else nothing to do here).
#[test]
fn the_real_services_run() {
    let Some(services) = std::env::var_os("YO_SERVICES").map(|dir| PathBuf::from(dir).join("yo")) else { return };
    let hub = TestHub::start();
    for (file, wanted) in [("ping", "Yo"), ("yogpt", "Photo")] {
        let bot = Bot::new(&hub, "", false);
        let source = services.join(file).join(file);
        std::fs::copy(source.with_extension("py"), bot.file("py")).unwrap();
        let toml = std::fs::read_to_string(source.with_extension("toml")).unwrap();
        let toml: Vec<_> = toml
            .lines()
            .map(|l| if l.starts_with("names") { format!("names = [{:?}]", bot.name) } else { l.to_owned() })
            .collect();
        std::fs::write(bot.file("toml"), toml.join("\n")).unwrap();
        for picture in std::fs::read_dir(services.join(file).join("assets")).into_iter().flatten().flatten() {
            let _ = std::fs::create_dir(bot.folder().join("assets"));
            std::fs::copy(picture.path(), bot.folder().join("assets").join(picture.file_name())).unwrap();
        }
        let bot = bot.start();
        let mut alice = someone(&hub);
        alice.send_yo(&bot.name).unwrap();
        let got = format!("{:?}", comes(&mut alice, Duration::from_secs(7)));
        assert!(got.starts_with(&format!("[{wanted}")), "{file}: {got}");
        assert_eq!(bot.book("SELECT count(*) FROM services_yos WHERE user IS NOT NULL"), "[(0,)]");
    }
}
