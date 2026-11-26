//! `yo listen`: the long poll, one JSON line per event on stdout, and the directory's control socket on which the other
//! commands run meanwhile, on this client.

use crate::account::{Config, Opened};
use crate::commands::{self, Context};
use crate::{Failure, FAILED, GONE};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use yo_core::media::{self, Media};
use yo_core::{Client, Content, Event, Transport};

/// What waits is answered and sent (spec 3) as the app does when it comes to the foreground.
const MAINTENANCE: Duration = Duration::from_secs(3600);
/// The long poll's wait, in seconds.
const WAIT_S: u64 = 25;
/// Commands running at once on the control socket; more are told to come again.
const MAX_COMMANDS: usize = 32;
/// A command line on the control socket, at most.
const MAX_REQUEST: u64 = 64 * 1024;
/// Blobs waiting to be fetched into nothing (`--drain-media`); more are left to expire at the hub.
const MAX_DRAINING: usize = 64;
/// How long after printing a message the listener waits for a command to come on the socket (a bot's answer), and
/// how long at most for the commands under way to finish, before the work after a fetch takes the client again.
const GIVE_WAY: Duration = Duration::from_millis(20);
const GIVE_WAY_MAX: Duration = Duration::from_millis(500);

struct Listener<'a> {
    client: &'a Mutex<Client>,
    config: &'a Config,
    out: Mutex<std::io::Stdout>,
    manual_ack: bool,
    /// `--drain-media`: the blobs for the drainer (fetched and checked, never opened).
    drain: Option<SyncSender<Media>>,
    /// Commands on the control socket under way, and begun so far.
    running: AtomicUsize,
    begun: AtomicUsize,
}

