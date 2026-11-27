//! The local agent owns replicated writes. SQLite reads its snapshot through the read-only attachment.

use crate::{blobs::hex, config::CorrosionConfig, db::Db, Hub};
use rusqlite::{params, types::ValueRef, Connection, OptionalExtension, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Duration;

pub(crate) const EPOCH: u64 = 1_767_225_600_000;
const MASK: i64 = (1 << 18) - 1;

struct Ids {
    index: u8,
    last: AtomicI64,
}

impl Ids {
    fn next(&self, ms: u64) -> i64 {
        let time = ms.saturating_sub(EPOCH) as i64;
        let mut last = self.last.load(Ordering::Relaxed);
        loop {
            let tick = time.max(last >> 22);
            let base = (tick << 22) | (i64::from(self.index) << 18);
            let next = if base > last {
                base
            } else if last & MASK == MASK {
                base + (1 << 22)
            } else {
                last + 1
            };
            match self.last.compare_exchange_weak(last, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return next,
                Err(actual) => last = actual,
            }
        }
    }
}

fn error(e: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(std::io::Error::other(e.to_string()).into())
}

type Job = (Vec<Value>, mpsc::SyncSender<Result<usize>>);

#[derive(Default)]
struct MinuteCount {
    prior: Option<u64>,
    added: u64,
}

pub(crate) struct Agent {
    pub config: CorrosionConfig,
    pub recent_acks: Mutex<crate::mail::RecentAcks>,
    // TRANSITION(#15): remove with the delivery checker.
    pub checks: Mutex<crate::lifecycle::Checks>,
    // TRANSITION(#15): remove when edges are equal hubs and the relay is gone.
    pub switch: Option<crate::killswitch::Switch>,
    pub shared: crate::shared::Shared,
    pub touches: Mutex<crate::upkeep::Touches>,
    ids: Ids,
    writes: mpsc::Sender<Job>,
    mesh_events: Mutex<BTreeMap<(i64, String), u64>>,
    counts: Mutex<BTreeMap<(i64, String), MinuteCount>>,
}

pub(crate) fn attach(conn: &Connection, path: &str) -> Result<()> {
    let mut url = reqwest::Url::from_file_path(std::path::absolute(path).map_err(error)?)
        .map_err(|_| error("invalid Corrosion database path"))?;
    url.set_query(Some("mode=ro"));
    conn.execute("ATTACH DATABASE ? AS corro", [url.as_str()])?;
    Ok(())
}

impl Agent {
    pub fn open(conn: &mut Connection, config: &CorrosionConfig) -> Result<Self> {
        if config.index > 15 {
            return Err(error("hub index exceeds 15"));
        }
        attach(conn, &config.db)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS main.peer_inbox_receipt (
                device_id BLOB NOT NULL, id INTEGER NOT NULL, seen_at INTEGER NOT NULL,
                PRIMARY KEY(device_id, id)
            );
            DROP VIEW temp.inbox_message;
            CREATE TEMP VIEW inbox_message AS SELECT * FROM corro.message",
        )?;
        let ids = Ids { index: config.index, last: AtomicI64::new(-1) };
        let url = format!("{}/v1/transactions", config.api.trim_end_matches('/'));
        let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(30)).build().map_err(error)?;
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(error)?;
        let (writes, jobs) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("yo-corrosion".into())
            .spawn(move || {
                for (batch, reply) in jobs {
                    let result = runtime.block_on(async {
                        let body: Value = client
                            .post(&url)
                            .json(&batch)
                            .send()
                            .await
                            .map_err(error)?
                            .error_for_status()
                            .map_err(error)?
                            .json()
                            .await
                            .map_err(error)?;
                        let results = body["results"]
                            .as_array()
                            .filter(|r| r.len() == batch.len())
                            .ok_or_else(|| error("incomplete Corrosion transaction"))?;
                        results.iter().try_fold(0, |n, r| {
                            if let Some(e) = r.get("error") {
                                return Err(error(e));
                            }
                            Ok(n + r["rows_affected"].as_u64().ok_or_else(|| error("missing rows_affected"))? as usize)
                        })
                    });
                    let _ = reply.send(result);
                }
            })
            .map_err(error)?;
        let agent = Self {
            config: config.clone(),
            shared: Default::default(),
            touches: Default::default(),
            ids,
            writes,
            mesh_events: Mutex::default(),
            counts: Mutex::default(),
            recent_acks: Mutex::default(),
            // TRANSITION(#15): remove with the delivery checker.
            checks: Mutex::default(),
            // TRANSITION(#15): remove when edges are equal hubs and the relay is gone.
            switch: crate::killswitch::Switch::open(config, conn)?,
        };
        if config.upstream.is_some() {
            let tx = conn.transaction()?;
            tx.execute_batch(crate::upkeep::SCHEMA)?;
            for table in [
                "message",
                "account_prekey",
                "account_contact_key",
                "account_device_gone",
                "account_device",
                "account_key",
                "account",
            ] {
                if tx.prepare(&format!("SELECT 1 FROM main.{table} LIMIT 1"))?.exists([])? {
                    return Err(error("an edge needs its own empty main database"));
                }
                tx.execute_batch(&format!("DROP TABLE main.{table}"))?;
            }
            tx.commit()?;
        } else {
            // TRANSITION(#15): remove once account reads are authoritative on every hub.
            reconcile(conn)?;
            agent.migrate(conn)?;
        }
        let last = conn.query_row(
            "SELECT coalesce(max(id), -1) FROM corro.message__crsql_pks WHERE ((id >> 18) & 15) = ?",
            [config.index],
            |r| r.get(0),
        )?;
        agent.ids.last.store(last, Ordering::Relaxed);
        Ok(agent)
    }

    pub fn write(&self, batch: Vec<Value>) -> Result<usize> {
        if batch.is_empty() {
            return Ok(0);
        }
        let (send, recv) = mpsc::sync_channel(1);
        self.writes.send((batch, send)).map_err(error)?;
        recv.recv().map_err(error)?
    }

    pub fn message(&self, device: &[u8], envelope: &[u8], now: u64) -> Value {
        json!([
            "INSERT INTO message(id, device_id, envelope, received_at, hub) VALUES (?,unhex(?),unhex(?),?,?)",
            [self.ids.next(crate::now_ms()), hex(device), hex(envelope), now, self.config.hub]
        ])
    }

    fn migrate(&self, conn: &mut Connection) -> Result<()> {
        loop {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let mut batch = Vec::new();
            let mut through = 0i64;
            {
                let mut stmt = tx.prepare(
                    "SELECT id, hex(device_id), hex(envelope), received_at FROM main.message ORDER BY id LIMIT 200",
                )?;
                let mut rows = stmt.query([])?;
                while let Some(r) = rows.next()? {
                    through = r.get(0)?;
                    batch.push(json!(["INSERT OR IGNORE INTO message(id,device_id,envelope,received_at,hub) VALUES (?,unhex(?),unhex(?),?,?)",
                        [through,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,self.config.hub]]));
                }
            }
            if batch.is_empty() {
                return Ok(());
            }
            self.write(batch)?;
            tx.execute("DELETE FROM main.message WHERE id <= ?", [through])?;
            tx.commit()?;
        }
    }
}

