//! `yo tui` (and `yo --tui`, bare `yo` on a terminal): the app in a small window. The friend list as the app's
//! coloured Yo bars, framed like hey's TUI: a titled rule, a footer rule with the key legend, `?` for help.
//! Events come from a `yo listen` it starts (or one already running, then the list is refreshed now and then);
//! every action is a command run in that listener over the directory's control socket, the path bots take.
//!
//! Colours: the frame takes the terminal's ANSI palette (so the theme decides; magenta is where you are, cyan a
//! key, bright black what is secondary); only the bars carry the app's own colours, in truecolor when the terminal
//! says so (COLORTERM), else the nearest of the 256. NO_COLOR (or --no-color): bold and reverse only.

use crate::account::Config;
use crate::{signin, Failure, NOT_REGISTERED};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};
use ratatui::{DefaultTerminal, Frame};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use yo_ui::line::bar_title;
use yo_ui::list::{Friend, Input, Kind, List, Row, Snapshot, YO_FADE_MS};
use yo_ui::settings::{self, Item, Platform};
use yo_ui::{Language::English, ALIZARIN, BAR_COLORS};

/// With a listener this TUI did not start, how often the list is asked again.
const POLL: Duration = Duration::from_secs(10);

const ACCENT: Color = Color::Magenta;
const KEY: Color = Color::Cyan;
const SECONDARY: Color = Color::DarkGray;

enum Msg {
    /// A line of the listener.
    Event(u32, Value),
    LinkOffer(String),
    ListenerEnded(u32, String),
    Done(Job, Result<Value, Failure>),
}