impl Listener<'_> {
    fn client(&self) -> std::sync::MutexGuard<'_, Client> {
        self.client.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// One line, flushed; the reader gone ends the listener.
    fn line(&self, value: &Value) {
        if !crate::mode().json {
            let Some(text) = self.text(value) else { return };
            let mut out = self.out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = writeln!(out, "{text}").and_then(|()| out.flush());
            return;
        }
        let mut out = self.out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if writeln!(out, "{value}").and_then(|()| out.flush()).is_err() {
            std::process::exit(FAILED);
        }
    }

    /// An event for a person on a terminal: who by name (when the client is free to ask), none for the quiet ones.
    fn text(&self, value: &Value) -> Option<String> {
        let name = |user: &str| -> String {
            let found = self.client.try_lock().ok().and_then(|c| {
                c.contacts().into_iter().find(|x| x.user == user).and_then(|x| x.handles.first().cloned().or(x.name))
            });
            found.unwrap_or_else(|| user.chars().take(8).collect()).to_uppercase()
        };
        let time = chrono_free_time();
        match value["type"].as_str()? {
            "ready" => Some(format!(
                "Listening as {} (Ctrl-C stops)",
                value["username"].as_str().unwrap_or("?").to_uppercase()
            )),
            "message" => {
                let what = match value["content"].as_str() {
                    Some("photo") => "Yo + photo",
                    Some("video") => "Yo + video",
                    _ => "Yo",
                };
                let from = name(value["from"].as_str().unwrap_or(""));
                let group = value["group"].as_str().map(|g| format!(" in group:{g}")).unwrap_or_default();
                Some(format!("{time}  {what} from {from}{group}"))
            }
            "new_chat" => Some(format!("{time}  new chat: {}", name(value["user"].as_str().unwrap_or("")))),
            "probe" | "connect" | "decline" | "disconnect" => Some(format!("{time}  {value}")),
            _ => None,
        }
    }

    /// The events printed, then acknowledged.
    fn emit(&self, events: Vec<Event>) {
        self.print_events(&events);
        self.acknowledge(&events);
    }

    /// The events printed (a photo's or video's blob to the drainer).
    fn print_events(&self, events: &[Event]) {
        for event in events {
            if let Event::Message { content, .. } | Event::GroupMessage { content, .. } = event {
                if let (Content::Photo(m) | Content::Video { media: m, .. }, Some(drain)) = (content, &self.drain) {
                    let _ = drain.try_send(m.clone());
                }
            }
            self.line(&commands::event(event, self.drain.is_none()));
        }
    }

    /// The events printed acknowledged: they do not come again after a restart.
    fn acknowledge(&self, events: &[Event]) {
        if !events.is_empty() && !self.manual_ack {
            self.client().acknowledge_only(|e| events.contains(e));
        }
    }

    /// A bot answers a message on the socket within milliseconds: its answer goes first, and the ack, receipts and
    /// saves after the fetch (a save each, tens of milliseconds with a large state) take the client after it.
    fn give_way(&self, begun: usize) {
        let start = Instant::now();
        while self.begun.load(Ordering::SeqCst) == begun && start.elapsed() < GIVE_WAY {
            std::thread::sleep(Duration::from_millis(1));
        }
        while self.running.load(Ordering::SeqCst) > 0 && start.elapsed() < GIVE_WAY_MAX {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// One command from the control socket, its answer back.
    fn serve(&self, stream: UnixStream) {
        let mut line = String::new();
        let read = BufReader::new((&stream).take(MAX_REQUEST)).read_line(&mut line);
        let command: Option<Vec<String>> = read.ok().and_then(|_| serde_json::from_str(&line).ok());
        let outcome = match command {
            Some(command) if !matches!(command.first().map(String::as_str), Some("register" | "listen") | None) => {
                let emit = |events| self.emit(events);
                let context = Context { client: self.client, config: self.config, listening: true, emit: &emit };
                commands::run(&context, &command)
            }
            _ => Err(Failure::usage("a command other than register and listen")),
        };
        let answer = match outcome {
            Ok(out) => json!({ "code": 0, "out": out }),
            Err(failure) => json!({ "code": failure.code, "out": failure.json() }),
        };
        let _ = (&stream).write_all(format!("{answer}\n").as_bytes());
    }
}

/// Fetches the blobs of photos and videos sent to the account into nothing, one at a time, and acknowledges each that
/// came whole, once: only that counts as fetched at the hub, which deletes a blob after its last (spec 4.3). Nothing
/// of them is kept, opened or printed.
fn drainer(transport: &dyn Transport, blobs: &Receiver<Media>) {
    while let Ok(blob) = blobs.recv() {
        drain(transport, &blob);
    }
}

/// One blob fetched into nothing and acknowledged if it came whole (`--drain-media`).
pub(crate) fn drain(transport: &dyn Transport, blob: &Media) {
    if let Ok(Some(_)) = media::fetch(transport, blob) {
        let _ = media::ack(transport, &blob.id);
    }
}

pub fn listen(config: &Config, opened: Opened, args: &[String]) -> i32 {
    let (mut drain_media, mut manual_ack) = (false, false);
    for arg in args {
        match arg.as_str() {
            "--drain-media" => drain_media = true,
            "--manual-ack" => manual_ack = true,
            _ => return crate::print("", Err(Failure::usage("listen [--drain-media] [--manual-ack]"))),
        }
    }
    let Opened { mut client, _lock } = opened;
    // It never opens a photo or video then: their keys are not even kept with the events.
    if drain_media {
        client.forget_media_keys();
    }
    let transport = client.transport();
    let path = config.control();
    let _ = std::fs::remove_file(&path); // a dead listener's: this process has the directory now
    let socket = match UnixListener::bind(&path) {
        Ok(socket) => socket,
        Err(error) => return crate::print("", Err(Failure::new(format!("{}: {error}", path.display())))),
    };
    let (drain, to_drain) = sync_channel(MAX_DRAINING);
    let client = Mutex::new(client);
    let listener = Listener {
        client: &client,
        config,
        out: Mutex::new(std::io::stdout()),
        manual_ack,
        drain: drain_media.then_some(drain),
        running: AtomicUsize::new(0),
        begun: AtomicUsize::new(0),
    };
    std::thread::scope(|scope| -> i32 {
        if drain_media {
            scope.spawn(move || drainer(transport.as_ref(), &to_drain));
        }
        scope.spawn(|| {
            for stream in socket.incoming().flatten() {
                let running = &listener.running;
                if running.fetch_add(1, Ordering::SeqCst) >= MAX_COMMANDS {
                    running.fetch_sub(1, Ordering::SeqCst);
                    let busy = Failure { retryable: true, ..Failure::new("busy") };
                    let _ =
                        (&stream).write_all(format!("{}\n", json!({ "code": FAILED, "out": busy.json() })).as_bytes());
                    continue;
                }
                listener.begun.fetch_add(1, Ordering::SeqCst);
                let listener = &listener;
                scope.spawn(move || {
                    listener.serve(stream);
                    listener.running.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        let (user, username) = {
            let client = listener.client();
            (client.user_id().to_owned(), client.username().map(str::to_owned))
        };
        listener.line(&json!({ "type": "ready", "user": user, "username": username }));
        receive(&listener)
    })
}

/// The long poll, without holding the client while it waits; until the hub no longer knows the device.
fn receive(listener: &Listener) -> ! {
    let mut failures = 0u32;
    // A fresh account needs its profile key before the first event can trigger a reply on the socket.
    let mut maintained: Option<Instant> = None;
    loop {
        let result = if maintained.is_none_or(|at| at.elapsed() >= MAINTENANCE) {
            maintained = Some(Instant::now());
            listener.client().maintenance()
        } else {
            let wait = listener.client().sync_begin(WAIT_S);
            wait.and_then(|wait| {
                let response = wait.send();
                // Printed before they are saved: a bot's answer goes first (`sync_end_unsaved`), the save after.
                listener.client().sync_end_unsaved(&wait, response)
            })
        };
        match result {
            Ok(events) => {
                failures = 0;
                let probes = listener.client().take_probe_requests();
                let connects = listener.client().take_connect_requests();
                let message = events.iter().any(|e| matches!(e, Event::Message { .. } | Event::GroupMessage { .. }));
                let begun = listener.begun.load(Ordering::SeqCst);
                listener.print_events(&events);
                let probed = !probes.is_empty();
                for (user, token) in probes {
                    listener.line(&commands::probe(&user, &token));
                }
                for request in connects {
                    listener.line(&commands::connect(&request));
                }
                if message || probed {
                    listener.give_way(begun);
                }
                // Not saved: the hub sends them again, after a restart they come once more.
                if let Err(error) = listener.client().save_handed() {
                    eprintln!("yo: {}", crate::reason(&error));
                }
                listener.acknowledge(&events);
            }
            Err(error) if crate::gone(&error) => {
                eprintln!("yo: this device is gone for good (replaced by another, or the name released)");
                std::process::exit(GONE);
            }
            // A `401` (a clock off, a hub restarting) is waited out like the network: never final.
            Err(error) => {
                // 1, 2, 4 … 60 seconds; back to 1 after a success.
                let s = (1u64 << failures.min(6)).min(60);
                eprintln!("yo: {}, again in {s} s", crate::reason(&error));
                std::thread::sleep(Duration::from_secs(s));
                failures += 1;
            }
        }
    }
}

/// The local time of day, HH:MM, without a time zone library: UTC offset from `date`, once.
fn chrono_free_time() -> String {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let offset = *OFFSET.get_or_init(|| {
        let out = std::process::Command::new("date").arg("+%z").output().ok();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
        let sign = if text.starts_with('-') { -1 } else { 1 };
        let digits: i64 = text.trim_start_matches(['+', '-']).parse().unwrap_or(0);
        sign * ((digits / 100) * 3600 + (digits % 100) * 60)
    });
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let day = (now + offset).rem_euclid(86400);
    format!("{:02}:{:02}", day / 3600, (day % 3600) / 60)
}
