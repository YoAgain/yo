//! Deterministic presentation rules for every platform's friend list.
//!
//! Times are monotonic milliseconds supplied by the caller. Call `tick` at `next_tick`,
//! and report viewport/foreground changes before suspending. No networking or wall clock; the caller's
//! clock must keep counting while the device sleeps (a seen Yo's clock runs in real time).
use std::collections::{BTreeMap, BTreeSet};

pub const YO_SOLID_MS: u64 = 3_000;
pub const YO_FADE_MS: u64 = 2_000;
/// A Yo counts as seen once its bar was on screen this long at a stretch while the app was active. From then on
/// its solid+fade clock runs in real time (background, off screen, camera); before, a glimpse leaves it fresh.
pub const YO_SEEN_MS: u64 = 1_000;
pub const UNDO_MS: u64 = 2_000;
pub const SENT_MS: u64 = 1_500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Yo,
    Photo,
    Video,
}
impl Kind {
    fn sent(self) -> &'static str {
        match self {
            Self::Yo => "Sent Yo",
            Self::Photo => "Sent 📷",
            Self::Video => "Sent 🎥",
        }
    }
    fn sign(self) -> &'static str {
        match self {
            Self::Yo => "",
            Self::Photo => "📷",
            Self::Video => "🎥",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Friend {
    pub id: String,
    pub name: String,
    pub request: bool,
    pub group: bool,
}
#[derive(Clone, Debug)]
pub enum Input {
    Friends(Vec<Friend>),
    Received {
        id: String,
        kind: Kind,
        token: String,
        sender: Option<String>,
    },
    SenderNamed {
        user: String,
        name: String,
    },
    SendStarted {
        id: String,
        kind: Kind,
        undo_ms: u64,
    },
    SendFinished {
        id: String,
        kind: Kind,
    },
    SendFailed {
        id: String,
        text: String,
    },
    SendUndone(String),
    /// Other short platform notices use the same core-owned text/count lifetime.
    Notice {
        id: String,
        text: String,
        rise: bool,
    },
    Press {
        id: String,
        down: bool,
    },
    Recording(Option<String>),
    Camera(bool),
    Foreground(bool),
    RowsVisible(Vec<String>),
    Flight(bool),
    RequestArrived(Friend),
    RequestAccepted(String),
    RequestDeclined(String),
    MediaSeen(String),
    MediaChanged {
        id: String,
        kinds: Vec<Kind>,
    },
    /// A group reply keeps its sender lines expanded for another solid period.
    KeepAlive(String),
    Delete(String),
    UndoDelete,
    Connectivity {
        offline: bool,
        failure: Option<String>,
    },
    Tick,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub count: u32,
    pub request: bool,
    pub sending: bool,
    pub undo_deadline: Option<u64>,
    pub notice_deadline: Option<u64>,
    pub senders: Vec<Sender>,
    pub yo_title: String,
    pub yo_count: u32,
    pub yo_total: u32,
    pub yo_token: String,
    pub fade_elapsed: Option<u64>,
    pub clock_running: bool,
    pub media_sign: String,
    pub mark_count: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sender {
    pub user: String,
    pub name: String,
    pub count: u32,
    pub elapsed: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deletion {
    pub id: String,
    pub name: String,
    pub deadline: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub banner: Option<String>,
    pub deletion: Option<Deletion>,
    pub committed_deletes: Vec<String>,
    pub rows: Vec<Row>,
    pub locked_row: Option<String>,
    pub next_tick: Option<u64>,
}
#[derive(Clone, Debug)]
struct Run {
    token: String,
    total: u32,
    count: u32,
    solid_count: u32,
    elapsed: u64,
    step: u64,
    waiting_order: bool,
    seen: bool,
}
#[derive(Clone, Debug)]
struct Sent {
    text: String,
    count: u32,
    until: u64,
    rise: bool,
    sequence: u64,
}
#[derive(Clone, Debug, Default)]
struct State {
    senders: Vec<Sender>,
    run: Option<Run>,
    media: Vec<Kind>,
    sent: Option<Sent>,
    outgoing: Option<(Kind, u32, u64)>,
}

#[derive(Default)]
pub struct List {
    offline: bool,
    failure: Option<String>,
    pending_delete: Option<Deletion>,
    deleted: BTreeSet<String>,
    committed_deletes: Vec<String>,
    now: u64,
    sequence: u64,
    friends: BTreeMap<String, Friend>,
    names: BTreeMap<String, String>,
    order: Vec<String>,
    proposed: Vec<String>,
    source_order: Vec<String>,
    states: BTreeMap<String, State>,
    rising: Vec<String>,
    batch_at: Option<u64>,
    visible: BTreeSet<String>,
    pressed: Vec<String>,
    recording: Option<String>,
    camera: bool,
    background: bool,
    flight: bool,
    tap_lock: Option<(String, u64)>,
}
impl List {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn apply(&mut self, now: u64, input: Input) -> Snapshot {
        self.committed_deletes.clear();
        // Settle already-due moves before a new input can introduce a freeze.
        self.advance(now);
        let target = match &input {
            Input::Received { id, .. }
            | Input::SendStarted { id, .. }
            | Input::SendFinished { id, .. }
            | Input::SendFailed { id, .. }
            | Input::Notice { id, .. }
            | Input::Press { id, .. }
            | Input::MediaChanged { id, .. }
            | Input::SendUndone(id)
            | Input::RequestAccepted(id)
            | Input::MediaSeen(id)
            | Input::KeepAlive(id)
            | Input::Recording(Some(id)) => Some(id),
            _ => None,
        };
        if target.is_some_and(|id| !self.friends.contains_key(id)) {
            return self.snapshot();
        }
        match input {
            Input::Friends(friends) => {
                self.deleted.retain(|id| friends.iter().any(|f| &f.id == id));
                let source: Vec<_> = friends.iter().map(|f| f.id.clone()).collect();
                if source != self.source_order {
                    self.proposed = source.clone();
                    self.source_order = source;
                }
                self.friends = friends.into_iter().map(|f| (f.id.clone(), f)).collect();
                self.order.retain(|id| self.friends.contains_key(id));
                self.states.retain(|id, _| self.friends.contains_key(id));
                for id in &self.source_order {
                    if !self.order.contains(id) {
                        self.order.push(id.clone());
                    }
                }
            }
            Input::SenderNamed { user, name } => {
                self.names.insert(user.clone(), name.clone());
                for state in self.states.values_mut() {
                    for sender in &mut state.senders {
                        if sender.user == user {
                            sender.name.clone_from(&name);
                        }
                    }
                }
            }
            Input::Received { id, kind, token, sender } => {
                if self.friends.contains_key(&id) {
                    let state = self.states.entry(id.clone()).or_default();
                    let total = state
                        .run
                        .as_ref()
                        .filter(|r| r.count > 0 && r.elapsed < Self::fade_start(r))
                        .map_or(1, |r| r.total.saturating_add(1));
                    let first =
                        state.run.as_ref().filter(|r| total > 1 && r.count > 0).map_or(token, |r| r.token.clone());
                    if total == 1 {
                        state.senders.clear();
                    }
                    if let Some(user) = sender {
                        if let Some(s) = state.senders.iter_mut().find(|s| s.user == user) {
                            s.count = s.count.saturating_add(1);
                            s.elapsed = 0;
                        } else {
                            let name = self.names.get(&user).cloned().unwrap_or_else(|| "?".into());
                            state.senders.push(Sender { user, name, count: 1, elapsed: 0 });
                        }
                    }
                    state.run = Some(Run {
                        waiting_order: true,
                        token: first,
                        total,
                        count: total,
                        solid_count: total,
                        elapsed: 0,
                        step: 200.min(2_000 / u64::from(total.max(1))).max(1),
                        seen: false,
                    });
                    if kind != Kind::Yo {
                        state.media.push(kind);
                    }
                    self.rise(id);
                }
            }
            Input::SendStarted { id, kind, undo_ms } => {
                let state = self.states.entry(id.clone()).or_default();
                let count = state.outgoing.map_or(1, |(_, n, _)| n.saturating_add(1));
                state.outgoing = Some((kind, count, self.now.saturating_add(undo_ms)));
                self.tap_lock = Some((id, self.now.saturating_add(SENT_MS)));
            }
            Input::SendFinished { id, kind } => {
                // A released stack can finish while a newer stack still has its undo ring.
                self.notice(id, kind.sent().into(), true);
            }
            Input::SendFailed { id, text } => {
                self.notice(id, text, false);
            }
            Input::SendUndone(id) => {
                self.states.entry(id.clone()).or_default().outgoing = None;
                self.notice(id, "Canceled".into(), false);
            }
            Input::Notice { id, text, rise } => self.notice(id, text, rise),
            Input::Press { id, down } => {
                self.pressed.retain(|key| key != &id);
                if down {
                    self.pressed.push(id.clone());
                }
                self.tap_lock = Some((id, self.now.saturating_add(SENT_MS)));
            }
            Input::Recording(id) => self.recording = id,
            Input::Camera(on) => self.camera = on,
            Input::Foreground(active) => {
                self.background = !active;
                if !active {
                    self.commit_delete();
                }
            }
            Input::RowsVisible(ids) => self.visible = ids.into_iter().collect(),
            Input::Flight(active) => self.flight = active,
            Input::RequestArrived(mut friend) => {
                friend.request = true;
                let id = friend.id.clone();
                if !self.order.contains(&id) {
                    self.order.push(id.clone());
                }
                self.friends.insert(id.clone(), friend);
                self.rise(id);
            }
            Input::RequestAccepted(id) => {
                if let Some(f) = self.friends.get_mut(&id) {
                    f.request = false;
                }
            }
            Input::RequestDeclined(id) => {
                self.friends.remove(&id);
                self.states.remove(&id);
                self.order.retain(|key| key != &id);
                self.proposed.retain(|key| key != &id);
            }
            Input::MediaSeen(id) => {
                self.states.entry(id).or_default().media.clear();
            }
            Input::MediaChanged { id, kinds } => {
                self.states.entry(id).or_default().media = kinds.into_iter().filter(|kind| *kind != Kind::Yo).collect();
            }
            Input::KeepAlive(id) => {
                if let Some(run) = self.states.entry(id).or_default().run.as_mut() {
                    run.elapsed = 0;
                    run.solid_count = run.count;
                }
            }
            Input::Delete(id) => {
                if self.friends.contains_key(&id) && !self.deleted.contains(&id) {
                    self.commit_delete();
                    let name = self.friends[&id].name.clone();
                    self.pending_delete = Some(Deletion { id, name, deadline: self.now.saturating_add(3_000) });
                    if self.background {
                        self.commit_delete();
                    }
                }
            }
            Input::UndoDelete => {
                self.pending_delete = None;
            }
            Input::Connectivity { offline, failure } => {
                self.offline = offline;
                self.failure = failure;
            }
            Input::Tick => {}
        }
        self.pressed.retain(|id| self.friends.contains_key(id));
        if self.recording.as_ref().is_some_and(|id| !self.friends.contains_key(id)) {
            self.recording = None;
        }
        self.flush();
        self.snapshot()
    }
    fn commit_delete(&mut self) {
        if let Some(deletion) = self.pending_delete.take() {
            self.deleted.insert(deletion.id.clone());
            self.committed_deletes.push(deletion.id);
        }
    }
    fn notice(&mut self, id: String, text: String, rise: bool) {
        self.sequence = self.sequence.saturating_add(1);
        let state = self.states.entry(id.clone()).or_default();
        let count = state.sent.as_ref().filter(|s| s.text == text).map_or(1, |s| s.count.saturating_add(1));
        state.sent = Some(Sent { text, count, until: self.now.saturating_add(SENT_MS), rise, sequence: self.sequence });
        self.tap_lock = Some((id, self.now.saturating_add(SENT_MS)));
    }
    fn rise(&mut self, id: String) {
        self.rising.retain(|key| key != &id);
        self.rising.push(id);
        if self.batch_at.is_none() {
            self.batch_at = Some(self.now.saturating_add(300));
        }
    }
    fn fade_start(run: &Run) -> u64 {
        YO_SOLID_MS.saturating_add(u64::from(run.solid_count.saturating_sub(1)) * run.step)
    }
    fn advance(&mut self, now: u64) {
        let now = now.max(self.now);
        // A pending move starts the visible clock at its deadline, even when no tick arrived then.
        // Expiring notices, undo windows and deletions can also settle pending moves.
        loop {
            let boundary = self
                .batch_at
                .filter(|_| !self.frozen())
                .into_iter()
                .chain(self.pending_delete.as_ref().map(|d| d.deadline))
                .chain(self.states.values().filter_map(|s| s.outgoing.map(|(_, _, until)| until)))
                .chain(self.states.values().filter_map(|s| s.sent.as_ref().filter(|s| s.rise).map(|s| s.until)))
                .min()
                .unwrap_or(now)
                .clamp(self.now, now);
            self.advance_to(boundary);
            self.flush();
            if boundary == now {
                break;
            }
        }
    }
    fn advance_to(&mut self, now: u64) {
        let elapsed = now - self.now;
        self.now = now;
        let mut rises = Vec::new();
        for (id, state) in &mut self.states {
            let watched = !self.camera
                && !self.background
                && !self.pending_delete.as_ref().is_some_and(|d| &d.id == id)
                && !state.run.as_ref().is_some_and(|r| r.waiting_order)
                && self.visible.contains(id)
                && !self.friends.get(id).is_some_and(|f| f.request);
            let seen = state.run.as_ref().is_some_and(|r| r.seen) && !self.friends.get(id).is_some_and(|f| f.request);
            if !watched && !seen {
                // A glimpse shorter than YO_SEEN_MS leaves the Yo fresh.
                if let Some(run) = &mut state.run {
                    run.elapsed = 0;
                }
            }
            if watched || seen {
                for sender in &mut state.senders {
                    sender.elapsed = sender.elapsed.saturating_add(elapsed).min(YO_SOLID_MS + YO_FADE_MS);
                }
                if let Some(run) = &mut state.run {
                    run.elapsed = run.elapsed.saturating_add(elapsed);
                    run.seen |= run.elapsed >= YO_SEEN_MS;
                    let removed =
                        if run.elapsed < YO_SOLID_MS { 0 } else { 1 + (run.elapsed - YO_SOLID_MS) / run.step.max(1) };
                    run.count = run
                        .solid_count
                        .saturating_sub(removed.min(u64::from(run.solid_count.saturating_sub(1))) as u32);
                    if run.elapsed >= Self::fade_start(run).saturating_add(YO_FADE_MS) {
                        state.run = None;
                        state.senders.clear();
                    }
                }
            }
            if state.outgoing.is_some_and(|(_, _, until)| until <= now) {
                state.outgoing = None;
            }
            if state.sent.as_ref().is_some_and(|s| s.until <= now) {
                if let Some(sent) = state.sent.take().filter(|s| s.rise) {
                    rises.push((sent.until, sent.sequence, id.clone()));
                }
            }
        }
        rises.sort();
        for (_, _, id) in rises {
            self.rise(id);
            self.batch_at = Some(now);
        }
        if self.pending_delete.as_ref().is_some_and(|d| d.deadline <= now) {
            self.commit_delete();
        }
    }
    fn frozen(&self) -> bool {
        self.pending_delete.is_some()
            || self.camera
            || !self.pressed.is_empty()
            || self.recording.is_some()
            || self.flight
            || self.states.values().any(|s| s.outgoing.is_some())
    }
    fn flush(&mut self) {
        if self.frozen() {
            return;
        }
        if !self.proposed.is_empty() {
            self.order = std::mem::take(&mut self.proposed);
        }
        if self.batch_at.is_some_and(|at| at <= self.now) {
            for id in self.rising.drain(..) {
                if self.friends.contains_key(&id) {
                    self.order.retain(|key| key != &id);
                    self.order.insert(0, id);
                }
            }
            self.batch_at = None;
            for state in self.states.values_mut() {
                if let Some(run) = &mut state.run {
                    run.waiting_order = false;
                }
            }
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut next = Vec::new();
        if let Some(d) = &self.pending_delete {
            next.push(d.deadline);
        }
        if !self.frozen() {
            if let Some(at) = self.batch_at {
                next.push(at.max(self.now));
            }
        }
        let locked = self.pressed.last().cloned().or_else(|| self.recording.clone()).or_else(|| {
            self.tap_lock
                .as_ref()
                .filter(|(_, until)| *until >= self.now)
                .filter(|(id, _)| self.friends.contains_key(id))
                .map(|(id, _)| id.clone())
        });
        if let Some((_, until)) = &self.tap_lock {
            if *until >= self.now {
                next.push(until.saturating_add(1));
            }
        }
        let rows = self
            .order
            .iter()
            .filter_map(|id| {
                if self.deleted.contains(id) || self.pending_delete.as_ref().is_some_and(|d| &d.id == id) {
                    return None;
                }
                let f = self.friends.get(id)?;
                let empty = State::default();
                let s = self.states.get(id).unwrap_or(&empty);
                let mut title = f.name.clone();
                let mut count = 1;
                if let Some(sent) = &s.sent {
                    next.push(sent.until);
                    title = sent.text.clone();
                    count = sent.count;
                    if title == "Sent 📷" || title == "Sent 🎥" {
                        title = "Sent Yo".into();
                    }
                }
                if let Some((_, n, _)) = s.outgoing {
                    title = "Sent Yo".into();
                    count = n;
                }
                if self.recording.as_ref() == Some(id) {
                    title = "● Recording".into();
                    count = 1;
                }
                if f.request {
                    title = format!("Yo Request{}{}", if f.group { ": " } else { " from " }, f.name);
                    count = 0;
                }
                if let Some((_, _, until)) = s.outgoing {
                    next.push(until);
                }
                let run = s.run.as_ref().filter(|_| !f.request);
                let clock_running = run.is_some_and(|r| r.seen)
                    || (!self.camera
                        && !self.background
                        && self.visible.contains(id)
                        && !f.request
                        && !run.is_some_and(|r| r.waiting_order));
                if clock_running {
                    for sender in &s.senders {
                        let boundary =
                            if sender.elapsed < YO_SOLID_MS { YO_SOLID_MS } else { YO_SOLID_MS + YO_FADE_MS };
                        if sender.elapsed < boundary {
                            next.push(self.now.saturating_add(boundary - sender.elapsed));
                        }
                    }
                    if let Some(r) = run {
                        let fade = Self::fade_start(r);
                        let boundary = if r.elapsed < YO_SOLID_MS {
                            YO_SOLID_MS
                        } else if r.elapsed < fade {
                            YO_SOLID_MS + ((r.elapsed - YO_SOLID_MS) / r.step.max(1) + 1) * r.step
                        } else {
                            fade + YO_FADE_MS
                        };
                        next.push(self.now.saturating_add(boundary.saturating_sub(r.elapsed)));
                    }
                }
                Some(Row {
                    clock_running,
                    id: id.clone(),
                    title,
                    count,
                    request: f.request,
                    sending: s.outgoing.is_some(),
                    undo_deadline: s.outgoing.map(|(_, _, until)| until),
                    notice_deadline: s.sent.as_ref().map(|sent| sent.until),
                    senders: if f.request { Vec::new() } else { s.senders.clone() },
                    yo_title: run.map_or(String::new(), |r| {
                        let bangs = r.count.saturating_sub(1);
                        format!(
                            "Yo{}{}",
                            "!".repeat(bangs.min(5) as usize),
                            if bangs > 5 { format!(" +{}", bangs - 5) } else { String::new() }
                        )
                    }),
                    yo_count: run.map_or(0, |r| r.count),
                    yo_total: run.map_or(0, |r| r.total),
                    yo_token: run.map_or(String::new(), |r| r.token.clone()),
                    fade_elapsed: run
                        .and_then(|r| (r.elapsed >= Self::fade_start(r)).then(|| r.elapsed - Self::fade_start(r))),
                    mark_count: if f.request {
                        0
                    } else {
                        run.filter(|r| r.elapsed < Self::fade_start(r))
                            .map_or(0, |r| r.count)
                            .max(s.media.len().min(u32::MAX as usize) as u32)
                    },
                    media_sign: if f.request {
                        String::new()
                    } else {
                        // A video among them shows 🎥 (as the notification says it), else what is there.
                        let video = s.media.contains(&Kind::Video);
                        s.media.first().map_or("", |&k| if video { Kind::Video } else { k }.sign()).into()
                    },
                })
            })
            .collect();
        Snapshot {
            banner: if self.pending_delete.is_some() {
                None
            } else if self.offline {
                Some("Offline".into())
            } else {
                self.failure.clone()
            },
            deletion: self.pending_delete.clone(),
            committed_deletes: self.committed_deletes.clone(),
            rows,
            locked_row: locked,
            next_tick: next.into_iter().min(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn friends(ids: &[&str]) -> Vec<Friend> {
        ids.iter()
            .map(|id| Friend { id: (*id).into(), name: id.to_uppercase(), request: false, group: false })
            .collect()
    }
    fn model() -> List {
        let mut list = List::new();
        list.apply(0, Input::Friends(friends(&["a", "b", "c"])));
        list.apply(0, Input::RowsVisible(vec!["a".into(), "b".into(), "c".into()]));
        list
    }
    fn receive(id: &str) -> Input {
        Input::Received { id: id.into(), kind: Kind::Yo, token: id.into(), sender: None }
    }
    fn row(list: &List, id: &str) -> Row {
        list.snapshot().rows.into_iter().find(|r| r.id == id).unwrap()
    }
    fn order(list: &List) -> Vec<String> {
        list.snapshot().rows.into_iter().map(|r| r.id).collect()
    }
    #[test]
    fn arrival_waits_for_sort_and_visible_time() {
        let mut l = model();
        l.apply(0, Input::RowsVisible(vec!["b".into()]));
        l.apply(0, Input::Press { id: "a".into(), down: true });
        l.apply(1, receive("b"));
        l.apply(10_000, Input::Tick);
        assert!(!row(&l, "b").clock_running);
        assert_eq!(row(&l, "b").yo_count, 1);
        assert_eq!(order(&l), ["a", "b", "c"]);
        l.apply(10_001, Input::Press { id: "a".into(), down: false });
        assert_eq!(order(&l)[0], "b");
        assert!(row(&l, "b").clock_running);
        l.apply(10_001, Input::RowsVisible(vec!["a".into()]));
        l.apply(20_000, Input::Tick);
        assert!(!row(&l, "b").clock_running);
        assert_eq!(row(&l, "b").fade_elapsed, None);
        l.apply(20_000, Input::RowsVisible(vec!["b".into()]));
        l.apply(23_000, Input::Tick);
        assert_eq!(row(&l, "b").fade_elapsed, Some(0));
        l.apply(25_000, Input::Tick);
        assert_eq!(row(&l, "b").yo_count, 0);
    }
    #[test]
    fn deletion_has_priority_and_reveals_latest_connection_notice() {
        let mut l = model();
        l.apply(0, Input::Connectivity { offline: true, failure: Some("Retry send".into()) });
        assert_eq!(l.snapshot().banner.as_deref(), Some("Offline"));
        l.apply(1, Input::Delete("b".into()));
        assert!(l.snapshot().banner.is_none());
        l.apply(2, Input::Connectivity { offline: false, failure: Some("Retry send".into()) });
        assert!(l.snapshot().banner.is_none());
        assert_eq!(l.apply(3, Input::UndoDelete).banner.as_deref(), Some("Retry send"));
        assert!(l.apply(4, Input::Connectivity { offline: false, failure: None }).banner.is_none());
    }
    #[test]
    fn deletion_undo_preserves_order_and_activity() {
        let mut l = model();
        l.apply(0, receive("b"));
        l.apply(300, Input::Tick);
        let before = row(&l, "b");
        let original = order(&l);
        let pending = l.apply(300, Input::Delete("b".into()));
        assert_eq!(pending.deletion.unwrap().deadline, 3300);
        assert!(!order(&l).contains(&"b".into()));
        l.apply(3299, Input::UndoDelete);
        assert_eq!(order(&l), original);
        assert_eq!(row(&l, "b").yo_count, before.yo_count);
        assert_eq!(row(&l, "b").yo_token, before.yo_token);
        assert!(l.apply(3300, Input::Tick).committed_deletes.is_empty());
    }
    #[test]
    fn deletion_commits_once_at_deadline_and_stays_hidden_until_source_updates() {
        let mut l = model();
        l.apply(10, Input::Delete("b".into()));
        assert!(l.apply(3009, Input::Tick).committed_deletes.is_empty());
        let done = l.apply(3010, Input::UndoDelete);
        assert_eq!(done.committed_deletes, ["b"]);
        assert!(done.deletion.is_none());
        assert_eq!(order(&l), ["a", "c"]);
        assert!(l.apply(3011, Input::Tick).committed_deletes.is_empty());
        l.apply(3012, Input::Friends(friends(&["a", "b", "c"])));
        assert_eq!(order(&l), ["a", "c"]);
        l.apply(3013, Input::Friends(friends(&["a", "c"])));
        l.apply(3014, Input::Friends(friends(&["a", "b", "c"])));
        assert!(order(&l).contains(&"b".into()));
    }
    #[test]
    fn replacement_and_background_commit_including_groups() {
        let mut l = model();
        l.apply(0, Input::RequestArrived(Friend { id: "g".into(), name: "Group".into(), request: false, group: true }));
        l.apply(0, Input::Delete("b".into()));
        let replaced = l.apply(1, Input::Delete("g".into()));
        assert_eq!(replaced.committed_deletes, ["b"]);
        assert_eq!(replaced.deletion.unwrap().id, "g");
        let background = l.apply(2, Input::Foreground(false));
        assert_eq!(background.committed_deletes, ["g"]);
        assert!(background.deletion.is_none());
        assert_eq!(order(&l), ["a", "c"]);
        assert!(l.apply(4000, Input::Tick).committed_deletes.is_empty());
    }
    #[test]
    fn latest_activity_first_in_one_batch() {
        let mut l = model();
        l.apply(0, receive("b"));
        l.apply(1, receive("c"));
        l.apply(2, receive("b"));
        assert_eq!(order(&l), ["a", "b", "c"]);
        l.apply(300, Input::Tick);
        assert_eq!(order(&l), ["b", "c", "a"]);
    }
    #[test]
    fn count_decay_and_fade_use_visible_time() {
        let mut l = model();
        for _ in 0..3 {
            l.apply(0, receive("a"));
        }
        assert_eq!(row(&l, "a").yo_title, "Yo!!");
        l.apply(300, Input::Tick);
        l.apply(3299, Input::Tick);
        assert_eq!(row(&l, "a").yo_count, 3);
        l.apply(3300, Input::Tick);
        assert_eq!(row(&l, "a").yo_count, 2);
        l.apply(3500, Input::Tick);
        assert_eq!(row(&l, "a").yo_count, 1);
        assert_eq!(row(&l, "a").fade_elapsed, None);
        l.apply(3700, Input::Tick);
        assert_eq!(row(&l, "a").fade_elapsed, Some(0));
        l.apply(5700, Input::Tick);
        assert_eq!(row(&l, "a").yo_count, 0);
    }
    #[test]
    fn fresh_yo_restores_run_but_fading_starts_new_run() {
        let mut l = model();
        l.apply(0, receive("a"));
        l.apply(0, receive("a"));
        l.apply(300, Input::Tick);
        l.apply(3300, Input::Tick);
        assert_eq!(row(&l, "a").yo_count, 1);
        l.apply(3301, receive("a"));
        assert_eq!(row(&l, "a").yo_count, 3);
        l.apply(3601, Input::Tick);
        l.apply(7001, Input::Tick);
        assert_eq!(row(&l, "a").fade_elapsed, Some(0));
        l.apply(7002, receive("a"));
        assert_eq!(row(&l, "a").yo_total, 1);
    }
    #[test]
    fn a_glimpse_leaves_the_yo_fresh() {
        let mut l = model();
        l.apply(0, receive("a"));
        l.apply(300, Input::Tick);
        l.apply(1299, Input::RowsVisible(vec!["b".into()]));
        l.apply(100_000, Input::Tick);
        assert_eq!(row(&l, "a").yo_count, 1);
        assert!(!row(&l, "a").clock_running);
        l.apply(100_000, Input::RowsVisible(vec!["a".into()]));
        l.apply(100_100, Input::Foreground(false));
        l.apply(200_000, Input::Foreground(true));
        l.apply(202_999, Input::Tick);
        assert_eq!(row(&l, "a").fade_elapsed, None, "both glimpses were too short; the clock starts afresh");
        l.apply(203_000, Input::Tick);
        assert_eq!(row(&l, "a").fade_elapsed, Some(0));
    }
    #[test]
    fn unseen_arrival_waits_for_viewport() {
        let mut l = model();
        l.apply(0, Input::RowsVisible(vec![]));
        l.apply(0, receive("a"));
        l.apply(100_000, Input::Tick);
        assert_eq!(row(&l, "a").yo_count, 1);
        assert_eq!(l.snapshot().next_tick, None);
        l.apply(100_000, Input::RowsVisible(vec!["a".into()]));
        assert_eq!(l.snapshot().next_tick, Some(103_000));
    }
    #[test]
    fn a_seen_yo_runs_in_real_time_away_and_off_screen() {
        for away in [Input::Foreground(false), Input::Camera(true), Input::RowsVisible(vec![])] {
            let back = match away {
                Input::Camera(_) => Input::Camera(false),
                Input::Foreground(_) => Input::Foreground(true),
                _ => Input::RowsVisible(vec!["a".into()]),
            };
            // Back before the fade ends: it continues at the right point.
            let mut l = model();
            l.apply(0, receive("a"));
            l.apply(300, Input::Tick);
            l.apply(3800, away.clone());
            assert_eq!(row(&l, "a").fade_elapsed, Some(500));
            assert!(row(&l, "a").clock_running);
            l.apply(4800, back.clone());
            assert_eq!(row(&l, "a").fade_elapsed, Some(1500));
            l.apply(5300, Input::Tick);
            assert_eq!(row(&l, "a").yo_count, 0);
            // Back hours later: gone at once.
            let mut l = model();
            l.apply(0, receive("a"));
            l.apply(300, Input::Tick);
            l.apply(1300, away.clone());
            assert_eq!(l.snapshot().next_tick, Some(3300), "the seen clock keeps ticking while away");
            let r = l.apply(3_600_000, back).rows.into_iter().find(|r| r.id == "a").unwrap();
            assert_eq!((r.yo_count, r.fade_elapsed, r.mark_count), (0, None, 0));
        }
    }
    #[test]
    fn yos_arriving_while_away_stay_fresh() {
        let mut l = model();
        l.apply(0, receive("a"));
        l.apply(300, Input::Tick);
        l.apply(4000, Input::Foreground(false));
        assert_eq!(row(&l, "a").fade_elapsed, Some(700));
        l.apply(4100, receive("a"));
        l.apply(4100, receive("b"));
        l.apply(3_600_000, Input::Tick);
        for id in ["a", "b"] {
            let r = row(&l, id);
            assert_eq!((r.yo_count, r.yo_total, r.fade_elapsed, r.clock_running), (1, 1, None, false));
        }
        l.apply(3_600_000, Input::Foreground(true));
        l.apply(3_602_999, Input::Tick);
        assert_eq!(row(&l, "a").fade_elapsed, None);
        l.apply(3_603_000, Input::Tick);
        assert_eq!(row(&l, "a").fade_elapsed, Some(0));
        assert_eq!(row(&l, "b").fade_elapsed, Some(0));
    }
    #[test]
    fn incoming_in_camera_is_immediate_but_countdown_waits_for_close() {
        let mut l = model();
        l.apply(0, Input::Camera(true));
        l.apply(100, receive("c"));
        l.apply(200, receive("c"));
        assert_eq!(row(&l, "c").yo_title, "Yo!");
        l.apply(50_000, Input::Tick);
        assert_eq!(row(&l, "c").yo_count, 2);
        assert_eq!(order(&l), ["a", "b", "c"]);
        l.apply(50_000, Input::Camera(false));
        assert_eq!(order(&l), ["c", "a", "b"]);
        l.apply(52_999, Input::Tick);
        assert_eq!(row(&l, "c").yo_count, 2);
        l.apply(53_000, Input::Tick);
        assert_eq!(row(&l, "c").yo_count, 1);
    }
    #[test]
    fn all_freezes_defer_both_source_order_and_arrival_moves() {
        for (start, stop) in [
            (Input::Press { id: "b".into(), down: true }, Input::Press { id: "b".into(), down: false }),
            (Input::Recording(Some("b".into())), Input::Recording(None)),
            (Input::Flight(true), Input::Flight(false)),
            (Input::Camera(true), Input::Camera(false)),
            (Input::SendStarted { id: "b".into(), kind: Kind::Photo, undo_ms: 2000 }, Input::SendUndone("b".into())),
        ] {
            let mut l = model();
            l.apply(0, start);
            l.apply(10, receive("c"));
            l.apply(20, Input::Friends(friends(&["b", "a", "c"])));
            l.apply(500, Input::Tick);
            assert_eq!(order(&l), ["a", "b", "c"]);
            assert_eq!(row(&l, "c").yo_count, 1);
            l.apply(500, stop);
            assert_eq!(order(&l), ["c", "b", "a"]);
        }
    }
    #[test]
    fn recording_freezes_even_when_the_hub_replaces_groups() {
        let mut l = model();
        l.apply(0, Input::Recording(Some("b".into())));
        l.apply(50, Input::Friends(friends(&["c", "b", "a"])));
        l.apply(5000, Input::Tick);
        assert_eq!(order(&l), ["a", "b", "c"]);
        assert_eq!(row(&l, "b").title, "● Recording");
        l.apply(5001, Input::Recording(None));
        assert_eq!(order(&l), ["c", "b", "a"]);
    }
    #[test]
    fn repeated_taps_lock_identity_while_lower_row_rises() {
        let mut l = model();
        l.apply(0, Input::Press { id: "b".into(), down: true });
        l.apply(1, receive("c"));
        l.apply(10, Input::Notice { id: "b".into(), text: "Sent Yo".into(), rise: true });
        l.apply(400, Input::Tick);
        assert_eq!(order(&l), ["a", "b", "c"]);
        let s = l.apply(410, Input::Press { id: "b".into(), down: false });
        assert_eq!(order(&l), ["c", "a", "b"]);
        assert_eq!(s.locked_row.as_deref(), Some("b"));
        l.apply(500, Input::Notice { id: "b".into(), text: "Sent Yo".into(), rise: true });
        assert_eq!(row(&l, "b").count, 2);
        l.apply(1510, Input::Tick);
        assert_eq!(order(&l), ["c", "a", "b"]);
        l.apply(2000, Input::Tick);
        assert_eq!(order(&l), ["b", "c", "a"]);
    }
    #[test]
    fn send_texts_counts_undo_and_failures() {
        let mut l = model();
        for kind in [Kind::Yo, Kind::Photo, Kind::Video] {
            for _ in 0..3 {
                l.apply(0, Input::SendFinished { id: "b".into(), kind });
            }
            let r = row(&l, "b");
            assert_eq!(r.title, "Sent Yo");
            assert_eq!(r.count, 3);
        }
        l.apply(0, Input::SendStarted { id: "c".into(), kind: Kind::Photo, undo_ms: 2000 });
        l.apply(1, Input::SendStarted { id: "c".into(), kind: Kind::Photo, undo_ms: 2000 });
        let r = row(&l, "c");
        assert_eq!(r.title, "Sent Yo");
        assert_eq!(r.count, 2);
        assert_eq!(r.undo_deadline, Some(2001));
        assert!(r.sending);
        l.apply(2, Input::SendUndone("c".into()));
        assert_eq!(row(&l, "c").title, "Canceled");
        assert!(!row(&l, "c").sending);
        l.apply(3, Input::SendFailed { id: "c".into(), text: "Not Sent".into() });
        assert_eq!(row(&l, "c").title, "Not Sent");
        l.apply(1503, Input::Tick);
        assert_eq!(row(&l, "c").title, "C");
        assert_eq!(order(&l)[0], "b");
        assert!(row(&l, "b").clock_running);
    }
    #[test]
    fn request_rows_hide_media_and_counts_until_acceptance() {
        let mut l = model();
        let mut f = friends(&["d"]).remove(0);
        f.group = true;
        l.apply(0, Input::RequestArrived(f));
        l.apply(0, Input::Received { id: "d".into(), kind: Kind::Video, token: "v".into(), sender: None });
        l.apply(0, Input::RowsVisible(vec!["d".into()]));
        l.apply(50_000, Input::Tick);
        let r = row(&l, "d");
        assert_eq!(r.title, "Yo Request: D");
        assert_eq!(r.count, 0);
        assert_eq!(r.yo_count, 0);
        assert_eq!(r.media_sign, "");
        l.apply(50_000, Input::RequestAccepted("d".into()));
        let r = row(&l, "d");
        assert_eq!(r.yo_count, 1);
        assert_eq!(r.media_sign, "🎥");
        l.apply(55_000, Input::Tick);
        assert_eq!(row(&l, "d").media_sign, "🎥");
        l.apply(55_000, Input::MediaSeen("d".into()));
        assert_eq!(row(&l, "d").media_sign, "");
        l.apply(55_000, Input::RequestDeclined("d".into()));
        assert!(!order(&l).contains(&"d".into()));
    }
    #[test]
    fn removal_and_renaming_keep_identity_and_drop_stale_moves() {
        let mut l = model();
        l.apply(0, receive("c"));
        l.apply(0, receive("b"));
        let mut fs = friends(&["a", "b"]);
        fs[1].name = "RENAMED".into();
        l.apply(100, Input::Friends(fs));
        l.apply(300, Input::Tick);
        assert_eq!(order(&l), ["b", "a"]);
        assert_eq!(row(&l, "b").title, "RENAMED");
        assert_eq!(row(&l, "b").yo_count, 1);
    }
    #[test]
    fn delayed_ticks_and_clock_regression_are_deterministic() {
        let mut a = model();
        let mut b = model();
        for l in [&mut a, &mut b] {
            for _ in 0..20 {
                l.apply(0, receive("b"));
            }
        }
        for now in (0..=6000).step_by(10) {
            a.apply(now, Input::Tick);
        }
        b.apply(300, Input::Tick);
        b.apply(6000, Input::Tick);
        assert_eq!(a.snapshot(), b.snapshot());
        let snapshot = b.snapshot();
        b.apply(1, Input::Tick);
        assert_eq!(snapshot, b.snapshot());
    }
    #[test]
    fn an_older_send_result_cannot_consume_a_newer_undo_stack() {
        let mut l = model();
        l.apply(0, Input::SendStarted { id: "b".into(), kind: Kind::Photo, undo_ms: 2000 });
        l.apply(2000, Input::Tick);
        l.apply(2100, Input::SendStarted { id: "b".into(), kind: Kind::Video, undo_ms: 2000 });
        l.apply(2200, Input::SendFinished { id: "b".into(), kind: Kind::Photo });
        assert_eq!(row(&l, "b").title, "Sent Yo");
        assert_eq!(row(&l, "b").undo_deadline, Some(4100));
        l.apply(2300, Input::SendFailed { id: "b".into(), text: "Not Sent".into() });
        assert!(row(&l, "b").sending);
        assert_eq!(row(&l, "b").title, "Sent Yo");
    }
    #[test]
    fn metadata_refresh_does_not_undo_activity_order() {
        let mut l = model();
        l.apply(0, receive("c"));
        let mut fs = friends(&["a", "b", "c"]);
        fs[2].name = "RENAMED".into();
        l.apply(500, Input::Friends(fs.clone()));
        assert_eq!(order(&l), ["c", "a", "b"]);
        fs[2].request = true;
        l.apply(600, Input::Friends(fs));
        assert_eq!(order(&l), ["c", "a", "b"]);
        assert_eq!(row(&l, "c").title, "Yo Request from RENAMED");
    }
    #[test]
    fn late_send_callback_for_removed_row_is_ignored() {
        let mut l = model();
        l.apply(0, Input::Friends(friends(&["a", "c"])));
        l.apply(0, Input::SendStarted { id: "b".into(), kind: Kind::Photo, undo_ms: 2000 });
        l.apply(0, receive("c"));
        l.apply(300, Input::Tick);
        assert_eq!(order(&l), ["c", "a"]);
    }
    #[test]
    fn due_batch_is_settled_before_a_new_press() {
        let mut l = model();
        l.apply(0, receive("c"));
        l.apply(1000, Input::Press { id: "b".into(), down: true });
        assert_eq!(order(&l), ["c", "a", "b"]);
        assert_eq!(l.snapshot().locked_row.as_deref(), Some("b"));
    }
    #[test]
    fn a_late_tick_spends_visible_time_from_the_move_deadline() {
        for undo_ms in [None, Some(0), Some(500)] {
            let mut on_time = model();
            let mut late = model();
            let moved_at = undo_ms.unwrap_or(300).max(300);
            for list in [&mut on_time, &mut late] {
                if let Some(undo_ms) = undo_ms {
                    list.apply(0, Input::SendStarted { id: "b".into(), kind: Kind::Photo, undo_ms });
                }
                list.apply(0, receive("c"));
            }
            on_time.apply(moved_at, Input::Tick);
            let now = moved_at + YO_SOLID_MS;
            assert_eq!(late.apply(now, Input::Tick), on_time.apply(now, Input::Tick));
            assert_eq!(row(&late, "c").fade_elapsed, Some(0));
        }
    }
    #[test]
    fn tap_lock_expires_without_another_user_event() {
        let mut l = model();
        l.apply(0, Input::Notice { id: "b".into(), text: "Sent Yo".into(), rise: true });
        let s = l.apply(1500, Input::Tick);
        assert_eq!(s.locked_row.as_deref(), Some("b"));
        assert_eq!(s.next_tick, Some(1501));
        let s = l.apply(1501, Input::Tick);
        assert_eq!(s.locked_row, None);
        assert_eq!(s.next_tick, None);
    }
    #[test]
    fn media_mark_uses_unseen_stack_and_shows_video_when_any() {
        let mut l = model();
        l.apply(0, receive("a"));
        l.apply(300, Input::Tick);
        l.apply(0, Input::MediaChanged { id: "a".into(), kinds: vec![Kind::Video, Kind::Photo, Kind::Yo] });
        assert_eq!(row(&l, "a").mark_count, 2);
        assert_eq!(row(&l, "a").media_sign, "🎥");
        l.apply(6300, Input::Tick);
        assert_eq!(row(&l, "a").mark_count, 2);
        l.apply(6300, Input::MediaChanged { id: "a".into(), kinds: vec![Kind::Photo] });
        assert_eq!(row(&l, "a").mark_count, 1);
        assert_eq!(row(&l, "a").media_sign, "📷");
        // Photos first, then a video: the video's sign.
        l.apply(6300, Input::MediaChanged { id: "a".into(), kinds: vec![Kind::Photo, Kind::Photo, Kind::Video] });
        assert_eq!(row(&l, "a").media_sign, "🎥");
        l.apply(6300, Input::MediaSeen("a".into()));
        assert_eq!(row(&l, "a").mark_count, 0);
    }
    #[test]
    fn group_reply_extends_current_count_without_resurrecting_bangs() {
        let mut l = model();
        for _ in 0..3 {
            l.apply(0, receive("a"));
        }
        l.apply(300, Input::Tick);
        l.apply(3300, Input::Tick);
        assert_eq!(row(&l, "a").yo_count, 2);
        l.apply(3301, Input::KeepAlive("a".into()));
        l.apply(4300, Input::Tick);
        assert_eq!(row(&l, "a").yo_count, 2);
        assert_eq!(row(&l, "a").yo_total, 3);
        l.apply(4301, receive("a"));
        assert_eq!(row(&l, "a").yo_count, 4);
    }
    #[test]
    fn removing_a_pressed_recording_row_cannot_leave_order_frozen() {
        let mut l = model();
        l.apply(0, Input::Press { id: "b".into(), down: true });
        l.apply(0, Input::Recording(Some("b".into())));
        l.apply(1, receive("c"));
        l.apply(500, Input::Friends(friends(&["a", "c"])));
        assert_eq!(order(&l), ["c", "a"]);
        assert_eq!(l.snapshot().locked_row, None);
    }
    #[test]
    fn unknown_group_senders_heal_without_resetting_counts_or_clocks() {
        let mut l = model();
        for user in ["one", "two", "one"] {
            l.apply(
                0,
                Input::Received { id: "b".into(), kind: Kind::Yo, token: user.into(), sender: Some(user.into()) },
            );
        }
        l.apply(300, Input::Tick);
        l.apply(1000, Input::Tick);
        let before = row(&l, "b");
        assert_eq!(before.senders.len(), 2);
        assert_eq!(before.senders[0].name, "?");
        l.apply(1000, Input::SenderNamed { user: "one".into(), name: "KNOWN".into() });
        let after = row(&l, "b");
        assert_eq!(after.senders[0].name, "KNOWN");
        assert_eq!(after.senders[0].count, 2);
        assert_eq!(after.senders[0].elapsed, before.senders[0].elapsed);
        assert_eq!(after.senders[1].name, "?");
        assert_eq!(after.yo_count, before.yo_count);
        l.apply(1000, Input::SenderNamed { user: "two".into(), name: "KNOWN".into() });
        assert_eq!(row(&l, "b").senders.len(), 2, "same names retain separate identities");
    }

    #[test]
    fn group_senders_use_the_same_viewport_clock() {
        let mut l = model();
        l.apply(0, Input::Received { id: "b".into(), kind: Kind::Yo, token: "1".into(), sender: Some("ONE".into()) });
        l.apply(300, Input::Tick);
        l.apply(
            1000,
            Input::Received { id: "b".into(), kind: Kind::Yo, token: "2".into(), sender: Some("TWO".into()) },
        );
        l.apply(1300, Input::Tick);
        l.apply(1500, Input::Camera(true));
        l.apply(100_000, Input::Tick);
        let senders = row(&l, "b").senders;
        assert_eq!(senders[0].elapsed, 900);
        assert_eq!(senders[1].elapsed, 200);
        l.apply(100_000, Input::Camera(false));
        l.apply(104_100, Input::Tick);
        assert_eq!(row(&l, "b").senders[0].elapsed, 5000);
    }
}
