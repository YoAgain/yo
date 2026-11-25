//! The executor: takes what comes for one account, starts the script for each, asks the gatekeeper what of its answer
//! goes, does it, and keeps the book. The script decides nothing but the answer, and never learns whom it is for.

use crate::book::{Book, Reminder};
use crate::gatekeeper::{action, Action, Asked, Header, Limits, Trigger};
use crate::script::Script;
use crate::yo::{Failure, Yo};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A reminder whose Yo could not go is tried again after this long.
const AGAIN: Duration = Duration::from_secs(600);

pub struct Executor {
    pub header: Header,
    /// The one of its names whose account this is.
    pub name: String,
    pub yo: Yo,
    pub script: Script,
    pub book: Book,
    pub limits: Mutex<Limits>,
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
}

#[derive(Default)]
pub struct Counts {
    pub taken: AtomicU64,
    pub sent: AtomicU64,
    pub limited: AtomicU64,
    pub refused: AtomicU64,
    pub failed: AtomicU64,
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
        }
    }
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn count(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn user_id(text: &str) -> bool {
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
        })
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

    fn answer(&self, job: &Job) -> Result<Vec<Action>, Failure> {
        let private = !job.group() && job.kind != "probe";
        let remembered = private && self.header.remembers() && self.book.remembers(&job.user, job.at).unwrap_or(false);
        // The reference itself holds the blob's key: the script gets a word for it.
        let media = job.media.as_ref().filter(|_| job.kind != "probe").map(|_| "it");
        let event = json!({
            "name": self.name, "trigger": job.trigger.word(), "kind": job.kind, "media": media, "group": job.group(),
            "remembered": remembered, "now": job.at,
        });
        let answer = self.script.call(format!("{event}\n").as_bytes());
        let answer = answer.map_err(|_| Failure { code: 1, retryable: true })?;
        let asked =
            Asked { kind: &job.kind, media, group: job.group(), remember_user: self.header.remembers(), now: job.at };
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
                Action::Forget | Action::Remind(_) => {}
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
        Ok(())
    }

    /// A reminder that waits no more.
    fn forgotten(&self, job: &Job) -> bool {
        job.reminder.is_some_and(|id| !self.book.waits(id).unwrap_or(false))
    }

    fn send(&self, job: &Job, action: &Action) -> Result<(), Failure> {
        if !locked(&self.limits).send(Instant::now()) {
            count(&self.counts.limited);
            return Err(Failure { code: 1, retryable: true });
        }
        let media = || job.media.as_deref().ok_or(Failure { code: 1, retryable: false });
        match action {
            Action::Back => self.yo.send_media(&job.to, media()?)?,
            Action::Drawn(jpeg) => self.send_picture(&job.to, jpeg)?,
            Action::File(name) => self.send_picture(&job.to, &self.own_picture(name)?)?,
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
