//! Opt-in, thread-local benchmark counters; no clock reads in production builds.
#[cfg(feature = "testing")]
thread_local! { static TIMES: std::cell::RefCell<[std::time::Duration; 3]> = const { std::cell::RefCell::new([std::time::Duration::ZERO; 3]) }; }
#[cfg(feature = "testing")]
pub fn take() -> [std::time::Duration; 3] {
    TIMES.with(|v| std::mem::replace(&mut *v.borrow_mut(), [std::time::Duration::ZERO; 3]))
}
pub(crate) struct Span {
    #[cfg(feature = "testing")]
    start: std::time::Instant,
    #[cfg(feature = "testing")]
    kind: usize,
}
impl Span {
    pub(crate) fn new(_kind: usize) -> Self {
        Self {
            #[cfg(feature = "testing")]
            start: std::time::Instant::now(),
            #[cfg(feature = "testing")]
            kind: _kind,
        }
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        #[cfg(feature = "testing")]
        TIMES.with(|v| v.borrow_mut()[self.kind] += self.start.elapsed());
    }
}

// Benchmark control: reproduce the previous deep snapshot cost with otherwise identical fixtures and code.
#[cfg(feature = "testing")]
thread_local! { static DEEP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[cfg(feature = "testing")]
pub fn baseline_snapshots(deep: bool) {
    DEEP.with(|v| v.set(deep));
}
#[cfg(feature = "testing")]
pub(crate) fn copies_snapshots() -> bool {
    DEEP.with(|v| v.get())
}
