//! Photos and videos (spec 6.8): one file each, streamed in and out, never held whole in memory.

use crate::config::BlobConfig;
use crate::db::{BlobPut, Db};
use crate::error::{refuse, Refusal, Res};
use crate::limiter::{take_place, Place, Places};
use crate::now_ms;
use crate::request::{b64_array, bad, header, octets, path_id, query, Hubs, Peer, DAY_MS, MINUTE};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::header::CONTENT_LENGTH;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use bytes::Bytes;
use futures_core::Stream;
use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;
use yo_wire::api::HEADER_BODY_HASH;
use yo_wire::content::{BlobId, BLOB_ID_LEN};
use yo_wire::{limits, UserId};

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
    /// Bytes of uploads under way; changed only under `admission`, so no upload in between is missed.
    incoming: AtomicU64,
    admission: Mutex<()>,
    /// Uploads under way, by user.
    uploads: Arc<Places>,
    /// Blob ids whose upload is under way, from its check until its row is written.
    claimed: Arc<Places>,
}

/// An upload under way of a user; its place is given back when it is dropped.
pub type Slot<'a> = Place;

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

pub(crate) fn unhex(text: &str) -> Option<BlobId> {
    if text.len() != 2 * BLOB_ID_LEN {
        return None;
    }
    let bytes: Option<Vec<u8>> =
        (0..BLOB_ID_LEN).map(|i| text.get(2 * i..2 * i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect();
    bytes?.try_into().ok()
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
            uploads: Arc::default(),
            claimed: Arc::default(),
        })
    }

    /// Holds `id` for one upload: `None` while another upload of it is under way.
    pub fn claim(&self, id: BlobId) -> Option<Place> {
        take_place(&self.claimed, id, 1, None)
    }

    /// A place for one more upload of `user`, within `UPLOADS_PER_USER` and `UPLOADS`.
    pub fn uploading(&self, user: UserId) -> Option<Slot<'_>> {
        take_place(&self.uploads, user, UPLOADS_PER_USER, Some(UPLOADS))
    }

    /// A blob's file: in one of 256 directories, named with when it was stored, so uploads under one id never meet.
    fn path(&self, (id, at): &Stored) -> PathBuf {
        self.dir.join(hex(&id[..1])).join(format!("{}.{at}", hex(id)))
    }

    /// Holds room for `size` bytes if the budget and the disk have it, with what is stored and what is coming in.
    pub fn reserve<E>(
        self: &Arc<Self>,
        size: u64,
        stored: impl FnOnce() -> Result<u64, E>,
    ) -> Result<Option<Reserved>, E> {
        let admission = self.admission.lock().unwrap_or_else(PoisonError::into_inner);
        let stored = stored()?;
        let incoming = self.incoming.fetch_add(size, Ordering::Relaxed) + size;
        drop(admission);
        let reserved = Reserved { blobs: self.clone(), size };
        // An error asking the disk counts as no room: better a refusal than a full disk.
        let free = || rustix::fs::statvfs(&self.dir).map_or(0, |stat| stat.f_bavail.saturating_mul(stat.f_frsize));
        Ok((stored.saturating_add(incoming) <= self.budget && free() >= self.min_free.saturating_add(incoming))
            .then_some(reserved))
    }

    /// A new file in `tmp/` for an upload.
    pub fn incoming(&self) -> io::Result<(tokio::fs::File, Incoming)> {
        let mut name = [0u8; 16];
        getrandom::fill(&mut name).map_err(io::Error::other)?;
        let path = self.dir.join("tmp").join(hex(&name));
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        Ok((tokio::fs::File::from_std(file), Incoming { path, kept: false }))
    }

    /// Moves a complete upload into place durably: a power cut must not keep the row without the file.
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

    /// Removes files the database does not know, and old uploads in `tmp/`; files are listed before rows are read.
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

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }
}

/// A GET slot taken (`Db::take_blob_get`): `done(complete)` when dropped, on a blocking thread.
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

