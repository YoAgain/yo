//! Photos and videos (spec 6.8): blobs the hub keeps for a chat's devices to fetch, encrypted under a key it never
//! learns. Files on disk, one per blob, never in SQLite (which holds a small row each: size, uploader, fetches left,
//! when). A blob goes when its last fetch is complete, or after its time (30 days), whatever comes first.
//!
//! An upload is signed by the body's hash, which comes in a header: the request is checked, and the room for it
//! counted, before the body is taken. The body goes to a file in `tmp/` as it comes, hashed on the way, and is moved
//! into place only when its hash is the signed one. A fetch is streamed from the file. No blob is ever held whole in
//! memory.

use crate::config::BlobConfig;
use crate::db::{BlobPut, Db};
use bytes::Bytes;
use futures_core::Stream;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;
use yo_wire::content::{BlobId, BLOB_ID_LEN};
use yo_wire::UserId;

/// A file in `tmp/` older than this belongs to no upload under way.
const TMP_KEPT: Duration = Duration::from_secs(3600);
/// Uploads of one user at once.
pub const UPLOADS_PER_USER: u32 = 4;
/// Uploads of all users at once.
pub const UPLOADS: u32 = 256;

pub struct Blobs {
    dir: PathBuf,
    budget: u64,
    min_free: u64,
    /// Bytes of uploads under way: counted against the budget before they are stored. Changed only under
    /// `admission`, so an upload being admitted never misses one that moves from coming in to stored meanwhile.
    incoming: AtomicU64,
    admission: Mutex<()>,
    /// Uploads under way, by user, and all.
    uploads: Mutex<HashMap<UserId, u32>>,
}

