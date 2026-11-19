//! What the CLI and skill tests share: the `yo` binary against a hub in-process, people as core clients.
#![allow(dead_code)]

use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};
use yo_core::media::{self, Media};
use yo_core::{Client, Content, Event};
pub use yo_tests::{random_u64, register, username, Http, TestHub};

pub const YO: &str = env!("CARGO_BIN_EXE_yo");

/// An account directory of `yo` on a hub, with the hub's pins in its environment.
pub struct Yo {
    pub state: PathBuf,
    pub hub: String,
}

impl Yo {
    pub fn new(hub: &TestHub) -> Yo {
        Yo { state: state_dir(), hub: hub.url() }
    }

    /// What `yo` and the skills find the account and its hub by.
    fn envs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("YO_STATE", self.state.display().to_string()),
            ("YO_HUB", self.hub.clone()),
            ("YO_HUB_KEYS", yo_wire::b64(&yo_tests::hub_key())),
            ("YO_TRUST_ROOTS", yo_wire::b64(&yo_tests::trust_root())),
            ("YO_ZK_PARAMS", yo_wire::b64(&yo_tests::zk_params())),
            ("YO_BIN", YO.to_owned()),
        ]
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(YO);
        command.envs(self.envs());
        command
    }

    /// Runs a command: its exit code and its JSON (stdout, or stderr's on a failure).
    pub fn run(&self, args: &[&str]) -> (i32, Value) {
        let out = self.command().args(args).output().unwrap();
        let text = if out.status.success() { &out.stdout } else { &out.stderr };
        let text = String::from_utf8_lossy(text);
        let json = serde_json::from_str(text.lines().last().unwrap_or("null")).unwrap_or(Value::Null);
        (out.status.code().unwrap_or(-1), json)
    }

    /// A command that must work: its JSON.
    pub fn ok(&self, args: &[&str]) -> Value {
        let (code, json) = self.run(args);
        assert_eq!(code, 0, "yo {args:?}: {json}");
        json
    }

    /// A skill (`skills/NAME/NAME.py`) on this directory, registering `username` at its first start.
    pub fn skill(&self, name: &str, username: &str, args: &[&str], env: &[(&str, &str)]) -> Process {
        Process::spawn(self.skill_command(name, username, args, env))
    }

    pub fn skill_command(&self, name: &str, username: &str, args: &[&str], env: &[(&str, &str)]) -> Command {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../skills/{name}/{name}.py"));
        let mut python = Command::new("python3");
        python.arg(script).args(args).envs(self.envs()).envs(env.iter().copied()).env("YO_USERNAME", username);
        python
    }
}

use std::os::unix::process::CommandExt;

/// A child in its own process group (a skill and its `yo listen`), killed with the group.
pub struct Process(pub Child);

impl Process {
    pub fn spawn(mut command: Command) -> Process {
        command.process_group(0);
        Process(command.spawn().unwrap())
    }

    pub fn spawn_piped(mut command: Command) -> (Process, BufReader<ChildStdout>) {
        command.stdout(Stdio::piped());
        let mut process = Process::spawn(command);
        let out = BufReader::new(process.0.stdout.take().unwrap());
        (process, out)
    }

    /// Its exit code, once it ended within `limit`.
    pub fn exit_within(&mut self, limit: Duration) -> Option<i32> {
        let start = Instant::now();
        while start.elapsed() < limit {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        None
    }

    pub fn running(&mut self) -> bool {
        self.0.try_wait().unwrap().is_none()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = Command::new("kill").args(["-9", &format!("-{}", self.0.id())]).stderr(Stdio::null()).status();
        let _ = self.0.wait();
    }
}

/// The next JSON line of a `yo listen`.
pub fn next_line(out: &mut BufReader<ChildStdout>) -> Value {
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap_or_else(|_| panic!("not a JSON line: {line:?}"))
}

pub fn state_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("yo-cli-{}", random_u64()))
}

pub fn someone(hub: &TestHub) -> Client {
    register(Http::new(hub.url()), &username(random_u64() % 1_000_000_000), None)
}