// TRANSITION(#15): remove once account reads are authoritative on every hub.
// Replicated table, source table and primary key. Prekey material stays in main.
const TABLES: &[(&str, &str, &str)] = &[
    ("account", "account", "id"),
    ("account_device", "account_device", "id"),
    ("account_device_gone", "account_device_gone", "id"),
    ("account_contact_key", "account_contact_key", "user_id"),
    ("account_prekey_left", "account_prekey", "device_id"),
];

/// A standalone hub no longer queues writes for the mirror.
// TRANSITION(#15): remove once account reads are authoritative on every hub.
pub(crate) fn detach_mirror(conn: &mut Connection) -> Result<()> {
    if !conn.prepare("SELECT 1 FROM sqlite_schema WHERE name='mirror_dirty' AND type='table'")?.exists([])? {
        return Ok(());
    }
    let tx = conn.transaction()?;
    for (table, _, _) in TABLES {
        for op in ["INSERT", "UPDATE", "DELETE"] {
            tx.execute_batch(&format!("DROP TRIGGER IF EXISTS main.mirror_{table}_{op}"))?;
        }
    }
    tx.commit()
}

// TRANSITION(#15): remove once account reads are authoritative on every hub.
fn reconcile(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS mirror_dirty(tbl TEXT, pk BLOB, v INTEGER, PRIMARY KEY(tbl,pk));
        CREATE TABLE IF NOT EXISTS stat_folded(hub TEXT, kind TEXT, minute INTEGER, n INTEGER, PRIMARY KEY(hub,kind,minute));")?;
    for &(table, src, pk) in TABLES {
        for (op, row) in [("INSERT", "new"), ("UPDATE", "new"), ("DELETE", "old")] {
            let previous = if op == "UPDATE" {
                format!(
                    "INSERT INTO mirror_dirty SELECT '{table}',old.{pk},1 WHERE old.{pk} != new.{pk}
                    ON CONFLICT(tbl,pk) DO UPDATE SET v=v+1;"
                )
            } else {
                String::new()
            };
            tx.execute_batch(&format!(
                "CREATE TRIGGER IF NOT EXISTS mirror_{table}_{op} AFTER {op} ON main.{src} BEGIN {previous}
                INSERT INTO mirror_dirty VALUES ('{table}',{row}.{pk},1) ON CONFLICT(tbl,pk) DO UPDATE SET v=v+1; END;"
            ))?;
        }
        tx.execute_batch(&format!(
            "INSERT OR IGNORE INTO mirror_dirty SELECT '{table}', {pk}, 1 FROM main.{src}
            UNION SELECT '{table}', {pk}, 1 FROM corro.{table};"
        ))?;
    }
    tx.commit()
}

