//! Timing notes on stderr for latency work, only with `YO_TIMING` set: never on by default.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Whether notes are on: callers skip formatting them otherwise.
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("YO_TIMING").is_some())
}

/// One note, with the wall clock in milliseconds.
pub fn note(what: &str) {
    if on() {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        // This thread's CPU time so far (ns), Linux only.
        let cpu = std::fs::read_to_string("/proc/thread-self/schedstat")
            .ok()
            .and_then(|s| s.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()))
            .map_or(String::new(), |ns| format!(" cpu {:.1}", ns as f64 / 1e6));
        eprintln!("yo-timing {ms:.1}{cpu} {what}");
    }
}

/// A path's route only (`/v1/inbox`): never an inbox id or a blob's path, which is its capability (spec 4.3).
pub fn route(path: &str) -> &str {
    let path = path.split('?').next().unwrap_or("");
    match path.match_indices('/').nth(2) {
        Some((at, _)) => &path[..at],
        None => path,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_route_names_no_id() {
        assert_eq!(super::route("/v1/inbox/53seDSmg?x=1"), "/v1/inbox");
        assert_eq!(super::route("/v1/inbox?wait=25"), "/v1/inbox");
        assert_eq!(super::route("/v1/ohttp"), "/v1/ohttp");
    }
}