/// Waits until `name` is registered on the hub (a skill's first start).
pub fn registered(hub: &TestHub, name: &str) {
    let ready = Instant::now();
    while Client::username_free(Http::new(hub.url()).as_ref(), name).unwrap() {
        assert!(ready.elapsed() < Duration::from_secs(20), "{name} never registered");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Yos from anyone until `quiet` passes without one: who sent each.
pub fn yos_until_quiet(client: &mut Client, quiet: Duration) -> Vec<String> {
    let mut from = Vec::new();
    let mut last = Instant::now();
    while last.elapsed() < quiet {
        for event in client.sync(1).unwrap() {
            if let Event::Message { user, content: Content::Yo, .. } = event {
                from.push(user);
                last = Instant::now();
            }
        }
        client.acknowledge_events();
    }
    from
}

/// Yos and pictures from anyone until `quiet` passes without one: how many Yos, and the pictures, opened.
pub fn answers_until_quiet(client: &mut Client, quiet: Duration) -> (usize, Vec<Vec<u8>>) {
    let (mut yos, mut pictures) = (0, Vec::<Media>::new());
    let mut last = Instant::now();
    while last.elapsed() < quiet {
        for event in client.sync(1).unwrap() {
            match event {
                Event::Message { content: Content::Yo, .. } => yos += 1,
                Event::Message { content: Content::Photo(media), .. } => pictures.push(media),
                _ => continue,
            }
            last = Instant::now();
        }
        client.acknowledge_events();
    }
    let transport = client.transport();
    let opened = pictures.iter().map(|m| media::fetch_open(transport.as_ref(), m).unwrap().unwrap()).collect();
    (yos, opened)
}

/// What comes from `from` (in `group`, or one to one) until `quiet` passes without anything: each as it came, a
/// photo's or video's media with its bytes, fetched and opened.
pub fn echoes(
    client: &mut Client,
    from: &str,
    group: Option<&str>,
    quiet: Duration,
) -> Vec<(Content, Option<Vec<u8>>)> {
    let (mut got, mut last) = (Vec::new(), Instant::now());
    while last.elapsed() < quiet {
        for event in client.sync(1).unwrap() {
            let content = match event {
                Event::Message { user, content, .. } if group.is_none() && user == from => content,
                Event::GroupMessage { group: g, user, content, .. } if Some(g.as_str()) == group && user == from => {
                    content
                }
                _ => continue,
            };
            let bytes = match &content {
                Content::Photo(m) | Content::Video { media: m, .. } => {
                    Some(media::fetch_open(client.transport().as_ref(), m).unwrap().expect("blob there"))
                }
                _ => None,
            };
            got.push((content, bytes));
            last = Instant::now();
        }
        client.acknowledge_events();
    }
    got
}

/// `echoes` that waits for the first however long a loaded machine takes, then gathers until `quiet`.
pub fn echoed(
    client: &mut Client,
    from: &str,
    group: Option<&str>,
    quiet: Duration,
) -> Vec<(Content, Option<Vec<u8>>)> {
    let start = Instant::now();
    loop {
        let got = echoes(client, from, group, quiet);
        if !got.is_empty() {
            return got;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "nothing came");
    }
}

/// Who sent the first Yo, waited for however long a loaded machine takes, then until it is quiet.
pub fn first_yo(client: &mut Client) -> String {
    let start = Instant::now();
    loop {
        if let Some(from) = yos_until_quiet(client, Duration::from_millis(500)).into_iter().next() {
            return from;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "no Yo came");
    }
}

/// Uploads `bytes` for `fetches` fetches: the media to send.
pub fn uploaded(client: &mut Client, bytes: &[u8], fetches: u32) -> Media {
    let (m, sealed) = media::seal(bytes, 4, 3).unwrap();
    client.upload(&m, fetches).unwrap().send(&sealed).unwrap();
    m
}

/// A start, a frame header (720 × 1280), a scan of `len` bytes (no 0xFF in them), the end: a JPEG as `yo send --photo`
/// takes it and sends it as it is.
pub fn picture_of(len: usize) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, 0x05, 0x00, 0x02, 0xD0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    bytes.extend_from_slice(&[0xFF, 0xDA, 0, 2]);
    bytes.extend((0..len).map(|_| random_u64() as u8 & 0x7F));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}