fn upsert(table: &str, pk: &str, names: &[&str], row: &rusqlite::Row<'_>) -> Result<Value> {
    let mut params = Vec::new();
    let mut slots = Vec::new();
    for i in 0..names.len() {
        let (slot, value) = match row.get_ref(i)? {
            ValueRef::Blob(b) => ("unhex(?)", json!(hex(b))),
            ValueRef::Text(t) => ("?", json!(std::str::from_utf8(t).map_err(error)?)),
            ValueRef::Integer(n) => ("?", json!(n)),
            ValueRef::Null => ("?", Value::Null),
            ValueRef::Real(_) => return Err(error("unexpected real in mirror")),
        };
        slots.push(slot);
        params.push(value);
    }
    let set = names.iter().filter(|n| **n != pk).map(|n| format!("{n}=excluded.{n}")).collect::<Vec<_>>().join(",");
    Ok(json!([
        format!(
            "INSERT INTO {table} ({}) VALUES ({}) ON CONFLICT({pk}) DO UPDATE SET {set}",
            names.join(","),
            slots.join(",")
        ),
        params
    ]))
}

const MESH_KINDS: &str = "'accepted','delivered','push-ok-apns','push-ok-fcm','push-fail-apns','push-fail-fcm',
    'peer-hit','peer-items','peer-miss','sync-in','sync-lt250','sync-lt500','sync-lt1000','sync-lt5000','sync-ge5000'";

impl Db {
    // TRANSITION(#15): remove once account reads are authoritative on every hub.
    pub(crate) fn mirror(&self) -> Result<()> {
        let agent = self.agent.as_ref().unwrap();
        let conn = self.conn();
        let dirty: Vec<(String, Vec<u8>, i64)> = conn
            .prepare("SELECT tbl,pk,v FROM mirror_dirty LIMIT 500")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_>>()?;
        if dirty.is_empty() {
            return Ok(());
        }
        let mut batch = Vec::new();
        for (name, key, _) in &dirty {
            let &(table, src, pk) =
                TABLES.iter().find(|(t, _, _)| t == name).ok_or_else(|| error("unknown mirror table"))?;
            let sql = if table == "account_prekey_left" {
                "SELECT device_id,min(count(*) FILTER (WHERE kind=2),count(*) FILTER (WHERE kind=3)) AS n FROM main.account_prekey WHERE device_id=? GROUP BY device_id".to_string()
            } else {
                format!("SELECT * FROM main.{src} WHERE {pk}=?")
            };
            let mut stmt = conn.prepare_cached(&sql)?;
            let names = stmt.column_names().map(str::to_owned).collect::<Vec<_>>();
            let names = names.iter().map(String::as_str).collect::<Vec<_>>();
            if let Some(statement) = stmt.query_row([key], |r| upsert(table, pk, &names, r)).optional()? {
                batch.push(statement);
            } else {
                batch.push(json!([format!("DELETE FROM {table} WHERE {pk}=unhex(?)"), [hex(key)]]));
                if table == "account_device" {
                    batch.push(json!(["DELETE FROM message WHERE device_id=unhex(?)", [hex(key)]]));
                }
            }
        }
        drop(conn);
        agent.write(batch)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for (name, key, v) in dirty {
            tx.execute("DELETE FROM mirror_dirty WHERE tbl=? AND pk=? AND v=?", params![name, key, v])?;
        }
        tx.commit()
    }

