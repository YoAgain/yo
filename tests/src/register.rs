//! yo-register: registration by username and backups under load, with the real core (spec 6.4, 7).
//!
//!   yo-register [--accounts 200] [--threads 16] [--races 20] [--url http://…]
//!
//! Without `--url` the hub runs in-process on a SQLite file, without rate limits, and a second one with the default
//! limits shows the per-address limit. With `--url` every phase runs against that hub (one whose limits allow the
//! load), and the limit phase only reports when the first 429 came.
//!
//! Phases: accounts registered at once; races for one name and for names that look alike (exactly one wins); the
//! per-address limit; new phones registering again with their account key; backups stored and fetched by user id and
//! by locator, a replaced locator finding nothing; malformed bodies refused with 400. Every check that fails is
//! printed and makes the exit status 1.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Instant;
use yo_core::{Backup, Client, Error, Push};
use yo_hub::config::{Config, Limits};
use yo_tests::{random_u64, user_id, Http, TestHub};
use yo_wire::api::{self, status};

struct Args {
    accounts: usize,
    threads: usize,
    races: usize,
    url: Option<String>,
}

fn args() -> Args {
    let mut args = Args { accounts: 200, threads: 16, races: 20, url: None };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().unwrap_or_default();
        match flag.as_str() {
            "--accounts" => args.accounts = value.parse().expect("--accounts"),
            "--threads" => args.threads = value.parse().expect("--threads"),
            "--races" => args.races = value.parse().expect("--races"),
            "--url" => args.url = Some(value),
            other => panic!("unknown flag {other}"),
        }
    }
    args
}

static FAILED: AtomicUsize = AtomicUsize::new(0);

fn check(ok: bool, what: &str) {
    if !ok {
        FAILED.fetch_add(1, Ordering::Relaxed);
        println!("FAILED: {what}");
    }
}

fn ms(micros: &mut [u64], p: f64) -> f64 {
    micros.sort_unstable();
    micros.get(((micros.len().max(1) - 1) as f64 * p).round() as usize).copied().unwrap_or(0) as f64 / 1000.0
}

/// Seconds, rate and latency of `n` operations that took `micros` each, `s` in all.
fn timing(what: &str, n: usize, s: f64, micros: &mut [u64]) -> serde_json::Value {
    let (p50, p99, max) = (ms(micros, 0.5), ms(micros, 0.99), ms(micros, 1.0));
    println!("{what}: {n} in {s:.2} s ({:.0}/s), p50 {p50:.1} ms, p99 {p99:.1} ms, max {max:.1} ms", n as f64 / s);
    serde_json::json!({ "n": n, "seconds": s, "per_s": n as f64 / s, "p50_ms": p50, "p99_ms": p99, "max_ms": max })
}

fn by_name(url: &str, name: &str, account: Option<[u8; 32]>) -> Result<Client, Error> {
    Client::register_username(Http::new(url.to_owned()), name, account, None, Push::none())
}

/// Runs `f` for every item on `threads` threads; returns the results in order and each one's time.
fn parallel<T: Send + 'static, R: Send + 'static>(
    items: Vec<T>,
    threads: usize,
    f: impl Fn(T) -> R + Send + Sync + 'static,
) -> (Vec<R>, Vec<u64>, f64) {
    let queue = Arc::new(Mutex::new(items.into_iter().enumerate().collect::<Vec<_>>()));
    let f = Arc::new(f);
    let started = Instant::now();
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let (queue, f) = (queue.clone(), f.clone());
            std::thread::spawn(move || {
                let mut done = Vec::new();
                loop {
                    let Some((i, item)) = queue.lock().unwrap().pop() else { break };
                    let t = Instant::now();
                    let result = f(item);
                    done.push((i, result, t.elapsed().as_micros() as u64));
                }
                done
            })
        })
        .collect();
    let mut done: Vec<_> = workers.into_iter().flat_map(|w| w.join().unwrap()).collect();
    let seconds = started.elapsed().as_secs_f64();
    done.sort_by_key(|(i, _, _)| *i);
    let micros = done.iter().map(|(_, _, t)| *t).collect();
    (done.into_iter().map(|(_, r, _)| r).collect(), micros, seconds)
}

