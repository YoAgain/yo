//! Cross-version upgrade and process-crash tests. See tests/README.md.
#![cfg(target_os = "linux")]

use rusqlite::{ffi, Connection};
use std::{
    env,
    ffi::{c_char, c_void, CStr},
    fs,
    io::{self, BufRead, BufReader, Write},
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr,
    sync::{
        atomic::{AtomicPtr, AtomicUsize, Ordering::Relaxed},
        Arc, OnceLock,
    },
};
use yo::{LegacyStateSource, StateDb as AppDb, StateDiagnostics, YoError};
use yo_core::{
    media,
    statedb::{DbStore, Loaded, StateDb},
    Client, Content, Event,
};
use yo_tests::{random_u64, Http, TestHub};
use yo_wire::{api, envelope};

const KEY: [u8; 32] = [42; 32];
const OLD: &str = "71c671e0ecbf1342c7eb5c9e1d1ed79e06e24b6b";
const FORMATS: [&str; 3] = ["android", "ios-file", "ios-db"];

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let parent = env::var_os("TMPDIR").expect("set TMPDIR for this test");
        let dir = PathBuf::from(parent).join(format!("yo-upgrade-{}", random_u64()));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn checked(command: &mut Command) {
    assert!(command.status().unwrap().success(), "{command:?}");
}

fn build_old(path: &Path) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let repo = path.join("repo");
    let old = path.join("old");
    // A local clone lets git write worktree metadata even when the original .git is read-only.
    checked(Command::new("git").args(["clone", "--quiet", "--shared", "--no-checkout"]).arg(root).arg(&repo));
    checked(Command::new("git").arg("-C").arg(repo).args(["worktree", "add", "--detach"]).arg(&old).arg(OLD));
    let bin = old.join("tests/src/bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("upgrade-fixture.rs"), include_str!("../fixtures/db-tables/generate.rs")).unwrap();
    let target = env::var_os("YO_UPGRADE_OLD_TARGET").map_or_else(|| path.join("target"), PathBuf::from);
    checked(
        Command::new("nice")
            .args(["-n", "15", "cargo", "build", "--locked", "-j", "2", "-p", "yo-tests", "--bin", "upgrade-fixture"])
            .env("CARGO_BUILD_JOBS", "4")
            .env("CARGO_TARGET_DIR", &target)
            .current_dir(&old),
    );
    target.join("debug/upgrade-fixture")
}

#[derive(Clone, Copy)]
struct Source<'a> {
    dir: &'a Path,
    format: &'a str,
}

impl Source<'_> {
    fn legacy_file(self) -> PathBuf {
        self.dir.join(if self.format == "ios-file" { "ios-state" } else { "android-state" })
    }

    fn legacy_db(self) -> PathBuf {
        self.dir.join("ios-v0.sqlite")
    }

    fn snapshot(self) -> (Option<String>, Vec<u8>) {
        match self.format {
            "android" => (None, fs::read(self.legacy_file()).unwrap()),
            "ios-file" => {
                let raw = fs::read(self.legacy_file()).unwrap();
                (Some(String::from_utf8(raw[..36].to_vec()).unwrap()), raw[36..].to_vec())
            }
            "ios-db" => {
                let conn =
                    Connection::open_with_flags(self.legacy_db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
                let (word, blob) = conn
                    .query_row("SELECT word, blob FROM state WHERE id = 1", [], |row| Ok((row.get(0)?, row.get(1)?)))
                    .unwrap();
                (Some(word), blob)
            }
            _ => unreachable!(),
        }
    }

    fn open(self, path: &Path) -> Result<Arc<AppDb>, YoError> {
        let path = path.to_str().unwrap().to_owned();
        if self.format == "android" {
            AppDb::open_encrypted(path, KEY.to_vec(), Arc::new(AndroidFile(self.legacy_file())), Arc::new(Diagnostic))
        } else {
            let legacy_db = if self.format == "ios-db" { self.legacy_db() } else { self.dir.join("absent.sqlite") };
            let legacy_file =
                if self.format == "ios-file" { self.legacy_file() } else { self.dir.join("absent-state") };
            AppDb::open_migrating(
                path,
                KEY.to_vec(),
                legacy_db.to_str().unwrap().into(),
                legacy_file.to_str().unwrap().into(),
                36,
                None,
                Arc::new(Diagnostic),
            )
        }
    }
}

