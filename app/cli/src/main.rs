//! yo: a Yo account on the command line. On a terminal it speaks to a person (text, `yo tui`); piped or with
//! `--json` every command prints one JSON value and `yo listen` one JSON line per event. Bots drive it (README.md).
//!
//!   yo [--profile NAME] [--state DIR] [--hub URL] [--hub-key KEY]… [--trust-root KEY]… [--zk-params PARAMS] [--ohttp-key CONFIG]…
//!      [--direct] [--json] [--quiet] [--no-color] COMMAND …
//!
//! Every request goes wrapped (Oblivious HTTP, spec 4.1) to the hub's pinned OHTTP key; `--direct` sends them as
//! they are.
//!
//! One process owns a state directory at a time. While `yo listen` runs, the other commands go to it over the
//! directory's control socket and run there, on its client; otherwise they open the directory themselves.

mod account;
mod commands;
mod human;
mod jpeg;
mod listen;
mod pins;
mod signin;
mod tui;

use account::{Config, Open};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Exit codes: 0 done; 1 failed (`retryable` says whether later may work); 2 usage; 3 this device is gone for good
/// (`410`: replaced by another, or the name released): stop; 4 nothing registered in the directory; 5 the hub did
/// not accept this device's signature (`401`: a clock still off after a reboot, or a hub reset): try again later.
pub const FAILED: i32 = 1;
pub const USAGE: i32 = 2;
pub const GONE: i32 = 3;
pub const NOT_REGISTERED: i32 = 4;
pub const UNAUTHORIZED: i32 = 5;

/// How long a command waits for a directory another command has (a listener answers at once).
const BUSY_WAIT: Duration = Duration::from_secs(30);

/// The release (build.rs: the tag, else version.properties), e.g. 1.0.0-RC47.
pub const VERSION: &str = env!("YO_VERSION");

pub const HELP: &str = "Yo from your terminal: send Yos, keep friends, run bots.

USAGE
  yo <command> [flags]
  yo NAME                 Send NAME a Yo
  yo tui                  Open the app (also bare yo on a terminal)

CORE COMMANDS
  send       Send a Yo to a friend, user ID or group:ID (--photo JPEG, --media -)
  friends    Friends and groups; friends add|remove NAME
  groups     Groups: create NAME MEMBER…, rename, add, remove, leave, accept
  requests   Requests from Strangers: list, accept|decline|block NAME, on|off
  block      Block someone; unblock NAME undoes it
  whois      Look up a name
  listen     Receive: one JSON line per event until stopped (--manual-ack, --drain-media)
  sync       Receive once (--wait S); inbox does the same, ack confirms

ACCOUNT
  signup     New account: a NAME of 12+ characters; a bot unless --human
  signin     Sign in: --recovery-key, or --link (QR code)
  signout    Sign out: deletes this device's account files (--yes)
  link       Link a new device to this account (asks first)
  status     Account, device, hub and listener (also whoami)
  profiles   Accounts kept as profiles (--profile NAME)
  devices    Devices; devices remove N
  recovery   Recovery Key: show, rotate
  bot        bot on: mark the account as a bot, for good

SERVICES
  services   list, connect SERVICE CODE, decline SERVICE CODE, disconnect SERVICE

SYSTEM
  tui         Open the app
  commands    All commands, one per line (JSON when piped)
  doctor      Check the setup and the account
  probe       Yo PING and time the answer
  completion  Tab completion: completion bash|zsh|fish
  version     Show the version

HELP TOPICS
  output       Text on a terminal, JSON when piped
  exit-codes   What each exit code means
  environment  Environment variables

