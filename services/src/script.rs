//! The script, one process per event: systemd starts a fresh one, locked in, for every connection to its socket
//! (`systemd/script.socket`). One JSON object in, one JSON list out, the end. One is started ahead, so nobody waits
//! for Python to start. The script is taken as hostile: what it says is bounded in time and size.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a script may take over one event.
const TIME: Duration = Duration::from_secs(10);
/// The most it may say: a drawn picture in base64, and some.
const MAX_ANSWER: u64 = 1024 * 1024;
/// One started ahead is replaced after this long (its unit ends it after 45 s, so none lingers).
const FRESH: Duration = Duration::from_secs(30);

pub struct Script {
    socket: PathBuf,
    ahead: Mutex<Option<(UnixStream, Instant)>>,
    warming: AtomicBool,
}

impl Script {
    pub fn new(socket: PathBuf) -> Script {
        Script { socket, ahead: Mutex::new(None), warming: AtomicBool::new(false) }
    }

    fn waiting(&self) -> std::sync::MutexGuard<'_, Option<(UnixStream, Instant)>> {
        self.ahead.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Starts one ahead, if none is.
    pub fn warm(&self) {
        // One worker at a time, and not under the lock: starting one holds up no other.
        if self.warming.swap(true, Ordering::AcqRel) {
            return;
        }
        let ready = self.waiting().as_ref().is_some_and(|(_, since)| since.elapsed() < FRESH);
        if !ready {
            let fresh = UnixStream::connect(&self.socket).ok().map(|stream| (stream, Instant::now()));
            *self.waiting() = fresh;
        }
        self.warming.store(false, Ordering::Release);
    }

    /// What the script says to `event`. One started ahead that is gone by now is replaced by a fresh one.
    pub fn call(&self, event: &[u8]) -> io::Result<Vec<u8>> {
        let end = Instant::now() + TIME;
        let ahead = self.waiting().take().filter(|(_, since)| since.elapsed() < FRESH);
        let answer = match ahead.map(|(stream, _)| ask(stream, event, end)) {
            Some(Ok(answer)) if !answer.is_empty() => Ok(answer),
            Some(Err(e)) if e.kind() == io::ErrorKind::TimedOut || e.kind() == io::ErrorKind::InvalidData => Err(e),
            _ => ask(UnixStream::connect(&self.socket)?, event, end),
        };
        self.warm();
        answer
    }
}

fn ask(mut stream: UnixStream, event: &[u8], end: Instant) -> io::Result<Vec<u8>> {
    stream.set_write_timeout(Some(TIME))?;
    // A script need not read what it is asked: it may have answered and ended by now.
    match stream.write_all(event).and_then(|()| stream.shutdown(Shutdown::Write)) {
        Err(e) if !ended(&e) => return Err(e),
        _ => {}
    }
    let (mut answer, mut chunk) = (Vec::new(), [0; 16 * 1024]);
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(left))?;
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(answer),
            Ok(n) => answer.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Err(io::ErrorKind::TimedOut.into()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if ended(&e) => return Ok(answer),
            Err(e) => return Err(e),
        }
        if answer.len() as u64 > MAX_ANSWER {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
}

fn ended(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset | io::ErrorKind::NotConnected)
}