struct AndroidFile(PathBuf);

impl LegacyStateSource for AndroidFile {
    fn load(&self) -> Result<Option<Vec<u8>>, YoError> {
        // A successfully finished Android AtomicFile consists of the base file, whose bytes are the sealed blob.
        Ok(Some(fs::read(&self.0).unwrap()))
    }
}

struct Diagnostic;

impl StateDiagnostics for Diagnostic {
    fn failure(&self, step: u32, before: u32, after: u32, kind: String) {
        eprintln!("V{step:04}: {before} -> {after}: {kind}");
    }
}

fn verify(path: &Path, expected: &(Option<String>, Vec<u8>)) -> (Arc<StateDb>, Loaded) {
    let db = Arc::new(StateDb::open_current(path, KEY, Arc::new(|_, _, _, _| {})).unwrap().unwrap());
    let loaded = db.load(None, &KEY).unwrap();
    let Loaded::Newer { word, state, records } = &loaded else { panic!("missing migrated state") };
    let (old_state, old_records) = yo_core::state::open(&expected.1, &KEY).unwrap();
    assert!(expected.0.as_ref().is_none_or(|old| old == word));
    assert!(serde_json::to_value(&**state).unwrap() == serde_json::to_value(&old_state).unwrap(), "state differs");
    assert!(records == &old_records, "libsignal records differ");
    (db, loaded)
}

fn inspect_uncommitted(path: &Path) {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let version: u32 = connection.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    let tables: u32 = connection.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0)).unwrap();
    assert_eq!((version, tables), (0, 0));
}

fn verify_sealed(path: &Path) {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let key_type: String = conn.query_row("SELECT typeof(key) FROM groups", [], |row| row.get(0)).unwrap();
    assert_eq!(key_type, "blob", "group master key stored in plaintext");
    for (table, column) in [("groups", "key"), ("storage_groups", "value"), ("inbox_events", "value")] {
        let mut stmt = conn.prepare(&format!("SELECT {column} FROM {table}")).unwrap();
        let values: Vec<Vec<u8>> = stmt.query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert!(!values.is_empty(), "{table} fixture is empty");
        assert!(
            values.iter().all(|value| serde_json::from_slice::<serde_json::Value>(value).is_err()),
            "{table} stores plaintext"
        );
    }
}

// The trace only runs in the dedicated child. PROFILE fires after each SQL statement has run,
// including the statements in execute_batch and all of the bound inserts made by the Rust importer.
static MIGRATION_DB: AtomicPtr<ffi::sqlite3> = AtomicPtr::new(ptr::null_mut());
static STATEMENTS: AtomicUsize = AtomicUsize::new(0);
static STOP_AT: OnceLock<usize> = OnceLock::new();

fn await_resume() {
    let mut message = String::new();
    if io::stdin().read_line(&mut message).is_err() || message != "resume\n" {
        std::process::exit(2);
    }
}

unsafe extern "C" fn trace(event: u32, db: *mut c_void, statement: *mut c_void, _: *mut c_void) -> i32 {
    // SAFETY: SQLite supplies a live sqlite3_stmt for STMT and PROFILE events and owns the SQL string.
    let sql = unsafe { CStr::from_ptr(ffi::sqlite3_sql(statement.cast())) }.to_bytes();
    let db = db.cast::<ffi::sqlite3>();
    let before = event == ffi::SQLITE_TRACE_STMT as u32;
    if before && sql == b"PRAGMA journal_mode = WAL" && STOP_AT.get() == Some(&usize::MAX) {
        println!("upgrade:journal");
        await_resume();
    }
    if before && sql == b"BEGIN IMMEDIATE" && STATEMENTS.load(Relaxed) == 0 {
        println!("upgrade:begin");
        MIGRATION_DB.store(db, Relaxed);
    }
    if MIGRATION_DB.load(Relaxed) == db && (!before || sql == b"COMMIT") {
        let step = STATEMENTS.fetch_add(1, Relaxed) + 1;
        println!("upgrade:step:{step}{}", if before { ":commit" } else { "" });
        if STOP_AT.get() == Some(&step) {
            await_resume();
        }
        if before {
            MIGRATION_DB.store(ptr::null_mut(), Relaxed);
        }
    }
    0
}

