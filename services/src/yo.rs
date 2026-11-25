//! The account: `yo listen` as a child (the keys and the decryption stay there), commands over its control socket.

use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(180);
const MAX_ANSWER: u64 = 16 * 1024 * 1024;

/// Why a command did not work: a class, never an id.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub code: i32,
    pub retryable: bool,
}

pub struct Yo {
    socket: PathBuf,
}

impl Yo {
    pub fn new(state: &Path) -> Yo {
        Yo { socket: state.join("control.sock") }
    }

    pub fn send(&self, to: &str) -> Result<(), Failure> {
        self.run(&["send", to]).map(drop)
    }

    pub fn send_picture(&self, to: &str, path: &Path) -> Result<(), Failure> {
        let path = path.to_str().ok_or(Failure { code: 1, retryable: false })?;
        self.run(&["send", to, "--photo", path]).map(drop)
    }

    pub fn send_media(&self, to: &str, reference: &str) -> Result<(), Failure> {
        self.run(&["send", to, "--media", reference]).map(drop)
    }

    pub fn echo_probe(&self, to: &str, token: &str) -> Result<(), Failure> {
        self.run(&["echo-probe", to, token]).map(drop)
    }

    /// Answers a connection event by its code: "connected", "refused" or "ended".
    pub fn answer(&self, to: &str, kind: &str, code: &str) -> Result<(), Failure> {
        self.run(&["service-answer", to, kind, code]).map(drop)
    }

    /// Its own user id and username, and whether it declared itself a bot.
    pub fn whoami(&self) -> Result<(String, String, bool), Failure> {
        let out = self.run(&["whoami"])?;
        let text = |key: &str| out[key].as_str().unwrap_or_default().to_owned();
        Ok((text("user"), text("username"), out["bot"] == true))
    }

    /// The user id of a name; None when nobody has it, or it is the account's own.
    pub fn whois(&self, name: &str) -> Result<Option<String>, Failure> {
        match self.run(&["whois", name]) {
            Ok(out) => Ok(out["user"].as_str().map(str::to_owned)),
            Err(failure) if !failure.retryable => Ok(None),
            Err(failure) => Err(failure),
        }
    }

    pub fn contacts(&self) -> Result<u64, Failure> {
        Ok(self.run(&["status"])?["contacts"].as_u64().unwrap_or(0))
    }

    /// Leaves every group it is alone in: nobody to answer there.
    pub fn leave_empty_groups(&self, me: &str) -> Result<(), Failure> {
        for group in self.run(&["groups"])?.as_array().into_iter().flatten() {
            let members: Vec<&str> =
                group["members"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            if group["left"] == false && !members.is_empty() && members.iter().all(|m| *m == me) {
                if let Some(id) = group["id"].as_str() {
                    self.run(&["groups", "leave", id])?;
                }
            }
        }
        Ok(())
    }

    fn run(&self, command: &[&str]) -> Result<Value, Failure> {
        let lost = |_| Failure { code: 1, retryable: true };
        let answer = self.exchange(command).map_err(lost)?;
        let code = answer["code"].as_i64().unwrap_or(1) as i32;
        if code == 0 {
            return Ok(answer["out"].clone());
        }
        Err(Failure { code, retryable: answer["out"]["retryable"] == true })
    }

    fn exchange(&self, command: &[&str]) -> io::Result<Value> {
        let mut stream = UnixStream::connect(&self.socket)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        stream.write_all(format!("{}\n", json!(command)).as_bytes())?;
        let mut line = String::new();
        BufReader::new(stream.take(MAX_ANSWER)).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    }
}

/// `yo listen`, ended with the executor.
pub struct Listener(Child);

impl Listener {
    pub fn start(yo: &Path, state: &Path, drain_media: bool) -> io::Result<Listener> {
        let mut command = Command::new(yo);
        command.arg("listen").env("YO_STATE", state).stdin(Stdio::null()).stdout(Stdio::piped());
        if drain_media {
            command.arg("--drain-media");
        }
        command.spawn().map(Listener)
    }

    /// Its events, one by one, until `each` fails or it ends: the exit code.
    pub fn events(mut self, mut each: impl FnMut(Value) -> Result<(), i32>) -> i32 {
        if let Some(out) = self.0.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Ok(event) = serde_json::from_str(&line) {
                    if let Err(code) = each(event) {
                        return code;
                    }
                }
            }
        }
        self.0.wait().ok().and_then(|status| status.code()).unwrap_or(1)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