FLAGS
  -j, --json        JSON output (also when piped, or YO_JSON=1)
  -q, --quiet       No output on success
      --no-color    No color (or NO_COLOR)
      --profile NAME  Use another account: ~/.local/share/yo/profiles/NAME (YO_PROFILE)
      --state DIR   Account folder (YO_STATE; default ~/.local/share/yo); overrides --profile
      --hub URL     Hub (YO_HUB; default https://hub.justyo.app)
      --direct      Skip Oblivious HTTP (debugging only)
      --hub-key K…  --trust-root K…  --zk-params P  --ohttp-key C…  Keys to pin (built in for known hubs)
  -h, --help        Show help

EXAMPLES
  $ yo anna
  $ yo tui
  $ yo friends add PING
  $ yo listen --json | jq

LEARN MORE
  yo commands          All commands
  yo help <topic>      A help topic
  yo <command> --help  Help for a command";

/// Help text styled as hey's on a terminal with colour: headings in the accent, examples in italics.
fn styled(text: &str) -> String {
    use std::io::IsTerminal;
    if !mode().color || !std::io::stdout().is_terminal() {
        return text.to_owned();
    }
    text.lines()
        .map(|line| {
            let heading = !line.is_empty() && !line.starts_with(' ') && line == line.to_uppercase();
            if heading {
                format!("\x1b[1;35m{line}\x1b[0m")
            } else if let Some(example) = line.strip_prefix("  $ ") {
                format!("  \x1b[2m$\x1b[0m \x1b[3m{example}\x1b[0m")
            } else if let Some((cmd, rest)) = line.strip_prefix("  ").and_then(|l| l.split_once("  ")) {
                format!("  \x1b[1m{cmd}\x1b[0m  {rest}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The help of one command: its line of the help, with the usage.
fn command_help(name: &str) -> Option<String> {
    let line =
        HELP.lines().find(|l| l.strip_prefix("  ").is_some_and(|l| l.split_whitespace().next() == Some(name)))?;
    let short = line.trim_start().trim_start_matches(name).trim();
    Some(format!("{short}\n\nUSAGE\n  yo {name} …\n\nLEARN MORE\n  yo help             All commands\n  yo commands         One command per line"))
}

/// `yo commands`: every command with what it does, from the help.
fn commands_list() -> Vec<(String, String, String)> {
    let mut group = String::new();
    let mut out = Vec::new();
    for line in HELP.lines() {
        if !line.is_empty() && !line.starts_with(' ') && line == line.to_uppercase() {
            group = line.trim_end_matches(" COMMANDS").to_lowercase();
            continue;
        }
        if !matches!(group.as_str(), "core" | "account" | "services" | "system") {
            continue;
        }
        if let Some((name, short)) = line.trim_start().split_once(' ') {
            out.push((group.clone(), name.to_owned(), short.trim().to_owned()));
        }
    }
    out
}

const HELP_OUTPUT: &str = "Output: text on a terminal. Piped, with --json or YO_JSON=1: one JSON value per command on
stdout (errors as {\"error\",\"retryable\"} on stderr); yo listen prints one JSON line per event:
  {\"type\":\"ready\",\"user\",\"username\"}  {\"type\":\"message\",\"from\",\"group\",\"reply_to\",\"sent_at_ms\",\"content\":\"yo|photo|video\",\"media\"}
  {\"type\":\"new_chat|profile|identity_changed|group_changed|device_added|connection_changed|sent|probe|connect\",...}
friends: [{user,name,handles,blocked}]  send: {sent}  add: {user,username}  status: {user,username,device,bot,hub,listening,...}";

const HELP_EXIT: &str =
    "Exit codes: 0 done · 1 failed (see retryable) · 2 wrong usage · 3 device removed for good: stop ·
4 no account · 5 hub refused the signature: try again later · 130 stopped (Ctrl-C)";

const HELP_ENV: &str = "Environment: YO_STATE, YO_HUB, YO_HUB_KEYS, YO_TRUST_ROOTS, YO_ZK_PARAMS, YO_OHTTP_KEYS (lists
comma separated), YO_JSON=1, NO_COLOR, YO_SMALL_ENVELOPES=1, YO_RECOVERY_LOOKUPS (default 10; a bot raises it),
YO_TIMING=1 (timing notes on stderr)";

/// The words that are commands: anything else as the first word is a friend to send a Yo to.
const VERBS: &[&str] = &[
    "register",
    "signup",
    "signin",
    "signout",
    "profiles",
    "link",
    "whoami",
    "status",
    "friends",
    "contacts",
    "whois",
    "add",
    "remove",
    "send",
    "requests",
    "groups",
    "listen",
    "sync",
    "inbox",
    "ack",
    "echo-probe",
    "bot",
    "connections",
    "disconnect",
    "service-answer",
    "connect",
    "decline",
    "services",
    "probe",
    "block",
    "unblock",
    "devices",
    "recovery",
    "tui",
    "doctor",
    "completion",
    "version",
    "help",
    "commands",
    "friends-view",
    "link-approve",
];

/// How this run prints.
pub struct Mode {
    pub json: bool,
    pub quiet: bool,
    pub color: bool,
}

static MODE: OnceLock<Mode> = OnceLock::new();

pub fn mode() -> &'static Mode {
    MODE.get_or_init(|| Mode { json: true, quiet: false, color: false })
}

/// A command that did not work: its exit code and why, for stderr as JSON.
#[derive(Debug)]
pub struct Failure {
    pub code: i32,
    pub error: String,
    pub retryable: bool,
}

impl Failure {
    pub fn new(error: impl Into<String>) -> Failure {
        Failure { code: FAILED, error: error.into(), retryable: false }
    }

    pub fn usage(error: impl Into<String>) -> Failure {
        Failure { code: USAGE, error: error.into(), retryable: false }
    }

    pub fn not_registered() -> Failure {
        Failure { code: NOT_REGISTERED, error: "no account here: yo signup NAME or yo signin".into(), retryable: false }
    }

    pub fn json(&self) -> Value {
        json!({ "error": self.error, "retryable": self.retryable })
    }
}

impl From<yo_core::Error> for Failure {
    fn from(error: yo_core::Error) -> Failure {
        let code = if gone(&error) {
            GONE
        } else if unauthorized(&error) {
            UNAUTHORIZED
        } else {
            FAILED
        };
        Failure { code, error: reason(&error), retryable: yo_core::retryable(&error) || unauthorized(&error) }
    }
}

/// `410`: replaced, or the name released. The only answer that stops a bot for good.
pub fn gone(error: &yo_core::Error) -> bool {
    matches!(error, yo_core::Error::Replaced)
}

/// `401`: the hub did not take this device's signature. A clock far off (before NTP after a reboot) or a hub
/// restarting gives it too, so it is never final: later, again.
pub fn unauthorized(error: &yo_core::Error) -> bool {
    matches!(error, yo_core::Error::Refused(401))
}

/// An error for the output, without text that came from outside (libsignal's reasons).
pub fn reason(error: &yo_core::Error) -> String {
    match error {
        yo_core::Error::Session(_) => "session".into(),
        other => other.to_string(),
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let env_json = std::env::var("YO_JSON").is_ok_and(|v| v.trim() == "1");
    let mut take = |flags: &[&str]| {
        let before = args.len();
        args.retain(|a| !flags.contains(&a.as_str()));
        args.len() != before
    };
    let json = take(&["-j", "--json"]) || env_json || !std::io::stdout().is_terminal();
    let quiet = take(&["-q", "--quiet"]);
    let no_color = take(&["--no-color"]) || std::env::var_os("NO_COLOR").is_some();
    let tui = take(&["--tui"]);
    let _ = MODE.set(Mode { json, quiet, color: !no_color });
    if take(&["--help"]) || take(&["-h"]) {
        let word = args.iter().find(|a| !a.starts_with('-') && VERBS.contains(&a.as_str()));
        let text = word.and_then(|w| command_help(w)).unwrap_or_else(|| HELP.to_owned());
        println!("{}", styled(&text));
        std::process::exit(0);
    }
    // `yo tui --state DIR`: the flags after `tui` count as before it.
    if let Some(at) = args
        .iter()
        .position(|a| !a.starts_with('-'))
        .filter(|&i| args[i] == "tui" && (i == 0 || !args[i - 1].starts_with("--") || args[i - 1] == "--direct"))
    {
        args.remove(at);
        args.push("tui".into());
        let tail = args.split_off(at);
        let (flags, words): (Vec<String>, Vec<String>) = tail.into_iter().partition(|a| a != "tui");
        args.extend(flags);
        args.extend(words);
    }
    let parsed = Config::parse(&args);
    let (config, command) = match parsed {
        Ok(parsed) => parsed,
        Err(failure) => std::process::exit(print("", Err(failure))),
    };
    let globals: Vec<String> = args[..args.len() - command.len()].to_vec();
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if tui || command.first().is_some_and(|c| c == "tui") || (command.is_empty() && interactive) {
        std::process::exit(tui::run(&config, &globals));
    }
    if command.is_empty() {
        println!("{}", styled(HELP));
        std::process::exit(if command.is_empty() { USAGE } else { 0 });
    }
    let code = match media_from_stdin(command) {
        Ok(command) => dispatch(&config, &command),
        Err(failure) => print("", Err(failure)),
    };
    std::process::exit(code);
}

/// `send … --media -`: the reference read from stdin, never from the arguments, which every local user can read
/// (it holds the blob's id and key). The command goes on with it, here or in the listener (its private socket).
fn media_from_stdin(command: &[String]) -> Result<Vec<String>, Failure> {
    let mut command = command.to_vec();
    let at = command.iter().position(|a| a == "--media").filter(|_| command.first().is_some_and(|c| c == "send"));
    if let Some(i) = at.map(|i| i + 1) {
        if command.get(i).map(String::as_str) != Some("-") {
            return Err(Failure::usage("--media -: pass the media reference on stdin, not as an argument"));
        }
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .take(4096)
            .read_line(&mut line)
            .map_err(|_| Failure::usage("--media -: nothing on stdin"))?;
        command[i] = line.trim().to_owned();
    }
    Ok(command)
}

/// Prints what a command gave (stdout) or why it failed (stderr), and its exit code: JSON, or text for a person.
pub fn print(kind: &str, outcome: Result<Value, Failure>) -> i32 {
    let mode = mode();
    match outcome {
        Ok(_) if mode.quiet => 0,
        Ok(value) if mode.json => {
            println!("{value}");
            0
        }
        Ok(value) => {
            let text = human::render(kind, &value);
            if !text.is_empty() {
                println!("{text}");
            }
            0
        }
        Err(failure) if mode.json => {
            eprintln!("{}", failure.json());
            failure.code
        }
        Err(failure) => {
            eprintln!("error: {}", failure.error);
            failure.code
        }
    }
}

/// A word that names someone to send a Yo to.
fn is_target(word: &str) -> bool {
    word.starts_with(yo_core::shared::GROUP_PREFIX)
        || (!word.starts_with('-')
            && !word.is_empty()
            && word.trim_start_matches('@').chars().all(|c| c.is_alphanumeric() || "_.-".contains(c)))
}

/// The commands of v1 (`friends add`, `services list`, `yo NAME`) as the verbs a listener runs (the old ones, which
/// stay for bots); the account and system commands here.
fn dispatch(config: &Config, command: &[String]) -> i32 {
    let human = !mode().json;
    let args: Vec<&str> = command.iter().map(String::as_str).collect();
    let words = |w: &[&str]| -> Vec<String> { w.iter().map(|s| s.to_string()).collect() };
    let internal: Vec<String> = match args.as_slice() {
        ["help", "output"] => return text(HELP_OUTPUT),
        ["help", "exit-codes"] => return text(HELP_EXIT),
        ["help", "environment"] => return text(HELP_ENV),
        ["help", name] if command_help(name).is_some() => return text(&command_help(name).unwrap_or_default()),
        ["help"] => return text(HELP),
        ["help", topic, ..] => return print("", Err(Failure::usage(format!("no help topic {topic}; see yo help")))),
        ["commands"] if human => {
            let list = commands_list();
            let width = list.iter().map(|c| c.1.len()).max().unwrap_or(0);
            let mut group = String::new();
            for (g, name, short) in &list {
                if *g != group {
                    if !group.is_empty() {
                        println!();
                    }
                    println!("{}", styled(&g.to_uppercase()));
                    group = g.clone();
                }
                println!("  {}  {}", human::paint("1", &format!("{name:<width$}")), human::paint("2", short));
            }
            return 0;
        }
        ["commands"] => {
            let list: Vec<Value> =
                commands_list().into_iter().map(|(g, n, s)| json!({ "group": g, "name": n, "short": s })).collect();
            return print("commands", Ok(json!(list)));
        }
        ["profiles"] => return print("profiles", Ok(profiles())),
        ["version"] => {
            return print(
                "version",
                Ok(json!({ "version": VERSION, "core_version": yo_core::VERSION, "protocol_version": yoip::VERSION })),
            )
        }
        ["completion", shell @ ("bash" | "zsh" | "fish")] => return text(&human::completion(shell, VERBS)),
        ["completion", ..] => return print("", Err(Failure::usage("usage: yo completion bash|zsh|fish"))),
        ["doctor"] => return print("doctor", Ok(doctor(config))),
        ["register", rest @ ..] => return signup(config, rest, false),
        ["signup", rest @ ..] => return signup(config, rest, true),
        ["signout"] | ["signout", "--yes"] => {
            if args.len() == 1 {
                if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                    return print("", Err(Failure::usage("piped: use yo signout --yes")));
                }
                eprintln!("{} [y/N]", account::SIGNOUT_PROMPT);
                let mut answer = String::new();
                if let Err(e) = std::io::stdin().read_line(&mut answer) {
                    return print("", Err(Failure::new(e.to_string())));
                }
                if !answer.trim().eq_ignore_ascii_case("y") {
                    return print("signout", Ok(json!({"signed_out": false})));
                }
            }
            return print("signout", account::signout(config).map(|()| json!({"signed_out": true})));
        }
        ["signout", ..] => return print("", Err(Failure::usage("usage: yo signout [--yes]"))),
        ["signin", "--recovery-key", key @ ..] => {
            let key = match key {
                [key] => key.to_string(),
                _ => match signin::read_secret("Recovery key: ") {
                    Ok(key) => key,
                    Err(failure) => return print("", Err(failure)),
                },
            };
            return print("signin", signin::recovery(config, &key));
        }
        ["signin", "--link"] => return print("signin", signin::link_cli(config)),
        ["signin", ..] => return print("", Err(Failure::usage("usage: yo signin --recovery-key | --link"))),
        ["recovery"] | ["recovery", "show"] if !account::has_account(config) => {
            return print("", Err(Failure::not_registered()))
        }
        ["recovery"] | ["recovery", "show"] => {
            let key = account::read_file(config, "recovery").map(|k| String::from_utf8_lossy(&k).into_owned());
            return print(
                "recovery",
                key.map(|k| json!({ "recovery_key": k }))
                    .ok_or_else(|| Failure::new("no Recovery Key saved here: yo recovery rotate makes a new one")),
            );
        }
        ["link", text @ ..] => match signin::confirm_link(text.first().copied()) {
            Ok(text) => return print("link-approve", signin::approve_link(config, &text)),
            Err(failure) => return print("", Err(failure)),
        },
        ["friends"] | ["friends", "list"] if human => words(&["friends-view"]),
        ["friends", "list"] => words(&["friends"]),
        ["friends", "add", name] => words(&["add", name]),
        ["friends", "remove", name] => words(&["remove", name]),
        ["requests"] if human => words(&["requests", "list"]),
        ["services"] | ["services", "list"] => words(&["connections"]),
        ["services", "answer", rest @ ..] => [vec!["service-answer".to_string()], words(rest)].concat(),
        ["services", rest @ ..] => words(rest),
        ["inbox"] => words(&["sync"]),
        [name, rest @ ..] if !VERBS.contains(name) && is_target(name) => {
            [vec!["send".to_string(), name.to_string()], words(rest)].concat()
        }
        _ => command.to_vec(),
    };
    let kind = internal.first().cloned().unwrap_or_default();
    if kind == "listen" {
        return listen_or_print(config, &internal);
    }
    let shown = match kind.as_str() {
        "send" => format!("send {}", internal.get(1).map(String::as_str).unwrap_or("")),
        _ => kind.clone(),
    };
    print(&shown, execute(config, &internal))
}

fn text(text: &str) -> i32 {
    println!("{}", styled(text));
    0
}

/// `yo listen`: the directory's listener, here.
fn listen_or_print(config: &Config, command: &[String]) -> i32 {
    let started = Instant::now();
    loop {
        match account::open(config) {
            Ok(Open::Opened(opened)) => return listen::listen(config, *opened, &command[1..]),
            Ok(Open::Busy) if UnixStream::connect(config.control()).is_ok() => {
                return print("", Err(Failure::new("yo listen is already running for this account")));
            }
            Ok(Open::Busy) if started.elapsed() > BUSY_WAIT => {
                return print(
                    "",
                    Err(Failure { retryable: true, ..Failure::new("busy: another yo command has this account") }),
                )
            }
            Ok(Open::Busy) => std::thread::sleep(Duration::from_millis(50)),
            Err(failure) => return print("", Err(failure)),
        }
    }
}

/// A command on the account: here, or in the listener that has the directory.
pub fn execute(config: &Config, command: &[String]) -> Result<Value, Failure> {
    command.first().ok_or_else(|| Failure::usage(HELP))?;
    let started = Instant::now();
    loop {
        match account::open(config)? {
            Open::Opened(opened) => {
                let client = std::sync::Mutex::new(opened.client);
                let context = commands::Context { client: &client, config, listening: false, emit: &|_| {} };
                return commands::run(&context, command);
            }
            Open::Busy => {
                if let Some(outcome) = forward(config, command) {
                    return outcome;
                }
            }
        }
        if started.elapsed() > BUSY_WAIT {
            return Err(Failure { retryable: true, ..Failure::new("busy: another yo command has this account") });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `signup NAME [--human] [--force]` (and `register`, the bots' verb, which makes no backup): the account, marked
/// as a bot unless `--human`; `signup` then makes its backup under a new recovery key, shown once.
fn signup(config: &Config, rest: &[&str], backup: bool) -> i32 {
    let usage = if backup { "usage: yo signup NAME [--human]" } else { "usage: yo register NAME [--human] [--force]" };
    let [username, flags @ ..] = rest else { return print("", Err(Failure::usage(usage))) };
    if !flags.iter().all(|f| matches!(*f, "--force" | "--bot" | "--human")) {
        return print("", Err(Failure::usage(usage)));
    }
    // `register` (the bots' verb) leaves the least to the hub, which may allow a bot's shorter name.
    if let Err(failure) = if backup { name_ok(username) } else { Ok(()) } {
        return print("", Err(failure));
    }
    let force = flags.contains(&"--force");
    let human = flags.contains(&"--human");
    print("signup", signup_run(config, username, human, force, backup))
}

/// A name the hub may take, checked here first: the form of spec 2.2, and 12 characters at the least (a client
/// that is not an attested app gets no shorter name).
pub fn name_ok(name: &str) -> Result<(), Failure> {
    use yoip::net::handle;
    if handle::check_username_form(name).is_err() {
        return Err(Failure::usage("name: letters, digits, _ and emoji, up to 16 characters, not starting with _"));
    }
    if handle::username_len(name) < 12 {
        return Err(Failure::usage("name too short: the CLI needs 12+ characters"));
    }
    Ok(())
}

pub fn signup_run(config: &Config, username: &str, human: bool, force: bool, backup: bool) -> Result<Value, Failure> {
    let started = Instant::now();
    loop {
        match account::register(config, username, force)? {
            Open::Opened(opened) => {
                let client = std::sync::Mutex::new(opened.client);
                let context = commands::Context { client: &client, config, listening: false, emit: &|_| {} };
                if !human {
                    commands::run(&context, &["bot".into(), "on".into()])?;
                }
                let mut out = commands::whoami(&context, false)?;
                if backup && account::read_file(config, "recovery").is_none() {
                    match commands::rotate_recovery(&context) {
                        Ok(made) => out["recovery_key"] = made["recovery_key"].clone(),
                        Err(failure) => out["recovery_error"] = json!(failure.error),
                    }
                }
                return Ok(out);
            }
            Open::Busy if started.elapsed() > BUSY_WAIT => {
                return Err(Failure { retryable: true, ..Failure::new("busy: another yo command has this account") })
            }
            Open::Busy => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// `yo profiles`: the default account's directory and each profile's (`--profile NAME`).
fn profiles() -> Value {
    let Some(base) = account::default_state() else { return json!([]) };
    let mut list = vec![json!({ "profile": null, "state": base })];
    let mut names: Vec<String> = std::fs::read_dir(format!("{base}/profiles"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    list.extend(names.into_iter().map(|n| json!({ "profile": n, "state": format!("{base}/profiles/{n}") })));
    json!(list)
}

/// What may be wrong: the directory, the pins, the account, a listener.
fn doctor(config: &Config) -> Value {
    let hub = config.hub.clone().unwrap_or_default();
    let ohttp = config.direct || !config.ohttp_keys.is_empty() || !yo_core::ohttp::pinned(&hub).is_empty();
    let status = execute(config, &["status".into()]);
    let check = |name: &str, ok: bool, detail: String| json!({ "check": name, "ok": ok, "detail": detail });
    let checks = vec![
        check("state", config.state.is_dir(), config.state.display().to_string()),
        check("hub", !hub.is_empty(), hub.clone()),
        check("ohttp", ohttp, if config.direct { "off (--direct)".into() } else { "on".into() }),
        check("hub keys", !config.hub_keys.is_empty(), format!("{} pinned", config.hub_keys.len())),
        check("trust roots", !config.trust_roots.is_empty(), format!("{} pinned", config.trust_roots.len())),
        check("group params", config.zk_params.is_some(), String::new()),
        match &status {
            Ok(s) => check("account", true, s["username"].as_str().unwrap_or("?").to_owned()),
            Err(f) => check("account", false, f.error.clone()),
        },
        match status.as_ref().is_ok_and(|s| s["listening"] == json!(true)) {
            true => check("listener", true, "running".into()),
            false => check("listener", false, "not running: start yo listen or yo tui".into()),
        },
    ];
    json!({ "checks": checks })
}

/// Runs the command in the listener that has the directory, if one answers.
pub fn forward(config: &Config, command: &[String]) -> Option<Result<Value, Failure>> {
    let mut stream = UnixStream::connect(config.control()).ok()?;
    let request = json!(command).to_string() + "\n";
    stream.write_all(request.as_bytes()).ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let answer: Value = serde_json::from_str(&line).ok()?;
    let code = answer["code"].as_i64().unwrap_or(FAILED as i64) as i32;
    let out = answer["out"].clone();
    Some(match code {
        0 => Ok(out),
        _ => Err(Failure {
            code,
            error: out["error"].as_str().unwrap_or("failed").to_owned(),
            retryable: out["retryable"].as_bool().unwrap_or(false),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a device gone for good stops a bot (3); a `401` (a clock off after a reboot) is tried again (5).
    #[test]
    fn only_a_replaced_device_is_final() {
        let gone = Failure::from(yo_core::Error::Replaced);
        assert_eq!(gone.code, GONE);
        let refused = Failure::from(yo_core::Error::Refused(401));
        assert_eq!((refused.code, refused.retryable), (UNAUTHORIZED, true));
        let busy = Failure::from(yo_core::Error::Refused(503));
        assert_eq!((busy.code, busy.retryable), (FAILED, true));
        assert!(!Failure::from(yo_core::Error::Refused(404)).retryable);
    }

    #[test]
    fn a_friend_is_a_target_a_flag_is_not() {
        assert!(is_target("anna") && is_target("@anna") && is_target("group:ab12") && is_target("ROBOTER_01"));
        assert!(!is_target("--photo") && !is_target("a b"));
    }
}