    pub(crate) fn count_mesh_events(&self, now: u64, counts: &[(&str, u64)]) {
        if let Some(agent) = &self.agent {
            let mut events = agent.mesh_events.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for &(kind, n) in counts {
                if n > 0 {
                    *events.entry(((now / 60_000) as i64, kind.to_owned())).or_default() += n;
                }
            }
        }
    }

    /// Keep each minute until the agent confirms it. A retry may follow a lost HTTP answer.
    pub(crate) fn flush_hub_counts(&self, now: u64, counts: &[(&str, u64)]) -> Result<()> {
        let agent = self.agent.as_ref().unwrap();
        let minute = (now / 60_000) as i64;
        let mut pending = agent.counts.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for &(kind, n) in counts {
            pending.entry((minute, kind.to_owned())).or_default().added += n;
        }
        for ((at, kind), n) in
            std::mem::take(&mut *agent.mesh_events.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
        {
            pending.entry((at, kind)).or_default().added += n;
        }
        let mut batch = Vec::with_capacity(pending.len() + 1);
        {
            let conn = self.conn();
            let mut get =
                conn.prepare_cached("SELECT n FROM corro.hub_stat_minute WHERE hub=? AND kind=? AND minute=?")?;
            for ((at, kind), count) in pending.iter_mut() {
                let prior = match count.prior {
                    Some(n) => n,
                    None => {
                        let n =
                            get.query_row(params![agent.config.hub, kind, at], |r| r.get(0)).optional()?.unwrap_or(0);
                        count.prior = Some(n);
                        n
                    }
                };
                batch.push(json!([
                    "INSERT INTO hub_stat_minute(hub,kind,minute,n)
                     VALUES (?1,?2,?3,?4)
                     ON CONFLICT(hub,kind,minute) DO UPDATE SET n=excluded.n",
                    [agent.config.hub, kind, at, prior + count.added]
                ]));
            }
        }
        batch.push(json!([
            "DELETE FROM hub_stat_minute WHERE hub=? AND minute<=?",
            [agent.config.hub, minute - 48 * 60]
        ]));
        agent.write(batch)?;
        pending.clear();
        Ok(())
    }

    /// Rolling 24 hours in 144 ten-minute bins, oldest first.
    pub(crate) fn hub_history(&self, now: u64) -> Result<BTreeMap<String, BTreeMap<String, Vec<u64>>>> {
        let last = (now / 60_000) as i64;
        let first = last - 1439;
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT hub,kind,(minute-?1)/10 AS slot,sum(n) FROM corro.hub_stat_minute
             WHERE minute BETWEEN ?1 AND ?2 AND (kind IN ({MESH_KINDS}) OR kind GLOB 'mesh-*' OR kind GLOB 'push-fail-*-*') GROUP BY hub,kind,slot"
        ))?;
        let mut rows = stmt.query(params![first, last])?;
        let mut hubs = BTreeMap::<String, BTreeMap<String, Vec<u64>>>::new();
        while let Some(row) = rows.next()? {
            let series = hubs.entry(row.get(0)?).or_default().entry(row.get(1)?).or_insert_with(|| vec![0; 144]);
            series[row.get::<_, usize>(2)?] = row.get(3)?;
        }
        Ok(hubs)
    }

    pub(crate) fn fold(&self, hub: &str, now: u64) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let rows: Vec<(String,String,i64,i64,i64)> = tx.prepare(&format!("SELECT s.hub,s.kind,s.minute,s.n,s.n-coalesce(f.n,0)
            FROM corro.hub_stat_minute s LEFT JOIN stat_folded f USING(hub,kind,minute) WHERE s.hub != ? AND s.kind NOT IN ({MESH_KINDS}) AND s.kind NOT GLOB 'mesh-*' AND s.kind NOT GLOB 'push-fail-*-*' AND s.n > coalesce(f.n,0)"))?
            .query_map([hub],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?.collect::<Result<_>>()?;
        for (origin, kind, minute, n, delta) in rows {
            if minute >= (now / 60_000) as i64 - 120 {
                tx.execute("INSERT INTO stat_minute(kind,minute,n) VALUES(?,?,?) ON CONFLICT(kind,minute) DO UPDATE SET n=n+excluded.n", params![kind,minute,delta])?;
            }
            let hourly = "INSERT INTO stat(metric,hour,value) VALUES(?,?,?) ON CONFLICT(metric,key,hour) DO UPDATE SET value=value+excluded.value";
            tx.execute(hourly, params![kind, minute / 60, delta])?;
            if matches!(kind.as_str(), "yo-in" | "yo-in-bot") {
                tx.execute(hourly, params!["deliveries", minute / 60, delta])?;
            }
            tx.execute("INSERT OR REPLACE INTO stat_folded VALUES(?,?,?,?)", params![origin, kind, minute, n])?;
        }
        tx.execute("DELETE FROM stat_folded WHERE NOT EXISTS (SELECT 1 FROM corro.hub_stat_minute s WHERE s.hub=stat_folded.hub AND s.kind=stat_folded.kind AND s.minute=stat_folded.minute)",[])?;
        tx.commit()
    }
}

async fn updates(
    hub: &Arc<Hub>,
    client: &reqwest::Client,
    url: &str,
    connected: &mut bool,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut res = client.post(url).send().await?.error_for_status()?;
    let mut pending = Vec::new();
    while let Some(chunk) = res.chunk().await? {
        pending.extend_from_slice(&chunk);
        while let Some(end) = pending.iter().position(|&b| b == b'\n') {
            let line: Value = serde_json::from_slice(&pending[..end])?;
            pending.drain(..=end);
            if let Some(err) = line.get("error") {
                return Err(error(err).into());
            }
            *connected = true;
            if line["notify"][0] != "update" {
                continue;
            }
            if let Some(id) = line["notify"][1][0].as_i64() {
                let db = hub.db.clone();
                // A row this hub cannot read (no device id of 16 bytes) wakes nobody; it never ends the stream.
                let arrival = hub.clock_ms();
                let message = tokio::task::spawn_blocking(move || {
                    db.conn()
                        .query_row("SELECT device_id,hub,received_at FROM corro.message WHERE id=?", [id], |r| {
                            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?, r.get::<_, u64>(2)?))
                        })
                        .optional()
                })
                .await?
                .ok()
                .flatten();
                let Some((bytes, origin, received)) = message else { continue };
                if origin != hub.db.agent.as_ref().unwrap().config.hub && !origin.is_empty() {
                    let bucket = match arrival.saturating_sub(received) {
                        0..250 => "sync-lt250",
                        250..500 => "sync-lt500",
                        500..1000 => "sync-lt1000",
                        1000..5000 => "sync-lt5000",
                        _ => "sync-ge5000",
                    };
                    hub.db.count_mesh_events(arrival, &[("sync-in", 1), (bucket, 1)]);
                }
                if let Ok(device) = yoip::DeviceId::try_from(bytes) {
                    if let Some(waiter) =
                        hub.waiting.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&device)
                    {
                        waiter.notify_waiters();
                    }
                }
            }
        }
    }
    Err("update stream ended".into())
}

