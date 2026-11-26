//! Signing a directory in to an account it does not hold yet: by the recovery key (spec 2.4) or by a link another
//! device approves (spec 2.5); and the other way, this account approving a new device's link.

use crate::account::{self, Config};
use crate::{commands, Failure};
use base64::prelude::{Engine, BASE64_STANDARD};
use serde_json::{json, Value};
use std::io::{BufRead, IsTerminal, Write};
use std::time::{Duration, Instant};

/// How long `signin --link` waits for the other device.
pub const LINK_WAIT: Duration = Duration::from_secs(600);

/// A secret line: on a terminal without echo, else stdin's first line.
pub fn read_secret(prompt: &str) -> Result<String, Failure> {
    let mut line = String::new();
    if std::io::stdin().is_terminal() {
        use ratatui::crossterm::event::{read, Event, KeyCode, KeyEventKind, KeyModifiers};
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        ratatui::crossterm::terminal::enable_raw_mode().map_err(|e| Failure::new(e.to_string()))?;
        let result = loop {
            match read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                    KeyCode::Enter => break Ok(()),
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break Err(()),
                    KeyCode::Esc => break Err(()),
                    KeyCode::Backspace => _ = line.pop(),
                    KeyCode::Char(c) => line.push(c),
                    _ => {}
                },
                Ok(Event::Paste(text)) => line.push_str(&text),
                Ok(_) => {}
                Err(_) => break Err(()),
            }
        };
        let _ = ratatui::crossterm::terminal::disable_raw_mode();
        eprintln!();
        result.map_err(|()| Failure::usage("cancelled"))?;
    } else {
        std::io::stdin().lock().read_line(&mut line).map_err(|e| Failure::new(e.to_string()))?;
    }
    Ok(line.trim().to_owned())
}

/// Signs this directory in with a recovery key: the backup it opens holds the account's key; registered with it,
/// this directory is a new device of that account.
pub fn recovery(config: &Config, typed: &str) -> Result<Value, Failure> {
    let compact: String = typed.to_lowercase().chars().filter(char::is_ascii_alphanumeric).collect();
    let code = yo_core::backup::parse_recovery_key(typed)
        .or_else(|| yo_core::backup::parse_recovery_key(&compact))
        .ok_or_else(|| Failure::usage("not a Recovery Key: 64 letters and digits"))?;
    let transport = account::transport(config)?;
    let door = yo_core::backup::Door::Code(&code);
    let opened = yo_core::Client::open_backup(transport.as_ref(), None, &door, &config.hub_keys)?
        .ok_or_else(|| Failure::new("no account for this Recovery Key"))?;
    let sealed: Value = serde_json::from_slice(&opened.plain).map_err(|_| Failure::new("the backup is damaged"))?;
    let username =
        sealed["account"]["username"].as_str().ok_or_else(|| Failure::new("the backup is damaged (no name)"))?;
    let secret: [u8; 32] = sealed["account"]["accountKey"]
        .as_str()
        .and_then(|k| BASE64_STANDARD.decode(k).ok())
        .and_then(|k| k.try_into().ok())
        .ok_or_else(|| Failure::new("the backup is damaged (no account key)"))?;
    account::put_account_secret(config, &secret)?;
    account::write_file(config, "recovery", code.to_string().as_bytes())?;
    registered(config, username, None)
}

/// Registered as a device of the account whose secret the directory now has.
fn registered(config: &Config, username: &str, legacy: Option<[u8; 32]>) -> Result<Value, Failure> {
    match account::register_with(config, username, false, legacy)? {
        account::Open::Opened(opened) => {
            let mut client = opened.client;
            // The bot mark is the account's, for good: a device signed in again takes it from the hub's lookup.
            let bot = client.whois_begin(username).and_then(|lookup| lookup.send()).is_ok_and(|(_, _, bot)| bot);
            if bot && !client.is_bot() {
                client.set_bot(true)?;
            }
            let client = std::sync::Mutex::new(client);
            let context = commands::Context { client: &client, config, listening: false, emit: &|_| {} };
            commands::whoami(&context, false)
        }
        account::Open::Busy => {
            Err(Failure { retryable: true, ..Failure::new("busy: another yo command has this account") })
        }
    }
}

/// A link opened at the hub: its text to show, kept to wait on.
pub fn link_start(config: &Config) -> Result<yo_core::link::Request, Failure> {
    if account::has_account(config) {
        return Err(Failure::new("an account is here already: use --profile NAME or --state DIR"));
    }
    Ok(yo_core::link::start(account::transport(config)?.as_ref())?)
}

/// Waits up to `wait_s` for the other device: `None` not yet; once it answered, signed in.
pub fn link_step(config: &Config, request: &yo_core::link::Request, wait_s: u64) -> Result<Option<Value>, Failure> {
    let transport = account::transport(config)?;
    let Some(linked) = yo_core::link::wait(transport.as_ref(), request, wait_s)? else { return Ok(None) };
    account::put_account_secret(config, &linked.account_secret)?;
    registered(config, &linked.username, linked.legacy_storage_key).map(Some)
}