/// A blob's bytes as read: a fetch counts once its last chunk is handed on, one cut off before gives it back.
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

/// The outcome of storing a blob's row, as the route answers it.
fn refused(put: &BlobPut) -> Option<StatusCode> {
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

// ---- routes ----

/// The longest an upload may take: a whole blob over a slow line, and no longer.
const UPLOAD_TIME: Duration = Duration::from_secs(300);

/// `PUT /v1/blobs/{id}?fetches=n`: signed by its body's hash, checked before the body is taken, kept only if it holds.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn put_blob(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Body,
) -> Res<StatusCode> {
    use futures_util::StreamExt;
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;
    let id: BlobId = path_id(&id)?;
    let size: u64 = header(&headers, CONTENT_LENGTH)
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| refuse(StatusCode::LENGTH_REQUIRED))?;
    if size > limits::MAX_BLOB_LEN as u64 {
        return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
    }
    // A ChaCha20-Poly1305 ciphertext has its 16-byte tag at least.
    if size < 16 {
        return Err(bad());
    }
    let fetches = query(&uri, "fetches").filter(|n| (1..=limits::MAX_BLOB_FETCHES).contains(n)).ok_or_else(bad)?;
    let body_hash: [u8; 32] = header(&headers, HEADER_BODY_HASH).and_then(b64_array).ok_or_else(bad)?;
    let limits = &hub.config.limits;
    hub.limit_ip("blob:put", peer, &headers, limits.blob_put_per_ip_minute, MINUTE)?;
    let (device, _claim, quota) = hub
        .blocking(move |hub| {
            let device = hub.authenticate_hash(&method, &uri, &headers, &body_hash)?;
            // Held before the body comes, so a second upload of the same id is "later" at once (spec 6.8).
            let claim = hub.blobs.claim(id).ok_or_else(|| refuse(StatusCode::SERVICE_UNAVAILABLE))?;
            match hub.db.blob_ready_state(&id)? {
                Some(true) => return Err(refuse(StatusCode::CONFLICT)),
                // Another upload of it is not done: "later", not "there" (spec 6.6).
                Some(false) => return Err(refuse(StatusCode::SERVICE_UNAVAILABLE)),
                None => {}
            }
            // The operator's bot accounts upload without a quota of their own (only the disk's room holds them).
            let quota = match hub.bot(&device.user_id) {
                Some(_) => crate::config::Limits::unlimited().blob_quota(),
                None => hub.config.limits.blob_quota(),
            };
            if !hub.db.blob_quota_left(&device.user_id, size, now_ms(), &quota)? {
                return Err(refuse(StatusCode::TOO_MANY_REQUESTS));
            }
            Ok((device, claim, quota))
        })
        .await??;
    // A few uploads of one user at once, no more: none can hold the room of everyone's.
    let _slot = hub.blobs.uploading(device.user_id).ok_or_else(|| refuse(StatusCode::TOO_MANY_REQUESTS))?;
    let failed = |error: std::io::Error| {
        eprintln!("yo-hub: blob upload: {error}");
        refuse(StatusCode::INTERNAL_SERVER_ERROR)
    };
    let (room, mut file, incoming) = hub
        .blocking(move |hub| {
            let room = hub.blobs.reserve(size, || hub.db.blobs_stored())?;
            let room = room.ok_or_else(|| refuse(StatusCode::INSUFFICIENT_STORAGE))?;
            let (file, incoming) = hub.blobs.incoming().map_err(failed)?;
            Ok::<_, Refusal>((room, file, incoming))
        })
        .await??;
    let taken = async {
        let mut stream = body.into_data_stream();
        let mut hash = sha2::Sha256::new();
        let mut got = 0u64;
        while let Some(chunk) = stream.next().await {
            // The client went away, or sent something that is no body.
            let chunk = chunk.map_err(|_| bad())?;
            got += chunk.len() as u64;
            if got > size {
                return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE));
            }
            hash.update(&chunk);
            file.write_all(&chunk).await.map_err(failed)?;
        }
        file.flush().await.map_err(failed)?;
        if got != size || <[u8; 32]>::from(hash.finalize()) != body_hash {
            return Err(bad());
        }
        Ok(())
    };
    tokio::time::timeout(UPLOAD_TIME, taken).await.map_err(|_| refuse(StatusCode::REQUEST_TIMEOUT))??;
    drop(file);
    hub.blocking(move |hub| {
        // The room is held until the blob's row counts it.
        let _room = room;
        let stored_at = now_ms();
        let put = hub.db.put_blob(&device.device_id, &id, size, fetches, stored_at, &quota)?;
        if let Some(status) = refused(&put) {
            return Err(refuse(status));
        }
        if let Err(error) = hub.blobs.keep(incoming, &(id, stored_at)) {
            let _ = hub.db.delete_blob(&id);
            return Err(failed(error));
        }
        // Handed out from now on. Left not ready (a crash just before), it goes at the next start or within the hour.
        match hub.db.blob_ready(&id, stored_at) {
            Ok(true) => {
                hub.count_upload(&device.user_id, size);
                Ok(StatusCode::CREATED)
            }
            // Its row went meanwhile (the account deleted while it came): no file left behind, and no 201.
            Ok(false) => {
                hub.blobs.remove(&(id, stored_at));
                Err(refuse(StatusCode::NOT_FOUND))
            }
            Err(error) => {
                let _ = hub.db.delete_blob(&id);
                hub.blobs.remove(&(id, stored_at));
                Err(error.into())
            }
        }
    })
    .await?
}