unsafe extern "C" fn install_trace(
    db: *mut ffi::sqlite3,
    _: *mut *mut c_char,
    _: *const ffi::sqlite3_api_routines,
) -> i32 {
    // SAFETY: SQLite invokes this extension on an open connection. The context is that same handle.
    unsafe {
        ffi::sqlite3_trace_v2(db, (ffi::SQLITE_TRACE_STMT | ffi::SQLITE_TRACE_PROFILE) as u32, Some(trace), db.cast())
    }
}

#[test]
fn migration_child() {
    let Ok(path) = env::var("YO_UPGRADE_DESTINATION") else { return };
    let dir = env::var("YO_UPGRADE_SOURCE").unwrap();
    let format = env::var("YO_UPGRADE_FORMAT").unwrap();
    STOP_AT.set(env::var("YO_UPGRADE_STOP").unwrap().parse().unwrap()).unwrap();
    // SAFETY: the callback lives for the entire process and is registered before any SQLite connections.
    assert_eq!(unsafe { ffi::sqlite3_auto_extension(Some(install_trace)) }, ffi::SQLITE_OK);
    let db = Source { dir: Path::new(&dir), format: &format }.open(Path::new(&path)).unwrap();
    assert!(db.encrypted());
    assert!(db.has_state().unwrap());
}

fn spawn(source: Source<'_>, path: &Path, stop: usize) -> Child {
    // Keep executing the same test image even if Cargo replaces its on-disk path during another build.
    Command::new("/proc/self/exe")
        .args(["--exact", "migration_child", "--nocapture"])
        .env("YO_UPGRADE_SOURCE", source.dir)
        .env("YO_UPGRADE_FORMAT", source.format)
        .env("YO_UPGRADE_DESTINATION", path)
        .env("YO_UPGRADE_STOP", stop.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap()
}

fn baseline(source: Source<'_>, path: &Path) -> usize {
    let output = spawn(source, path, 0).wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let before_commit = stdout
        .lines()
        .find_map(|line| line.strip_prefix("upgrade:step:")?.strip_suffix(":commit")?.parse::<usize>().ok())
        .expect("a commit boundary");
    assert!(before_commit > 20);
    before_commit
}

fn wait_for(child: &mut Child, marker: &str) {
    // Keep the reader unbuffered beyond the delimiter so the next wait can reuse the pipe.
    let mut lines = BufReader::with_capacity(1, child.stdout.as_mut().unwrap()).lines();
    assert!(lines.any(|line| line.unwrap() == marker), "child exited before {marker}");
}

fn crash_and_retry(source: Source<'_>, expected: &(Option<String>, Vec<u8>), steps: usize) -> (Arc<StateDb>, Loaded) {
    let path = source.dir.join("crash.sqlite");
    for step in 1..=steps {
        let mut child = spawn(source, &path, step);
        let suffix = if step == steps { ":commit" } else { "" };
        wait_for(&mut child, &format!("upgrade:step:{step}{suffix}"));
        inspect_uncommitted(&path);
        assert!(source.snapshot() == *expected, "source changed before commit at step {step}");
        child.kill().unwrap();
        assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
        inspect_uncommitted(&path);
        assert!(source.snapshot() == *expected, "source changed after process death at step {step}");
        assert!(source.open(&path).unwrap().encrypted());
        let result = verify(&path, expected);
        if step == steps {
            return result;
        }
        fs::remove_file(&path).unwrap();
    }
    unreachable!("baseline includes a commit boundary")
}

fn resume(child: &mut Child) {
    child.stdin.take().unwrap().write_all(b"resume\n").unwrap();
}

fn both_open(children: [Child; 2]) {
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    }
}

