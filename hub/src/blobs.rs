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
pub struct Reserved<'a> {
    blobs: &'a Blobs,
    size: u64,
}

impl Drop for Reserved<'_> {
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
        &self,
        size: u64,
        stored: impl FnOnce() -> Result<u64, E>,
    ) -> Result<Result<Reserved<'_>, NoRoom>, E> {
        let admission = self.admission.lock().unwrap_or_else(PoisonError::into_inner);
        let stored = stored()?;
        let incoming = self.incoming.fetch_add(size, Ordering::Relaxed) + size;
        drop(admission);
        let reserved = Reserved { blobs: self, size };
        Ok(self.room(reserved, stored, incoming))
    }

    fn room<'a>(&self, reserved: Reserved<'a>, stored: u64, incoming: u64) -> Result<Reserved<'a>, NoRoom> {
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

    /// Moves a complete upload into place.
    pub fn keep(&self, mut incoming: Incoming, blob: &Stored) -> io::Result<()> {
        let path = self.path(blob);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&incoming.path, &path)?;
        incoming.kept = true;
        Ok(())
    }

    /// Opens a blob of `size` bytes; none when the file is not there or not whole.
    pub async fn read(&self, blob: &Stored, size: u64) -> Option<tokio::fs::File> {
        let file = tokio::fs::File::open(self.path(blob)).await.ok()?;
        (file.metadata().await.ok()?.len() == size).then_some(file)
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

/// A blob's bytes as they are read, counting: when the last byte is handed on, the fetch counts (`done`). A fetch cut
/// off before does not.
pub struct Counted<S> {
    pub inner: S,
    pub left: u64,
    pub done: Option<Box<dyn FnOnce() + Send>>,
}

impl<S: Stream<Item = io::Result<Bytes>> + Unpin> Stream for Counted<S> {
    type Item = io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let polled = Pin::new(&mut self.inner).poll_next(cx);
        if let Poll::Ready(Some(Ok(chunk))) = &polled {
            self.left = self.left.saturating_sub(chunk.len() as u64);
            if self.left == 0 {
                if let Some(done) = self.done.take() {
                    done();
                }
            }
        }
        polled
    }
}

/// What a fetch that completed does: one fetch fewer, and the blob gone after the last.
pub fn fetched(db: &Arc<Db>, blobs: &Arc<Blobs>, id: BlobId, created_at: u64) {
    match db.blob_fetched(&id, created_at) {
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
        BlobPut::Quota => Some(StatusCode::TOO_MANY_REQUESTS),
        BlobPut::DeviceGone => Some(StatusCode::GONE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::sync::atomic::AtomicBool;

    fn counted(
        chunks: usize,
        done: Arc<AtomicBool>,
    ) -> Counted<futures_util::stream::Iter<std::vec::IntoIter<io::Result<Bytes>>>> {
        let items: Vec<io::Result<Bytes>> = (0..chunks).map(|_| Ok(Bytes::from(vec![0u8; 1000]))).collect();
        Counted {
            inner: futures_util::stream::iter(items),
            left: (chunks * 1000) as u64,
            done: Some(Box::new(move || done.store(true, Ordering::Relaxed))),
        }
    }

    #[tokio::test]
    async fn a_fetch_counts_only_when_its_last_byte_is_handed_on() {
        let done = Arc::new(AtomicBool::new(false));
        let mut stream = counted(3, done.clone());
        stream.next().await.unwrap().unwrap();
        stream.next().await.unwrap().unwrap();
        drop(stream); // cut off
        assert!(!done.load(Ordering::Relaxed));
        let mut stream = counted(3, done.clone());
        while stream.next().await.is_some() {}
        assert!(done.load(Ordering::Relaxed));
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