#[derive(Clone, Debug)]
enum Job {
    Refresh,
    LearnBots(Vec<String>),
    Whoami,
    Send(String, Option<String>),
    Change(Vec<String>),
    Connections,
    Details {
        target: String,
        name: String,
    },
    Diagnostics,
    Add(String),
    Request(&'static str, String),
    SetRequests(bool),
    Devices,
    Blocked,
    Unblock(String),
    RemoveDevice(u64),
    Rotate,
    LinkApprove(String),
    /// A group's members, for the add or remove picker.
    Members {
        title: &'static str,
        words: Vec<String>,
        group: Option<String>,
        remove: bool,
    },
    Probe,
    Signup,
    Recovery,
    Link,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum Screen {
    Loading,
    Welcome,
    Signup,
    RecoveryInput,
    LinkWait,
    ShowKey,
    Main,
    Add,
    Settings,
    Blocked,
    Info(&'static str),
    Signout,
    Recovery,
    LinkInput,
    LinkConfirm,
}

const ADD_MENU: &[&str] = &["Add Friend", "New Group"];

fn bar_menu(group: bool, blocked: bool) -> &'static [&'static str] {
    if group {
        &["Rename", "Add Members", "Remove Members", "Leave Group"]
    } else if blocked {
        &["Remove Friend", "Unblock"]
    } else {
        &["Remove Friend", "Block"]
    }
}

enum Dialog {
    Menu {
        items: &'static [&'static str],
        target: String,
        cursor: usize,
    },
    Prompt {
        title: &'static str,
        words: Vec<String>,
    },
    Members {
        title: &'static str,
        words: Vec<String>,
        friends: Vec<(String, String, bool)>,
        cursor: usize,
        /// What it says with nobody to pick.
        empty: &'static str,
    },
    Confirm {
        title: &'static str,
        text: String,
        words: Vec<String>,
    },
    Report {
        title: &'static str,
        target: String,
        lines: Vec<String>,
        scroll: usize,
    },
}

fn words(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

/// What a mouse click hits, as the last frame drew it.
#[derive(Clone, Copy)]
enum Hit {
    Blocked(usize),
    Bar(usize),
    AddBar,
    Choice(usize, &'static str),
    Setting(usize),
    Toggle,
}

const FEEDBACK: &str = "https://github.com/YoAgain/yo/issues";

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsRow {
    Item(&'static Item),
    /// One of the account's devices, by its place in the list.
    Device(usize),
    Connection(usize),
    Connect,
    Decline,
}

/// The settings as boxes, from the core's menu: each section's title and rows (each device a row under Devices).
fn settings_boxes(devices: usize, connections: usize) -> Vec<(&'static str, Vec<SettingsRow>)> {
    let mut boxes: Vec<_> = settings::sections(Platform::Cli)
        .into_iter()
        .map(|(section, items)| {
            let mut rows = Vec::new();
            for item in items {
                match item.id {
                    "devices" => rows.extend((0..devices).map(SettingsRow::Device)),
                    _ => rows.push(SettingsRow::Item(item)),
                }
            }
            (section.title, rows)
        })
        .collect();
    let mut services: Vec<_> = (0..connections).map(SettingsRow::Connection).collect();
    services.extend([SettingsRow::Connect, SettingsRow::Decline]);
    let after_friends = boxes.iter().position(|(title, _)| *title == "Friends").unwrap() + 1;
    boxes.insert(after_friends, ("Services", services));
    boxes
}

fn selectable_settings(devices: usize, connections: usize) -> impl Iterator<Item = SettingsRow> {
    settings_boxes(devices, connections).into_iter().flat_map(|(_, rows)| rows)
}

/// A device as people say it: the app it runs ("ios/…" an iPhone, "yogpt/…" this CLI on a computer).
fn device_label(app: &str) -> &'static str {
    match app.split('/').next().unwrap_or("") {
        "ios" => "iPhone",
        "android" => "Android",
        "yogpt" => "Computer (CLI)",
        "watch" | "watchos" => "Apple Watch",
        _ => "Device",
    }
}

/// When a device was last active, by local day as the app says it: "active today", "yesterday", else the date.
fn active(last_seen_ms: i64, now_ms: i64) -> String {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let offset = *OFFSET.get_or_init(|| {
        let out = Command::new("date").arg("+%z").output().ok();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
        let sign = if text.starts_with('-') { -1 } else { 1 };
        let digits: i64 = text.trim_start_matches(['+', '-']).parse().unwrap_or(0);
        sign * ((digits / 100) * 3600 + (digits % 100) * 60)
    });
    let day = |ms: i64| (ms / 1000 + offset).div_euclid(86_400);
    match day(now_ms) - day(last_seen_ms) {
        i64::MIN..=0 => "active today".into(),
        1 => "active yesterday".into(),
        _ => {
            // Days since 1970 to a civil date (Howard Hinnant's algorithm).
            let z = day(last_seen_ms) + 719_468;
            let era = z.div_euclid(146_097);
            let doe = z - era * 146_097;
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let d = doy - (153 * mp + 2) / 5 + 1;
            let m = if mp < 10 { mp + 3 } else { mp - 9 };
            let y = yoe + era * 400 + i64::from(m <= 2);
            const MONTHS: [&str; 12] =
                ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            format!("active {} {d}, {y}", MONTHS[(m - 1) as usize])
        }
    }
}

struct App {
    config: Arc<Config>,
    tx: Sender<Msg>,
    screen: Screen,
    color: bool,
    truecolor: bool,
    me: Value,
    items: HashMap<String, Value>,
    list: List,
    snapshot: Snapshot,
    clock: Instant,
    snapshot_at: u64,
    visible: Vec<String>,
    pending: Vec<Value>,
    token: u64,
    learned: bool,
    /// A friend just added: the Yo the add sends, once the list has the bar.
    greet: Option<String>,
    refreshing: bool,
    refresh_again: bool,
    blocked: Vec<Value>,
    jobs: usize,
    signing_out: bool,
    /// Your Code's QR shown under its row (enter shows and hides it).
    show_code: bool,
    /// The recovery key shown in full (`v`); hidden as dots otherwise.
    reveal: bool,
    /// Mouse capture on (off while a key is shown, so the terminal can select it).
    mouse: bool,
    /// A device asked to be removed: `y` removes it, any other key not.
    confirm_device: Option<u64>,
    requests: Vec<Value>,
    requests_open: bool,
    devices: Vec<Value>,
    connections: Vec<Value>,
    dialog: Option<Dialog>,
    selected: usize,
    input: String,
    human: bool,
    status: String,
    busy: bool,
    link_text: String,
    shown_key: String,
    confirm_rotate: bool,
    help: bool,
    /// How far the help is scrolled (a small window shows it in parts).
    help_scroll: usize,
    /// How far it scrolls at most, as the last frame had it.
    help_max: std::cell::Cell<usize>,
    /// The settings row a sub-screen was opened from: back on it after.
    settings_at: usize,
    /// Bars whose "Sent" is a photo's or a video's, not a Yo's.
    photos: HashMap<String, Kind>,
    /// The Status card's PING line, once the probe answered.
    ping: Option<Vec<String>>,
    clipboard: Option<Vec<&'static str>>,
    cancel_link: Arc<AtomicBool>,
    child: Option<Child>,
    external: bool,
    retry_at: Option<Instant>,
    retry_secs: u64,
    listener_status: String,
    polled: Instant,
    hits: RefCell<Vec<(Rect, Hit)>>,
    quit: bool,
}

pub fn run(config: &Config, globals: &[String]) -> i32 {
    let (tx, rx) = channel();
    let mut app = App::new(config, tx);
    app.job(Job::Whoami);
    let mut terminal = ratatui::init();
    let _ = ratatui::crossterm::execute!(std::io::stdout(), event::EnableBracketedPaste, event::EnableMouseCapture);
    let code = app.main_loop(&mut terminal, &rx, globals);
    let _ = ratatui::crossterm::execute!(std::io::stdout(), event::DisableMouseCapture, event::DisableBracketedPaste);
    ratatui::restore();
    code
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl App {
    fn new(config: &Config, tx: Sender<Msg>) -> Self {
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        let mut list = List::new();
        let snapshot = list.apply(0, Input::Foreground(true));
        App {
            config: Arc::new(config.clone()),
            tx,
            screen: Screen::Loading,
            color: crate::mode().color,
            truecolor: matches!(colorterm.as_str(), "truecolor" | "24bit"),
            me: Value::Null,
            items: HashMap::new(),
            list,
            snapshot,
            clock: Instant::now(),
            snapshot_at: 0,
            visible: vec![],
            pending: vec![],
            token: 0,
            learned: false,
            greet: None,
            refreshing: false,
            refresh_again: false,
            blocked: Vec::new(),
            jobs: 0,
            signing_out: false,
            show_code: false,
            reveal: false,
            mouse: true,
            confirm_device: None,
            requests: Vec::new(),
            requests_open: true,
            devices: Vec::new(),
            connections: Vec::new(),
            dialog: None,
            selected: 0,
            input: String::new(),
            human: false,
            status: String::new(),
            busy: false,
            link_text: String::new(),
            shown_key: String::new(),
            confirm_rotate: false,
            help: false,
            help_scroll: 0,
            help_max: std::cell::Cell::new(0),
            settings_at: 0,
            photos: HashMap::new(),
            ping: None,
            clipboard: clipboard(),
            cancel_link: Arc::new(AtomicBool::new(false)),
            child: None,
            external: false,
            retry_at: None,
            retry_secs: 3,
            listener_status: String::new(),
            polled: Instant::now(),
            hits: RefCell::new(Vec::new()),
            quit: false,
        }
    }

    fn main_loop(&mut self, terminal: &mut DefaultTerminal, rx: &Receiver<Msg>, globals: &[String]) -> i32 {
        while !self.quit {
            if self.signing_out && self.jobs == 0 {
                self.finish_signout(globals);
            }
            if !self.signing_out && self.retry_at.is_some_and(|at| Instant::now() >= at) {
                self.start_listener(globals);
            }
            // A key on screen can be selected with the mouse: the terminal's own selection, without capture.
            let mouse = !(self.screen == Screen::ShowKey || (self.screen == Screen::Recovery && self.reveal));
            if mouse != self.mouse {
                self.mouse = mouse;
                let _ = if mouse {
                    ratatui::crossterm::execute!(std::io::stdout(), event::EnableMouseCapture)
                } else {
                    ratatui::crossterm::execute!(std::io::stdout(), event::DisableMouseCapture)
                };
            }
            if terminal.draw(|f| self.draw(f)).is_err() {
                return 1;
            }
            let visible: Vec<_> = self
                .hits
                .borrow()
                .iter()
                .filter_map(|(_, hit)| match hit {
                    Hit::Bar(i) if !self.help => Some(self.snapshot.rows[*i].id.clone()),
                    _ => None,
                })
                .collect();
            if visible != self.visible {
                self.visible.clone_from(&visible);
                self.apply(Input::RowsVisible(visible));
                continue;
            }
            if self.snapshot.next_tick.is_some_and(|at| self.now() >= at) {
                self.apply(Input::Tick);
                continue;
            }
            let wait_ms = self.snapshot.next_tick.map_or(100, |at| at.saturating_sub(self.now()).min(100));
            if event::poll(Duration::from_millis(wait_ms)).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => self.key(key),
                    Ok(Event::Mouse(mouse)) => self.mouse(mouse),
                    Ok(Event::Paste(text)) => self.input.push_str(text.trim()),
                    _ => {}
                }
            }
            while let Ok(msg) = rx.try_recv() {
                self.message(msg, globals);
            }
            // Changes the listener has no event for (a friend added from another shell): the list asked again.
            if self.screen == Screen::Main && self.polled.elapsed() > POLL {
                self.polled = Instant::now();
                if self.external && std::os::unix::net::UnixStream::connect(self.config.control()).is_err() {
                    self.start_listener(globals);
                } else {
                    self.job(Job::Refresh);
                }
            }
        }
        0
    }

    /// Runs a job off the screen's thread; its answer comes back as a message.
    fn job(&mut self, job: Job) {
        if matches!(job, Job::Refresh) {
            if self.refreshing {
                self.refresh_again = true;
                return;
            }
            self.refreshing = true;
        }
        let (config, tx) = (self.config.clone(), self.tx.clone());
        let command = words;
        let input = self.input.trim().to_owned();
        let human = self.human;
        let cancel = self.cancel_link.clone();
        if matches!(
            job,
            Job::Signup | Job::Recovery | Job::Link | Job::Send(..) | Job::Add(_) | Job::Rotate | Job::LinkApprove(_)
        ) {
            self.busy = true;
        }
        self.jobs += 1;
        std::thread::spawn(move || {
            let words: Option<Vec<String>> = match &job {
                Job::Whoami => Some(command(&["status"])),
                Job::Send(to, None) => Some(command(&["send", to])),
                Job::Send(to, Some(path)) => Some(command(&["send", to, "--photo", path])),
                Job::Connections => Some(command(&["connections"])),
                Job::Change(words) => Some(words.clone()),
                Job::Add(name) => Some(command(&["add", name])),
                Job::Request(verb, name) => Some(command(&["requests", verb, name])),
                Job::SetRequests(on) => Some(command(&["requests", if *on { "on" } else { "off" }])),
                Job::Unblock(id) => Some(command(&["unblock", id])),
                Job::RemoveDevice(n) => Some(command(&["devices", "remove", &n.to_string()])),
                Job::Devices => Some(command(&["devices"])),
                Job::Rotate => Some(command(&["recovery", "rotate"])),
                _ => None,
            };
            let outcome = match (&job, words) {
                (_, Some(words)) => crate::execute(&config, &words),
                (Job::Details { target, name }, _) => (|| {
                    if let Some(id) = target.strip_prefix(yo_core::shared::GROUP_PREFIX) {
                        let groups = crate::execute(&config, &command(&["groups", "list"]))?;
                        let group = groups
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|g| g["id"] == id)
                            .ok_or_else(|| Failure::new("Group not found"))?;
                        Ok(json!([
                            format!("Name      {}", upper(&group["name"])),
                            format!("Members   {}", group["members"].as_array().map_or(0, Vec::len))
                        ]))
                    } else {
                        let user = crate::execute(&config, &command(&["whois", name]))?;
                        Ok(json!([
                            format!("User ID   {}", user["user"].as_str().unwrap_or("—")),
                            format!("Username  {}", user["username"].as_str().map_or("—".into(), str::to_uppercase)),
                            format!("Bot       {}", if user["bot"] == true { "yes" } else { "no" })
                        ]))
                    }
                })(),
                (Job::Diagnostics, _) => Ok(diagnostics(&config)),
                (Job::Refresh, _) => (|| {
                    let friends = crate::execute(&config, &command(&["friends-view"]))?;
                    let mut view = crate::execute(&config, &command(&["requests", "list"]))?;
                    view["friends"] = friends;
                    Ok(view)
                })(),
                (Job::Blocked, _) => (|| {
                    let mut blocked = std::collections::BTreeMap::new();
                    for cmd in ["contacts", "friends"] {
                        let rows = crate::execute(&config, &command(&[cmd]))?;
                        for row in rows.as_array().into_iter().flatten().filter(|r| r["blocked"] == true) {
                            if let Some(id) = row["user"].as_str() {
                                blocked.insert(id.to_owned(), row.clone());
                            }
                        }
                    }
                    Ok(json!(blocked.into_values().collect::<Vec<_>>()))
                })(),
                (Job::LearnBots(names), _) => {
                    for name in names {
                        let _ = crate::execute(&config, &["whois".into(), name.clone()]);
                    }
                    Ok(Value::Null)
                }
                (Job::LinkApprove(text), _) => signin::approve_link(&config, text),
                (Job::Members { group, .. }, _) => (|| {
                    let members = match group {
                        Some(id) => crate::execute(&config, &command(&["groups", "list"]))?
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|g| g["id"] == id.as_str())
                            .map_or(json!([]), |g| g["members"].clone()),
                        None => json!([]),
                    };
                    let contacts = crate::execute(&config, &command(&["contacts"]))?;
                    Ok(json!({ "members": members, "contacts": contacts }))
                })(),
                (Job::Probe, _) => Ok(probe_and_checks(&config)),
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
        self.start_listener(globals);
    }

    fn start_listener(&mut self, globals: &[String]) {
        self.retry_at = None;
        self.polled = Instant::now();
        self.external = std::os::unix::net::UnixStream::connect(self.config.control()).is_ok();
        if self.external {
            self.listener_status.clear();
            self.job(Job::Refresh);
        } else if self.child.is_none() {
            // A fresh state directory: made (private) before the listener needs it.
            let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&self.config.state);
            // This program, or (replaced on disk while it runs: "… (deleted)") the one by the name it was started as.
            let exe = std::env::current_exe()
                .ok()
                .filter(|p| p.exists())
                .or_else(|| std::env::args_os().next().map(Into::into))
                .unwrap_or_else(|| "yo".into());
            let spawned = Command::new(exe)
                .args(globals)
                .args(["--json", "listen"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            match spawned {
                Ok(mut child) => {
                    let pid = child.id();
                    let out = child.stdout.take().expect("piped");
                    let err = child.stderr.take().expect("piped");
                    let last = std::thread::spawn(move || {
                        BufReader::new(err).lines().map_while(Result::ok).last().unwrap_or_default()
                    });
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        for line in BufReader::new(out).lines().map_while(Result::ok) {
                            if let Ok(value) = serde_json::from_str(&line) {
                                let _ = tx.send(Msg::Event(pid, value));
                            }
                        }
                        let _ = tx.send(Msg::ListenerEnded(pid, last.join().unwrap_or_default()));
                    });
                    self.child = Some(child);
                }
                Err(e) => self.retry_listener(e.to_string()),
            }
        }
    }

    fn retry_listener(&mut self, reason: String) {
        self.listener_status = format!("Stopped receiving: {reason}");
        self.retry_at = Some(Instant::now() + Duration::from_secs(self.retry_secs));
        self.retry_secs = (self.retry_secs * 2).min(60);
    }

    fn message(&mut self, msg: Msg, globals: &[String]) {
        if matches!(msg, Msg::Done(..)) {
            self.jobs -= 1;
        }
        if self.signing_out {
            return;
        }
        match msg {
            Msg::LinkOffer(text) => self.link_text = text,
            Msg::Event(pid, _) if self.child.as_ref().is_none_or(|child| child.id() != pid) => {}
            Msg::Event(_, event) => self.event(event),
            Msg::ListenerEnded(pid, last) => {
                if self.child.as_ref().is_none_or(|child| child.id() != pid) {
                    return;
                }
                let Some(mut child) = self.child.take() else { return };
                if self.polled.elapsed() >= Duration::from_secs(60) {
                    self.retry_secs = 3;
                }
                match child.wait() {
                    Ok(exit) if exit.code() == Some(crate::GONE) => {
                        self.listener_status = "Stopped: this device was removed".into();
                    }
                    Ok(exit) => {
                        let reason = serde_json::from_str::<Value>(&last)
                            .ok()
                            .and_then(|v| v["error"].as_str().map(str::to_owned))
                            .unwrap_or(last);
                        self.retry_listener(if reason.is_empty() {
                            exit.to_string()
                        } else {
                            format!("{reason} ({exit})")
                        });
                    }
                    Err(e) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        self.retry_listener(e.to_string());
                    }
                }
            }
            Msg::Done(job, outcome) => {
                if matches!(job, Job::Refresh) {
                    self.refreshing = false;
                    if std::mem::take(&mut self.refresh_again) {
                        self.job(Job::Refresh);
                    }
                }
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
                    (Job::Refresh, Ok(mut view)) => {
                        self.requests = view["requests"].as_array_mut().map(std::mem::take).unwrap_or_default();
                        self.requests_open = view["requests_open"] != false;
                        self.refresh(view["friends"].take());
                    }
                    (Job::LearnBots(_), Ok(_)) => self.job(Job::Refresh),
                    (Job::Blocked, Ok(v)) => {
                        self.blocked = v.as_array().cloned().unwrap_or_default();
                        if self.screen == Screen::Blocked {
                            self.selected = self.selected.min(self.blocked.len().saturating_sub(1));
                        }
                    }
                    (Job::RemoveDevice(n), Ok(_)) => {
                        self.status = format!("Removed device {n}");
                        self.job(Job::Devices);
                    }
                    (Job::Unblock(_), Ok(_)) => {
                        self.job(Job::Blocked);
                        self.job(Job::Refresh);
                    }
                    (Job::Change(args), Ok(_)) => {
                        let n = args.len().saturating_sub(3);
                        let people = |n: usize| if n == 1 { "1 member".to_owned() } else { format!("{n} members") };
                        self.status = match (args[0].as_str(), args.get(1).map(String::as_str)) {
                            ("connect", _) => "Connecting: waiting for the service".into(),
                            ("decline", _) => "Declined".into(),
                            ("disconnect", _) => "Disconnected".into(),
                            ("groups", Some("add")) => format!("Added {}", people(n)),
                            ("groups", Some("remove")) => format!("Removed {}", people(n)),
                            ("groups", Some("create")) => "Group made".into(),
                            ("groups", Some("rename")) => "Renamed".into(),
                            ("groups", Some("leave")) => "Left the group".into(),
                            ("block", _) => "Blocked".into(),
                            ("remove", _) => "Removed".into(),
                            _ => String::new(),
                        };
                        match args[0].as_str() {
                            "connect" | "decline" | "disconnect" => self.job(Job::Connections),
                            "bot" => self.job(Job::Whoami),
                            _ => self.job(Job::Refresh),
                        }
                    }
                    (Job::Details { target, .. }, result) => self.report(&target, result),
                    (Job::Diagnostics, result) => {
                        self.report("", result);
                        self.show_ping();
                    }
                    (Job::Send(to, path), Ok(_)) => {
                        let kind = if path.is_some() { Kind::Photo } else { Kind::Yo };
                        self.media_sent(&to, kind);
                        self.apply(Input::SendFinished { id: to, kind });
                    }
                    (Job::Send(to, _), Err(f)) => {
                        self.photos.remove(&to);
                        self.apply(Input::SendFailed { id: to, text: format!("Not sent: {}", f.error) });
                    }
                    (Job::Add(_), Ok(v)) => {
                        self.status = format!("Added {}", v["username"].as_str().unwrap_or("").to_uppercase());
                        self.input.clear();
                        self.screen = Screen::Main;
                        // As the app: adding someone sends them a Yo (the bar says "Sent Yo", then rises).
                        self.greet = v["user"].as_str().map(str::to_owned);
                        self.job(Job::Refresh);
                    }
                    (Job::Request(verb, id), Ok(_)) => {
                        self.requests.retain(|r| r["user"] != id);
                        self.apply(if verb == "accept" {
                            Input::RequestAccepted(id)
                        } else {
                            Input::RequestDeclined(id)
                        });
                        self.job(Job::Refresh);
                    }
                    (Job::SetRequests(on), Ok(_)) => {
                        self.requests_open = on;
                        self.job(Job::Refresh);
                    }
                    (job @ (Job::Devices | Job::Connections), Ok(d)) => {
                        let selected =
                            selectable_settings(self.devices.len(), self.connections.len()).nth(self.selected);
                        let list = if matches!(job, Job::Devices) { &mut self.devices } else { &mut self.connections };
                        *list = d.as_array().cloned().unwrap_or_default();
                        if self.screen == Screen::Settings {
                            let mut rows = selectable_settings(self.devices.len(), self.connections.len());
                            self.selected = rows
                                .position(|row| Some(row) == selected)
                                .unwrap_or_else(|| self.selected.min(self.count().saturating_sub(1)));
                        }
                    }
                    (Job::Rotate, Ok(v)) => {
                        self.shown_key = v["recovery_key"].as_str().unwrap_or("").to_owned();
                        self.confirm_rotate = false;
                        self.status = "New Recovery Key. The old one no longer works.".into();
                    }
                    (Job::LinkApprove(_), Ok(v)) => {
                        let device = &v["device"];
                        self.status = format!(
                            "Linked: {} ({})",
                            device_label(device["app"].as_str().unwrap_or("")),
                            device["number"]
                        );
                        self.go(Screen::Settings);
                    }
                    (Job::LinkApprove(_), Err(f)) => {
                        self.status = format!("Not linked: {}", f.error.trim_start_matches("not linked: "));
                        self.go(Screen::Settings);
                    }
                    (Job::Members { title, words, remove, .. }, Ok(v)) => self.members_loaded(title, words, remove, &v),
                    (Job::Probe, result) => {
                        let ping = match result {
                            Ok(v) => v
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect(),
                            Err(f) => vec![format!("PING: {}", f.error)],
                        };
                        self.ping = Some(ping);
                        self.show_ping();
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
                    (_, Err(f)) => self.status = format!("Error: {}", f.error),
                }
            }
        }
    }

    /// A line of the listener.
    fn event(&mut self, event: Value) {
        match event["type"].as_str() {
            Some("ready") => {
                self.listener_status.clear();
                self.job(Job::Refresh);
                self.job(Job::Whoami);
            }
            Some("message") => {
                if event["reply_to"].as_str().is_some_and(|id| self.items.contains_key(id)) {
                    self.received(&event);
                } else {
                    self.pending.push(event);
                    self.job(Job::Refresh);
                }
            }
            // Another device of this account sent it: the bar says so and rises, as after a send here.
            Some("sent") => match event["user"].as_str() {
                Some(user) if self.items.contains_key(user) => {
                    let kind = match event["content"].as_str() {
                        Some("photo") => Kind::Photo,
                        Some("video") => Kind::Video,
                        _ => Kind::Yo,
                    };
                    self.media_sent(user, kind);
                    self.apply(Input::SendFinished { id: user.to_owned(), kind });
                }
                _ => self.job(Job::Refresh),
            },
            Some(kind) => {
                match kind {
                    "connection_changed" => self.job(Job::Connections),
                    "device_added" => self.job(Job::Devices),
                    _ => {}
                }
                self.job(Job::Refresh);
            }
            None => {}
        }
    }

    fn now(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    fn apply(&mut self, input: Input) {
        self.snapshot_at = self.now();
        let mut next = self.list.apply(self.snapshot_at, input);
        next.rows.sort_by_key(|row| row.request);
        if self.screen == Screen::Main {
            let selected = self.snapshot.rows.get(self.selected);
            self.selected = selected
                .and_then(|r| next.rows.iter().position(|n| r.id == n.id))
                .unwrap_or(self.selected.min(next.rows.len().saturating_sub(1)));
        }
        self.snapshot = next;
    }

    fn refresh(&mut self, items: Value) {
        let Value::Array(items) = items else { return };
        let mut friends = Vec::new();
        let mut names = Vec::new();
        self.items.clear();
        for item in items {
            let Some(target) = item["target"].as_str() else { continue };
            let group = item["kind"] == "group";
            if !self.learned && !group && item["bot"] == false {
                if let Some(name) = item["username"].as_str() {
                    names.push(name.to_owned());
                }
            }
            // The name as the bar shows it (the core's rule: "#WG", "/PING"), as the apps give it to the list.
            let name = bar_title(&upper(&item["name"]), group, item["bot"] == true);
            friends.push(Friend { id: target.into(), name, group, request: group && item["accepted"] == false });
            self.items.insert(target.into(), item);
        }
        for r in &self.requests {
            let Some(id) = r["user"].as_str() else { continue };
            if !self.items.contains_key(id) {
                friends.push(Friend {
                    id: id.to_owned(),
                    name: upper(&r["name"]),
                    group: r["kind"] == "group",
                    request: true,
                });
            }
        }
        self.apply(Input::Friends(friends));
        for event in std::mem::take(&mut self.pending) {
            if self.refreshing && event["reply_to"].as_str().is_some_and(|id| !self.items.contains_key(id)) {
                self.pending.push(event);
            } else {
                self.received(&event);
            }
        }
        self.learned = true;
        if let Some(user) = self.greet.take_if(|user| self.items.contains_key(user.as_str())) {
            self.media_sent(&user, Kind::Yo);
            self.apply(Input::SendStarted { id: user.clone(), kind: Kind::Yo, undo_ms: 0 });
            self.job(Job::Send(user, None));
        }
        if !names.is_empty() {
            self.job(Job::LearnBots(names));
        }
    }

    fn received(&mut self, event: &Value) {
        let Some(id) = event["reply_to"].as_str() else { return };
        self.token += 1;
        let sender = event["group"].as_str().and(event["from"].as_str());
        if let Some(user) = sender {
            let name = event["from_name"]
                .as_str()
                .or_else(|| self.items.get(user)?.get("name")?.as_str())
                .map_or_else(|| user.chars().take(8).collect::<String>(), str::to_owned);
            self.apply(Input::SenderNamed { user: user.into(), name: name.to_uppercase() });
        }
        let kind = match event["content"].as_str() {
            Some("photo") => Kind::Photo,
            Some("video") => Kind::Video,
            _ => Kind::Yo,
        };
        self.apply(Input::Received {
            id: id.into(),
            kind,
            token: self.token.to_string(),
            sender: sender.map(str::to_owned),
        });
    }

    fn signed(&self) -> bool {
        matches!(
            self.screen,
            Screen::Main
                | Screen::Info(_)
                | Screen::Signout
                | Screen::Add
                | Screen::Settings
                | Screen::Blocked
                | Screen::Recovery
                | Screen::LinkInput
                | Screen::LinkConfirm
        )
    }

    fn go(&mut self, screen: Screen) {
        if self.screen == Screen::Settings {
            self.settings_at = self.selected;
        }
        // Back in Settings from one of its pages: on the row it was opened from.
        self.selected = if screen == Screen::Settings && self.screen != Screen::Main { self.settings_at } else { 0 };
        self.confirm_rotate = false;
        self.reveal = false;
        self.show_code = false;
        self.shown_key.clear();
        self.screen = screen;
        match screen {
            Screen::Blocked => self.job(Job::Blocked),
            Screen::Settings => {
                self.job(Job::Whoami);
                self.job(Job::Devices);
                self.job(Job::Connections);
                self.job(Job::Refresh);
            }
            _ => {}
        }
    }

    /// What a bar's "Sent" is of: a Yo, or a photo or video (the bar then says "Sent 📷!").
    fn media_sent(&mut self, id: &str, kind: Kind) {
        if kind == Kind::Yo {
            self.photos.remove(id);
        } else {
            self.photos.insert(id.to_owned(), kind);
        }
    }

    fn send(&mut self, i: usize) {
        if let Some(row) = self.snapshot.rows.get(i).filter(|r| !r.request) {
            let target = row.id.clone();
            self.media_sent(&target, Kind::Yo);
            self.apply(Input::SendStarted { id: target.clone(), kind: Kind::Yo, undo_ms: 0 });
            self.job(Job::Send(target, None));
        }
    }

    fn answer(&mut self, i: usize, verb: &'static str) {
        if let Some(row) = self.snapshot.rows.get(i).filter(|r| r.request) {
            if let Some(id) = row.id.strip_prefix(yo_core::shared::GROUP_PREFIX) {
                if verb != "block" {
                    self.job(Job::Change(words(&["groups", if verb == "accept" { "accept" } else { "leave" }, id])));
                }
            } else {
                self.job(Job::Request(verb, row.id.clone()));
            }
        }
    }

    fn add_menu(&mut self) {
        self.dialog = Some(Dialog::Menu { items: ADD_MENU, target: String::new(), cursor: 0 });
    }

    fn bar_action(&mut self, key: char) {
        let Some(row) = self.snapshot.rows.get(self.selected) else { return };
        let target = row.id.clone();
        let Some(item) = self.items.get(&target) else { return };
        match key {
            'p' => self.prompt("Photo (JPEG file)", words(&["send", &target, "--photo"])),
            'i' => {
                let name = item["username"].as_str().or(item["name"].as_str()).unwrap_or("").to_owned();
                self.open_report("Details", &target);
                self.job(Job::Details { target, name });
            }
            'm' => {
                let items = bar_menu(item["kind"] == "group", item["blocked"] == true);
                self.dialog = Some(Dialog::Menu { items, target, cursor: 0 });
            }
            _ => {}
        }
    }

    fn prompt(&mut self, title: &'static str, words: Vec<String>) {
        self.input.clear();
        self.dialog = Some(Dialog::Prompt { title, words });
    }

    fn confirm(&mut self, title: &'static str, text: String, words: Vec<String>) {
        self.dialog = Some(Dialog::Confirm { title, text, words });
    }

    /// The member picker: adding lists the friends not in the group yet, removing the group's members.
    fn members(&mut self, title: &'static str, words: Vec<String>, group: Option<&str>, remove: bool) {
        self.dialog =
            Some(Dialog::Members { title, words: words.clone(), friends: Vec::new(), cursor: 0, empty: "Loading…" });
        self.job(Job::Members { title, words, group: group.map(str::to_owned), remove });
    }

    fn members_loaded(&mut self, title: &'static str, words: Vec<String>, remove: bool, v: &Value) {
        if !matches!(&self.dialog, Some(Dialog::Members { title: t, .. }) if *t == title) {
            return;
        }
        let me = self.me["user"].as_str().unwrap_or("");
        let members: Vec<&str> = v["members"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let (friends, empty) = if remove {
            let contacts = v["contacts"].as_array().cloned().unwrap_or_default();
            let name = |id: &str| {
                self.items
                    .get(id)
                    .and_then(|item| item["name"].as_str())
                    .or_else(|| {
                        let c = contacts.iter().find(|c| c["user"] == id)?;
                        c["handles"][0].as_str().or(c["name"].as_str())
                    })
                    .map_or_else(|| id.chars().take(8).collect(), str::to_uppercase)
            };
            let list = members.iter().filter(|id| **id != me).map(|id| ((*id).to_owned(), name(id), false)).collect();
            (list, "No one else is in the group")
        } else {
            let list = self
                .snapshot
                .rows
                .iter()
                .filter(|r| !r.request && !members.contains(&r.id.as_str()))
                .filter_map(|r| {
                    let item = self.items.get(&r.id)?;
                    let can = item["kind"] == "friend"
                        && item["blocked"] != true
                        && item["bot"] != true
                        && item["gone"] != true;
                    can.then(|| (r.id.clone(), upper(&item["name"]), false))
                })
                .collect();
            (list, if words[1] == "add" { "Every friend is in it already" } else { "No friends yet" })
        };
        self.dialog = Some(Dialog::Members { title, words, friends, cursor: 0, empty });
    }

    fn open_report(&mut self, title: &'static str, target: &str) {
        self.dialog =
            Some(Dialog::Report { title, target: target.into(), lines: vec!["Loading…".into()], scroll: 0 });
    }

    /// The probe's answer on the Status card, in the line that waits for it.
    fn show_ping(&mut self) {
        let Some(ping) = self.ping.clone() else { return };
        if let Some(Dialog::Report { target, lines, .. }) = &mut self.dialog {
            if let Some(at) = lines.iter().position(|l| target.is_empty() && l == "PING …") {
                lines.splice(at..=at, ping);
            }
        }
    }

    fn report(&mut self, id: &str, result: Result<Value, Failure>) {
        if let Some(Dialog::Report { target, lines, .. }) = &mut self.dialog {
            if target == id {
                *lines = match result {
                    Ok(value) => {
                        value.as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect()
                    }
                    Err(f) => vec![f.error],
                };
            }
        }
    }

    fn menu_choice(&mut self, choice: &'static str, target: &str) {
        let id = target.strip_prefix(yo_core::shared::GROUP_PREFIX).unwrap_or(target);
        match choice {
            "Add Friend" => {
                self.input.clear();
                self.screen = Screen::Add;
            }
            "New Group" => self.prompt("New Group", words(&["groups", "create"])),
            "Rename" => self.prompt("Rename Group", words(&["groups", "rename", id])),
            "Add Members" => self.members(choice, words(&["groups", "add", id]), Some(id), false),
            "Remove Members" => self.members(choice, words(&["groups", "remove", id]), Some(id), true),
            "Remove Friend" | "Leave Group" => {
                let label = self.items.get(target).map(|v| upper(&v["name"])).unwrap_or_else(|| target.into());
                let command =
                    if choice == "Leave Group" { words(&["groups", "leave", id]) } else { words(&["remove", target]) };
                self.confirm(choice, format!("{choice}: {label}?"), command);
            }
            "Block" => self.job(Job::Change(words(&["block", target]))),
            "Unblock" => self.job(Job::Unblock(target.into())),
            _ => {}
        }
    }

    fn dialog_key(&mut self, key: KeyCode) {
        let Some(mut dialog) = self.dialog.take() else { return };
        if key == KeyCode::Esc {
            self.input.clear();
            return;
        }
        match &mut dialog {
            Dialog::Menu { items, target, cursor } => match key {
                KeyCode::Up | KeyCode::Char('k') => *cursor = cursor.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *cursor = (*cursor + 1).min(items.len() - 1),
                KeyCode::Enter => {
                    self.menu_choice(items[*cursor], target);
                    return;
                }
                _ => {}
            },
            Dialog::Prompt { words, .. } => match key {
                KeyCode::Char(c) => self.input.push(c),
                KeyCode::Backspace => _ = self.input.pop(),
                KeyCode::Enter if !self.input.trim().is_empty() => {
                    let input = self.input.trim().to_owned();
                    match words[0].as_str() {
                        "send" => {
                            let id = words[1].clone();
                            self.media_sent(&id, Kind::Photo);
                            self.apply(Input::SendStarted { id: id.clone(), kind: Kind::Photo, undo_ms: 0 });
                            self.job(Job::Send(id, Some(input)));
                        }
                        "connect" | "decline" => {
                            let parts: Vec<_> = input.split_whitespace().collect();
                            if parts.len() != 2 {
                                self.status = "Enter: SERVICE CODE".into();
                                self.dialog = Some(dialog);
                                return;
                            }
                            words.extend(parts.into_iter().map(str::to_owned));
                            self.job(Job::Change(std::mem::take(words)));
                        }
                        _ => {
                            words.push(input);
                            if words[1] == "create" {
                                self.members("New Group · Members", std::mem::take(words), None, false);
                            } else {
                                self.job(Job::Change(std::mem::take(words)));
                            }
                        }
                    }
                    self.input.clear();
                    return;
                }
                _ => {}
            },
            Dialog::Members { cursor, friends, words, .. } => match key {
                KeyCode::Up | KeyCode::Char('k') => *cursor = cursor.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *cursor = (*cursor + 1).min(friends.len().saturating_sub(1)),
                KeyCode::Char(' ') => {
                    if let Some((_, _, picked)) = friends.get_mut(*cursor) {
                        *picked = !*picked;
                    }
                }
                KeyCode::Enter => {
                    if friends.iter().any(|f| f.2) {
                        words.extend(friends.iter().filter(|f| f.2).map(|f| f.0.clone()));
                        self.job(Job::Change(std::mem::take(words)));
                        return;
                    }
                    if !friends.is_empty() {
                        self.status = "Choose at least one with space".into();
                    }
                }
                _ => {}
            },
            Dialog::Confirm { words, .. } => {
                if key == KeyCode::Char('y') {
                    self.job(Job::Change(std::mem::take(words)));
                }
                return;
            }
            Dialog::Report { scroll, lines, .. } => match key {
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *scroll = (*scroll + 1).min(lines.len().saturating_sub(1)),
                KeyCode::Char('q') => return,
                _ => {}
            },
        }
        self.dialog = Some(dialog);
    }

    fn draw_dialog(&self, f: &mut Frame, area: Rect, dialog: &Dialog) {
        let selected = |label: String, on| {
            if on {
                Line::styled(format!("▶ {label}"), Style::new().reversed().bold())
            } else {
                Line::from(format!("  {label}"))
            }
        };
        let room = usize::from(area.height.saturating_sub(2)).max(1);
        let (title, lines) = match dialog {
            Dialog::Menu { items, target, cursor } => {
                let title = self.items.get(target).map_or_else(|| "Add".to_owned(), |i| upper(&i["name"]));
                let first = (cursor + 1).saturating_sub(room);
                (
                    title,
                    items
                        .iter()
                        .enumerate()
                        .skip(first)
                        .take(room)
                        .map(|(i, s)| selected((*s).into(), i == *cursor))
                        .collect(),
                )
            }
            Dialog::Prompt { title, .. } => (
                (*title).to_owned(),
                vec![self.field("", &tail(&self.input, usize::from(area.width.min(72).saturating_sub(9))))],
            ),
            Dialog::Confirm { title, text, .. } => ((*title).to_owned(), wrap_card(text, area.width)),
            Dialog::Members { title, friends, cursor, empty, .. } => {
                let first = (cursor + 1).saturating_sub(room);
                let mut lines: Vec<_> = friends
                    .iter()
                    .enumerate()
                    .skip(first)
                    .take(room)
                    .map(|(i, (_, name, on))| {
                        selected(format!("[{}] {name}", if *on { "x" } else { " " }), i == *cursor)
                    })
                    .collect();
                if lines.is_empty() {
                    lines.push(Line::from(*empty).style(self.dim()));
                }
                ((*title).to_owned(), lines)
            }
            Dialog::Report { title, lines, scroll, .. } => {
                ((*title).to_owned(), lines.iter().skip(*scroll).flat_map(|s| wrap_card(s, area.width)).collect())
            }
        };
        self.card(f, area, &title, lines);
    }

    fn setting(&mut self, i: usize) {
        self.settings_at = i;
        let item = match selectable_settings(self.devices.len(), self.connections.len()).nth(i) {
            Some(SettingsRow::Item(item)) => item,
            Some(SettingsRow::Connection(i)) => {
                let c = &self.connections[i];
                self.confirm(
                    "Disconnect?",
                    upper(&c["name"]),
                    words(&["disconnect", c["user"].as_str().unwrap_or("")]),
                );
                return;
            }
            Some(row @ (SettingsRow::Connect | SettingsRow::Decline)) => {
                let verb = if row == SettingsRow::Connect { "connect" } else { "decline" };
                self.prompt("SERVICE CODE", words(&[verb]));
                return;
            }
            Some(SettingsRow::Device(d)) => {
                let device = &self.devices[d];
                if device["this"] == true {
                    self.status = "This device: use Sign Out to remove it".into();
                } else if let Some(number) = device["number"].as_u64() {
                    self.confirm_device = Some(number);
                }
                return;
            }
            None => return,
        };
        match item.id {
            "requests" => self.job(Job::SetRequests(!self.requests_open)),
            "sign_out" => self.screen = Screen::Signout,
            "blocked" => self.go(Screen::Blocked),
            "add_friend" => {
                self.input.clear();
                self.go(Screen::Add);
            }
            "bot" => {
                if self.me["bot"] == true {
                    self.status = "Bot account: people see / before the name. Can't be undone.".into();
                } else {
                    self.confirm(
                        "Bot Marker",
                        "Mark this account as a bot? People see / before its name. Can't be undone.".into(),
                        words(&["bot", "on"]),
                    );
                }
            }
            "recovery_key" => {
                self.go(Screen::Recovery);
                self.shown_key = crate::account::read_file(&self.config, "recovery")
                    .map(|k| String::from_utf8_lossy(&k).into_owned())
                    .unwrap_or_default();
            }
            "status" => {
                self.open_report("Status", "");
                self.ping = None;
                self.job(Job::Diagnostics);
                self.job(Job::Probe);
            }
            "profile" => self.show_code = !self.show_code,
            "about" | "feedback" => self.screen = Screen::Info(item.id),
            "link_device" => {
                self.input.clear();
                self.screen = Screen::LinkInput;
            }
            _ => {}
        }
    }

    fn finish_signout(&mut self, globals: &[String]) {
        self.retry_at = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        match crate::account::signout(&self.config) {
            Ok(()) => {
                *self = Self::new(&self.config, self.tx.clone());
                self.screen = Screen::Welcome;
            }
            Err(f) => {
                self.signing_out = false;
                self.status = f.error;
                self.go(Screen::Settings);
                self.start_listener(globals);
            }
        }
    }

    fn count(&self) -> usize {
        match self.screen {
            // The bars and, last, the add bar.
            Screen::Main => self.snapshot.rows.len() + 1,
            Screen::Settings => selectable_settings(self.devices.len(), self.connections.len()).count(),
            Screen::Blocked => self.blocked.len(),
            _ => 0,
        }
    }

    fn copy(&mut self) {
        let Some(tool) = &self.clipboard else { return };
        let key = grouped(&self.shown_key).join(" ");
        let done = Command::new(tool[0])
            .args(&tool[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                child.stdin.take().expect("piped").write_all(key.as_bytes())?;
                child.wait()
            });
        self.status = match done {
            Ok(s) if s.success() => "Copied".into(),
            _ => format!("Couldn't copy ({})", tool[0]),
        };
    }

    fn mouse(&mut self, mouse: MouseEvent) {
        if self.help {
            match mouse.kind {
                MouseEventKind::ScrollDown => self.help_scroll = (self.help_scroll + 1).min(self.help_max.get()),
                MouseEventKind::ScrollUp => self.help_scroll = self.help_scroll.saturating_sub(1),
                _ => {}
            }
            return;
        }
        if self.signing_out || self.dialog.is_some() {
            return;
        }
        let hit = self
            .hits
            .borrow()
            .iter()
            .find(|(r, _)| r.contains(ratatui::layout::Position { x: mouse.column, y: mouse.row }))
            .map(|(_, h)| *h);
        match mouse.kind {
            MouseEventKind::ScrollDown if self.count() > 0 => {
                self.selected = (self.selected + 1).min(self.count().saturating_sub(1))
            }
            MouseEventKind::ScrollUp if self.count() > 0 => self.selected = self.selected.saturating_sub(1),
            MouseEventKind::Down(MouseButton::Left) => {
                self.status.clear();
                match hit {
                    Some(Hit::Blocked(i)) => self.selected = i,
                    Some(Hit::Bar(i)) => {
                        self.selected = i;
                        self.screen = Screen::Main;
                        self.send(i);
                    }
                    Some(Hit::AddBar) => self.add_menu(),
                    Some(Hit::Choice(i, verb)) => self.answer(i, verb),
                    Some(Hit::Setting(i)) => {
                        self.selected = i;
                        self.setting(i);
                    }
                    Some(Hit::Toggle) => self.human = !self.human,
                    None => {}
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, key: KeyEvent) {
        if self.signing_out {
            return;
        }
        if self.screen == Screen::Signout {
            if key.code == KeyCode::Char('y') {
                self.signing_out = true;
            } else {
                self.screen = Screen::Settings;
            }
            return;
        }
        if let Some(number) = self.confirm_device.take() {
            if key.code == KeyCode::Char('y') {
                self.job(Job::RemoveDevice(number));
            }
            return;
        }
        let ctrl_c = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl_c {
            self.quit = true;
            return;
        }
        // A new recovery key: only y makes it, any other key cancels (the old key and passkeys stop working).
        if self.screen == Screen::Recovery && self.confirm_rotate {
            self.confirm_rotate = false;
            if key.code == KeyCode::Char('y') {
                self.job(Job::Rotate);
            } else {
                self.status = "No new key".into();
            }
            return;
        }
        self.status.clear();
        if self.dialog.is_some() {
            self.dialog_key(key.code);
            return;
        }
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
        if key.code == KeyCode::Char('?') {
            self.help = !self.help;
            self.help_scroll = 0;
            return;
        }
        if self.help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.help = false,
                KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown => {
                    self.help_scroll = (self.help_scroll + 1).min(self.help_max.get())
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::Home => self.help_scroll = 0,
                _ => {}
            }
            return;
        }
        let down = matches!(key.code, KeyCode::Down | KeyCode::Char('j'));
        let up = matches!(key.code, KeyCode::Up | KeyCode::Char('k'));
        let count = self.count();
        if down && count > 0 {
            self.selected = (self.selected + 1).min(count - 1);
            return;
        }
        if up && count > 0 {
            self.selected = self.selected.saturating_sub(1);
            return;
        }
        // A page: as many bars as the last frame showed (at least one).
        let page = self.hits.borrow().iter().filter(|(_, h)| matches!(h, Hit::Bar(_))).count().max(1);
        let to = match key.code {
            KeyCode::Home => Some(0),
            KeyCode::End => Some(count.saturating_sub(1)),
            KeyCode::PageDown => Some((self.selected + page).min(count.saturating_sub(1))),
            KeyCode::PageUp => Some(self.selected.saturating_sub(page)),
            _ => None,
        };
        if let Some(to) = to.filter(|_| count > 0) {
            self.selected = to;
            return;
        }
        let request = self.snapshot.rows.get(self.selected).is_some_and(|r| r.request);
        match (self.screen, key.code) {
            (Screen::Welcome, KeyCode::Char('1' | 's')) => {
                self.input.clear();
                self.screen = Screen::Signup
            }
            (Screen::Welcome, KeyCode::Char('2' | 'r')) => {
                self.input.clear();
                self.screen = Screen::RecoveryInput
            }
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
            (Screen::ShowKey | Screen::Recovery, KeyCode::Char('c')) if !self.shown_key.is_empty() => self.copy(),
            (Screen::Recovery, KeyCode::Char('v')) if !self.shown_key.is_empty() => self.reveal = !self.reveal,
            (Screen::ShowKey, KeyCode::Enter) => {
                self.shown_key.clear();
                self.screen = Screen::Loading;
                self.job(Job::Whoami);
            }
            (Screen::Main | Screen::Welcome | Screen::Loading, KeyCode::Char('q') | KeyCode::Esc) => self.quit = true,
            (Screen::Main, KeyCode::Char('s')) => self.go(Screen::Settings),
            (Screen::Main, KeyCode::Enter | KeyCode::Char(' ')) if request => self.answer(self.selected, "accept"),
            (Screen::Main, KeyCode::Char('x')) if request => self.answer(self.selected, "decline"),
            (Screen::Main, KeyCode::Char('b')) if request => self.answer(self.selected, "block"),
            (Screen::Blocked, KeyCode::Char('u')) => {
                if let Some(id) = self.blocked.get(self.selected).and_then(|r| r["user"].as_str()) {
                    self.job(Job::Unblock(id.to_owned()));
                }
            }
            (Screen::Info("about"), KeyCode::Char('o')) => open_urls(&[settings::TERMS_URL, settings::PRIVACY_URL]),
            (Screen::Info("feedback"), KeyCode::Char('o')) => open_urls(&[FEEDBACK]),
            (Screen::Main, KeyCode::Enter | KeyCode::Char(' ')) if self.selected == self.snapshot.rows.len() => {
                self.add_menu()
            }
            (Screen::Main, KeyCode::Enter | KeyCode::Char(' ')) => self.send(self.selected),
            (Screen::Main, KeyCode::Char('+' | 'a')) => self.add_menu(),
            (Screen::Main, KeyCode::Char(key @ ('p' | 'i' | 'm'))) if !request => self.bar_action(key),
            (Screen::Settings, KeyCode::Enter | KeyCode::Char(' ')) => self.setting(self.selected),
            (Screen::Recovery, KeyCode::Char('R')) => self.confirm_rotate = true,
            // Waiting for the new device to take it: keys wait too.
            (Screen::LinkConfirm, _) if self.busy => {}
            (Screen::LinkConfirm, KeyCode::Char('y')) => {
                let text = std::mem::take(&mut self.input);
                self.job(Job::LinkApprove(text));
            }
            (Screen::LinkConfirm, _) => {
                self.input.clear();
                self.status = "Not linked".into();
                self.screen = Screen::Settings;
            }
            (Screen::Settings, KeyCode::Char('q') | KeyCode::Esc) if self.show_code => self.show_code = false,
            (Screen::Settings, KeyCode::Char('q') | KeyCode::Esc) => self.go(Screen::Main),
            (Screen::Recovery | Screen::Blocked | Screen::Info(_), KeyCode::Char('q') | KeyCode::Esc) => {
                self.go(Screen::Settings);
            }
            _ => {}
        }
    }

    fn submit(&mut self) {
        match self.screen {
            Screen::Signup => match crate::name_ok(self.input.trim()) {
                Ok(()) => self.job(Job::Signup),
                Err(f) => self.status = f.error,
            },
            Screen::RecoveryInput => self.job(Job::Recovery),
            Screen::Add => {
                let name = self.input.trim().to_owned();
                if !name.is_empty() {
                    self.job(Job::Add(name));
                }
            }
            Screen::LinkInput => {
                self.input = signin::compact_link(&self.input);
                match yo_core::link::offer(&self.input) {
                    Ok(_) => self.screen = Screen::LinkConfirm,
                    Err(_) => self.status = "Not a Yo link".into(),
                }
            }
            _ => {}
        }
    }

    // Styles: none of them colour without colour.

    fn fg(&self, c: Color) -> Style {
        if self.color {
            Style::new().fg(c)
        } else {
            Style::new()
        }
    }

    fn dim(&self) -> Style {
        if self.color {
            Style::new().fg(SECONDARY)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        }
    }

    fn accent(&self) -> Style {
        self.fg(ACCENT).add_modifier(Modifier::BOLD)
    }

    /// One of the app's colours: exact in truecolor, else the nearest of the 256.
    fn rgb(&self, hex: u32) -> Color {
        let (r, g, b) = ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        if self.truecolor {
            Color::Rgb(r, g, b)
        } else {
            Color::Indexed(nearest_256(r, g, b))
        }
    }

    fn draw(&self, f: &mut Frame) {
        self.hits.borrow_mut().clear();
        let area = f.area();
        let legend = self.legend();
        let legend_lines = self.legend_lines(&legend, area.width.saturating_sub(4) as usize);
        let foot = legend_lines.len() as u16 + 1;

        let body = Rect {
            x: area.x + 2,
            y: area.y + 2,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2 + foot),
        };
        if area.width < 20 || body.height < 3 {
            f.render_widget(Paragraph::new("Make the window bigger"), area);
            return;
        }
        let buf = f.buffer_mut();
        self.title(buf, Rect { height: 1, ..area });
        let foot_y = area.y + area.height.saturating_sub(foot);
        let status = if self.signing_out {
            "Signing out…".to_string()
        } else if !self.listener_status.is_empty() {
            self.listener_status.clone()
        } else if let Some(banner) = &self.snapshot.banner {
            banner.clone()
        } else if self.busy {
            "Working…".to_owned()
        } else {
            self.status.clone()
        };
        let status = (!status.is_empty()).then(|| Span::styled(format!(" {status} "), self.accent()));
        rule(buf, Rect { x: area.x, y: foot_y, width: area.width, height: 1 }, self.dim(), status);
        for (i, line) in legend_lines.into_iter().enumerate() {
            buf.set_line(area.x + 2, foot_y + 1 + i as u16, &line, area.width.saturating_sub(4));
        }
        match self.screen {
            Screen::Loading => self.card(f, body, "Yo", vec![Line::from("Opening…").style(self.dim())]),
            Screen::Main | Screen::Add => self.draw_bars(f, body),
            Screen::Signout => {
                let lines = wrap_card(crate::account::SIGNOUT_PROMPT, body.width);
                self.card(f, body, "Sign Out", lines);
            }
            Screen::Info(id) => self.draw_info(f, body, id),
            Screen::Blocked => self.draw_blocked(f, body),
            Screen::Settings => self.draw_settings(f, body),
            Screen::Welcome => self.draw_welcome(f, body),
            Screen::Signup => self.draw_signup(f, body),
            Screen::RecoveryInput => self.card(
                f,
                body,
                "Sign In with Recovery Key",
                vec![
                    Line::from(""),
                    self.field("Recovery Key", &"•".repeat(self.input.chars().count())),
                    Line::from(""),
                    Line::from("64 letters and digits. Dashes and spaces are fine.").style(self.dim()),
                    Line::from("Paste works.").style(self.dim()),
                ],
            ),
            Screen::LinkWait => {
                let mut lines: Vec<Line> = Vec::new();
                if self.link_text.is_empty() {
                    lines.push(Line::from("Getting a link…").style(self.dim()));
                } else {
                    let width = usize::from(body.width.min(72) - 2);
                    let token = &self.link_text;
                    let token_rows = token.len().div_ceil(width);
                    let qr = signin::qr(token);
                    let fits = qr.lines().count() + token_rows + 3 <= usize::from(body.height)
                        && qr.lines().all(|row| row.chars().count() <= width);
                    if fits {
                        lines.extend(qr.lines().map(|row| Line::from(row.to_owned())));
                    }
                    for chunk in token.as_bytes().chunks(width) {
                        lines.push(Line::from(std::str::from_utf8(chunk).expect("ASCII link")).style(self.accent()));
                    }
                    if fits {
                        lines.push(Line::from("Scan or paste this on your other device.").style(self.dim()));
                    } else {
                        let hint = "Make the window bigger for the QR code";
                        let mut row = String::new();
                        for word in hint.split_whitespace() {
                            if row.len() + 1 + word.len() > width {
                                lines.push(Line::from(std::mem::take(&mut row)).style(self.dim()));
                            }
                            if !row.is_empty() {
                                row.push(' ');
                            }
                            row.push_str(word);
                        }
                        lines.push(Line::from(row).style(self.dim()));
                        if lines.len() + 2 > usize::from(body.height) {
                            f.render_widget(Paragraph::new("Make the window bigger"), body);
                            return;
                        }
                    }
                }
                self.card(f, body, "Sign In with Another Device", lines)
            }
            Screen::ShowKey => {
                let me = bar_title(&upper(&self.me["username"]), false, self.me["bot"] == true);
                let mut lines =
                    vec![Line::from(vec![Span::raw("Signed up as "), Span::styled(me, self.accent())]), Line::from("")];
                if body.height < 12 {
                    lines.clear();
                }
                let key = self.key_lines(body.width.min(72) - 2);
                if key.len() + 2 > usize::from(body.height) {
                    f.render_widget(Paragraph::new("Make the window bigger"), body);
                    return;
                }
                lines.extend(key);
                if lines.len() + 2 == usize::from(body.height) {
                    self.card(f, body, "Recovery Key", lines);
                    return;
                }
                lines.extend([
                    Line::from(""),
                    Line::from("Save it now, e.g. in a password manager."),
                    Line::from("You need it to sign in on a new device."),
                    Line::from("yo recovery show shows it again.").style(self.dim()),
                ]);
                self.card(f, body, "Recovery Key", lines)
            }
            Screen::Recovery => self.draw_recovery(f, body),
            Screen::LinkInput => {
                let lines = vec![
                    self.field("Link", &tail(&self.input, usize::from(body.width.saturating_sub(7)))),
                    Line::from(""),
                    Line::from("Paste the link the new device shows (yo signin --link).").style(self.dim()),
                ];
                self.section(f, body, "Link a New Device", lines)
            }
            Screen::LinkConfirm => self.section(
                f,
                body,
                "Link a New Device",
                vec![Line::from(vec![
                    Span::raw("Give the new device access to your account?  "),
                    Span::styled("y", self.fg(KEY).add_modifier(Modifier::BOLD)),
                    Span::styled(" link • ", self.dim()),
                    Span::styled("any key", self.fg(KEY).add_modifier(Modifier::BOLD)),
                    Span::styled(" cancel", self.dim()),
                ])],
            ),
        }
        if let Some(dialog) = &self.dialog {
            self.hits.borrow_mut().clear();
            self.draw_dialog(f, body, dialog);
        }
        if self.help {
            self.hits.borrow_mut().clear();
            self.draw_help(f, body);
        }
    }

    /// "──── Yo ──── NAME ──": the title centred in the rule, the account at its right end.
    fn title(&self, buf: &mut Buffer, area: Rect) {
        let w = area.width as usize;
        let me = if self.signed() {
            bar_title(&upper(&self.me["username"]), false, self.me["bot"] == true)
        } else {
            String::new()
        };
        // A profile (`--profile NAME`) says so next to the account.
        let me = match crate::account::profile_of(&self.config.state) {
            Some(profile) if me.is_empty() => format!("profile {profile}"),
            Some(profile) => format!("{me} · {profile}"),
            None => me,
        };
        let mid = w.saturating_sub(4) / 2;
        let mut spans = vec![Span::styled("─".repeat(mid), self.dim()), Span::styled(" Yo ", self.accent())];
        let used = mid + 4;
        let right = if me.is_empty() { 0 } else { me.chars().count() + 2 };
        let fill = if right == 0 { w.saturating_sub(used) } else { w.saturating_sub(used + right + 2) };
        spans.push(Span::styled("─".repeat(fill), self.dim()));
        if right > 0 && used + right + 2 <= w {
            spans.push(Span::styled(format!(" {me} "), Style::new().add_modifier(Modifier::BOLD)));
            spans.push(Span::styled("──", self.dim()));
        }
        buf.set_line(area.x, area.y, &Line::from(spans), area.width);
    }

    /// The keys of this screen, as (key, what) pairs.
    fn legend(&self) -> Vec<(&'static str, &'static str)> {
        if let Some(dialog) = &self.dialog {
            return match dialog {
                Dialog::Menu { .. } => vec![("↑↓", "navigate"), ("enter", "choose"), ("esc", "back")],
                Dialog::Prompt { .. } => vec![("enter", "continue"), ("esc", "back")],
                Dialog::Members { .. } => {
                    vec![("↑↓", "navigate"), ("space", "toggle"), ("enter", "submit"), ("esc", "back")]
                }
                Dialog::Confirm { .. } => vec![("y", "confirm"), ("esc", "cancel")],
                Dialog::Report { .. } => vec![("↑↓", "scroll"), ("esc", "back")],
            };
        }
        let mut keys: Vec<(&str, &str)> = match self.screen {
            Screen::Loading => vec![("q", "quit")],
            Screen::Welcome => vec![("1", "sign up"), ("2", "recovery key"), ("3", "another device"), ("q", "quit")],
            Screen::Signup => vec![("type", "a name"), ("tab", "person/bot"), ("enter", "sign up"), ("esc", "back")],
            Screen::RecoveryInput => vec![("paste", "the key"), ("enter", "sign in"), ("esc", "back")],
            Screen::LinkWait => vec![("esc", "cancel")],
            Screen::ShowKey => vec![("mouse", "select to copy"), ("enter", "continue")],
            Screen::Main if self.snapshot.rows.get(self.selected).is_some_and(|r| r.request) => vec![
                ("↑↓", "navigate"),
                ("enter", "accept"),
                ("x", "decline"),
                ("b", "block"),
                ("a", "add"),
                ("s", "settings"),
                ("?", "help"),
            ],
            Screen::Main => vec![
                ("↑↓", "navigate"),
                ("enter", "Yo"),
                ("p", "photo"),
                ("i", "info"),
                ("m", "menu"),
                ("a", "add"),
                ("s", "settings"),
                ("?", "toggle help"),
                ("q", "quit"),
            ],
            Screen::Add => vec![("type", "a name"), ("enter", "add"), ("esc", "back")],
            Screen::Settings if self.confirm_device.is_some() => {
                vec![("y", "remove the device"), ("any key", "cancel")]
            }
            Screen::Settings => vec![("↑↓", "navigate"), ("enter", "open/toggle"), ("esc", "back"), ("?", "help")],
            Screen::Blocked => vec![("↑↓", "navigate"), ("u", "unblock"), ("esc", "back")],
            Screen::Info("about" | "feedback") => vec![("o", "open"), ("esc", "back")],
            Screen::Info(_) => vec![("esc", "back")],
            Screen::Signout => vec![("y", "sign out"), ("any other key", "cancel")],
            Screen::Recovery if self.reveal => {
                vec![("mouse", "select to copy"), ("v", "hide"), ("R", "new key"), ("esc", "back")]
            }
            Screen::Recovery => vec![("v", "show"), ("R", "new key"), ("esc", "back")],
            Screen::LinkInput => vec![("paste", "the link"), ("enter", "link"), ("esc", "back")],
            Screen::LinkConfirm => vec![("y", "link"), ("any key", "cancel")],
        };
        if self.screen == Screen::Main
            && self
                .snapshot
                .rows
                .get(self.selected)
                .is_some_and(|r| r.request && r.id.starts_with(yo_core::shared::GROUP_PREFIX))
        {
            keys.retain(|(key, _)| *key != "b");
        }
        if matches!(self.screen, Screen::ShowKey | Screen::Recovery)
            && self.clipboard.is_some()
            && !self.shown_key.is_empty()
        {
            keys.insert(0, ("c", "copy"));
        }
        if self.help {
            keys = vec![("?", "close help"), ("esc", "close")];
        }
        keys
    }

    /// The legend wrapped to the width: bold keys, dim words, " • " between.
    fn legend_lines(&self, keys: &[(&str, &str)], width: usize) -> Vec<Line<'static>> {
        let mut lines = vec![Vec::new()];
        let mut used = 0;
        for (key, what) in keys {
            let len = key.chars().count() + 1 + what.chars().count();
            let sep = if used == 0 { 0 } else { 3 };
            if used > 0 && used + sep + len > width {
                lines.push(Vec::new());
                used = 0;
            }
            let row = lines.last_mut().expect("a line");
            if used > 0 {
                row.push(Span::styled(" • ", self.dim()));
                used += 3;
            }
            row.push(Span::styled(key.to_string(), self.fg(KEY).add_modifier(Modifier::BOLD)));
            row.push(Span::styled(format!(" {what}"), self.dim()));
            used += len;
        }
        lines.into_iter().map(Line::from).collect()
    }

    /// A rounded card in the middle of the area, as hey's dialogs: the title in its top border.
    fn card(&self, f: &mut Frame, area: Rect, title: &str, lines: Vec<Line>) {
        render_card(f, area, title, lines, self.dim());
    }

    /// A page: its heading as a label then a rule ("Settings ────"), the lines under it.
    fn section(&self, f: &mut Frame, area: Rect, title: &str, lines: Vec<Line>) {
        heading(f.buffer_mut(), area, title, self.dim());
        let rest = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
        f.render_widget(Paragraph::new(lines), rest);
    }

    /// "Name  ANNA_SMITH▏", the cursor at the end, on an underlined field.
    fn field(&self, label: &str, value: &str) -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("{label}  "), self.dim()),
            Span::styled(format!("{value}▏"), Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)),
        ])
    }

    /// The recovery key in fixed groups of four, up to four groups a line: a group never splits.
    fn key_lines(&self, width: u16) -> Vec<Line<'static>> {
        let hidden = self.screen == Screen::Recovery && !self.reveal;
        let groups: Vec<String> =
            grouped(&self.shown_key).into_iter().map(|g| if hidden { "••••".into() } else { g }).collect();
        groups
            .chunks(((usize::from(width) + 2) / 6).clamp(1, 4))
            .map(|row| Line::from(Span::styled(row.join("  "), self.accent())))
            .collect()
    }

    fn draw_welcome(&self, f: &mut Frame, body: Rect) {
        let option = |n: &'static str, text: &'static str| {
            Line::from(vec![
                Span::styled(n, self.fg(KEY).add_modifier(Modifier::BOLD)),
                Span::raw(format!("  {text:<44}")),
            ])
        };
        let mut lines = vec![
            Line::from("No account here yet.").style(self.dim()),
            Line::from(""),
            option("1", "Sign Up"),
            option("2", "Sign In with Recovery Key"),
            option("3", "Sign In with Another Device"),
            Line::from(""),
            Line::from(format!("Folder {}", short_path(&self.config.state, 40))).style(self.dim()),
            Line::from(format!("Hub    {}", self.config.hub.as_deref().unwrap_or("").trim_start_matches("https://")))
                .style(self.dim()),
        ];
        if body.height < 12 {
            lines.retain(|line| line.width() > 0);
        }
        self.card(f, body, "Welcome to Yo", lines)
    }

    /// Sign up: the name, a one-line hint, the person toggle; the bot warning only while it is off.
    fn draw_signup(&self, f: &mut Frame, body: Rect) {
        let count = self.input.chars().count();
        let mut lines = vec![
            self.field("Name", &self.input),
            Line::from("12+ characters").style(if count >= 12 { self.fg(Color::Green) } else { self.dim() }),
            Line::from(""),
            Line::from(vec![
                Span::styled(if self.human { "[x]" } else { "[ ]" }, Style::new().bold()),
                Span::raw(" I'm a person  "),
                Span::styled("tab", self.fg(KEY).add_modifier(Modifier::BOLD)),
            ]),
        ];
        if !self.human {
            lines.push(Line::from("Bots can't become people later.").style(self.dim()));
        }
        let inner = render_card(f, body, "Sign Up", lines, self.dim());
        if inner.height > 3 {
            self.hits.borrow_mut().push((Rect { y: inner.y + 3, height: 1, ..inner }, Hit::Toggle));
        }
    }

    fn draw_recovery(&self, f: &mut Frame, body: Rect) {
        let mut lines = if self.shown_key.is_empty() {
            vec![Line::from("No Recovery Key saved here.").style(self.dim())]
        } else {
            self.key_lines(body.width)
        };
        lines.push(Line::from(""));
        lines.push(if self.confirm_rotate {
            Line::from(vec![
                Span::raw("A new key replaces the old one; the old key and any passkey stop working.  "),
                Span::styled("y", self.fg(KEY).add_modifier(Modifier::BOLD)),
                Span::styled(" continue", self.dim()),
            ])
        } else {
            Line::from(vec![
                Span::styled("R", self.fg(KEY).add_modifier(Modifier::BOLD)),
                Span::styled("  new key", self.dim()),
            ])
        });
        self.section(f, body, "Recovery Key", lines)
    }

    /// One bar per friend in the app's colours, with room for the marks and count before the name. The bar under the cursor brighter and edged; the add bar last, the field while typing.
    fn draw_bars(&self, f: &mut Frame, area: Rect) {
        let rows = self.snapshot.rows.len() + 1;
        // Three rows a bar, the name in its middle, when the whole list fits so; else one row a bar.
        let height = if rows * 3 <= usize::from(area.height) { 3 } else { 1 };
        let per = (area.height / height).max(1) as usize;
        let focus = if self.screen == Screen::Add { rows - 1 } else { self.selected };
        let first = (focus + 1).saturating_sub(per);
        let mut y = area.y;
        for (i, row) in self.snapshot.rows.iter().enumerate().skip(first) {
            if y + height > area.y + area.height {
                break;
            }
            let rect = Rect { x: area.x, y, width: area.width, height };
            let stored = self.items.get(&row.id).and_then(|item| item["color"].as_u64());
            let index = stored.map_or(i, |v| v as usize);
            let color = BAR_COLORS[index % BAR_COLORS.len()];
            // fade_elapsed starts at zero after the solid period. Interpolate between core deadlines.
            let flash = if matches!(row.line, yo_ui::list::Line::Title) || row.yo_count == 0 {
                0.0
            } else if let Some(elapsed) = row.fade_elapsed {
                let delta = if row.clock_running { self.now() - self.snapshot_at } else { 0 };
                1.0 - ((elapsed + delta) as f32 / YO_FADE_MS as f32).min(1.0)
            } else {
                1.0
            };
            self.bar(
                f.buffer_mut(),
                rect,
                color,
                row_text(row, usize::from(rect.width.saturating_sub(6)), self.photos.get(&row.id).copied()),
                i == self.selected && self.screen == Screen::Main,
                flash,
            );
            self.hits.borrow_mut().push((rect, Hit::Bar(i)));
            y += height;
            if row.request && i == self.selected && self.screen == Screen::Main && y < area.bottom() {
                // A person's request can be blocked too (b); a group's not.
                let block = !row.id.starts_with(yo_core::shared::GROUP_PREFIX);
                let parts = if block { 3 } else { 2 };
                let no = Rect::new(area.x, y, area.width / parts, 1);
                let mid = Rect::new(no.right(), y, if block { area.width / parts } else { 0 }, 1);
                let yes = Rect::new(mid.right(), y, area.width - no.width - mid.width, 1);
                let mut choices = vec![(no, ALIZARIN, "No  x", "decline"), (yes, color, "Yo  enter", "accept")];
                if block {
                    choices.insert(1, (mid, 0x5c5c5c, "Block  b", "block"));
                }
                for (rect, color, label, verb) in choices {
                    self.bar(f.buffer_mut(), rect, color, BarText::Center(label.into()), false, 0.0);
                    self.hits.borrow_mut().push((rect, Hit::Choice(i, verb)));
                }
                y += 1;
            }
        }
        if y + height <= area.y + area.height {
            let rect = Rect { x: area.x, y, width: area.width, height };
            let color = BAR_COLORS[self.snapshot.rows.len() % BAR_COLORS.len()];
            let text = if self.screen == Screen::Add {
                BarText::Center(format!(
                    "{}▏",
                    tail(&self.input.to_uppercase(), usize::from(area.width.saturating_sub(8)))
                ))
            } else {
                BarText::Center("+ ADD".into())
            };
            let selected = self.screen == Screen::Add || self.selected == self.snapshot.rows.len();
            self.bar(f.buffer_mut(), rect, color, text, selected, 0.0);
            self.hits.borrow_mut().push((rect, Hit::AddBar));
            y += height;
        }
        if self.snapshot.rows.is_empty() && y + 2 <= area.y + area.height {
            let hint = Line::from("No friends yet. Press a to add one (try PING).").style(self.dim());
            f.render_widget(Paragraph::new(hint).alignment(Alignment::Center), Rect { y: y + 1, height: 1, ..area });
        }
    }

    fn bar(&self, buf: &mut Buffer, rect: Rect, color: u32, text: BarText, selected: bool, flash: f32) {
        let shade = lighten(color, flash);
        let incoming = flash >= 0.5;
        // The selected bar as terminal lists mark it: reversed (white, the bar's colour as its text) and "▶".
        let style = if self.color && selected {
            Style::new().bg(Color::White).fg(self.rgb(color))
        } else if self.color {
            Style::new().bg(self.rgb(shade)).fg(if incoming { Color::Black } else { Color::White })
        } else if selected || incoming {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        }
        .add_modifier(Modifier::BOLD);
        buf.set_style(rect, style);
        if selected {
            buf.set_string(rect.x, rect.y + rect.height / 2, "▶", style);
        }
        let label = match &text {
            BarText::Center(t) => t.as_str(),
            BarText::Split(lead, _) => lead.as_str(),
        };
        let label_width = Span::raw(label).width();
        if label_width > usize::from(rect.width)
            && (matches!(text, BarText::Split(..)) || label.starts_with("Sending ") || label.starts_with("Sent "))
        {
            Paragraph::new("Make the window bigger").style(style).render(rect, buf);
            return;
        }
        let pad = 3.min(usize::from(rect.width).saturating_sub(label_width) / 2) as u16;
        let inner = Rect { x: rect.x + pad, y: rect.y + rect.height / 2, width: rect.width - pad * 2, height: 1 };
        let room = inner.width as usize;
        match text {
            BarText::Center(t) => {
                let t = cut(&t, room);
                Paragraph::new(t).style(style).alignment(Alignment::Center).render(inner, buf);
            }
            BarText::Split(lead, name) => {
                let left = inner.width.saturating_sub(Span::raw(&lead).width() as u16 + 1);
                Paragraph::new(lead).style(style).render(inner, buf);
                if left > 0 {
                    let tail = Rect { x: inner.right() - left, width: left, ..inner };
                    Paragraph::new(cut(&name, left.into())).style(style).alignment(Alignment::Right).render(tail, buf);
                }
            }
        }
    }

    fn draw_info(&self, f: &mut Frame, area: Rect, id: &str) {
        let title = settings::SECTIONS.iter().flat_map(|s| s.items).find(|i| i.id == id).map_or("", |i| i.title);
        let lines = match id {
            "profile" => {
                let url = format!("https://justyo.co/{}", upper(&self.me["username"]));
                let qr = signin::qr(&url);
                let mut lines = wrap_card(&url, area.width);
                if qr.lines().count() + lines.len() + 2 <= usize::from(area.height)
                    && qr.lines().all(|row| Span::raw(row).width() + 2 <= usize::from(area.width.min(72)))
                {
                    lines.splice(0..0, qr.lines().map(|row| Line::from(row.to_owned())));
                }
                self.card(f, area, title, lines);
                return;
            }
            "about" => vec![
                format!("yo {} · core {} · protocol {}", crate::VERSION, yo_core::VERSION, yoip::VERSION),
                String::new(),
                format!("Terms    {}", settings::TERMS_URL),
                format!("Privacy  {}", settings::PRIVACY_URL),
                String::new(),
                "Licenses: MIT; libraries under their own (see the repository)".into(),
            ],
            "feedback" => vec![format!("Feedback: {FEEDBACK}")],
            _ => vec![],
        };
        self.card(f, area, title, lines.iter().flat_map(|line| wrap_card(line, area.width)).collect());
    }

    fn draw_blocked(&self, f: &mut Frame, area: Rect) {
        heading(f.buffer_mut(), area, "Blocked", self.dim());
        if self.blocked.is_empty() {
            f.render_widget(Paragraph::new("No Blocked Contacts"), Rect { y: area.y + 1, height: 1, ..area });
        }
        let first = (self.selected + 1).saturating_sub(usize::from(area.height - 1));
        for ((i, contact), y) in self.blocked.iter().enumerate().skip(first).zip(area.y + 1..area.bottom()) {
            let name = contact["name"]
                .as_str()
                .or_else(|| contact["handles"][0].as_str())
                .or_else(|| contact["user"].as_str())
                .unwrap_or("");
            let gutter = if i == self.selected { "▌ " } else { "  " };
            let rect = Rect { y, height: 1, ..area };
            f.render_widget(Paragraph::new(format!("{gutter}{}", name.to_uppercase())), rect);
            self.hits.borrow_mut().push((rect, Hit::Blocked(i)));
        }
    }

    /// Settings as boxes from the core's menu (yo_ui::settings): a rounded box a section, its title in the top
    /// border, a row a line (label left, value right), the selected row reversed with "▶"; two columns when wide.
    fn draw_settings(&self, f: &mut Frame, area: Rect) {
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        let on = |b: bool| if b { "ON" } else { "OFF" };
        let boxes = settings_boxes(self.devices.len(), self.connections.len());
        let two = area.width >= 100 && boxes.len() > 2;
        let width = if two { (area.width.saturating_sub(2) / 2).min(50) } else { area.width.min(56) };
        let total = if two { width * 2 + 2 } else { width };
        let left = area.x + area.width.saturating_sub(total) / 2;
        let inner = usize::from(width.saturating_sub(4));
        // Each box as lines: (text spans, the row's index when selectable, its style when selected).
        let mut columns: Vec<Vec<(Line<'static>, Option<usize>)>> = vec![Vec::new(), Vec::new()];
        let mut index = 0;
        let split = boxes.len().div_ceil(2);
        for (b, (title, rows)) in boxes.iter().enumerate() {
            let column = &mut columns[usize::from(two && b >= split)];
            if !column.is_empty() {
                column.push((Line::from(""), None));
            }
            let fill = inner.saturating_sub(title.chars().count() + 1);
            column.push((
                Line::from(vec![
                    Span::styled("╭─ ", self.dim()),
                    Span::styled(title.to_string(), Style::new().bold()),
                    Span::styled(format!(" {}╮", "─".repeat(fill)), self.dim()),
                ]),
                None,
            ));
            let edge = |line: Vec<Span<'static>>| {
                let mut spans = vec![Span::styled("│ ", self.dim())];
                spans.extend(line);
                spans.push(Span::styled(" │", self.dim()));
                Line::from(spans)
            };
            for row in rows {
                let selected = index == self.selected;
                let (label, value, value_style, label_style) = self.setting_text(*row, now, on);
                let marker = if selected { "▶ " } else { "  " };
                let pad = inner.saturating_sub(2 + Span::raw(&label).width() + Span::raw(&value).width());
                let label = cut(&label, inner.saturating_sub(3 + Span::raw(&value).width()));
                let base = if selected {
                    Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                } else {
                    Style::new()
                };
                column.push((
                    edge(vec![
                        Span::styled(marker, base.patch(self.accent())),
                        Span::styled(label, base.patch(label_style)),
                        Span::styled(" ".repeat(pad.max(1)), base),
                        Span::styled(value, base.patch(value_style)),
                    ]),
                    Some(index),
                ));
                // What a row opens shows right under it while it is selected: the code's QR, how to link.
                if selected {
                    for extra in self.setting_inline(*row, inner) {
                        column.push((edge(vec![Span::raw(extra)]), None));
                    }
                }
                index += 1;
            }
            column.push((Line::from(Span::styled(format!("╰{}╯", "─".repeat(inner + 2)), self.dim())), None));
        }
        // Scrolled so the selected row (and what shows under it) is in view.
        let at = columns.iter().find_map(|c| c.iter().position(|(_, i)| *i == Some(self.selected))).unwrap_or(0);
        let height = usize::from(area.height);
        let first = (at + 6).saturating_sub(height).min(at);
        for (c, column) in columns.iter().enumerate() {
            let x = left + if c == 1 { width + 2 } else { 0 };
            for ((line, row), y) in column.iter().skip(first).zip(area.y..area.bottom()) {
                f.buffer_mut().set_line(x, y, line, width);
                if let Some(i) = row {
                    self.hits.borrow_mut().push((Rect { x, y, width, height: 1 }, Hit::Setting(*i)));
                }
            }
        }
    }

    /// A settings row's label and value, and their styles.
    fn setting_text(
        &self,
        row: SettingsRow,
        now: i64,
        on: impl Fn(bool) -> &'static str,
    ) -> (String, String, Style, Style) {
        let value_style = |v: &str| match v {
            "ON" => self.fg(Color::Green).bold(),
            "OFF" | "›" => self.dim(),
            _ => self.fg(ACCENT),
        };
        let (label, value) = match row {
            SettingsRow::Connect => ("Connect a Service…".into(), "›".into()),
            SettingsRow::Decline => ("Decline a Service…".into(), "›".into()),
            SettingsRow::Connection(i) => {
                let c = &self.connections[i];
                let state = ["ending", "refused", "connected", "waiting"]
                    .into_iter()
                    .find(|k| c[*k] == true)
                    .unwrap_or("disconnected");
                (format!("{} · {state}", upper(&c["name"])), "›".into())
            }
            SettingsRow::Device(d) => {
                let device = &self.devices[d];
                let seen = active(device["last_seen_at"].as_i64().unwrap_or(0), now);
                let label = if device["this"] == true {
                    "This computer".to_owned()
                } else {
                    format!("{} ({})", device_label(device["app"].as_str().unwrap_or("")), device["number"])
                };
                (label, seen)
            }
            SettingsRow::Item(item) => match item.id {
                "profile" => {
                    (bar_title(&upper(&self.me["username"]), false, self.me["bot"] == true), "Your Code ›".into())
                }
                "bot" => (item.title.into(), on(self.me["bot"] == true).into()),
                "requests" => (item.title.into(), on(self.requests_open).into()),
                "blocked" if !self.blocked.is_empty() => (item.title.into(), format!("{} ›", self.blocked.len())),
                "about" => (item.title.into(), format!("{} ›", crate::VERSION)),
                "status" => {
                    let (dot, word, color) = if !self.listener_status.is_empty() {
                        ("●", "stopped", Color::Red)
                    } else if self.snapshot.banner.is_some() {
                        ("●", "offline", Color::Yellow)
                    } else {
                        ("●", "online", Color::Green)
                    };
                    return (item.title.into(), format!("{dot} {word}"), self.fg(color).bold(), Style::new());
                }
                "sign_out" => return (item.title.into(), String::new(), Style::new(), self.fg(Color::Red).bold()),
                _ => (item.title.into(), "›".into()),
            },
        };
        let style = value_style(&value);
        (label, value, style, Style::new())
    }

    /// What a selected row shows under it: the code as a QR (when it fits), how a new device is linked.
    fn setting_inline(&self, row: SettingsRow, inner: usize) -> Vec<String> {
        let SettingsRow::Item(item) = row else { return Vec::new() };
        let pad = |text: &str| {
            let text = cut(text, inner);
            let room = inner.saturating_sub(Span::raw(&text).width());
            format!("{}{text}{}", " ".repeat(room / 2), " ".repeat(room - room / 2))
        };
        match item.id {
            "profile" if self.show_code => {
                let url = format!("https://justyo.co/{}", upper(&self.me["username"]));
                let qr = signin::qr(&url);
                let mut lines = vec![pad("")];
                if qr.lines().all(|l| Span::raw(l).width() <= inner) {
                    lines.extend(qr.lines().map(pad));
                }
                lines.push(pad(&url));
                lines.push(pad(""));
                lines
            }
            "link_device" => vec![
                pad(""),
                pad("On the new device: yo signin --link"),
                pad("(or in the app: Sign In with Another Device)."),
                pad("Then press enter here and paste the link it shows."),
                pad(""),
            ],
            _ => Vec::new(),
        }
    }

    fn draw_help(&self, f: &mut Frame, area: Rect) {
        let key = |k: &str, what: &str| {
            Line::from(vec![
                Span::styled(format!("{k:>9}"), self.fg(KEY).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {what:<28}"), Style::new()),
            ])
        };
        let head = |t: &str| Line::from(Span::styled(format!("{t:<39}"), self.accent()));
        let lines = if !self.signed() {
            vec![
                head("Welcome"),
                key("1 s", "sign up"),
                key("2 r", "sign in with the recovery key"),
                key("3 l", "sign in with another device"),
                Line::from(""),
                key("tab", "person/bot, when signing up"),
                key("esc", "back"),
                key("?", "this help"),
                key("q", "quit"),
            ]
        } else {
            vec![
                head("Friends"),
                key("enter", "send a Yo (or click a bar)"),
                key("p / i / m", "photo / info / menu"),
                key("a +", "add a friend or group"),
                key("↑↓ j k", "move"),
                Line::from(""),
                head("Requests"),
                key("enter", "accept the selected request"),
                key("x / b", "decline / block"),
                key("click", "select a request"),
                Line::from(""),
                head("Settings"),
                key("s", "Settings"),
                key("enter", "open a row or toggle"),
                key("?", "this help"),
                key("esc q", "back, quit"),
            ]
        };
        // Scrolled in a small window (↑↓, the wheel): the card shows what fits from there.
        let room = usize::from(area.height.saturating_sub(4)).max(1);
        self.help_max.set(lines.len().saturating_sub(room));
        let first = self.help_scroll.min(self.help_max.get());
        let mut shown: Vec<Line> = lines.into_iter().skip(first).collect();
        if first > 0 && !shown.is_empty() {
            shown[0] = Line::from("↑ more").style(self.dim());
        }
        self.card(f, area, "Keys", shown)
    }
}