fn main() {
    let args = args();
    let dir = std::env::temp_dir().join(format!("yo-register-{}", random_u64()));
    let local = args.url.is_none().then(|| {
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = Config::for_tests();
        config.db_path = dir.join("hub.sqlite").to_string_lossy().into_owned();
        config.limits = Limits::unlimited();
        TestHub::start_with(config, "127.0.0.1:0")
    });
    let url = args.url.clone().unwrap_or_else(|| local.as_ref().unwrap().url());
    let run = random_u64() % 100_000;
    println!(
        "yo-register: {} accounts, {} threads, {} races, hub {}",
        args.accounts,
        args.threads,
        args.races,
        if local.is_some() { "in-process" } else { url.as_str() }
    );

    // ---- accounts at once ----
    let names: Vec<String> = (0..args.accounts).map(|i| format!("U{run:05}X{i:05}")).collect();
    let u = url.clone();
    let (registered, mut micros, s) = parallel(names.clone(), args.threads, move |name| by_name(&u, &name, None));
    let failures = registered.iter().filter(|r| r.is_err()).count();
    check(
        failures == 0,
        &format!(
            "{failures} of {} registrations failed: {:?}",
            args.accounts,
            registered.iter().find_map(|r| r.as_ref().err())
        ),
    );
    let registration = timing("registered", args.accounts, s, &mut micros);
    let mut clients: Vec<Client> = registered.into_iter().flatten().collect();

    // ---- races: the same name twice, and two names that look like it; exactly one wins ----
    let mut race_micros = Vec::new();
    let started = Instant::now();
    for k in 0..args.races {
        let contenders =
            [format!("LEO{run}{k}"), format!("LEO{run}{k}"), format!("LE0{run}{k}"), format!("LÉO{run}{k}")];
        let start = Arc::new(Barrier::new(contenders.len()));
        let racing: Vec<_> = contenders
            .into_iter()
            .map(|name| {
                let (url, start) = (url.clone(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    let t = Instant::now();
                    let result = by_name(&url, &name, None).map(|_| ());
                    (result, t.elapsed().as_micros() as u64)
                })
            })
            .collect();
        let results: Vec<_> = racing.into_iter().map(|r| r.join().unwrap()).collect();
        race_micros.extend(results.iter().map(|(_, t)| *t));
        let won = results.iter().filter(|(r, _)| r.is_ok()).count();
        let taken = results.iter().filter(|(r, _)| *r == Err(Error::Refused(status::CONFLICT))).count();
        check(won == 1 && taken == 3, &format!("race {k}: {won} won, {taken} refused with 409: {results:?}"));
    }
    let races = timing("raced (4 for one name)", race_micros.len(), started.elapsed().as_secs_f64(), &mut race_micros);

    // ---- the per-address limit ----
    let limited_hub = local.is_some().then(|| {
        let mut config = Config::for_tests();
        config.limits = Limits::default();
        TestHub::start_with(config, "127.0.0.1:0")
    });
    let limited_url = limited_hub.as_ref().map(|h| h.url()).unwrap_or_else(|| url.clone());
    let allowed = Limits::default().register_per_ip_hour as usize;
    let mut first_429 = None;
    for i in 0..allowed + 10 {
        match by_name(&limited_url, &format!("L{run:05}Y{i:04}"), None) {
            Err(Error::Refused(status::RATE_LIMITED)) => {
                first_429 = Some(i + 1);
                break;
            }
            other => check(other.is_ok(), &format!("limited registration {i}: {:?}", other.err())),
        }
    }
    match limited_hub {
        Some(_) => check(first_429 == Some(allowed + 1), &format!("first 429 at {first_429:?}, want {}", allowed + 1)),
        None => println!("limit: first 429 at registration {first_429:?}"),
    }
    let fetch = Http::new(limited_url.clone());
    let fetched: Vec<_> = (0..allowed + 1).map(|_| Client::fetch_backup(fetch.as_ref(), &[0; 16])).collect();
    if local.is_some() {
        check(fetched[..allowed].iter().all(|r| *r == Ok(None)), "backup fetches within the limit");
        check(fetched[allowed] == Err(Error::Refused(status::RATE_LIMITED)), "backup fetch over the limit is 429");
    }
    println!("limit: registration 429 after {first_429:?}, backup fetch {:?} after {allowed}", fetched.last());

    // ---- backups: every account stores one, found by user id and by locator ----
    let locator = move |i: usize, round: u8| {
        let mut l = [round; 16];
        l[..8].copy_from_slice(&((run as u64) << 20 | i as u64).to_le_bytes());
        l
    };
    let record = move |i: usize, round: u8| {
        Backup::new(vec![1; 60], vec![2; 60], vec![round; 1000 + i % 500], locator(i, round).to_vec())
    };
    let items: Vec<(usize, Client)> = clients.drain(..).enumerate().collect();
    let (stored, mut micros, s) = parallel(items, args.threads, move |(i, mut c)| {
        let result = c.put_backup(&record(i, 1));
        (i, c, result)
    });
    check(stored.iter().all(|(_, _, r)| r.is_ok()), "every backup stored");
    let backup_put = timing("backups stored", stored.len(), s, &mut micros);
    let ids: Vec<(usize, [u8; 16])> = stored.iter().map(|(i, c, _)| (*i, user_id(c.user_id()))).collect();
    let mut clients: Vec<(usize, Client)> = stored.into_iter().map(|(i, c, _)| (i, c)).collect();
    let u = url.clone();
    let (found, mut micros, s) = parallel(ids.clone(), args.threads, move |(i, user)| {
        let http = Http::new(u.clone());
        let by_user = Client::fetch_backup(http.as_ref(), &user);
        let by_locator = Client::fetch_backup(http.as_ref(), &locator(i, 1));
        by_user == Ok(Some(record(i, 1))) && by_locator == Ok(Some(record(i, 1)))
    });
    check(found.iter().all(|ok| *ok), "every backup found by user id and by locator");
    let backup_get = timing("backups fetched (2 each)", 2 * found.len(), s, &mut micros);
    // A new recovery code for some: the old locator finds nothing, the new one the new record.
    let http = Http::new(url.clone());
    for (i, client) in clients.iter_mut().take(20) {
        check(client.put_backup(&record(*i, 2)).is_ok(), "backup replaced");
        check(Client::fetch_backup(http.as_ref(), &locator(*i, 1)) == Ok(None), "old locator finds nothing");
        check(Client::fetch_backup(http.as_ref(), &locator(*i, 2)) == Ok(Some(record(*i, 2))), "new locator");
    }
    // Another account cannot take a locator.
    if let Some((_, other)) = clients.get_mut(20) {
        check(other.put_backup(&record(0, 2)) == Err(Error::Refused(status::CONFLICT)), "a taken locator is 409");
    }

    // ---- new phones: registering again with the account key ----
    let again: Vec<(String, [u8; 32], String)> =
        clients.iter().take(50).map(|(i, c)| (names[*i].clone(), c.account_secret(), c.user_id().to_owned())).collect();
    let n_again = again.len();
    let u = url.clone();
    let (restored, mut micros, s) = parallel(again, args.threads, move |(name, secret, user)| {
        by_name(&u, &name, Some(secret)).map(|c| c.user_id() == user)
    });
    check(restored.iter().all(|r| *r == Ok(true)), "every new phone is its user again");
    let restore = timing("registered again with the account key", n_again, s, &mut micros);
    for (_, old) in clients.iter_mut().take(n_again) {
        check(old.sync(0) == Err(Error::Replaced), "the old phone is replaced (410)");
    }

    // ---- malformed bodies ----
    let raw = reqwest::blocking::Client::new();
    let garbage: [Vec<u8>; 4] = [Vec::new(), vec![0], vec![0xFF; 40], vec![7; 5000]];
    let mut statuses = Vec::new();
    for body in &garbage {
        let status = raw.post(format!("{url}{}", api::REGISTER)).body(body.clone()).send().unwrap().status().as_u16();
        statuses.push(status);
        check(status == status::BAD_REQUEST, &format!("garbage registration answers {status}"));
    }
    if let Some((_, client)) = clients.iter_mut().skip(n_again).last() {
        let bad_version = yo_wire::encode(&Backup { version: 9, ..record(0, 3) });
        let short_locator = yo_wire::encode(&Backup::new(vec![1], vec![2], vec![3], vec![4; 15]));
        for body in garbage.iter().cloned().chain([bad_version, short_locator]) {
            let status = client.signed_for_tests("PUT", api::BACKUP, &body).unwrap().status;
            statuses.push(status);
            check(status == status::BAD_REQUEST, &format!("garbage backup answers {status}"));
        }
    }
    let too_large = raw.put(format!("{url}{}", api::BACKUP)).body(vec![0; 70_000]).send().unwrap().status().as_u16();
    check(too_large == status::TOO_LARGE, &format!("an oversized backup answers {too_large}"));
    println!("malformed: {} bodies, statuses {statuses:?}, oversized {too_large}", statuses.len());

    let failed = FAILED.load(Ordering::Relaxed);
    let result = serde_json::json!({
        "accounts": args.accounts,
        "threads": args.threads,
        "cores": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        "registration": registration,
        "races": races,
        "backup_put": backup_put,
        "backup_get": backup_get,
        "registration_again": restore,
        "failed_checks": failed,
    });
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
    drop(local);
    let _ = std::fs::remove_dir_all(dir);
    if failed > 0 {
        println!("{failed} checks failed");
        std::process::exit(1);
    }
    println!("all checks passed");
}