/// `signin --link`: the text and its QR code (stderr on a terminal, else the text as a JSON line), then the wait.
pub fn link_cli(config: &Config) -> Result<Value, Failure> {
    let request = link_start(config)?;
    if crate::mode().json {
        println!("{}", json!({ "link": request.text }));
    } else {
        eprintln!("{}", qr(&request.text));
        eprintln!("Scan with Yo on your other device (Settings › Link a New Device), or paste:");
        eprintln!("  {}", request.text);
        eprintln!("Waiting, up to 10 minutes…");
    }
    let started = Instant::now();
    while started.elapsed() < LINK_WAIT {
        // Under the transport's 60 s timeout: the hub's 204 comes back before the request gives up.
        if let Some(done) = link_step(config, &request, 45)? {
            return Ok(done);
        }
    }
    Err(Failure { retryable: true, ..Failure::new("no answer from the other device in 10 minutes") })
}

pub fn compact_link(text: &str) -> String {
    text.split_whitespace().collect()
}

// A link contains a 16-byte id and a 32-byte public key in unpadded base64url.
// Leave the following line for the separate approval prompt.
fn read_link(reader: &mut impl BufRead) -> std::io::Result<String> {
    let mut text = String::new();
    let prefix = "yo-link:";
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        text.push_str(&compact_link(&line));
        if n == 0
            || text.len() >= prefix.len() + 22 + 1 + 43
            || !(prefix.starts_with(&text) || text.starts_with(prefix))
        {
            return Ok(text);
        }
    }
}

/// `yo link [TEXT]`: the new device's link read (from the argument or asked), and the owner asked once (spec 2.5:
/// never without that yes). The text for `link-approve`.
pub fn confirm_link(text: Option<&str>) -> Result<String, Failure> {
    let text = match text {
        Some(text) => compact_link(text),
        None => {
            if std::io::stdin().is_terminal() {
                eprint!("Paste the link: ");
                let _ = std::io::stderr().flush();
            }
            read_link(&mut std::io::stdin().lock()).map_err(|e| Failure::new(e.to_string()))?
        }
    };
    yo_core::link::offer(&text).map_err(|_| Failure::usage("not a Yo link"))?;
    if std::io::stdin().is_terminal() {
        eprint!("Give the new device access to your account? [y/N] ");
        let _ = std::io::stderr().flush();
    }
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).map_err(|e| Failure::new(e.to_string()))?;
    match answer.trim() {
        "y" | "Y" | "yes" => Ok(text),
        _ => Err(Failure::new("not linked")),
    }
}

/// How long the old device waits, after the approval, for the new device to take the bundle and register.
const LINK_TAKEN_WAIT: Duration = Duration::from_secs(75);

/// Approves a link (`link-approve`), then waits until the new device shows up in the account's devices: the hub
/// keeps a link 10 minutes, so a device that stopped waiting never takes it, and that is said, not "linked".
pub fn approve_link(config: &Config, text: &str) -> Result<Value, Failure> {
    let devices = || -> Result<Vec<Value>, Failure> {
        Ok(crate::execute(config, &["devices".into()])?.as_array().cloned().unwrap_or_default())
    };
    let before: Vec<Value> = devices()?.iter().map(|d| d["number"].clone()).collect();
    crate::execute(config, &["link-approve".into(), text.to_owned()])?;
    let started = Instant::now();
    let mut found = None;
    while started.elapsed() < LINK_TAKEN_WAIT {
        std::thread::sleep(Duration::from_secs(2));
        let Ok(now) = devices() else { continue };
        if let Some(new) = now.into_iter().find(|d| !before.contains(&d["number"])) {
            // Its app is known with its first signed request; a little later at most.
            let named = new["app"].as_str().is_some_and(|a| !a.is_empty());
            found = Some(new);
            if named || started.elapsed() > Duration::from_secs(20) {
                break;
            }
        }
    }
    match found {
        Some(device) => Ok(json!({ "linked": true, "device": device })),
        None => Err(Failure::new(
            "not linked: the new device did not take the link (it stopped waiting); start again there",
        )),
    }
}

/// A QR code in half blocks, dark on light with its quiet zone.
pub fn qr(text: &str) -> String {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else { return String::new() };
    let width = code.width();
    let colors = code.to_colors();
    let dark = |x: isize, y: isize| {
        x >= 0
            && y >= 0
            && (x as usize) < width
            && (y as usize) < width
            && colors[y as usize * width + x as usize] == qrcode::Color::Dark
    };
    let (q, w) = (2isize, width as isize);
    let mut out = String::new();
    let mut y = -q;
    while y < w + q {
        for x in -q..w + q {
            out.push(match (dark(x, y), dark(x, y + 1)) {
                (false, false) => '█',
                (true, false) => '▄',
                (false, true) => '▀',
                (true, true) => ' ',
            });
        }
        out.push('\n');
        y += 2;
    }
    out.pop();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_stdin_link_leaves_approval_on_its_own_line() {
        let token = format!("yo-link:{}.{}", "A".repeat(22), "A".repeat(43));
        let input = format!("  {}\n  {}\t{} \nyes\n", &token[..4], &token[4..31], &token[31..]);
        let mut reader = input.as_bytes();
        let text = read_link(&mut reader).unwrap();
        assert_eq!(text, token);
        assert!(yo_core::link::offer(&text).is_ok());
        assert_eq!(reader, b"yes\n");
    }
}