fn diagnostics(config: &Config) -> Value {
    let mut lines = match crate::execute(config, &words(&["status"])) {
        Ok(s) => vec![
            if s["listening"] == true { "Receiving" } else { "Not receiving" }.into(),
            format!("Hub       {}", s["hub"].as_str().unwrap_or("—")),
            format!("Contacts  {} · {} friends · {} groups", s["contacts"], s["friends"], s["groups"]),
            format!("Unsent    {} · {} dropped", s["waiting"], s["dropped"]),
            format!("Bot       {}", if s["bot"] == true { "yes" } else { "no" }),
        ],
        Err(f) => vec![format!("Status: {}", f.error)],
    };
    // Asked after (Job::Probe): the PING and the checks can take seconds.
    lines.push("PING …".into());
    json!(lines)
}

/// The Status card's slow part: the PING's answer, then the checks.
fn probe_and_checks(config: &Config) -> Value {
    let mut lines = vec![match crate::execute(config, &words(&["probe", "--timeout-ms", "5000"])) {
        Ok(v) if v["ms"].is_number() => format!("PING answered in {} ms", v["ms"]),
        Ok(_) => "PING: no answer".to_owned(),
        Err(f) => format!("PING: {}", f.error),
    }];
    for check in crate::doctor(config)["checks"].as_array().into_iter().flatten() {
        let mark = if check["ok"] == true { "✓" } else { "✗" };
        lines.push(format!(
            "{mark} {}  {}",
            check["check"].as_str().unwrap_or(""),
            check["detail"].as_str().unwrap_or("")
        ));
    }
    json!(lines)
}

