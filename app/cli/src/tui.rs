//! `yo tui` (and `yo --tui`, bare `yo` on a terminal): the friend list as Yo bars, in this binary. Events come from a
//! `yo listen` it starts (or one already running, then the list is refreshed now and then); every action is a command
//! run in that listener over the directory's control socket, the path bots take.

use crate::account::Config;
use crate::{human, signin, Failure, NOT_REGISTERED};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The app's amethyst, and its darker shade for the bar under the cursor's neighbours.
const AMETHYST: Color = Color::Rgb(0x9B, 0x59, 0xB6);
const FLASH: Duration = Duration::from_millis(1200);
/// With a listener this TUI did not start, how often the list is asked again.
const POLL: Duration = Duration::from_secs(10);

enum Msg {
    /// A line of the listener.
    Event(Value),
    ListenerEnded,
    Done(Job, Result<Value, Failure>),
}

#[derive(Clone, Debug)]
enum Job {
    Refresh,
    Requests,
    Whoami,
    Send(String),
    Add(String),
    Request(&'static str, String),
    SetRequests(bool),
    Devices,
    Rotate,
    LinkApprove(String),
    Signup,
    Recovery,
    Link,
}

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    Loading,
    Welcome,
    Signup,
    RecoveryInput,
    LinkWait,
    ShowKey,
    Main,
    Requests,
    Add,
    Settings,
    Devices,
    Recovery,
    LinkInput,
    LinkConfirm,
}

struct App {
    config: Arc<Config>,
    tx: Sender<Msg>,
    screen: Screen,
    color: bool,
    me: Value,
    items: Vec<Value>,
    requests: Vec<Value>,
    requests_open: bool,
    devices: Vec<Value>,
    selected: usize,
    unseen: HashMap<String, u32>,
    last: HashMap<String, Instant>,
    flash: HashMap<String, (Instant, String)>,
    input: String,
    human: bool,
    status: String,
    busy: bool,
    link_text: String,
    shown_key: String,
    confirm_rotate: bool,
    cancel_link: Arc<AtomicBool>,
    child: Option<Child>,
    external: bool,
    polled: Instant,
    quit: bool,
}

pub fn run(config: &Config, globals: &[String]) -> i32 {
    let (tx, rx) = channel();
    let mut app = App {
        config: Arc::new(config.clone()),
        tx,
        screen: Screen::Loading,
        color: crate::mode().color,
        me: Value::Null,
        items: Vec::new(),
        requests: Vec::new(),
        requests_open: true,
        devices: Vec::new(),
        selected: 0,
        unseen: HashMap::new(),
        last: HashMap::new(),
        flash: HashMap::new(),
        input: String::new(),
        human: false,
        status: String::new(),
        busy: false,
        link_text: String::new(),
        shown_key: String::new(),
        confirm_rotate: false,
        cancel_link: Arc::new(AtomicBool::new(false)),
        child: None,
        external: false,
        polled: Instant::now(),
        quit: false,
    };
    app.job(Job::Whoami);
    let mut terminal = ratatui::init();
    let _ = ratatui::crossterm::execute!(std::io::stdout(), event::EnableBracketedPaste);
    let code = app.main_loop(&mut terminal, &rx, globals);
    let _ = ratatui::crossterm::execute!(std::io::stdout(), event::DisableBracketedPaste);
    ratatui::restore();
    if let Some(mut child) = app.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    code
}

