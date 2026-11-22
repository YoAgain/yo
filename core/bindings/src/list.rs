//! The list's presentation model (`yo_product::list`): synchronous, safe to call on the UI thread.

use std::sync::Mutex;
use yo_product::line::{self as flow, Flow, FlowWidths};
use yo_product::list::{self, Deletion, Friend, Input, Kind, Line, Row, Sender, Snapshot};
use yo_product::Language;

#[uniffi::remote(Enum)]
#[uniffi(name = "ListKind")]
pub enum Kind {
    Yo,
    Photo,
    Video,
}

#[uniffi::remote(Record)]
#[uniffi(name = "ListFriend")]
pub struct Friend {
    pub id: String,
    pub name: String,
    pub request: bool,
    pub group: bool,
}

#[uniffi::remote(Record)]
#[uniffi(name = "ListRow")]
pub struct Row {
    pub notice_deadline: Option<u64>,
    pub senders: Vec<Sender>,
    pub id: String,
    pub title: String,
    pub count: u32,
    pub request: bool,
    pub sending: bool,
    pub unsent: bool,
    pub undo_deadline: Option<u64>,
    pub yo_title: String,
    pub yo_count: u32,
    pub yo_total: u32,
    pub yo_token: String,
    pub fade_elapsed: Option<u64>,
    pub clock_running: bool,
    pub media_sign: String,
    pub mark_count: u32,
    pub line: Line,
}

#[uniffi::remote(Enum)]
#[uniffi(name = "ListLine")]
pub enum Line {
    Title,
    Arrival { count: u32, name: String },
    Senders { senders: Vec<Sender> },
}

#[uniffi::remote(Record)]
#[uniffi(name = "ListSender")]
pub struct Sender {
    pub user: String,
    pub name: String,
    pub count: u32,
}

#[uniffi::remote(Record)]
#[uniffi(name = "ListFlow")]
pub struct Flow {
    pub prefix: String,
    pub tokens: Vec<String>,
    pub separator: String,
    pub comma: String,
    pub more: Vec<String>,
}

#[uniffi::remote(Record)]
#[uniffi(name = "ListFlowWidths")]
pub struct FlowWidths {
    pub prefix: f32,
    pub tokens: Vec<f32>,
    pub separator: f32,
    pub comma: f32,
    pub more: Vec<f32>,
}

/// A bar's senders (`ListLine.senders`, newest first, or the one of `ListLine.arrival` when its line does not fit at
/// the bar's size) as the pieces to measure at the small size.
#[uniffi::export]
pub fn list_sender_flow(senders: Vec<Sender>, lang: Language) -> Flow {
    let senders: Vec<_> = senders.into_iter().map(|s| (s.name, s.count)).collect();
    flow::sender_flow(&senders, lang)
}

/// A bar's name as shown: a group's with one "#".
#[uniffi::export]
pub fn list_bar_title(name: String, group: bool) -> String {
    flow::bar_title(&name, group)
}

/// A bar title's words, to measure at the small size when it does not fit in one line.
#[uniffi::export]
pub fn list_title_words(text: String) -> Vec<String> {
    flow::title_words(&text)
}

/// The title's whole words over two small lines (the second may be empty).
#[uniffi::export]
pub fn list_wrap_two_lines(words: Vec<String>, widths: Vec<f32>, space: f32, room: f32) -> Vec<String> {
    flow::wrap_two_lines(&words, &widths, space, room).into()
}

/// The two small lines (the second may be empty) for the room the bar's words have.
#[uniffi::export]
pub fn list_flow_two_lines(flow: Flow, widths: FlowWidths, room: f32) -> Vec<String> {
    flow::flow_two_lines(&flow, &widths, room).into()
}

#[uniffi::remote(Record)]
#[uniffi(name = "ListDeletion")]
pub struct Deletion {
    pub id: String,
    pub name: String,
    pub deadline: u64,
}