/// `GET /v1/blobs/{id}`, anonymous: spends only the GET budget; a cut-off stream gives it back (`Taken`).
pub(crate) async fn get_blob(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<Response> {
    hub.limit_ip("blob:get", peer, &headers, hub.config.limits.blob_get_per_ip_minute, MINUTE)?;
    let id: BlobId = path_id(&id)?;
    let ttl = hub.config.limits.blob_ttl_days * DAY_MS;
    let taking = hub.blocking(move |hub| hub.db.take_blob_get(&id, now_ms(), ttl)).await??;
    let (size, created_at) = taking.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    let counter = hub.clone();
    let taken = Taken::new(move |complete| match complete {
        true => counter.count(crate::Event::MediaOut, 1),
        false => drop(counter.db.return_blob_get(&id, created_at)),
    });
    let file = hub.blobs.read(&(id, created_at), size).await.ok_or_else(|| refuse(StatusCode::NOT_FOUND))?;
    let stream = Counted::new(tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024), size, taken);
    Ok(octets(size, axum::body::Body::from_stream(stream)))
}

/// `DELETE /v1/blobs/{id}`, anonymous like GET: one fetch stored for good; the last removes the blob.
pub(crate) async fn delete_blob(
    State(hub): Hubs,
    ConnectInfo(peer): Peer,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res<StatusCode> {
    hub.limit_ip("blob:get", peer, &headers, hub.config.limits.blob_get_per_ip_minute, MINUTE)?;
    let id: BlobId = path_id(&id)?;
    let ttl = hub.config.limits.blob_ttl_days * DAY_MS;
    hub.blocking(move |hub| -> Res<()> {
        if let Some(created_at) = hub.db.ack_blob(&id, now_ms(), ttl)? {
            hub.blobs.remove(&(id, created_at));
        }
        Ok(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
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
    fn an_id_is_held_by_one_upload_at_a_time() {
        let blobs = Arc::new(Blobs::open(&crate::config::BlobConfig::for_tests()).unwrap());
        let claim = blobs.claim([7; 16]).expect("free");
        assert!(blobs.claim([7; 16]).is_none(), "the same id while its upload is under way");
        assert!(blobs.claim([8; 16]).is_some(), "another id");
        drop(claim);
        assert!(blobs.claim([7; 16]).is_some(), "let go");
        assert!(blobs.claimed.lock().unwrap().is_empty());
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
