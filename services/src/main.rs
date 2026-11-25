//! yo-services: the executor of one service (a bot). See README.md.
//!
//!   yo-services SERVICE      a folder, e.g. /opt/yo-services/yo/ping: its ping.toml, ping.py, assets/
//!
//!   yo-services entrance     HTTP for all of them (entrance.rs): ENTRANCE_LISTEN, ENTRANCE_SOCKETS, ENTRANCE_SERVICES
//!
//! Environment: YO_STATE (the account's directory, registered by hand once), YO_BIN, the hub's pins (app/cli/README.md),
//! SCRIPT_SOCKET (where a connection starts the script), SERVICES_DB (the book, one for all services).
//! Optional: SERVICES_NAME (the account's, for a service of several names; else the first), SERVICES_PER_HOUR,
//! SERVICES_ONLY_USER (a user id: nobody else is served), SERVICES_DRAIN=1 (media is fetched into nothing),
//! SERVICES_BOTS (names of other bots, comma separated, never answered).
//! Of a service with a `[bridge]`: SERVICES_SOCKET (the entrance's way to it), and
//! SERVICES_KEY (the partner's own key), SERVICES_CLIENT_SECRET (OAuth 2, for the TOML's client_id). Optional:
//! SERVICES_TEST_USER and SERVICES_TEST_KEY (the account of the partner's tests, a user id, and a key for it).

mod book;
mod entrance;
mod executor;
mod gatekeeper;
mod help;
mod list;
mod partner;
mod picture;
mod script;
mod sources;
mod yo;

use executor::{now, Executor, Job};
use gatekeeper::{Header, Limits};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

const WORKERS: usize = 16;
const WAITING: usize = 256;
const PER_HOUR: usize = 2000;
/// The exit code of a wrong setting: systemd does not start it again.
const USAGE: i32 = 2;

fn fail(code: i32, why: &str) -> ! {
    eprintln!("stopped: {why}");
    std::process::exit(code)
}

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fail(USAGE, &format!("{key} is not set")))
}

fn main() {
    let service =
        PathBuf::from(std::env::args_os().nth(1).unwrap_or_else(|| fail(USAGE, "usage: yo-services SERVICE")));
    if service.as_os_str() == "entrance" {
        let why = entrance::serve(
            &env("ENTRANCE_LISTEN"),
            PathBuf::from(env("ENTRANCE_SOCKETS")),
            Path::new(&env("ENTRANCE_SERVICES")),
        );
        fail(1, &format!("the entrance: {why:?}"));
    }
    let folder = service.file_name().and_then(|n| n.to_str()).unwrap_or_else(|| fail(USAGE, "not a service's folder"));
    let text = std::fs::read_to_string(service.join(format!("{folder}.toml")))
        .unwrap_or_else(|_| fail(USAGE, "the TOML cannot be read"));
    let header = Header::parse(&text, folder).unwrap_or_else(|why| fail(USAGE, &format!("TOML: {why}")));
    let name = match std::env::var("SERVICES_NAME") {
        Ok(name) if header.names.contains(&name) => name,
        Ok(_) => fail(USAGE, "SERVICES_NAME: not one of its names"),
        Err(_) => header.names[0].clone(),
    };
    let state = PathBuf::from(env("YO_STATE"));
    let book =
        book::Book::open(Path::new(&env("SERVICES_DB")), &name).unwrap_or_else(|e| fail(1, &format!("the book: {e}")));
    let per_hour = match std::env::var("SERVICES_PER_HOUR") {
        Ok(n) => n.parse().unwrap_or_else(|_| fail(USAGE, "SERVICES_PER_HOUR: a number")),
        Err(_) => PER_HOUR,
    };
    let sent = book.sent_since(now() - 3600).unwrap_or_else(|e| fail(1, &format!("the book: {e}")));
    tidy_spool(&state);
    let (answers, to_answer) = sync_channel(executor::ANSWERS);
    let executor = Arc::new(Executor {
        answers,
        yo: yo::Yo::new(&state),
        script: script::Script::new(PathBuf::from(env("SCRIPT_SOCKET"))),
        book,
        limits: Mutex::new(Limits::new(per_hour, sent, Instant::now())),
        connects: Mutex::new(Limits::new(0, 0, Instant::now())),
        pictures: service.join("assets"),
        spool: state.clone(),
        bots: Mutex::new(HashSet::new()),
        only: std::env::var("SERVICES_ONLY_USER").ok(),
        under_way: Mutex::default(),
        counts: executor::Counts::default(),
        help: header.bridge.as_ref().and_then(|bridge| {
            let drawn = help::draw(&name, &bridge.help_url, std::fs::read(service.join("logo.png")).ok().as_deref());
            if drawn.is_none() {
                eprintln!("no help picture");
            }
            drawn
        }),
        partner: header.bridge.as_ref().map(partner),
        header,
        name,
    });

    let answering = executor.clone();
    std::thread::spawn(move || answering.answer_all(to_answer));
    if executor.partner.is_some() {
        let listener = partner::listen(Path::new(&env("SERVICES_SOCKET")))
            .unwrap_or_else(|e| fail(1, &format!("SERVICES_SOCKET: {e}")));
        let executor = executor.clone();
        std::thread::spawn(move || partner::serve(executor, listener));
    }
    let drain = std::env::var("SERVICES_DRAIN").is_ok_and(|v| v == "1");
    let yo = std::env::var_os("YO_BIN").map_or_else(|| PathBuf::from("yo"), PathBuf::from);
    let listener = yo::Listener::start(&yo, &state, drain).unwrap_or_else(|_| fail(1, "yo did not start"));
    let (jobs, queue) = sync_channel::<Job>(WAITING);
    let queue = Arc::new(Mutex::new(queue));
    for _ in 0..WORKERS {
        let (executor, queue) = (executor.clone(), queue.clone());
        std::thread::spawn(move || work(&executor, &queue));
    }
    let groups_changed = Arc::new(AtomicBool::new(true));
    let mut started = false;
    let code = listener.events(|event| match event["type"].as_str() {
        Some("ready") if !started => {
            started = true;
            check_account(&executor)?;
            let mut unknown = other_bots(&executor);
            find(&executor, &mut unknown);
            let (executor, jobs, changed) = (executor.clone(), jobs.clone(), groups_changed.clone());
            std::thread::spawn(move || upkeep(&executor, &jobs, &changed, unknown));
            Ok(())
        }
        Some("connect") => {
            executor.connect(&event);
            Ok(())
        }
        Some("decline") => {
            executor.decline(&event);
            Ok(())
        }
        Some("disconnect") => {
            executor.disconnect(&event);
            Ok(())
        }
        Some("group_changed") => {
            groups_changed.store(true, Ordering::Relaxed);
            Ok(())
        }
        _ => {
            if let Some(job) = executor.take(&event) {
                // More than can wait is dropped: a flood holds up nobody's receiving.
                let _ = jobs.try_send(job);
            }
            Ok(())
        }
    });
    std::process::exit(if code == 0 { 1 } else { code })
}