pub(crate) async fn watch(hub: Arc<Hub>, api: String) {
    let client = reqwest::Client::builder().no_proxy().build().expect("a client without TLS settings builds");
    let url = format!("{}/v1/updates/message", api.trim_end_matches('/'));
    let mut delay = 200;
    let mut outage = false;
    loop {
        let mut connected = false;
        let result = updates(&hub, &client, &url, &mut connected).await;
        if connected {
            delay = 200;
            outage = false;
        }
        if !outage {
            eprintln!("yo-hub: Corrosion updates: {result:?}");
            outage = true;
        }
        tokio::time::sleep(Duration::from_millis(delay)).await;
        delay = (delay * 2).min(5000);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_keep_the_hub_across_clock_steps_and_overflow() {
        let ids = Ids { index: 7, last: AtomicI64::new(-1) };
        let first = ids.next(EPOCH + 5);
        assert_eq!(first, (5 << 22) | (7 << 18));
        assert_eq!(ids.next(EPOCH), first + 1);
        ids.last.store(first | MASK, Ordering::Relaxed);
        assert_eq!(ids.next(EPOCH + 5), first + (1 << 22));
    }
    #[test]
    fn blobs_and_nulls_keep_their_types_in_upserts() {
        let conn = Connection::open_in_memory().unwrap();
        let stmt =
            conn.query_row("SELECT x'01af', NULL, 3", [], |r| upsert("t", "id", &["id", "app", "n"], r)).unwrap();
        assert_eq!(stmt,json!(["INSERT INTO t (id,app,n) VALUES (unhex(?),?,?) ON CONFLICT(id) DO UPDATE SET app=excluded.app,n=excluded.n",["01af",null,3]]));
    }
}