fn concurrent_open(source: Source<'_>, expected: &(Option<String>, Vec<u8>), steps: usize) {
    let path = source.dir.join("concurrent.sqlite");
    let mut app = spawn(source, &path, steps);
    wait_for(&mut app, &format!("upgrade:step:{steps}:commit"));
    let mut extension = spawn(source, &path, 0);
    wait_for(&mut extension, "upgrade:begin");
    inspect_uncommitted(&path);
    resume(&mut app);
    both_open([app, extension]);
    verify(&path, expected);

    // Both processes have read version zero before either one tries to switch the fresh file to WAL.
    let path = source.dir.join("cold-concurrent.sqlite");
    let mut app = spawn(source, &path, usize::MAX);
    let mut extension = spawn(source, &path, usize::MAX);
    for child in [&mut app, &mut extension] {
        wait_for(child, "upgrade:journal");
    }
    resume(&mut app);
    resume(&mut extension);
    both_open([app, extension]);
    verify(&path, expected);
}

fn refuse_future(source: Source<'_>, path: &Path, expected: &(Option<String>, Vec<u8>)) {
    let connection = Connection::open(path).unwrap();
    connection.pragma_update(None, "user_version", 2).unwrap();
    drop(connection);
    let before = fs::read(path).unwrap();
    let error = source.open(path).err().expect("future schema opened");
    assert!(matches!(error, YoError::Save { message } if message == "UnsupportedVersion"));
    assert!(fs::read(path).unwrap() == before, "refusal modified the future database");
    assert!(source.snapshot() == *expected, "refusal modified the source");
}