/// The other side of its connections, from the environment.
fn partner(bridge: &gatekeeper::header::Bridge) -> partner::Partner {
    let secret = |key: &str| {
        Some(env(key)).filter(|s| s.len() >= 16).unwrap_or_else(|| fail(USAGE, &format!("{key}: too short")))
    };
    let test = std::env::var("SERVICES_TEST_USER").ok().map(|user| {
        if !executor::user_id(&user) {
            fail(USAGE, "SERVICES_TEST_USER: not a user id");
        }
        (user, secret("SERVICES_TEST_KEY"))
    });
    partner::Partner {
        header: bridge.key_header.to_ascii_lowercase(),
        return_url: bridge.return_url.clone(),
        key: secret("SERVICES_KEY"),
        client_id: bridge.client_id.clone(),
        client_secret: secret("SERVICES_CLIENT_SECRET"),
        test,
        client: partner::Partner::client().unwrap_or_else(|_| fail(1, "no HTTP client")),
    }
}

/// A picture left by a run that was cut short.
fn tidy_spool(state: &Path) {
    for entry in std::fs::read_dir(state).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("picture-") && name.ends_with(".jpg") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn work(executor: &Executor, queue: &Mutex<Receiver<Job>>) {
    loop {
        let job = queue.lock().unwrap_or_else(PoisonError::into_inner).recv();
        match job {
            Ok(job) => executor.run(&job),
            Err(_) => return,
        }
    }
}

fn leave_empty_groups(executor: &Executor) {
    if let Ok((me, ..)) = executor.yo.whoami() {
        let _ = executor.yo.leave_empty_groups(&me);
    }
}

/// Is it the account it should be? Asked before the first Yo is taken.
fn check_account(executor: &Executor) -> Result<(), i32> {
    match executor.yo.whoami() {
        Ok((_, username, bot)) if username.eq_ignore_ascii_case(&executor.name) => {
            if !bot {
                eprintln!("the account is not declared a bot (yo bot on): no app connects it");
            }
            return Ok(());
        }
        Ok(_) => eprintln!("stopped: the account is not the one named"),
        Err(_) => {
            eprintln!("stopped: the account did not answer");
            return Err(1);
        }
    }
    Err(USAGE)
}

/// The names of the other bots.
fn other_bots(executor: &Executor) -> Vec<String> {
    let names = std::env::var("SERVICES_BOTS").unwrap_or_default();
    let own = &executor.name;
    names.split(',').map(str::trim).filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(own)).map(Into::into).collect()
}

/// Those of `names` that have an account are known by its id from now on.
fn find(executor: &Executor, names: &mut Vec<String>) {
    names.retain(|name| match executor.yo.whois(name) {
        Ok(Some(user)) => {
            executor.bots.lock().unwrap_or_else(PoisonError::into_inner).insert(user);
            false
        }
        _ => true,
    });
}

/// The reminders every few seconds, the rest every minute.
fn upkeep(executor: &Executor, jobs: &SyncSender<Job>, groups_changed: &AtomicBool, mut unknown: Vec<String>) {
    loop {
        find(executor, &mut unknown);
        if let Ok(contacts) = executor.yo.contacts() {
            stats(&executor.spool, contacts);
        }
        let _ = executor.book.tidy(now());
        if let Some(tally) = executor.tally() {
            eprintln!("{tally}");
        }
        for _ in 0..12 {
            executor.script.warm();
            if groups_changed.swap(false, Ordering::Relaxed) {
                leave_empty_groups(executor);
            }
            for job in executor.due() {
                if let Err(TrySendError::Full(job) | TrySendError::Disconnected(job)) = jobs.try_send(job) {
                    executor.put_off(&job);
                }
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    }
}

/// How many people it knows, for the admin page.
fn stats(state: &Path, contacts: u64) {
    let new = state.join("stats.json.new");
    if std::fs::write(&new, format!("{{\"contacts\": {contacts}}}")).is_ok() {
        let _ = std::fs::rename(new, state.join("stats.json"));
    }
}
