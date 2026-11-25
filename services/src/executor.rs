//! The executor: takes what comes for one account, starts the script for each, asks the gatekeeper what of its answer
//! goes, does it, and keeps the book. The script decides nothing but the answer, and never learns whom it is for.

use crate::book::{Book, Reminder};
use crate::gatekeeper::{action, Action, Asked, Header, Limits, Trigger};
use crate::partner::{Call, Partner};
use crate::script::Script;
use crate::yo::{Failure, Yo};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A reminder whose Yo could not go is tried again after this long.
const AGAIN: Duration = Duration::from_secs(600);
/// What a user decided is tried so often to be kept, at once: what comes after it waits.
const TRIES: usize = 3;
const PAUSE: Duration = Duration::from_millis(200);
/// An answer that could not go, the listener being busy, is tried again after each of these.
const ANSWER_WAITS: &[Duration] = &[
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
];
/// What a call still needs to be answered in time.
const MARGIN_MS: i64 = 5_000;

pub struct Executor {
    pub header: Header,
    /// The one of its names whose account this is.
    pub name: String,
    pub yo: Yo,
    pub script: Script,
    pub book: Book,
    pub limits: Mutex<Limits>,
    /// Answers to decisions that changed nothing: apart from Yos.
    pub connects: Mutex<Limits>,
    /// The bot's own pictures.
    pub pictures: PathBuf,
    /// Where a picture lies while it is sent.
    pub spool: PathBuf,
    /// Other bots, by user id: never answered.
    pub bots: Mutex<HashSet<String>>,
    /// The only user served, if set.
    pub only: Option<String>,
    /// Reminders being sent (None), or not to be tried again before a time.
    pub under_way: Mutex<HashMap<i64, Option<Instant>>>,
    pub counts: Counts,
    /// The other side of its connections, for a service that has one.
    pub partner: Option<Partner>,
    /// What a user who is not connected gets back.
    pub help: Option<Vec<u8>>,
}

#[derive(Default)]
pub struct Counts {
    pub taken: AtomicU64,
    pub sent: AtomicU64,
    pub limited: AtomicU64,
    pub refused: AtomicU64,
    pub failed: AtomicU64,
    pub uncalled: AtomicU64,
}

/// One start of the script.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    trigger: Trigger,
    kind: String,
    /// The user it is about, and where the answer goes (a group, or the same user).
    user: String,
    to: String,
    media: Option<String>,
    at: i64,
    reminder: Option<i64>,
    /// The other side called.
    call: Option<Call>,
}

impl Job {
    fn group(&self) -> bool {
        self.to != self.user
    }

