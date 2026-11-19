//! yo: a Yo account on the command line. Every command prints one JSON value; `yo listen` prints one JSON line per
//! event. The bots in `skills/` drive it (README.md).
//!
//!   yo [--state DIR] [--hub URL] [--hub-key KEY]… [--trust-root KEY]… [--zk-params PARAMS] COMMAND …
//!
//! One process owns a state directory at a time. While `yo listen` runs, the other commands go to it over the
//! directory's control socket and run there, on its client; otherwise they open the directory themselves.

mod account;
mod commands;
mod jpeg;
mod listen;

use account::{Config, Open};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
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

pub const HELP: &str =
    "usage: yo [--state DIR] [--hub URL] [--hub-key KEY]... [--trust-root KEY]... [--zk-params P] COMMAND
  register NAME [--force]           register the account (once; again finishes an interrupted one); --force
                                    only to replace this directory's device when its state is lost
  whoami | status                   who this is; status adds counts and whether a listener runs
  friends | contacts                the friend list; everyone this account has a chat with
  whois NAME                        a username's user id
  add NAME                          add a friend
  send TO [--photo JPEG | --media -]
                                    a Yo to a username, user id or group:ID, with a photo, or the photo
                                    or video of a received event sealed anew (its media.ref on stdin)
  groups [list]                     the groups
  groups create NAME MEMBER...      make a group (members by username or user id)
  groups leave|accept ID            leave or accept a group
  groups rename ID NAME             rename a group
  groups add|remove ID MEMBER...    change its members
  listen [--drain-media] [--manual-ack]
                                    receive: one JSON line per event, until stopped
  sync [--wait S] [--drain-media]   receive once (no listener running)
  ack                               acknowledge what `listen --manual-ack` handed out
  echo-probe USER TOKEN             answer a round-trip probe (the PINGTEST service)
  probe [--timeout-ms N]            measure the round trip to PINGTEST
environment: YO_STATE, YO_HUB, YO_HUB_KEYS, YO_TRUST_ROOTS, YO_ZK_PARAMS (lists comma separated)";

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
        Failure { code: NOT_REGISTERED, error: "not registered: yo register NAME".into(), retryable: false }
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help" | "help") {
        println!("{HELP}");
        std::process::exit(if args.is_empty() { USAGE } else { 0 });
    }
    let parsed = Config::parse(&args).and_then(|(config, command)| Ok((config, media_from_stdin(command)?)));
    let code = match parsed {
        Ok((config, command)) => run(&config, &command),
        Err(failure) => print(Err(failure)),
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
            return Err(Failure::usage("--media -: the media.ref on stdin, never as an argument"));
        }
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .take(4096)
            .read_line(&mut line)
            .map_err(|_| Failure::usage("--media -: no reference on stdin"))?;
        command[i] = line.trim().to_owned();
    }
    Ok(command)
}

/// Prints what a command gave (stdout) or why it failed (stderr), and its exit code.
pub fn print(outcome: Result<Value, Failure>) -> i32 {
    match outcome {
        Ok(value) => {
            println!("{value}");
            0
        }
        Err(failure) => {
            eprintln!("{}", failure.json());
            failure.code
        }
    }
}

fn run(config: &Config, command: &[String]) -> i32 {
    let Some(name) = command.first() else { return print(Err(Failure::usage(HELP))) };
    let started = Instant::now();
    loop {
        let opened = match name.as_str() {
            "register" => match command {
                [_, username] => account::register(config, username, false),
                [_, username, force] if force == "--force" => account::register(config, username, true),
                _ => return print(Err(Failure::usage("register NAME [--force]"))),
            },
            _ => account::open(config),
        };
        match opened {
            Ok(Open::Opened(opened)) if name == "listen" => return listen::listen(config, *opened, &command[1..]),
            Ok(Open::Opened(opened)) => {
                let client = std::sync::Mutex::new(opened.client);
                let context = commands::Context { client: &client, config, listening: false, emit: &|_| {} };
                return print(match name.as_str() {
                    "register" => commands::whoami(&context, false),
                    _ => commands::run(&context, command),
                });
            }
            Ok(Open::Busy) if !matches!(name.as_str(), "register" | "listen" | "sync") => {
                if let Some(code) = forward(config, command) {
                    return code;
                }
            }
            Ok(Open::Busy) => {}
            Err(failure) => return print(Err(failure)),
        }
        if started.elapsed() > BUSY_WAIT {
            return print(Err(Failure { retryable: true, ..Failure::new("the state directory is busy") }));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Runs the command in the listener that has the directory, if one answers: its exit code.
fn forward(config: &Config, command: &[String]) -> Option<i32> {
    let mut stream = UnixStream::connect(config.control()).ok()?;
    let request = json!(command).to_string() + "\n";
    stream.write_all(request.as_bytes()).ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let answer: Value = serde_json::from_str(&line).ok()?;
    let code = answer["code"].as_i64().unwrap_or(FAILED as i64) as i32;
    match code {
        0 => println!("{}", answer["out"]),
        _ => eprintln!("{}", answer["out"]),
    }
    Some(code)
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
}