#[uniffi::remote(Record)]
#[uniffi(name = "ListSnapshot")]
pub struct Snapshot {
    pub banner: Option<String>,
    pub deletion: Option<Deletion>,
    pub committed_deletes: Vec<String>,
    pub rows: Vec<Row>,
    pub locked_row: Option<String>,
    pub next_tick: Option<u64>,
}

#[derive(uniffi::Object, Default)]
pub struct YoList {
    state: Mutex<list::List>,
}
impl YoList {
    fn apply(&self, now: u64, input: Input) -> Snapshot {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).apply(now, input)
    }
}
#[uniffi::export]
impl YoList {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn friends(&self, now: u64, rows: Vec<Friend>) -> Snapshot {
        self.apply(now, Input::Friends(rows))
    }
    pub fn received(&self, now: u64, id: String, kind: Kind, token: String, sender: Option<String>) -> Snapshot {
        self.apply(now, Input::Received { id, kind, token, sender })
    }
    pub fn sender_named(&self, now: u64, user: String, name: String) -> Snapshot {
        self.apply(now, Input::SenderNamed { user, name })
    }
    pub fn send_started(&self, now: u64, id: String, kind: Kind, undo_ms: u64) -> Snapshot {
        self.apply(now, Input::SendStarted { id, kind, undo_ms })
    }
    pub fn send_finished(&self, now: u64, id: String, kind: Kind) -> Snapshot {
        self.apply(now, Input::SendFinished { id, kind })
    }
    pub fn send_failed(&self, now: u64, id: String, text: String) -> Snapshot {
        self.apply(now, Input::SendFailed { id, text })
    }
    /// Something kept for this bar has not gone (`on`), or nothing waits any more: "Not Sent · Tap to Retry" stays.
    pub fn unsent(&self, now: u64, id: String, on: bool) -> Snapshot {
        self.apply(now, Input::Unsent { id, on })
    }
    pub fn send_undone(&self, now: u64, id: String) -> Snapshot {
        self.apply(now, Input::SendUndone(id))
    }
    pub fn notice(&self, now: u64, id: String, text: String, rise: bool) -> Snapshot {
        self.apply(now, Input::Notice { id, text, rise })
    }
    pub fn press(&self, now: u64, id: String, down: bool) -> Snapshot {
        self.apply(now, Input::Press { id, down })
    }
    pub fn recording(&self, now: u64, id: Option<String>) -> Snapshot {
        self.apply(now, Input::Recording(id))
    }
    pub fn camera(&self, now: u64, on: bool) -> Snapshot {
        self.apply(now, Input::Camera(on))
    }
    pub fn foreground(&self, now: u64, active: bool) -> Snapshot {
        self.apply(now, Input::Foreground(active))
    }
    pub fn rows_visible(&self, now: u64, ids: Vec<String>) -> Snapshot {
        self.apply(now, Input::RowsVisible(ids))
    }
    pub fn flight(&self, now: u64, active: bool) -> Snapshot {
        self.apply(now, Input::Flight(active))
    }
    pub fn media_changed(&self, now: u64, id: String, kinds: Vec<Kind>) -> Snapshot {
        self.apply(now, Input::MediaChanged { id, kinds })
    }
    pub fn media_sender(&self, now: u64, id: String, user: String) -> Snapshot {
        self.apply(now, Input::MediaSender { id, user })
    }
    pub fn keep_alive(&self, now: u64, id: String) -> Snapshot {
        self.apply(now, Input::KeepAlive(id))
    }
    pub fn delete(&self, now: u64, id: String) -> Snapshot {
        self.apply(now, Input::Delete(id))
    }
    pub fn undo_delete(&self, now: u64) -> Snapshot {
        self.apply(now, Input::UndoDelete)
    }
    pub fn connectivity(&self, now: u64, offline: bool, failure: Option<String>) -> Snapshot {
        self.apply(now, Input::Connectivity { offline, failure })
    }
    pub fn tick(&self, now: u64) -> Snapshot {
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