    fn reminder(reminder: Reminder, now: i64) -> Job {
        Job {
            trigger: Trigger::Cron,
            kind: "cron".into(),
            to: reminder.user.clone(),
            user: reminder.user,
            media: None,
            at: now,
            reminder: Some(reminder.id),
            call: None,
        }
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn count(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// As often as it takes, a few times, before anything else.
fn kept<T>(mut keep: impl FnMut() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    for _ in 1..TRIES {
        if let Ok(done) = keep() {
            return Ok(done);
        }
        std::thread::sleep(PAUSE);
    }
    keep()
}

/// As often as it may help. An answer counts only for the code it has, so a late one does no harm.
fn answered(waits: &[Duration], mut answer: impl FnMut() -> Result<(), Failure>) -> Result<(), Failure> {
    for wait in waits {
        match answer() {
            Err(failure) if failure.retryable => std::thread::sleep(*wait),
            done => return done,
        }
    }
    answer()
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn user_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl Executor {
    /// A Yo, picture, video or probe that came, as a job; None for anything else, and for what the limits stop.
    pub fn take(&self, event: &Value) -> Option<Job> {
        let text = |key: &str| event[key].as_str();
        let probe = text("type") == Some("probe");
        let kind = match (text("type")?, text("content")) {
            ("probe", _) => "probe",
            ("message", Some("yo")) => "yo",
            ("message", Some("photo")) => "picture",
            ("message", Some("video")) => "video",
            _ => return None,
        };
        let user = text("from").filter(|u| user_id(u))?;
        if !self.header.has(Trigger::Yo) || !self.serves(user) || (probe && !self.header.probed()) {
            return None;
        }
        let group = text("group").filter(|_| !probe);
        let to = if group.is_some() { text("reply_to")? } else { user };
        if !locked(&self.limits).take(user, group, Instant::now()) {
            count(&self.counts.limited);
            return None;
        }
        count(&self.counts.taken);
        let at = now();
        let media = if probe { text("token") } else { event["media"]["ref"].as_str() };
        Some(Job {
            trigger: Trigger::Yo,
            kind: kind.into(),
            user: user.into(),
            to: to.into(),
            media: media.map(str::to_owned),
            at,
            reminder: None,
            call: None,
        })
    }

    /// A user allowed a request to connect, or they are told it is refused. In order with what they decide next: what
    /// failed is not done later.
    pub fn connect(self: &Arc<Self>, event: &Value) {
        let Some((user, code)) = self.connecting(event) else { return };
        let taken = self.partner.is_some() && self.keep(|| self.book.allow(user, code, now())).unwrap_or(false);
        if !taken {
            self.tell_nothing(user, "refused", code);
        }
    }

    /// A user declined a request to connect: whoever waits is told. Of any other code nothing comes.
    pub fn decline(&self, event: &Value) {
        let Some((_, code)) = self.connecting(event) else { return };
        if self.partner.is_some() {
            let _ = self.keep(|| self.book.decline(code, now()));
        }
    }

    /// A user ends their connection: forgotten, with what they allowed before, whatever the limits say, and told so.
    pub fn disconnect(self: &Arc<Self>, event: &Value) {
        let Some((user, code)) = self.connecting(event) else { return };
        match self.keep(|| self.book.disconnect(user)) {
            Ok(true) => self.tell(user, "ended", code),
            Ok(false) => self.tell_nothing(user, "ended", code),
            Err(_) => {}
        }
    }

    fn keep<T>(&self, keep: impl FnMut() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        kept(keep).inspect_err(|e| {
            count(&self.counts.failed);
            eprintln!("a decision is not kept: {e}");
        })
    }

    fn connecting<'a>(&self, event: &'a Value) -> Option<(&'a str, &'a str)> {
        let user = event["from"].as_str().filter(|u| user_id(u) && self.serves(u))?;
        Some((user, event["code"].as_str()?))
    }

    /// Answers what changed nothing: a flood of that is answered a little.
    fn tell_nothing(self: &Arc<Self>, user: &str, kind: &'static str, code: &str) {
        let now = Instant::now();
        if locked(&self.connects).take(user, None, now) && locked(&self.limits).send(now) {
            return self.tell(user, kind, code);
        }
        count(&self.counts.limited);
    }

    /// Answers a connection, beside the events: the account must not wait for itself. Whatever the limits say: one answer
    /// for a connection made or ended.
    pub fn tell(self: &Arc<Self>, user: &str, kind: &'static str, code: &str) {
        let (executor, user, code) = (self.clone(), user.to_owned(), code.to_owned());
        std::thread::spawn(move || {
            if answered(ANSWER_WAITS, || executor.yo.answer(&user, kind, &code)).is_err() {
                count(&executor.counts.failed);
            }
        });
    }

    /// The other side called, for `user` or for nobody: what it gets back.
    pub fn called(&self, call: Call, user: Option<String>) -> Result<(u16, Value), u16> {
        let user = user.unwrap_or_default();
        if !user.is_empty() && !self.serves(&user) {
            return Err(401);
        }
        let job = Job {
            trigger: Trigger::Webhook,
            kind: "webhook".into(),
            to: user.clone(),
            user,
            media: None,
            at: now(),
            reminder: None,
            call: Some(call),
        };
        let done = self.answer(&job).and_then(|actions| {
            self.act(&job, None, &actions)?;
            Ok(actions.into_iter().find_map(|a| {
                if let Action::Reply(status, json) = a {
                    Some((status, json))
                } else {
                    None
                }
            }))
        });
        match done {
            Ok(reply) => reply.ok_or(500),
            Err(failure) => {
                count(&self.counts.failed);
                Err(if failure.retryable { 503 } else { 400 })
            }
        }
    }

    /// The reminders whose time has come, each as a job once.
    pub fn due(&self) -> Vec<Job> {
        let at = now();
        if !self.header.has(Trigger::Cron) {
            return Vec::new();
        }
        let mut under_way = locked(&self.under_way);
        under_way.retain(|_, again| again.is_none_or(|again| Instant::now() < again));
        let mut jobs = Vec::new();
        for reminder in self.book.due(at).unwrap_or_default() {
            if self.serves(&reminder.user) && !under_way.contains_key(&reminder.id) {
                under_way.insert(reminder.id, None);
                jobs.push(Job::reminder(reminder, at));
            }
        }
        jobs
    }

    fn serves(&self, user: &str) -> bool {
        self.only.as_ref().is_none_or(|only| only == user) && !locked(&self.bots).contains(user)
    }

    /// A job for which there was no room: a reminder is tried again later.
    pub fn put_off(&self, job: &Job) {
        if let Some(id) = job.reminder {
            locked(&self.under_way).insert(id, Some(Instant::now() + AGAIN));
        }
    }

    /// The script started for a job, and what the gatekeeper lets through of its answer done.
    pub fn run(&self, job: &Job) {
        // Forgotten while it waited.
        if self.forgotten(job) {
            return self.put_off(job);
        }
        let came = match job.trigger {
            // A probe is no Yo: nothing is kept of it, not even that it was.
            Trigger::Yo if job.kind != "probe" => self.book.received(job.at).ok(),
            _ => None,
        };
        if let (Some(row), false, Some(_)) = (came, job.group(), self.out(job)) {
            let _ = self.book.keep(row, &job.user, &job.kind);
        }
        let done = self.answer(job).and_then(|actions| self.act(job, came, &actions));
        match (&done, job.reminder) {
            (Ok(()), Some(id)) if self.book.done(id, now()).is_ok() => drop(locked(&self.under_way).remove(&id)),
            _ => self.put_off(job),
        }
        if let Err(failure) = done {
            count(&self.counts.failed);
            eprintln!("failed: {}", failure.code);
        }
    }

    /// What the other side knows the user by, if they are connected.
    fn out(&self, job: &Job) -> Option<String> {
        let partner = self.partner.as_ref().filter(|_| !job.user.is_empty() && job.kind != "probe")?;
        partner.test_out(&job.user).or_else(|| self.book.out(&job.user).ok().flatten())
    }

    /// A message of one user alone, not connected; not when the book cannot tell.
    fn unconnected(&self, job: &Job) -> bool {
        let written = matches!(job.kind.as_str(), "yo" | "picture" | "video");
        let partner = self.partner.as_ref().filter(|_| written && !job.group());
        partner.is_some_and(|p| p.test_out(&job.user).is_none() && matches!(self.book.out(&job.user), Ok(None)))
    }

    fn answer(&self, job: &Job) -> Result<Vec<Action>, Failure> {
        let private = !job.group() && job.kind != "probe";
        let remembered = private && self.header.remembers() && self.book.remembers(&job.user, job.at).unwrap_or(false);
        if let Some(jpeg) = self.help.as_ref().filter(|_| self.unconnected(job)) {
            return Ok(vec![Action::Drawn(jpeg.clone())]);
        }
        // The reference itself holds the blob's key: the script gets a word for it.
        let media = job.media.as_ref().filter(|_| job.kind != "probe").map(|_| "it");
        let out = self.out(job);
        let mut event = json!({
            "name": self.name, "trigger": job.trigger.word(), "kind": job.kind, "media": media, "group": job.group(),
            "remembered": remembered, "now": job.at,
            "out": out, "id": null, "method": null, "path": null, "body": null, "yos": null, "test": null,
            "keyed": false,
        });
        if let (Some(call), Some(partner)) = (&job.call, &self.partner) {
            let setup =
                call.keyed && out.is_none() && (call.method.as_str(), call.path.as_str()) == ("POST", "v1/test/setup");
            let connected = out.is_some() && partner.test_out(&job.user).is_none();
            let yos = if connected { self.book.kept(&job.user, job.at).unwrap_or_default() } else { Vec::new() };
            event["id"] = json!(call.id);
            event["method"] = json!(call.method);
            event["path"] = json!(call.path);
            event["body"] = json!(call.body);
            event["yos"] = json!(yos);
            event["test"] = json!(setup.then(|| partner.test_key()).flatten());
            event["keyed"] = json!(call.keyed);
        }
        let answer = self.script.call(format!("{event}\n").as_bytes());
        let answer = answer.map_err(|_| Failure { code: 1, retryable: true })?;
        let asked = Asked {
            kind: &job.kind,
            media,
            group: job.group(),
            remember_user: self.header.remembers(),
            now: job.at,
            connected: out.is_some(),
            header: &self.header,
        };
        action::parse(&answer, &asked).map_err(|why| {
            count(&self.counts.refused);
            eprintln!("refused: {why}");
            Failure { code: 1, retryable: false }
        })
    }

    /// Sends first, then writes the book: what fails and is tried again leaves no reminder behind.
    fn act(&self, job: &Job, came: Option<i64>, actions: &[Action]) -> Result<(), Failure> {
        // Forgotten while the script was asked.
        if self.forgotten(job) {
            return Err(Failure { code: 1, retryable: false });
        }
        for action in actions {
            match action {
                Action::Wait(ms) => std::thread::sleep(Duration::from_millis(*ms)),
                Action::Forget | Action::Remind(_) | Action::Reply(..) | Action::Call(..) => {}
                send => self.send(job, send)?,
            }
        }
        for action in actions {
            let written = match action {
                Action::Forget => self.book.forget(&job.user),
                Action::Remind(due) => came.map_or(Ok(()), |row| self.book.remind(row, &job.user, *due)),
                _ => Ok(()),
            };
            written.map_err(|_| Failure { code: 1, retryable: true })?;
        }
        // Last: what the other side then asks for is in the book. One that fails is only late: it asks by itself too.
        for action in actions {
            if let (Action::Call(url, body), Some(partner)) = (action, &self.partner) {
                if !partner.call(url, body) {
                    count(&self.counts.uncalled);
                }
            }
        }
        Ok(())
    }

    /// A reminder that waits no more.
    fn forgotten(&self, job: &Job) -> bool {
        job.reminder.is_some_and(|id| !self.book.waits(id).unwrap_or(false))
    }

    /// Whether who called has given up, or is about to: it will try again, and one Yo is enough.
    fn late(job: &Job) -> bool {
        job.call.as_ref().is_some_and(|call| now_ms() + MARGIN_MS > call.until_ms)
    }

    fn send(&self, job: &Job, action: &Action) -> Result<(), Failure> {
        let limited = || {
            count(&self.counts.limited);
            Err(Failure { code: 1, retryable: true })
        };
        // Nothing is fetched that could not go.
        if !locked(&self.limits).room(Instant::now()) {
            return limited();
        }
        let fetched = match action {
            Action::Fetched(url) => crate::picture::fetch(url).map_err(|retryable| Failure { code: 1, retryable })?,
            _ => Vec::new(),
        };
        if Executor::late(job) {
            return Err(Failure { code: 1, retryable: true });
        }
        if !locked(&self.limits).send(Instant::now()) {
            return limited();
        }
        let media = || job.media.as_deref().ok_or(Failure { code: 1, retryable: false });
        match action {
            Action::Back => self.yo.send_media(&job.to, media()?)?,
            Action::Drawn(jpeg) => self.send_picture(&job.to, jpeg)?,
            Action::File(name) => self.send_picture(&job.to, &self.own_picture(name)?)?,
            Action::Fetched(_) => self.send_picture(&job.to, &fetched)?,
            Action::Probe => return self.yo.echo_probe(&job.to, media()?),
            _ => self.yo.send(&job.to)?,
        }
        count(&self.counts.sent);
        let _ = self.book.sent(now());
        Ok(())
    }

    fn own_picture(&self, name: &str) -> Result<Vec<u8>, Failure> {
        let mut bytes = Vec::new();
        let path = self.pictures.join(name);
        // No link out of the folder.
        if !path.symlink_metadata().is_ok_and(|m| m.is_file()) {
            return Err(Failure { code: 1, retryable: false });
        }
        let read =
            std::fs::File::open(path).and_then(|f| f.take(action::MAX_PICTURE as u64 + 1).read_to_end(&mut bytes));
        match read {
            Ok(n) if (1..=action::MAX_PICTURE).contains(&n) => Ok(bytes),
            _ => Err(Failure { code: 1, retryable: false }),
        }
    }

    /// From a file of its own in the state directory, gone afterwards.
    fn send_picture(&self, to: &str, jpeg: &[u8]) -> Result<(), Failure> {
        let mut random = [0; 8];
        let failed = Failure { code: 1, retryable: true };
        getrandom::fill(&mut random).map_err(|_| failed.clone())?;
        let path = self.spool.join(format!("picture-{:016x}.jpg", u64::from_le_bytes(random)));
        let file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path);
        let sent = match file.and_then(|mut f| f.write_all(jpeg)) {
            Ok(()) => self.yo.send_picture(to, &path),
            Err(_) => Err(failed),
        };
        let _ = std::fs::remove_file(&path);
        sent
    }

    /// The counts since the last time, for the log: never who.
    pub fn tally(&self) -> Option<String> {
        let c = &self.counts;
        let all = [
            ("taken", &c.taken),
            ("sent", &c.sent),
            ("limited", &c.limited),
            ("refused", &c.refused),
            ("failed", &c.failed),
            ("calls failed", &c.uncalled),
        ];
        let words: Vec<String> = all
            .iter()
            .map(|(word, n)| (word, n.swap(0, Ordering::Relaxed)))
            .filter(|(_, n)| *n > 0)
            .map(|(word, n)| format!("{n} {word}"))
            .collect();
        (!words.is_empty()).then(|| words.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decision_is_kept_at_once_or_not_at_all() {
        let mut failures = 1;
        let tried = kept(|| {
            failures -= 1;
            if failures < 0 {
                Ok(1)
            } else {
                Err(rusqlite::Error::InvalidQuery)
            }
        });
        assert_eq!(tried, Ok(1));
        let mut tries = 0;
        let failed: rusqlite::Result<()> = kept(|| {
            tries += 1;
            Err(rusqlite::Error::InvalidQuery)
        });
        assert!(failed.is_err() && tries == TRIES);
    }

    #[test]
    fn an_answer_waits_for_a_busy_listener() {
        let failing = |retryable| Err(Failure { code: 1, retryable });
        let (mut tries, waits) = (0, &[Duration::ZERO; 3]);
        let done = answered(waits, || {
            tries += 1;
            if tries < 4 {
                failing(true)
            } else {
                Ok(())
            }
        });
        assert_eq!((done, tries), (Ok(()), 4));
        tries = 0;
        let done = answered(waits, || {
            tries += 1;
            failing(false)
        });
        assert!(done.is_err() && tries == 1);
    }

    #[test]
    fn nothing_is_sent_for_who_gave_up() {
        let job = |until_ms| {
            let call = Call {
                id: "i".into(),
                method: "POST".into(),
                path: "p".into(),
                body: json!({}),
                keyed: false,
                until_ms,
            };
            let mut job = Job::reminder(Reminder { id: 1, user: "u".into() }, 1);
            job.call = Some(call);
            job
        };
        assert!(Executor::late(&job(now_ms())) && !Executor::late(&job(now_ms() + 2 * MARGIN_MS)));
        assert!(!Executor::late(&Job::reminder(Reminder { id: 1, user: "u".into() }, 1)));
    }
}