impl App {
    fn main_loop(&mut self, terminal: &mut DefaultTerminal, rx: &Receiver<Msg>, globals: &[String]) -> i32 {
        while !self.quit {
            if terminal.draw(|f| self.draw(f)).is_err() {
                return 1;
            }
            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => self.key(key),
                    Ok(Event::Paste(text)) => self.input.push_str(text.trim()),
                    _ => {}
                }
            }
            while let Ok(msg) = rx.try_recv() {
                self.message(msg, globals);
            }
            if self.external && self.screen == Screen::Main && self.polled.elapsed() > POLL {
                self.polled = Instant::now();
                self.job(Job::Refresh);
            }
        }
        0
    }

    /// Runs a job off the screen's thread; its answer comes back as a message.
    fn job(&mut self, job: Job) {
        let (config, tx) = (self.config.clone(), self.tx.clone());
        let command = |words: &[&str]| -> Vec<String> { words.iter().map(|w| w.to_string()).collect() };
        let input = self.input.trim().to_owned();
        let human = self.human;
        let cancel = self.cancel_link.clone();
        if matches!(job, Job::Signup | Job::Recovery | Job::Link | Job::Send(_) | Job::Add(_) | Job::Rotate) {
            self.busy = true;
        }
        std::thread::spawn(move || {
            let words: Option<Vec<String>> = match &job {
                Job::Refresh => Some(command(&["friends-view"])),
                Job::Requests => Some(command(&["requests", "list"])),
                Job::Whoami => Some(command(&["status"])),
                Job::Send(to) => Some(command(&["send", to])),
                Job::Add(name) => Some(command(&["add", name])),
                Job::Request(verb, name) => Some(command(&["requests", verb, name])),
                Job::SetRequests(on) => Some(command(&["requests", if *on { "on" } else { "off" }])),
                Job::Devices => Some(command(&["devices"])),
                Job::Rotate => Some(command(&["recovery", "rotate"])),
                Job::LinkApprove(text) => Some(command(&["link-approve", text])),
                _ => None,
            };
            let outcome = match (&job, words) {
                (_, Some(words)) => crate::execute(&config, &words),
                (Job::Signup, _) => crate::signup_run(&config, &input, human, false, true),
                (Job::Recovery, _) => signin::recovery(&config, &input),
                (_, None) => link(&config, &tx, &cancel),
            };
            let _ = tx.send(Msg::Done(job, outcome));
        });
    }

    /// Signed in: the listener (this TUI's own, unless one runs), then the list.
    fn signed_in(&mut self, globals: &[String]) {
        self.screen = Screen::Main;
        self.external = std::os::unix::net::UnixStream::connect(self.config.control()).is_ok();
        if !self.external && self.child.is_none() {
            let exe = std::env::current_exe().unwrap_or_else(|_| "yo".into());
            let spawned = Command::new(exe)
                .args(globals)
                .args(["--json", "listen"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            match spawned {
                Ok(mut child) => {
                    let out = child.stdout.take().expect("piped");
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        for line in BufReader::new(out).lines().map_while(Result::ok) {
                            if let Ok(value) = serde_json::from_str(&line) {
                                let _ = tx.send(Msg::Event(value));
                            }
                        }
                        let _ = tx.send(Msg::ListenerEnded);
                    });
                    self.child = Some(child);
                }
                Err(e) => self.status = format!("no listener: {e}"),
            }
        }
        self.job(Job::Refresh);
        self.job(Job::Requests);
    }

    fn message(&mut self, msg: Msg, globals: &[String]) {
        match msg {
            Msg::Event(event) => match event["type"].as_str() {
                Some("ready") => {
                    self.job(Job::Refresh);
                    self.job(Job::Whoami);
                }
                Some("message") => {
                    let target = event["reply_to"].as_str().unwrap_or("").to_owned();
                    *self.unseen.entry(target.clone()).or_default() += 1;
                    self.last.insert(target.clone(), Instant::now());
                    self.flash.insert(target.clone(), (Instant::now(), String::new()));
                    if !self.items.iter().any(|i| i["target"] == json!(target)) {
                        self.job(Job::Refresh);
                        self.job(Job::Requests);
                    }
                    self.sort();
                }
                Some("sent") => {}
                Some("link") => self.link_text = event["text"].as_str().unwrap_or("").to_owned(),
                Some(_) => {
                    self.job(Job::Refresh);
                    self.job(Job::Requests);
                }
                None => {}
            },
            Msg::ListenerEnded => {
                self.child = None;
                self.status = "the listener stopped (yo status)".into();
            }
            Msg::Done(job, outcome) => {
                self.busy = false;
                match (job, outcome) {
                    (Job::Whoami, Err(f)) if f.code == NOT_REGISTERED => self.screen = Screen::Welcome,
                    (Job::Whoami, Ok(me)) => {
                        let first = self.screen == Screen::Loading;
                        self.me = me;
                        if first {
                            self.signed_in(globals);
                        }
                    }
                    (Job::Refresh, Ok(items)) => {
                        self.items = items.as_array().cloned().unwrap_or_default();
                        self.sort();
                    }
                    (Job::Requests, Ok(r)) => {
                        self.requests = r["requests"].as_array().cloned().unwrap_or_default();
                        self.requests_open = r["requests_open"] != false;
                    }
                    (Job::Send(to), Ok(_)) => {
                        self.unseen.remove(&to);
                        self.last.insert(to.clone(), Instant::now());
                        self.flash.insert(to, (Instant::now(), "Yo sent ✓".into()));
                        self.sort();
                    }
                    (Job::Send(to), Err(f)) => {
                        self.flash.insert(to, (Instant::now(), format!("not sent: {}", f.error)));
                    }
                    (Job::Add(_), Ok(v)) => {
                        self.status = human::render("add", &v);
                        self.screen = Screen::Main;
                        self.job(Job::Refresh);
                    }
                    (Job::Request(..), Ok(_)) => {
                        self.job(Job::Requests);
                        self.job(Job::Refresh);
                    }
                    (Job::SetRequests(on), Ok(_)) => self.requests_open = on,
                    (Job::Devices, Ok(d)) => self.devices = d.as_array().cloned().unwrap_or_default(),
                    (Job::Rotate, Ok(v)) => {
                        self.shown_key = v["recovery_key"].as_str().unwrap_or("").to_owned();
                        self.confirm_rotate = false;
                        self.status = "New recovery key: the old one no longer works. Store this one.".into();
                    }
                    (Job::LinkApprove(_), Ok(_)) => {
                        self.status = "Linked: the new device has the account".into();
                        self.screen = Screen::Settings;
                    }
                    (Job::Signup | Job::Recovery | Job::Link, Ok(me)) => {
                        self.shown_key = me["recovery_key"].as_str().unwrap_or("").to_owned();
                        self.me = me;
                        self.input.clear();
                        if self.shown_key.is_empty() {
                            self.signed_in(globals);
                        } else {
                            self.screen = Screen::ShowKey;
                        }
                    }
                    (_, Err(f)) => self.status = format!("error: {}", f.error),
                }
            }
        }
    }

    /// Bars with a fresh Yo first, then as the list has them.
    fn sort(&mut self) {
        let target = self.items.get(self.selected).map(|i| i["target"].clone());
        let last = &self.last;
        let at = |i: &Value| last.get(i["target"].as_str().unwrap_or("")).copied();
        self.items.sort_by_key(|i| std::cmp::Reverse(at(i)));
        if let Some(t) = target {
            self.selected = self.items.iter().position(|i| i["target"] == t).unwrap_or(0);
        }
    }

    fn key(&mut self, key: KeyEvent) {
        let ctrl_c = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl_c {
            self.quit = true;
            return;
        }
        self.status.clear();
        let typing = matches!(self.screen, Screen::Signup | Screen::RecoveryInput | Screen::Add | Screen::LinkInput);
        if typing {
            match key.code {
                KeyCode::Esc => {
                    self.input.clear();
                    self.screen = match self.screen {
                        Screen::Add => Screen::Main,
                        Screen::LinkInput => Screen::Settings,
                        _ => Screen::Welcome,
                    };
                }
                KeyCode::Tab if self.screen == Screen::Signup => self.human = !self.human,
                KeyCode::Enter if !self.busy => self.submit(),
                KeyCode::Backspace => _ = self.input.pop(),
                KeyCode::Char(c) => self.input.push(c),
                _ => {}
            }
            return;
        }
        match (self.screen, key.code) {
            (Screen::Welcome, KeyCode::Char('1' | 's')) => self.screen = Screen::Signup,
            (Screen::Welcome, KeyCode::Char('2' | 'r')) => self.screen = Screen::RecoveryInput,
            (Screen::Welcome, KeyCode::Char('3' | 'l')) => {
                self.screen = Screen::LinkWait;
                self.link_text.clear();
                self.cancel_link.store(false, Ordering::SeqCst);
                self.job(Job::Link);
            }
            (Screen::LinkWait, KeyCode::Esc | KeyCode::Char('q')) => {
                self.cancel_link.store(true, Ordering::SeqCst);
                self.screen = Screen::Welcome;
            }
            (Screen::ShowKey, KeyCode::Enter) => {
                self.shown_key.clear();
                self.screen = Screen::Loading;
                self.job(Job::Whoami);
            }
            (Screen::Main, KeyCode::Char('q') | KeyCode::Esc) => self.quit = true,
            (Screen::Welcome | Screen::Loading, KeyCode::Char('q') | KeyCode::Esc) => self.quit = true,
            (Screen::Main, KeyCode::Down | KeyCode::Char('j')) => {
                self.selected = (self.selected + 1).min(self.items.len().saturating_sub(1))
            }
            (Screen::Main, KeyCode::Up | KeyCode::Char('k')) => self.selected = self.selected.saturating_sub(1),
            (Screen::Main, KeyCode::Enter | KeyCode::Char(' ')) => {
                if let Some(target) = self.items.get(self.selected).and_then(|i| i["target"].as_str()) {
                    let target = target.to_owned();
                    self.flash.insert(target.clone(), (Instant::now(), "Yo …".into()));
                    self.job(Job::Send(target));
                }
            }
            (Screen::Main, KeyCode::Char('+' | 'a')) => {
                self.input.clear();
                self.screen = Screen::Add;
            }
            (Screen::Main, KeyCode::Char('r')) => {
                self.selected = 0;
                self.screen = Screen::Requests;
                self.job(Job::Requests);
            }
            (Screen::Main, KeyCode::Char(',' | 's')) => {
                self.selected = 0;
                self.screen = Screen::Settings;
                self.job(Job::Whoami);
            }
            (Screen::Requests, KeyCode::Down | KeyCode::Char('j')) => {
                self.selected = (self.selected + 1).min(self.requests.len().saturating_sub(1))
            }
            (Screen::Requests, KeyCode::Up | KeyCode::Char('k')) => self.selected = self.selected.saturating_sub(1),
            (Screen::Requests, KeyCode::Char(c @ ('a' | 'b' | 'x'))) => {
                if let Some(user) = self.requests.get(self.selected).and_then(|r| r["user"].as_str()) {
                    let verb = match c {
                        'a' => "accept",
                        'b' => "block",
                        _ => "decline",
                    };
                    self.job(Job::Request(verb, user.to_owned()));
                }
            }
            (Screen::Settings, KeyCode::Down | KeyCode::Char('j')) => self.selected = (self.selected + 1).min(4),
            (Screen::Settings, KeyCode::Up | KeyCode::Char('k')) => self.selected = self.selected.saturating_sub(1),
            (Screen::Settings, KeyCode::Enter | KeyCode::Char(' ')) => match self.selected {
                0 => self.job(Job::SetRequests(!self.requests_open)),
                1 => {
                    self.status = if self.me["bot"] == true {
                        "Marked as a bot: people see /BOT. A bot cannot become a person.".into()
                    } else {
                        "A person (no /BOT). yo bot on marks it as a bot, for good.".into()
                    }
                }
                2 => {
                    self.screen = Screen::Devices;
                    self.job(Job::Devices);
                }
                3 => {
                    self.shown_key = crate::account::read_file(&self.config, "recovery")
                        .map(|k| String::from_utf8_lossy(&k).into_owned())
                        .unwrap_or_default();
                    self.confirm_rotate = false;
                    self.screen = Screen::Recovery;
                }
                _ => {
                    self.input.clear();
                    self.screen = Screen::LinkInput;
                }
            },
            (Screen::Recovery, KeyCode::Char('R')) => self.confirm_rotate = true,
            (Screen::Recovery, KeyCode::Char('y')) if self.confirm_rotate => self.job(Job::Rotate),
            (Screen::LinkConfirm, KeyCode::Char('y')) => {
                let text = std::mem::take(&mut self.input);
                self.job(Job::LinkApprove(text));
            }
            (Screen::LinkConfirm, _) => {
                self.input.clear();
                self.status = "not linked".into();
                self.screen = Screen::Settings;
            }
            (Screen::Requests | Screen::Settings, KeyCode::Char('q') | KeyCode::Esc) => {
                self.selected = 0;
                self.screen = Screen::Main;
            }
            (Screen::Devices | Screen::Recovery, KeyCode::Char('q') | KeyCode::Esc) => {
                self.shown_key.clear();
                self.screen = Screen::Settings;
            }
            _ => {}
        }
    }

    fn submit(&mut self) {
        match self.screen {
            Screen::Signup => {
                let name = self.input.trim();
                if name.chars().count() < 12 {
                    self.status = "a name of at least 12 characters (the CLI is not an attested app)".into();
                } else {
                    self.job(Job::Signup);
                }
            }
            Screen::RecoveryInput => self.job(Job::Recovery),
            Screen::Add => {
                let name = self.input.trim().to_owned();
                if !name.is_empty() {
                    self.job(Job::Add(name));
                }
            }
            Screen::LinkInput => match yo_core::link::offer(self.input.trim()) {
                Ok(_) => self.screen = Screen::LinkConfirm,
                Err(_) => self.status = "not a yo-link: text".into(),
            },
            _ => {}
        }
    }

    fn bar_style(&self, selected: bool) -> Style {
        match (self.color, selected) {
            (true, true) => Style::new().bg(Color::White).fg(Color::Black).add_modifier(Modifier::BOLD),
            (true, false) => Style::new().bg(AMETHYST).fg(Color::White).add_modifier(Modifier::BOLD),
            (false, true) => Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            (false, false) => Style::new().add_modifier(Modifier::BOLD),
        }
    }

    fn draw(&self, f: &mut Frame) {
        let area = f.area();
        let [header, body, footer] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(2)]).areas(area);
        let me = self.me["username"].as_str().unwrap_or("").to_uppercase();
        let bot = if self.me["bot"] == true { " /BOT" } else { "" };
        let reqs =
            if self.requests.is_empty() { String::new() } else { format!(" · {} requests (r)", self.requests.len()) };
        f.render_widget(
            Paragraph::new(format!(" Yo · {me}{bot}{reqs}")).style(Style::new().add_modifier(Modifier::BOLD)),
            header,
        );
        let keys = match self.screen {
            Screen::Main => "⏎/space Yo · j/k move · + add · r requests · , settings · q quit",
            Screen::Requests => "a accept · x decline · b block · esc back",
            Screen::Settings => "⏎ open/toggle · j/k move · esc back",
            Screen::Recovery => "R new key · esc back",
            Screen::Signup => "type a name · tab: person/bot · ⏎ sign up · esc back",
            Screen::Welcome => "1 sign up · 2 recovery key · 3 link · q quit",
            Screen::LinkWait => "esc cancel",
            Screen::ShowKey => "⏎ I stored it",
            _ => "⏎ ok · esc back",
        };
        let status = if self.busy { "working…" } else { self.status.as_str() };
        f.render_widget(
            Paragraph::new(vec![
                Line::from(format!(" {status}")),
                Line::from(format!(" {keys}")).style(Style::new().add_modifier(Modifier::DIM)),
            ]),
            footer,
        );
        match self.screen {
            Screen::Loading => f.render_widget(Paragraph::new(" opening…"), body),
            Screen::Main => self.draw_bars(f, body),
            Screen::Welcome => self.text(
                f,
                body,
                "Welcome to Yo",
                vec![
                    "No account in this state directory yet.".into(),
                    String::new(),
                    "  1  Sign up: a new account".into(),
                    "  2  Sign in with a recovery key".into(),
                    "  3  Sign in with a link from a device that has the account (QR code)".into(),
                    String::new(),
                    format!("State: {}", self.config.state.display()),
                    format!("Hub:   {}", self.config.hub.as_deref().unwrap_or("")),
                ],
            ),
            Screen::Signup => {
                let check = if self.human { "[x]" } else { "[ ]" };
                self.text(
                    f,
                    body,
                    "Sign up",
                    vec![
                        format!("Name: {}▏", self.input),
                        "      at least 12 characters".into(),
                        String::new(),
                        format!("{check} I'm a person, not a bot   (tab)"),
                        String::new(),
                        if self.human {
                            "People see you as a person.".into()
                        } else {
                            "Default: marked as a bot, people see /BOT.".into()
                        },
                        "The bot mark is for good: a bot can't become a person later.".into(),
                    ],
                )
            }
            Screen::RecoveryInput => self.text(
                f,
                body,
                "Sign in with a recovery key",
                vec![
                    format!("Recovery key: {}▏", "•".repeat(self.input.chars().count())),
                    String::new(),
                    "64 letters and digits; dashes and spaces are fine. Paste works.".into(),
                ],
            ),
            Screen::LinkWait => {
                let mut lines: Vec<String> = Vec::new();
                if self.link_text.is_empty() {
                    lines.push("opening a link…".into());
                } else {
                    lines.extend(signin::qr(&self.link_text).lines().map(str::to_owned));
                    lines.push(String::new());
                    lines.push("Scan with Yo on a device that has the account, or paste there:".into());
                    lines.push(self.link_text.clone());
                    lines.push("Waiting up to 10 minutes…".into());
                }
                self.text(f, body, "Sign in with a link", lines)
            }
            Screen::ShowKey => self.text(
                f,
                body,
                "Your recovery key",
                vec![
                    format!("Signed up as {me}{bot}."),
                    String::new(),
                    format!("  {}", grouped(&self.shown_key)),
                    String::new(),
                    "Store it now (a password manager). It brings the account back on a new device.".into(),
                    "It is kept in the state directory too: yo recovery show.".into(),
                ],
            ),
            Screen::Add => self.text(f, body, "Add a friend", vec![format!("Name: {}▏", self.input)]),
            Screen::Requests => {
                let mut lines = vec![
                    format!("Requests from strangers are {}", if self.requests_open { "on" } else { "off" }),
                    String::new(),
                ];
                if self.requests.is_empty() {
                    lines.push("No requests.".into());
                }
                for (i, r) in self.requests.iter().enumerate() {
                    let mark = if i == self.selected { "▌" } else { " " };
                    lines.push(format!("{mark} {}", human::bar_name(r)));
                }
                self.text(f, body, "Requests", lines)
            }
            Screen::Settings => {
                let rows = [
                    format!("Requests from strangers   [{}]", if self.requests_open { "on" } else { "off" }),
                    format!("Bot marker                [{}]", if self.me["bot"] == true { "on" } else { "off" }),
                    "Devices                   ›".into(),
                    "Recovery key              ›".into(),
                    "Link a new device         ›".into(),
                ];
                let mut lines: Vec<String> = rows
                    .iter()
                    .enumerate()
                    .map(|(i, r)| format!("{} {r}", if i == self.selected { "▌" } else { " " }))
                    .collect();
                lines.push(String::new());
                lines.push(human::render("status", &self.me));
                self.text(f, body, "Settings", lines)
            }
            Screen::Devices => {
                let text = human::render("devices", &json!(self.devices));
                let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
                lines.push(String::new());
                lines.push("Removing a device: yo devices remove N".into());
                self.text(f, body, "Devices", lines)
            }
            Screen::Recovery => {
                let mut lines = vec![if self.shown_key.is_empty() {
                    "No recovery key kept in this directory.".into()
                } else {
                    format!("  {}", grouped(&self.shown_key))
                }];
                lines.push(String::new());
                lines.push(if self.confirm_rotate {
                    "A new key replaces the account's backup: the old key and any passkey stop working. y: go on".into()
                } else {
                    "R: a new recovery key".into()
                });
                self.text(f, body, "Recovery key", lines)
            }
            Screen::LinkInput => self.text(
                f,
                body,
                "Link a new device",
                vec![
                    format!("yo-link: {}▏", self.input),
                    String::new(),
                    "Paste the text the new device shows (yo signin --link).".into(),
                ],
            ),
            Screen::LinkConfirm => {
                self.text(f, body, "Link a new device", vec!["This device gets access to your account [y/N]".into()])
            }
        }
    }

    fn text(&self, f: &mut Frame, area: Rect, title: &str, lines: Vec<String>) {
        let lines: Vec<Line> = lines.into_iter().map(Line::from).collect();
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(Block::new().borders(Borders::TOP).title(format!(" {title} "))),
            area,
        );
    }

    /// One bar a friend: "!" first, "Yo N" never cut, the name last (cut first), as the app's bars.
    fn draw_bars(&self, f: &mut Frame, area: Rect) {
        if self.items.is_empty() {
            f.render_widget(Paragraph::new(" No friends yet: + adds one (try PING)."), area);
            return;
        }
        const HEIGHT: u16 = 3;
        let per = (area.height / (HEIGHT + 1)).max(1) as usize;
        let first = self.selected.saturating_sub(per - 1);
        for (row, (i, item)) in self.items.iter().enumerate().skip(first).take(per).enumerate() {
            let rect = Rect {
                x: area.x + 1,
                y: area.y + row as u16 * (HEIGHT + 1),
                width: area.width.saturating_sub(2),
                height: HEIGHT,
            };
            let target = item["target"].as_str().unwrap_or("");
            let style = self.bar_style(i == self.selected);
            let unseen = self.unseen.get(target).copied().unwrap_or(0);
            let flash = self.flash.get(target).filter(|(at, _)| at.elapsed() < FLASH).map(|(_, t)| t.as_str());
            let mut prefix = String::from(" ");
            if unseen > 0 {
                prefix += &format!("! Yo {unseen}  ");
            }
            if let Some(text) = flash.filter(|t| !t.is_empty()) {
                prefix += &format!("{text}  ");
            }
            let mut name = human::bar_name(item);
            if item["gone"] == true {
                name += " (gone)";
            }
            let room = (rect.width as usize).saturating_sub(prefix.chars().count() + 1);
            if name.chars().count() > room {
                name = name.chars().take(room.saturating_sub(1)).collect::<String>() + "…";
            }
            let style = if flash == Some("") && self.color { style.bg(Color::White).fg(Color::Black) } else { style };
            let line = Line::from(vec![Span::raw(prefix), Span::raw(name)]);
            let blank = Line::from("");
            f.render_widget(Paragraph::new(vec![blank.clone(), line, blank]).style(style), rect);
        }
    }
}

/// The link flow off the screen: the text shown first (as an event), then waiting in steps until done or cancelled.
fn link(config: &Config, tx: &Sender<Msg>, cancel: &AtomicBool) -> Result<Value, Failure> {
    let request = signin::link_start(config)?;
    let _ = tx.send(Msg::Event(json!({ "type": "link", "text": request.text })));
    let started = Instant::now();
    while started.elapsed() < signin::LINK_WAIT && !cancel.load(Ordering::SeqCst) {
        if let Some(done) = signin::link_step(config, &request, 10)? {
            return Ok(done);
        }
    }
    Err(Failure::new("no device answered the link"))
}

fn grouped(key: &str) -> String {
    key.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).to_uppercase()).collect::<Vec<_>>().join("-")
}