fn wrap_card(text: &str, width: u16) -> Vec<Line<'static>> {
    // Inside the card's border with a little room on each side.
    let width = usize::from(width.min(72).saturating_sub(6)).max(1);
    // A line that fits keeps its spacing (labels and values in columns).
    if text.chars().count() <= width {
        return vec![Line::from(text.to_owned())];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + word.chars().count() + 1 > width {
            lines.push(Line::from(std::mem::take(&mut line)));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        for c in word.chars() {
            if line.chars().count() == width {
                lines.push(Line::from(std::mem::take(&mut line)));
            }
            line.push(c);
        }
    }
    lines.push(Line::from(line));
    lines
}

fn open_urls(urls: &'static [&'static str]) {
    std::thread::spawn(move || {
        for url in urls {
            for opener in ["xdg-open", "open"] {
                if Command::new(opener)
                    .arg(url)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok()
                {
                    break;
                }
            }
        }
    });
}

/// Keep every content row inside the frame, reserving an overflow indicator when needed.
fn render_card(f: &mut Frame, area: Rect, title: &str, mut lines: Vec<Line>, border: Style) -> Rect {
    let content = lines.iter().map(Line::width).max().unwrap_or(0).max(title.len());
    let width = (content.saturating_add(6).clamp(40, 72) as u16).min(area.width);
    let height = (lines.len().saturating_add(4).min(usize::from(area.height))) as u16;
    let rect = Rect::new(area.x + (area.width - width) / 2, area.y + (area.height - height) / 2, width, height);
    f.render_widget(Clear, rect);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title(Span::styled(format!(" {title} "), Style::new().bold()))
        .title_alignment(Alignment::Center);
    let mut inner = block.inner(rect);
    f.render_widget(block, rect);
    if usize::from(inner.height) >= lines.len() + 2 {
        inner.y += 1;
        inner.height -= 2;
    }
    if lines.len() > usize::from(inner.height) {
        lines.truncate(usize::from(inner.height));
        if let Some(last) = lines.last_mut() {
            *last = Line::from("…");
        }
    }
    // Left-aligned in a column centred in the card: labels and values line up.
    let column = lines.iter().map(Line::width).max().unwrap_or(0).min(usize::from(inner.width)) as u16;
    let inner = Rect { x: inner.x + (inner.width - column) / 2, width: column, ..inner };
    f.render_widget(Paragraph::new(lines), inner);
    inner
}

