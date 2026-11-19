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

/// What waits is answered and sent (spec 4) as the app does when it comes to the foreground.
const MAINTENANCE: Duration = Duration::from_secs(3600);
/// The long poll's wait, in seconds.
const WAIT_S: u64 = 25;
/// Commands running at once on the control socket; more are told to come again.
const MAX_COMMANDS: usize = 32;
/// A command line on the control socket, at most.
const MAX_REQUEST: u64 = 64 * 1024;
/// Blobs waiting to be fetched into nothing (`--drain-media`); more are left to expire at the hub.
const MAX_DRAINING: usize = 64;

struct Listener<'a> {
    client: &'a Mutex<Client>,
    config: &'a Config,
    out: Mutex<std::io::Stdout>,
    manual_ack: bool,
    /// `--drain-media`: the blobs for the drainer (fetched and checked, never opened).
    drain: Option<SyncSender<Media>>,
}

impl Listener<'_> {
    fn client(&self) -> std::sync::MutexGuard<'_, Client> {
        self.client.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// One line, flushed; the reader gone ends the listener.
    fn line(&self, value: &Value) {
        let mut out = self.out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if writeln!(out, "{value}").and_then(|()| out.flush()).is_err() {
            std::process::exit(FAILED);
        }
    }

    /// The events printed, then acknowledged: they do not come again after a restart. A photo's or video's blob goes
    /// to the drainer.
    fn emit(&self, events: Vec<Event>) {
        if events.is_empty() {
            return;
        }
        for event in &events {
            if let Event::Message { content, .. } | Event::GroupMessage { content, .. } = event {
                if let (Content::Photo(m) | Content::Video { media: m, .. }, Some(drain)) = (content, &self.drain) {
                    let _ = drain.try_send(m.clone());
                }
            }
            self.line(&commands::event(event, self.drain.is_none()));
        }
        if !self.manual_ack {
            self.client().acknowledge_only(|e| events.contains(e));
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
/// came whole, once: only that counts as fetched at the hub, which deletes a blob after its last (spec 6.8). Nothing
/// of them is kept, opened or printed.
fn drainer(transport: &dyn Transport, blobs: &Receiver<Media>) {
    while let Ok(blob) = blobs.recv() {
        if let Ok(Some(_)) = media::fetch(transport, &blob) {
            let _ = media::ack(transport, &blob.id);
        }
    }
}

pub fn listen(config: &Config, opened: Opened, args: &[String]) -> i32 {
    let (mut drain_media, mut manual_ack) = (false, false);
    for arg in args {
        match arg.as_str() {
            "--drain-media" => drain_media = true,
            "--manual-ack" => manual_ack = true,
            _ => return crate::print(Err(Failure::usage("listen [--drain-media] [--manual-ack]"))),
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
        Err(error) => return crate::print(Err(Failure::new(format!("{}: {error}", path.display())))),
    };
    let (drain, to_drain) = sync_channel(MAX_DRAINING);
    let client = Mutex::new(client);
    let listener = Listener {
        client: &client,
        config,
        out: Mutex::new(std::io::stdout()),
        manual_ack,
        drain: drain_media.then_some(drain),
    };
    let running = AtomicUsize::new(0);
    std::thread::scope(|scope| -> i32 {
        if drain_media {
            scope.spawn(move || drainer(transport.as_ref(), &to_drain));
        }
        scope.spawn(|| {
            for stream in socket.incoming().flatten() {
                if running.fetch_add(1, Ordering::SeqCst) >= MAX_COMMANDS {
                    running.fetch_sub(1, Ordering::SeqCst);
                    let busy = Failure { retryable: true, ..Failure::new("busy") };
                    let _ =
                        (&stream).write_all(format!("{}\n", json!({ "code": FAILED, "out": busy.json() })).as_bytes());
                    continue;
                }
                let (listener, running) = (&listener, &running);
                scope.spawn(move || {
                    listener.serve(stream);
                    running.fetch_sub(1, Ordering::SeqCst);
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
    let mut maintained = Instant::now();
    loop {
        let result = if maintained.elapsed() >= MAINTENANCE {
            maintained = Instant::now();
            listener.client().maintenance()
        } else {
            let wait = listener.client().sync_begin(WAIT_S);
            wait.and_then(|wait| {
                let response = wait.send();
                listener.client().sync_end(&wait, response)
            })
        };
        match result {
            Ok(events) => {
                failures = 0;
                let probes = listener.client().take_probe_requests();
                listener.emit(events);
                for (user, token) in probes {
                    listener.line(&commands::probe(&user, &token));
                }
            }
            Err(error) if crate::gone(&error) => {
                eprintln!("yo: the hub no longer knows this device (replaced by another, the name released, or the hub reset)");
                std::process::exit(GONE);
            }
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
