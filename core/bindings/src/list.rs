//! Small synchronous presentation model: safe to call on the UI thread.
use std::sync::Mutex;
use yo_core::list::{self, Input};

#[derive(Clone, Copy, uniffi::Enum)]
pub enum ListKind {
    Yo,
    Photo,
    Video,
}
impl From<ListKind> for list::Kind {
    fn from(kind: ListKind) -> Self {
        match kind {
            ListKind::Yo => Self::Yo,
            ListKind::Photo => Self::Photo,
            ListKind::Video => Self::Video,
        }
    }
}
#[derive(uniffi::Record)]
pub struct ListFriend {
    pub id: String,
    pub name: String,
    pub request: bool,
    pub group: bool,
}
#[derive(uniffi::Record)]
pub struct ListRow {
    pub notice_deadline: Option<u64>,
    pub senders: Vec<ListSender>,
    pub id: String,
    pub title: String,
    pub count: u32,
    pub request: bool,
    pub sending: bool,
    pub undo_deadline: Option<u64>,
    pub yo_title: String,
    pub yo_count: u32,
    pub yo_total: u32,
    pub yo_token: String,
    pub fade_elapsed: Option<u64>,
    pub clock_running: bool,
    pub media_sign: String,
    pub mark_count: u32,
}
#[derive(uniffi::Record)]
pub struct ListSender {
    pub name: String,
    pub count: u32,
    pub elapsed: u64,
}
impl From<list::Row> for ListRow {
    fn from(row: list::Row) -> Self {
        Self {
            notice_deadline: row.notice_deadline,
            senders: row
                .senders
                .into_iter()
                .map(|s| ListSender { name: s.name, count: s.count, elapsed: s.elapsed })
                .collect(),
            id: row.id,
            title: row.title,
            count: row.count,
            request: row.request,
            sending: row.sending,
            undo_deadline: row.undo_deadline,
            yo_title: row.yo_title,
            yo_count: row.yo_count,
            yo_total: row.yo_total,
            yo_token: row.yo_token,
            fade_elapsed: row.fade_elapsed,
            clock_running: row.clock_running,
            media_sign: row.media_sign,
            mark_count: row.mark_count,
        }
    }
}
#[derive(uniffi::Record)]
pub struct ListDeletion {
    pub id: String,
    pub name: String,
    pub deadline: u64,
}
#[derive(uniffi::Record)]
pub struct ListSnapshot {
    pub banner: Option<String>,
    pub deletion: Option<ListDeletion>,
    pub committed_deletes: Vec<String>,
    pub rows: Vec<ListRow>,
    pub locked_row: Option<String>,
    pub next_tick: Option<u64>,
}
impl From<list::Snapshot> for ListSnapshot {
    fn from(s: list::Snapshot) -> Self {
        Self {
            banner: s.banner,
            deletion: s.deletion.map(|d| ListDeletion { id: d.id, name: d.name, deadline: d.deadline }),
            committed_deletes: s.committed_deletes,
            rows: s.rows.into_iter().map(Into::into).collect(),
            locked_row: s.locked_row,
            next_tick: s.next_tick,
        }
    }
}
#[derive(uniffi::Object, Default)]
pub struct YoList {
    state: Mutex<list::List>,
}
impl YoList {
    fn apply(&self, now: u64, input: Input) -> ListSnapshot {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).apply(now, input).into()
    }
}
#[uniffi::export]
impl YoList {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn friends(&self, now: u64, rows: Vec<ListFriend>) -> ListSnapshot {
        self.apply(
            now,
            Input::Friends(
                rows.into_iter()
                    .map(|f| list::Friend { id: f.id, name: f.name, request: f.request, group: f.group })
                    .collect(),
            ),
        )
    }
    pub fn received(
        &self,
        now: u64,
        id: String,
        kind: ListKind,
        token: String,
        sender: Option<String>,
    ) -> ListSnapshot {
        self.apply(now, Input::Received { id, kind: kind.into(), token, sender })
    }
    pub fn send_started(&self, now: u64, id: String, kind: ListKind, undo_ms: u64) -> ListSnapshot {
        self.apply(now, Input::SendStarted { id, kind: kind.into(), undo_ms })
    }
    pub fn send_finished(&self, now: u64, id: String, kind: ListKind) -> ListSnapshot {
        self.apply(now, Input::SendFinished { id, kind: kind.into() })
    }
    pub fn send_failed(&self, now: u64, id: String, text: String) -> ListSnapshot {
        self.apply(now, Input::SendFailed { id, text })
    }
    pub fn send_undone(&self, now: u64, id: String) -> ListSnapshot {
        self.apply(now, Input::SendUndone(id))
    }
    pub fn notice(&self, now: u64, id: String, text: String, rise: bool) -> ListSnapshot {
        self.apply(now, Input::Notice { id, text, rise })
    }
    pub fn press(&self, now: u64, id: String, down: bool) -> ListSnapshot {
        self.apply(now, Input::Press { id, down })
    }
    pub fn recording(&self, now: u64, id: Option<String>) -> ListSnapshot {
        self.apply(now, Input::Recording(id))
    }
    pub fn camera(&self, now: u64, on: bool) -> ListSnapshot {
        self.apply(now, Input::Camera(on))
    }
    pub fn foreground(&self, now: u64, active: bool) -> ListSnapshot {
        self.apply(now, Input::Foreground(active))
    }
    pub fn rows_visible(&self, now: u64, ids: Vec<String>) -> ListSnapshot {
        self.apply(now, Input::RowsVisible(ids))
    }
    pub fn flight(&self, now: u64, active: bool) -> ListSnapshot {
        self.apply(now, Input::Flight(active))
    }
    pub fn request_arrived(&self, now: u64, row: ListFriend) -> ListSnapshot {
        self.apply(
            now,
            Input::RequestArrived(list::Friend { id: row.id, name: row.name, request: true, group: row.group }),
        )
    }
    pub fn request_accepted(&self, now: u64, id: String) -> ListSnapshot {
        self.apply(now, Input::RequestAccepted(id))
    }
    pub fn request_declined(&self, now: u64, id: String) -> ListSnapshot {
        self.apply(now, Input::RequestDeclined(id))
    }
    pub fn media_seen(&self, now: u64, id: String) -> ListSnapshot {
        self.apply(now, Input::MediaSeen(id))
    }
    pub fn media_changed(&self, now: u64, id: String, kinds: Vec<ListKind>) -> ListSnapshot {
        self.apply(now, Input::MediaChanged { id, kinds: kinds.into_iter().map(Into::into).collect() })
    }
    pub fn keep_alive(&self, now: u64, id: String) -> ListSnapshot {
        self.apply(now, Input::KeepAlive(id))
    }
    pub fn delete(&self, now: u64, id: String) -> ListSnapshot {
        self.apply(now, Input::Delete(id))
    }
    pub fn undo_delete(&self, now: u64) -> ListSnapshot {
        self.apply(now, Input::UndoDelete)
    }
    pub fn connectivity(&self, now: u64, offline: bool, failure: Option<String>) -> ListSnapshot {
        self.apply(now, Input::Connectivity { offline, failure })
    }
    pub fn tick(&self, now: u64) -> ListSnapshot {
        self.apply(now, Input::Tick)
    }
}

#[derive(uniffi::Record)]
pub struct ListTimings {
    pub yo_solid_ms: u64,
    pub yo_fade_ms: u64,
    pub undo_ms: u64,
}
#[uniffi::export]
pub fn list_timings() -> ListTimings {
    ListTimings { yo_solid_ms: list::YO_SOLID_MS, yo_fade_ms: list::YO_FADE_MS, undo_ms: list::UNDO_MS }
}