enum BarText {
    Center(String),
    /// The marks and Yo count left, never cut; the name right, cut first.
    Split(String, String),
}

fn upper(value: &Value) -> String {
    value.as_str().unwrap_or("").to_uppercase()
}

/// A bar's line as the core gives it: the name or a send's words, else the Yos ("2× Yo from" first, never cut; the
/// name last, cut first), a photo's or video's sign after them.
fn row_text(row: &Row, room: usize, media: Option<Kind>) -> BarText {
    use yo_ui::list::Line;
    let (mut lead, words) = match &row.line {
        // A photo's or video's send says so ("Sent 📷!"), as the app's bar shows its picture.
        Line::Title if row.title == yo_ui::Text::SentYo.get(English) => {
            let mark = match media {
                Some(Kind::Photo) => "📷".to_owned(),
                Some(Kind::Video) => "🎥".to_owned(),
                _ => yo_ui::counted(row.count.into()),
            };
            (String::new(), yo_ui::send_words(&mark, row.sending, English))
        }
        Line::Title => (String::new(), row.title.clone()),
        Line::Arrival { count, name } => {
            let prefix = yo_ui::arrival_line(&yo_ui::counted((*count).into()), "", English);
            (prefix.trim_end().into(), name.clone())
        }
        Line::Senders { senders } => {
            let names = senders.iter().map(|s| (s.name.clone(), s.count)).collect::<Vec<_>>();
            let flow = yo_ui::line::sender_flow(&names, English);
            let detail = flow.tokens.join(&flow.separator);
            if Span::raw(format!("{}{detail} {}", flow.prefix, row.media_sign)).width() <= room {
                (flow.prefix.trim_end().into(), detail)
            } else {
                let total = senders.iter().map(|s| i64::from(s.count)).sum();
                let prefix = yo_ui::arrival_line(&yo_ui::counted(total), "", English);
                (prefix.trim_end().into(), senders.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "))
            }
        }
    };
    if !row.media_sign.is_empty() {
        lead.push_str(&format!(" {}", row.media_sign));
    }
    if lead.is_empty() {
        BarText::Center(words)
    } else {
        BarText::Split(lead.trim_start().into(), words)
    }
}

/// A path with ~ for home, its start cut to fit.
fn short_path(path: &std::path::Path, room: usize) -> String {
    let mut text = path.display().to_string();
    if let Some(home) = std::env::var_os("HOME").map(|h| h.to_string_lossy().into_owned()) {
        if let Some(rest) = text.strip_prefix(&home) {
            text = format!("~{rest}");
        }
    }
    let n = text.chars().count();
    if n <= room {
        return text;
    }
    "…".to_owned() + &text.chars().skip(n + 1 - room).collect::<String>()
}

/// The end of a text being typed, in `room` columns: what is cut goes at its start ("…the end").
fn tail(text: &str, room: usize) -> String {
    let n = text.chars().count();
    if n <= room {
        return text.to_owned();
    }
    "…".to_owned() + &text.chars().skip(n + 1 - room.max(1)).collect::<String>()
}

/// The text cut to `room` columns, an ellipsis where it was cut.
fn cut(text: &str, room: usize) -> String {
    if text.chars().count() <= room {
        return text.to_owned();
    }
    text.chars().take(room.saturating_sub(1)).collect::<String>() + "…"
}

/// A rule the whole width, a label set into its left end when there is one.
fn rule(buf: &mut Buffer, area: Rect, style: Style, label: Option<Span>) {
    let mut spans = Vec::new();
    let mut used = 0;
    if let Some(label) = label {
        spans.push(Span::styled("──", style));
        used = 2 + label.width();
        spans.push(label);
    }
    spans.push(Span::styled("─".repeat((area.width as usize).saturating_sub(used)), style));
    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
}

/// A section's heading: the label in bold, then a rule to the end ("Previously Seen ────").
fn heading(buf: &mut Buffer, area: Rect, title: &str, dim: Style) {
    buf.set_line(area.x, area.y, &heading_line(area.width, title, dim), area.width);
}

fn heading_line(width: u16, title: &str, dim: Style) -> Line<'static> {
    let rest = (width as usize).saturating_sub(title.chars().count() + 1);
    Line::from(vec![
        Span::styled(title.to_owned(), Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled("─".repeat(rest), dim),
    ])
}

/// The colour a share of the way to white.
fn lighten(hex: u32, by: f32) -> u32 {
    let ch = |shift: u32| {
        let c = ((hex >> shift) & 0xff) as f32;
        ((c + (255.0 - c) * by) as u32).min(255)
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// The nearest of xterm's 256 colours: the 6×6×6 cube or the gray ramp.
fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const STEPS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let near = |c: u8| STEPS.iter().enumerate().min_by_key(|(_, s)| (**s - c as i32).abs()).map_or(0, |(i, _)| i);
    let (ri, gi, bi) = (near(r), near(g), near(b));
    let cube = (STEPS[ri], STEPS[gi], STEPS[bi]);
    let dist = |(x, y, z): (i32, i32, i32)| (x - r as i32).pow(2) + (y - g as i32).pow(2) + (z - b as i32).pow(2);
    let avg = (r as i32 + g as i32 + b as i32) / 3;
    let gray_i = ((avg - 8).max(0) / 10).min(23);
    let gray = 8 + gray_i * 10;
    if dist((gray, gray, gray)) < dist(cube) {
        232 + gray_i as u8
    } else {
        16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8
    }
}

/// A program that takes text for the clipboard, if one is here.
fn clipboard() -> Option<Vec<&'static str>> {
    let path = std::env::var_os("PATH")?;
    let has = |tool: &str| std::env::split_paths(&path).any(|d| d.join(tool).is_file());
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    [
        (wayland, vec!["wl-copy"]),
        (x11, vec!["xclip", "-selection", "clipboard"]),
        (x11, vec!["xsel", "--clipboard", "--input"]),
        (true, vec!["pbcopy"]),
    ]
    .into_iter()
    .find(|(on, tool)| *on && has(tool[0]))
    .map(|(_, tool)| tool)
}

/// The link flow off the screen: the text shown first (as an event), then waiting in steps until done or cancelled.
fn link(config: &Config, tx: &Sender<Msg>, cancel: &AtomicBool) -> Result<Value, Failure> {
    let request = signin::link_start(config)?;
    let _ = tx.send(Msg::LinkOffer(request.text.clone()));
    let started = Instant::now();
    while started.elapsed() < signin::LINK_WAIT && !cancel.load(Ordering::SeqCst) {
        if let Some(done) = signin::link_step(config, &request, 10)? {
            return Ok(done);
        }
    }
    Err(Failure::new("No answer from the other device"))
}

/// The key in capitals, in groups of four.
fn grouped(key: &str) -> Vec<String> {
    key.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).to_uppercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(app: &App, w: u16, h: u16) -> Vec<String> {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect()).collect()
    }

    #[test]
    fn link_card_keeps_the_complete_offer_with_or_without_qr() {
        let config = Config::parse(&[]).unwrap().0;
        let mut app = App::new(&config, channel().0);
        app.screen = Screen::LinkWait;
        app.link_text = format!("yo-link:{}.{}", "a".repeat(22), "b".repeat(43));
        for (w, h, qr) in [(120, 35, true), (80, 24, false), (40, 12, false)] {
            let rows = screen(&app, w, h);
            let text: String = rows.iter().filter_map(|r| r.split('│').nth(1)).map(str::trim).collect();
            assert!(text.contains(&app.link_text), "{rows:?}");
            assert_eq!(text.contains('█'), qr);
            if !qr {
                assert!(text.contains("Make the window bigger"));
            }
            assert!(rows.iter().any(|row| row.contains('╰') && row.contains('╯')));
        }
    }

    #[test]
    fn narrow_key_card_shows_every_group_inside_its_border() {
        let mut app = App::new(&Config::parse(&[]).unwrap().0, channel().0);
        app.screen = Screen::ShowKey;
        app.shown_key = "abcd".repeat(16);
        for (w, h) in [(40, 12), (22, 30)] {
            let rows = screen(&app, w, h);
            let inside = rows.iter().filter_map(|row| row.split('│').nth(1)).collect::<String>();
            assert_eq!(inside.matches("ABCD").count(), 16, "{rows:?}");
            assert!(rows.iter().any(|r| r.contains('╰') && r.contains('╯')));
        }
        assert!(screen(&app, 22, 5).join("").contains("Make the window bigger"));
    }

    #[test]
    fn small_window_keeps_interactive_rows_in_view() {
        let config = Config::parse(&[]).unwrap().0;
        let mut app = App::new(&config, channel().0);
        app.apply(Input::Friends(
            (0..10)
                .map(|i| Friend { id: i.to_string(), name: format!("FRIEND{i}"), request: false, group: false })
                .collect(),
        ));
        app.selected = 8;
        app.screen = Screen::Main;
        screen(&app, 40, 12);
        assert!(app.hits.borrow().iter().any(|(_, hit)| matches!(hit, Hit::Bar(8))));
        app.screen = Screen::Add;
        app.input = "PIN".into();
        assert!(screen(&app, 40, 12).join("").contains("PIN▏"));
        app.screen = Screen::Settings;
        let ids: Vec<_> = selectable_settings(0, 0)
            .filter_map(|row| match row {
                SettingsRow::Item(item) => Some(item.id),
                _ => None,
            })
            .collect();
        assert_eq!(
            ids,
            settings::sections(Platform::Cli)
                .into_iter()
                .flat_map(|(_, items)| items.into_iter().map(|i| i.id))
                .filter(|id| *id != "devices")
                .collect::<Vec<_>>()
        );
        assert!(["passkey", "look", "delete_account"].iter().all(|id| !ids.contains(id)));
        app.selected = selectable_settings(0, 0)
            .position(|row| matches!(row, SettingsRow::Item(item) if item.id == "link_device"))
            .unwrap();
        let rows = screen(&app, 40, 12);
        assert!(rows.iter().any(|r| r.contains("▶ Link a New Device")));
        assert!(app.hits.borrow().iter().any(|(_, h)| matches!(h, Hit::Setting(i) if *i == app.selected)));
        app.help = true;
        let rows = screen(&app, 40, 12);
        assert!(rows.iter().any(|r| r.contains("│") && r.contains("…")));
        assert!(rows.iter().any(|r| r.contains("╰") && r.contains("╯")));
    }

    #[test]
    fn clicking_a_request_selects_it_without_sending_or_accepting() {
        let mut app = App::new(&Config::parse(&[]).unwrap().0, channel().0);
        app.screen = Screen::Main;
        app.requests = vec![json!({"user":"request", "name":"stranger"})];
        app.refresh(json!([]));
        app.refresh(json!([{"target":"friend","name":"friend"}]));
        app.selected = 0;
        assert!(!app.snapshot.rows[0].request);
        assert!(app.snapshot.rows[1].request);
        let rows = screen(&app, 80, 24);
        let rect = app.hits.borrow().iter().find(|(_, hit)| matches!(hit, Hit::Bar(1))).unwrap().0;
        assert_eq!(rect.height, 3);
        let title = yo_ui::request_row("STRANGER", false, English);
        assert!(rows[usize::from(rect.y + 1)].contains(&title));
        assert!(!app.hits.borrow().iter().any(|(_, hit)| matches!(hit, Hit::Choice(..))));
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected, 1);
        assert_eq!(app.jobs, 0);
        screen(&app, 80, 24);
        assert_eq!(
            app.hits.borrow().iter().filter(|(_, hit)| matches!(hit, Hit::Choice(1, _))).count(),
            3,
            "no, block, yo"
        );
        for key in [KeyCode::Tab, KeyCode::BackTab, KeyCode::Char('2')] {
            app.key(KeyEvent::new(key, KeyModifiers::NONE));
            assert_eq!(app.screen, Screen::Main);
        }
    }

    #[test]
    fn arrival_and_selection_share_the_first_screen_when_they_fit() {
        let mut app = App::new(&Config::parse(&[]).unwrap().0, channel().0);
        app.screen = Screen::Main;
        app.apply(Input::Friends(
            (0..10)
                .map(|i| Friend { id: i.to_string(), name: format!("FRIEND{i}"), request: false, group: false })
                .collect(),
        ));
        screen(&app, 80, 24);
        let capacity = app.hits.borrow().iter().filter(|(_, hit)| matches!(hit, Hit::Bar(_))).count();
        app.selected = capacity - 2;
        let selected = app.snapshot.rows[app.selected].id.clone();
        let sender = app.snapshot.rows.last().unwrap().id.clone();
        app.received(&json!({"reply_to": sender}));
        app.clock -= Duration::from_millis(app.snapshot.next_tick.unwrap());
        app.apply(Input::Tick);
        assert_eq!(app.snapshot.rows[0].id, sender);
        assert_eq!(app.snapshot.rows[app.selected].id, selected);
        screen(&app, 80, 24);
        for index in [0, app.selected] {
            assert!(app.hits.borrow().iter().any(|(_, hit)| matches!(hit, Hit::Bar(i) if *i == index)));
        }
    }

    #[test]
    fn bot_title_and_arrival_count_come_from_the_list() {
        let mut list = List::new();
        let friend = Friend { id: "p".into(), name: bar_title("PING", false, true), request: false, group: false };
        let snapshot = list.apply(0, Input::Friends(vec![friend]));
        assert!(matches!(row_text(&snapshot.rows[0], 40, None), BarText::Center(t) if t == "/PING"));
        for token in ["one", "two"] {
            list.apply(1, Input::Received { id: "p".into(), kind: Kind::Yo, token: token.into(), sender: None });
        }
        assert!(matches!(row_text(&list.snapshot().rows[0], 40, None),
            BarText::Split(lead, name) if lead == "2× Yo from" && name == "/PING"));
    }

    #[test]
    fn test_round_fixes_hold() {
        let mut app = App::new(&Config::parse(&[]).unwrap().0, channel().0);
        // The rotate confirmation: any key but y cancels it.
        app.screen = Screen::Recovery;
        app.key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE));
        assert!(app.confirm_rotate);
        app.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        assert!(!app.confirm_rotate);
        app.key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(app.jobs, 0, "no rotation");
        // The Welcome help names its own keys.
        app.screen = Screen::Welcome;
        app.help = true;
        let text = screen(&app, 60, 20).join("\n");
        assert!(text.contains("sign in with the recovery key") && !text.contains("settings"), "{text}");
        // Signed in, the help scrolls at 60x20 down to its last line.
        app.screen = Screen::Main;
        screen(&app, 60, 20);
        for _ in 0..30 {
            app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        assert!(screen(&app, 60, 20).join("").contains("back, quit"));
        app.help = false;
        // A copy from a linked device: "Sent Yo!"; a photo: "Sent 📷!".
        app.me = json!({"user": "me"});
        app.refresh(json!([{"target":"f1","kind":"friend","name":"bravo"},{"target":"f2","kind":"friend","name":"carol","blocked":true}]));
        app.event(json!({"type":"sent","user":"f1","content":"yo"}));
        let row = app.snapshot.rows.iter().find(|r| r.id == "f1").unwrap().clone();
        assert!(matches!(row_text(&row, 40, None), BarText::Center(t) if t.starts_with("Sent")), "{row:?}");
        assert!(matches!(row_text(&row, 40, Some(Kind::Photo)), BarText::Center(t) if t == "Sent 📷!"));
        // Members: adding offers friends not in the group (never blocked ones), removing only its members.
        app.dialog =
            Some(Dialog::Members { title: "Add Members", words: vec![], friends: vec![], cursor: 0, empty: "" });
        app.members_loaded("Add Members", words(&["groups", "add", "g"]), false, &json!({"members": ["me"]}));
        let Some(Dialog::Members { friends, .. }) = &app.dialog else { panic!() };
        assert_eq!(friends.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["f1"]);
        app.members_loaded("Add Members", words(&["groups", "add", "g"]), false, &json!({"members": ["me", "f1"]}));
        let Some(Dialog::Members { friends, .. }) = &app.dialog else { panic!() };
        assert!(friends.is_empty());
        app.members_loaded(
            "Add Members",
            words(&["groups", "remove", "g"]),
            true,
            &json!({"members": ["me", "x9"], "contacts": [{"user": "x9", "handles": ["dora"]}]}),
        );
        let Some(Dialog::Members { friends, .. }) = &app.dialog else { panic!() };
        assert_eq!(friends, &[("x9".to_owned(), "DORA".to_owned(), false)]);
    }

    #[test]
    fn a_recovery_key_never_splits_a_group() {
        let key = "a".repeat(64);
        let groups = grouped(&key);
        assert_eq!(groups.len(), 16);
        assert!(groups.iter().all(|g| g.len() == 4));
    }

    #[test]
    fn the_256_fallback_is_near() {
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(cut("ANNA_SMITH", 5), "ANNA…");
    }
}