fn texts(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Message { content: Content::Text(text), .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn resume_client(db: Arc<StateDb>, http: Arc<Http>, loaded: Loaded) -> Client {
    let Loaded::Newer { word, state, records } = loaded else { panic!("missing account") };
    let (mut app, changed) = Client::from_snapshot(http, *state, records, false).unwrap();
    let store = Arc::new(DbStore::new(db, Some(word), None));
    if changed {
        app.persist_to(store, KEY).unwrap();
    } else {
        app.keep_in(store, KEY);
    }
    app
}

fn ratchet_only(http: &Http, peer: &Client) {
    let to = api::deliver_path(&yo_tests::user_id(peer.user_id()));
    let sent = http.sent.lock().unwrap();
    let mut count = 0;
    for (_, _, bytes) in sent.iter().filter(|(method, path, _)| method == "POST" && *path == to) {
        let (plain, _) = yo_core::sealed::testing::open(&peer.account_secret(), bytes).unwrap();
        let payload: envelope::Payload = yo_wire::decode(&plain).unwrap();
        assert!(!payload.copies.is_empty());
        assert!(payload.copies.iter().all(|copy| copy.kind == envelope::KIND_MESSAGE), "session restarted");
        count += 1;
    }
    assert!(count > 0, "no delivery to inspect");
}

fn continue_account(hub: &TestHub, source: Source<'_>, db: Arc<StateDb>, loaded: Loaded) {
    let Loaded::Newer { state, records, .. } = &loaded else { panic!("missing migrated data") };
    assert_eq!(state.contacts.len(), 3);
    assert!(state.contacts.values().all(|contact| contact.profile_key.is_some()));
    assert!(state.contacts.values().any(|contact| contact.verified));
    assert!(state.profile_key.is_some());
    assert_eq!(state.name.as_deref(), Some("Upgrade Fixture"));
    assert!(!state.requests_open);
    assert_eq!(state.groups.len(), 1);
    assert_eq!(state.outbox.len(), 2);
    assert!(state.outbox.iter().all(|item| item.payload.is_some()));
    assert!(records.keys().filter(|key| key.starts_with("s/")).count() >= 3);
    let group = state.groups.keys().next().unwrap().clone();

    let http = Http::new(hub.url());
    let mut app = resume_client(db.clone(), http.clone(), loaded);
    assert_eq!(app.left_handed(), Some(true));
    assert_eq!(app.app_icon().as_deref(), Some("amethyst"));
    let inbox = app.signed_for_tests("GET", &api::inbox_path(0), &[]).unwrap();
    assert_eq!(inbox.status, api::status::OK);
    let inbox = yo_wire::decode::<api::Inbox>(&inbox.body).unwrap();
    assert!(inbox.items.len() >= 2, "handed message and unread photo still at the hub");
    let mut peers: Vec<_> = (0..3)
        .map(|index| {
            let blob = fs::read(source.dir.join(format!("peer-{index}"))).unwrap();
            Client::restore(Http::new(hub.url()), &blob, &KEY).unwrap()
        })
        .collect();
    let events = app.sync(0).unwrap();
    assert_eq!(texts(&events), ["handed but unacked"]);
    let photos: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::Message { content: Content::Photo(photo), .. } => Some(photo),
            _ => None,
        })
        .collect();
    assert_eq!(photos.len(), 1);
    verify_sealed(db.path());
    assert_eq!(media::fetch_open(http.as_ref(), photos[0]).unwrap().unwrap(), b"upgrade photo");
    media::ack(http.as_ref(), &photos[0].id).unwrap();
    app.acknowledge_events();
    app.after_sync().unwrap();
    assert_eq!(app.waiting(), 0);
    let acked: Vec<_> = http
        .sent
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, path, _)| path == api::INBOX_ACK)
        .flat_map(|(_, _, body)| yo_wire::decode::<api::Ack>(body).unwrap().ids)
        .collect();
    assert!(inbox.items.iter().all(|item| acked.contains(&item.id)));
    for (index, peer) in peers.iter_mut().enumerate() {
        let events = peer.sync(0).unwrap();
        let expected: Vec<_> = if index < 2 { vec![format!("queued {index}")] } else { vec![] };
        assert_eq!(texts(&events), expected);
        peer.after_sync().unwrap();
    }

    // A second open must retain the drained queue and the app's acknowledgement.
    drop(app);
    let mut app = resume_client(db.clone(), http.clone(), db.load(None, &KEY).unwrap());
    assert_eq!(app.waiting(), 0);
    let sent_before = http.sent.lock().unwrap().len();
    let events = app.sync(0).unwrap();
    assert!(texts(&events).is_empty());
    assert!(!events.iter().any(|event| matches!(event, Event::Message { content: Content::Photo(_), .. })));
    assert!(!http.sent.lock().unwrap()[sent_before..].iter().any(|(method, path, _)| api::anonymous(method, path)));
    for peer in &mut peers {
        assert!(texts(&peer.sync(0).unwrap()).is_empty());
    }
    for peer in &mut peers {
        app.send(peer.user_id(), Content::Text("new to peer".into())).unwrap();
        assert_eq!(texts(&peer.sync(0).unwrap()), ["new to peer"]);
        peer.send(app.user_id(), Content::Text("back to upgraded".into())).unwrap();
        assert_eq!(texts(&app.sync(0).unwrap()), ["back to upgraded"]);
        assert_eq!(peer.dropped(), 0, "{:?}", peer.drop_reasons());
    }
    app.send_group(&group, Content::Yo).unwrap();
    for peer in &mut peers {
        let events = peer.sync(0).unwrap();
        assert_eq!(events.iter().filter(|event| matches!(
            event, Event::GroupMessage { group: id, user, content: Content::Yo, .. } if id == &group && user == app.user_id()
        )).count(), 1);
        ratchet_only(&http, peer);
    }
    assert_eq!(app.dropped(), 0, "{:?}", app.drop_reasons());
}

#[test]
#[ignore = "requires the pre-V0001 Git revision and a nested Cargo build; see tests/README.md"]
fn old_client_state_survives_v0001() {
    let scratch = Scratch::new();
    let old = build_old(&scratch.0);
    for format in FORMATS {
        let dir = scratch.0.join(format);
        fs::create_dir(&dir).unwrap();
        let mut config = yo_hub::config::Config::for_tests();
        config.blobs.dir = dir.join("blobs");
        let hub = TestHub::start_with(config, "127.0.0.1:0");
        checked(Command::new(&old).arg(hub.url()).arg(&dir));

        let source = Source { dir: &dir, format };
        let expected = source.snapshot();
        let path = dir.join("baseline.sqlite");
        let steps = baseline(source, &path);
        println!("{format}: {steps} crash boundaries");
        verify(&path, &expected);
        verify_sealed(&path);
        concurrent_open(source, &expected, steps);
        refuse_future(source, &dir.join("concurrent.sqlite"), &expected);
        let (db, loaded) = crash_and_retry(source, &expected, steps);
        continue_account(&hub, source, db, loaded);
    }
}