/// An upload under way of a user; its place is given back when it is dropped.
pub struct Slot<'a> {
    blobs: &'a Blobs,
    user: UserId,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut uploads = self.blobs.uploads.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = uploads.get_mut(&self.user) {
            *n -= 1;
            if *n == 0 {
                uploads.remove(&self.user);
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A stored blob: its id and when it was stored, which tells one upload under an id from another.
pub type Stored = (BlobId, u64);

/// A blob file's name back to the blob: `{hex id}.{stored at}`.
fn stored_of(name: &str) -> Option<Stored> {
    let (id, at) = name.split_once('.')?;
    Some((unhex(id)?, at.parse().ok()?))
}

fn unhex(text: &str) -> Option<BlobId> {
    if text.len() != 2 * BLOB_ID_LEN {
        return None;
    }
    let bytes: Option<Vec<u8>> =
        (0..BLOB_ID_LEN).map(|i| text.get(2 * i..2 * i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect();
    bytes?.try_into().ok()
}

/// Why there is no room for a blob now.
#[derive(Debug, PartialEq, Eq)]
pub enum NoRoom {
    /// The budget for all blobs, or the disk's free space, is used up.
    Full,
}

/// Room held for an upload under way; given back when it is dropped.
pub struct Reserved {
    blobs: Arc<Blobs>,
    size: u64,
}

impl Drop for Reserved {
    fn drop(&mut self) {
        // After the blob's row (if it was stored): for a moment it counts twice, never not at all.
        let _admission = self.blobs.admission.lock().unwrap_or_else(PoisonError::into_inner);
        self.blobs.incoming.fetch_sub(self.size, Ordering::Relaxed);
    }
}

/// A file in `tmp/`, deleted when dropped unless it was moved into place.
pub struct Incoming {
    pub path: PathBuf,
    kept: bool,
}

impl Drop for Incoming {
    fn drop(&mut self) {
        if !self.kept {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl Blobs {
    pub fn open(config: &BlobConfig) -> io::Result<Self> {
        std::fs::create_dir_all(config.dir.join("tmp"))?;
        Ok(Blobs {
            dir: config.dir.clone(),
            budget: config.budget_bytes,
            min_free: config.min_free_bytes,
            incoming: AtomicU64::new(0),
            admission: Mutex::default(),
            uploads: Mutex::default(),
        })
    }

    /// A place for one more upload of `user`, if it has fewer than `UPLOADS_PER_USER` under way and all users fewer
    /// than `UPLOADS`.
    pub fn uploading(&self, user: UserId) -> Option<Slot<'_>> {
        let mut uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
        let all: u32 = uploads.values().sum();
        let n = uploads.entry(user).or_default();
        if *n >= UPLOADS_PER_USER || all >= UPLOADS {
            if *n == 0 {
                uploads.remove(&user);
            }
            return None;
        }
        *n += 1;
        Some(Slot { blobs: self, user })
    }

    /// Where a blob lives: by the first byte of its id in one of 256 directories, named with when it was stored, so
    /// what is done to one upload under an id never touches another's file.
    fn path(&self, (id, at): &Stored) -> PathBuf {
        self.dir.join(hex(&id[..1])).join(format!("{}.{at}", hex(id)))
    }

    /// The disk's space free for this process.
    fn free_bytes(&self) -> io::Result<u64> {
        let stat = rustix::fs::statvfs(&self.dir)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }

    /// Holds room for `size` bytes, if the budget and the disk have it with what is stored (`stored()`, asked under
    /// the admission lock) and what is coming in.
    pub fn reserve<E>(
        self: &Arc<Self>,
        size: u64,
        stored: impl FnOnce() -> Result<u64, E>,
    ) -> Result<Result<Reserved, NoRoom>, E> {
        let admission = self.admission.lock().unwrap_or_else(PoisonError::into_inner);
        let stored = stored()?;
        let incoming = self.incoming.fetch_add(size, Ordering::Relaxed) + size;
        drop(admission);
        let reserved = Reserved { blobs: self.clone(), size };
        Ok(self.room(reserved, stored, incoming))
    }

    fn room(&self, reserved: Reserved, stored: u64, incoming: u64) -> Result<Reserved, NoRoom> {
        if stored.saturating_add(incoming) > self.budget {
            return Err(NoRoom::Full);
        }
        // An error asking the disk counts as no room: better a refusal than a full disk.
        let free = self.free_bytes().unwrap_or(0);
        if free < self.min_free.saturating_add(incoming) {
            return Err(NoRoom::Full);
        }
        Ok(reserved)
    }

    /// A new file in `tmp/` for an upload.
    pub fn incoming(&self) -> io::Result<(tokio::fs::File, Incoming)> {
        let mut name = [0u8; 16];
        getrandom::getrandom(&mut name).map_err(io::Error::other)?;
        let path = self.dir.join("tmp").join(hex(&name));
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        Ok((tokio::fs::File::from_std(file), Incoming { path, kept: false }))
    }

    /// Moves a complete upload into place, durably: its bytes, its name and a new shard directory are on the disk
    /// before the blob is marked ready and its upload answered (a power cut must not keep the row without the file).
    pub fn keep(&self, mut incoming: Incoming, blob: &Stored) -> io::Result<()> {
        std::fs::File::open(&incoming.path)?.sync_all()?;
        let path = self.path(blob);
        let shard = path.parent().unwrap_or(&self.dir);
        let new_shard = !shard.exists();
        std::fs::create_dir_all(shard)?;
        std::fs::rename(&incoming.path, &path)?;
        incoming.kept = true;
        sync_dir(shard)?;
        if new_shard {
            sync_dir(&self.dir)?;
        }
        Ok(())
    }

    /// Opens a blob of `size` bytes; none when the file is not there or not whole.
    pub async fn read(&self, blob: &Stored, size: u64) -> Option<tokio::fs::File> {
        let file = tokio::fs::File::open(self.path(blob)).await.ok()?;
        (file.metadata().await.ok()?.len() == size).then_some(file)
    }

    /// Whether a blob's file is in place.
    pub fn has(&self, blob: &Stored) -> bool {
        self.path(blob).is_file()
    }

    pub fn remove(&self, blob: &Stored) {
        let _ = std::fs::remove_file(self.path(blob));
    }

    /// Removes files the database does not know (a blob deleted with its account, an upload cut off by a crash) and
    /// uploads left in `tmp/`. The files are listed before the rows are read: a blob is in the database before its
    /// file is in place, so none stored meanwhile is taken for a stray.
    pub fn sweep(&self, db: &Db) -> io::Result<usize> {
        let mut files = Vec::new();
        for shard in std::fs::read_dir(&self.dir)? {
            let shard = shard?;
            if shard.file_name() == "tmp" || !shard.file_type()?.is_dir() {
                continue;
            }
            for file in std::fs::read_dir(shard.path())? {
                files.push(file?.path());
            }
        }
        let known: HashSet<Stored> = db.blob_ids().map_err(io::Error::other)?.into_iter().collect();
        let mut removed = 0;
        for path in files {
            let blob = path.file_name().and_then(|n| n.to_str()).and_then(stored_of);
            if blob.is_none_or(|blob| !known.contains(&blob)) && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        for file in std::fs::read_dir(self.dir.join("tmp"))? {
            let file = file?;
            let old = file.metadata()?.modified()?.elapsed().is_ok_and(|age| age > TMP_KEPT);
            if old && std::fs::remove_file(file.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// A fetch taken (`Db::take_fetch`), over when dropped: `done(true)` if its last chunk was handed on, else
/// `done(false)`. On a blocking thread (it writes to the database), never on the async workers.
pub struct Taken {
    done: Option<Box<dyn FnOnce(bool) + Send>>,
    complete: bool,
}

impl Taken {
    pub fn new(done: impl FnOnce(bool) + Send + 'static) -> Self {
        Taken { done: Some(Box::new(done)), complete: false }
    }
}

impl Drop for Taken {
    fn drop(&mut self) {
        if let Some(done) = self.done.take() {
            let complete = self.complete;
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => drop(runtime.spawn_blocking(move || done(complete))),
                Err(_) => done(complete),
            }
        }
    }
}

/// A blob's bytes as they are read, counting: a fetch counts once its last chunk is handed on (the server drops the
/// body right after it), one cut off before gives it back. Nothing is counted while a chunk still waits to go.
pub struct Counted<S> {
    inner: S,
    left: u64,
    taken: Taken,
}

impl<S> Counted<S> {
    pub fn new(inner: S, size: u64, taken: Taken) -> Self {
        Counted { inner, left: size, taken }
    }
}

impl<S: Stream<Item = io::Result<Bytes>> + Unpin> Stream for Counted<S> {
    type Item = io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let polled = std::task::ready!(Pin::new(&mut self.inner).poll_next(cx));
        if let Some(Ok(chunk)) = &polled {
            self.left = self.left.saturating_sub(chunk.len() as u64);
            // Handed on as it is returned: from here the fetch is complete.
            self.taken.complete = self.left == 0;
        }
        Poll::Ready(polled)
    }
}

/// What a fetch taken does when it is over: counted if complete (the blob gone after the last), else given back.
pub fn fetched(db: &Arc<Db>, blobs: &Arc<Blobs>, id: BlobId, created_at: u64, complete: bool) {
    match db.blob_fetched(&id, created_at, complete) {
        Ok(true) => blobs.remove(&(id, created_at)),
        Ok(false) => {}
        Err(error) => eprintln!("yo-hub: database: {error}"),
    }
}

/// The outcome of storing a blob's row, as the route answers it.
pub fn refused(put: &BlobPut) -> Option<axum::http::StatusCode> {
    use axum::http::StatusCode;
    match put {
        BlobPut::Stored => None,
        BlobPut::Taken => Some(StatusCode::CONFLICT),
        // Not "there" yet: a 409 would tell the uploader it is, and its message would name a blob nobody can fetch.
        BlobPut::Unfinished => Some(StatusCode::SERVICE_UNAVAILABLE),
        BlobPut::Quota => Some(StatusCode::TOO_MANY_REQUESTS),
        BlobPut::DeviceGone => Some(StatusCode::GONE),
    }
}

/// A directory's entries on the disk (a file moved or made in it).
fn sync_dir(dir: &std::path::Path) -> io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    /// A fetch of three chunks, which records how it ended.
    fn counted(
        ended: Arc<Mutex<Vec<bool>>>,
    ) -> Counted<futures_util::stream::Iter<std::vec::IntoIter<io::Result<Bytes>>>> {
        let items: Vec<io::Result<Bytes>> = (0..3).map(|_| Ok(Bytes::from(vec![0u8; 1000]))).collect();
        Counted::new(
            futures_util::stream::iter(items),
            3000,
            Taken::new(move |complete| ended.lock().unwrap().push(complete)),
        )
    }

    async fn poll_once<S: Stream + Unpin>(stream: &mut S) -> Poll<Option<S::Item>> {
        std::future::poll_fn(|cx| Poll::Ready(Pin::new(&mut *stream).poll_next(cx))).await
    }

    async fn ended_as(ended: &Mutex<Vec<bool>>, count: usize) -> Vec<bool> {
        while ended.lock().unwrap().len() < count {
            tokio::task::yield_now().await;
        }
        ended.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn a_fetch_counts_once_its_last_chunk_is_handed_on() {
        let ended = Arc::new(Mutex::new(Vec::new()));
        let mut stream = counted(ended.clone());
        stream.next().await.unwrap().unwrap();
        stream.next().await.unwrap().unwrap();
        drop(stream); // cut off
        assert_eq!(ended_as(&ended, 1).await, [false], "given back");
        let mut stream = counted(ended.clone());
        for _ in 0..3 {
            stream.next().await.unwrap().unwrap();
        }
        drop(stream); // as the server does after the last byte
        assert_eq!(ended_as(&ended, 2).await, [false, true], "counted");
        let mut stream = counted(ended.clone());
        for _ in 0..3 {
            stream.next().await.unwrap().unwrap();
        }
        assert!(stream.next().await.is_none());
        drop(stream);
        assert_eq!(ended_as(&ended, 3).await, [false, true, true], "once");
    }

    /// The last chunk was held back while the fetch was counted: cut off then, it counted (the blob deleted after its
    /// last fetch) though the last chunk never went. Now a fetch counts only once its last chunk is handed on.
    #[tokio::test]
    async fn a_fetch_cut_off_at_its_last_chunk_is_given_back() {
        let ended = Arc::new(Mutex::new(Vec::new()));
        let mut stream = counted(ended.clone());
        stream.next().await.unwrap().unwrap();
        stream.next().await.unwrap().unwrap();
        let handed = matches!(poll_once(&mut stream).await, Poll::Ready(Some(Ok(_))));
        drop(stream); // cut off right after the last chunk was asked for
        assert_eq!(ended_as(&ended, 1).await, [handed], "counted only if it was handed on");
        // A last chunk not read yet when cut off: given back.
        let first: Vec<io::Result<Bytes>> = vec![Ok(Bytes::from(vec![0u8; 1000]))];
        let inner = futures_util::stream::iter(first).chain(futures_util::stream::pending());
        let mut stream = Counted::new(inner, 2000, {
            let ended = ended.clone();
            Taken::new(move |complete| ended.lock().unwrap().push(complete))
        });
        stream.next().await.unwrap().unwrap();
        assert!(poll_once(&mut stream).await.is_pending());
        drop(stream);
        assert_eq!(ended_as(&ended, 2).await, [handed, false]);
    }

    #[test]
    fn a_user_has_a_few_uploads_at_once() {
        let blobs = Blobs::open(&crate::config::BlobConfig::for_tests()).unwrap();
        let held: Vec<Slot<'_>> = (0..UPLOADS_PER_USER).map(|_| blobs.uploading([1; 16]).unwrap()).collect();
        assert!(blobs.uploading([1; 16]).is_none());
        assert!(blobs.uploading([2; 16]).is_some(), "another user's are their own");
        drop(held);
        assert!(blobs.uploading([1; 16]).is_some());
        assert!(blobs.uploads.lock().unwrap().len() <= 1);
        let _ = std::fs::remove_dir_all(blobs.dir());
    }

    #[test]
    fn ids_and_their_file_names() {
        let id = [0xab; 16];
        assert_eq!(unhex(&hex(&id)), Some(id));
        assert_eq!(unhex("ab"), None);
        assert_eq!(unhex(&"zz".repeat(16)), None);
        assert_eq!(stored_of(&format!("{}.1762000000000", hex(&id))), Some((id, 1_762_000_000_000)));
        assert_eq!(stored_of(&hex(&id)), None);
    }
}
