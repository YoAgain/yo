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
        .ok_or_else(|| Failure::usage("that is not a recovery key (64 letters and digits)"))?;
    let transport = account::transport(config)?;
    let door = yo_core::backup::Door::Code(&code);
    let opened = yo_core::Client::open_backup(transport.as_ref(), None, &door, &config.hub_keys)?
        .ok_or_else(|| Failure::new("no account for this recovery key"))?;
    let sealed: Value = serde_json::from_slice(&opened.plain).map_err(|_| Failure::new("the backup is damaged"))?;
    let username = sealed["account"]["username"].as_str().ok_or_else(|| Failure::new("the backup names no account"))?;
    let secret: [u8; 32] = sealed["account"]["accountKey"]
        .as_str()
        .and_then(|k| BASE64_STANDARD.decode(k).ok())
        .and_then(|k| k.try_into().ok())
        .ok_or_else(|| Failure::new("the backup holds no account key"))?;
    account::put_account_secret(config, &secret)?;
    account::write_file(config, "recovery", code.to_string().as_bytes())?;
    registered(config, username, None)
}

/// Registered as a device of the account whose secret the directory now has.
fn registered(config: &Config, username: &str, legacy: Option<[u8; 32]>) -> Result<Value, Failure> {
    match account::register_with(config, username, false, legacy)? {
        account::Open::Opened(opened) => {
            let client = std::sync::Mutex::new(opened.client);
            let context = commands::Context { client: &client, config, listening: false, emit: &|_| {} };
            commands::whoami(&context, false)
        }
        account::Open::Busy => Err(Failure { retryable: true, ..Failure::new("the state directory is busy") }),
    }
}

/// A link opened at the hub: its text to show, kept to wait on.
pub fn link_start(config: &Config) -> Result<yo_core::link::Request, Failure> {
    if account::has_account(config) {
        return Err(Failure::new("this directory has an account already: another --state DIR"));
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
        eprintln!("Scan this with Yo on a device that has the account (Settings > Devices > Link), or paste:");
        eprintln!("  {}", request.text);
        eprintln!("Waiting up to 10 minutes...");
    }
    let started = Instant::now();
    while started.elapsed() < LINK_WAIT {
        if let Some(done) = link_step(config, &request, 60)? {
            return Ok(done);
        }
    }
    Err(Failure { retryable: true, ..Failure::new("no device answered the link in 10 minutes") })
}

/// `yo link [TEXT]`: the new device's link read (from the argument or asked), and the owner asked once (spec 2.5:
/// never without that yes). The text for `link-approve`.
pub fn confirm_link(text: Option<&str>) -> Result<String, Failure> {
    let text = match text {
        Some(text) => text.to_owned(),
        None => {
            eprint!("Paste the yo-link: ");
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line).map_err(|e| Failure::new(e.to_string()))?;
            line.trim().to_owned()
        }
    };
    yo_core::link::offer(&text).map_err(|_| Failure::usage("not a yo-link: text"))?;
    eprint!("This device gets access to your account [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).map_err(|e| Failure::new(e.to_string()))?;
    match answer.trim() {
        "y" | "Y" | "yes" => Ok(text),
        _ => Err(Failure::new("not linked")),
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
