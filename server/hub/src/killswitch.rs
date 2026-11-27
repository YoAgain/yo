// TRANSITION(#15): remove when edges are equal hubs and the relay is gone.
use crate::{config::CorrosionConfig, corrosion::EPOCH, db::Db, lifecycle, Hub, DAY_MS};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{watch, Notify};

const SECOND: Duration = Duration::from_secs(1);
const CENTRAL: &str = "yo-falkenstein";
const READ: &str = "SELECT value FROM hub_setting WHERE key='edges'";

pub async fn command(config: Option<&CorrosionConfig>, args: &[String]) -> Result<(), String> {
    let config = config.ok_or("edges requires Corrosion")?;
    if config.upstream.is_some() {
        return Err("run edges at the central hub".into());
    }
    let [action] = args else { return Err("usage: yo-hub edges off|on|status".into()) };
    let client = reqwest::Client::builder().no_proxy().timeout(10 * SECOND).build().map_err(|e| e.to_string())?;
    const WRITE: &str =
        "INSERT INTO hub_setting(key,value) VALUES ('edges',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value";
    let (path, body) = match action.as_str() {
        "on" | "off" => ("transactions", json!([[WRITE, [action]]])),
        "status" => ("queries", json!(READ)),
        _ => return Err("usage: yo-hub edges off|on|status".into()),
    };
    let response = client
        .post(format!("{}/v1/{path}", config.api.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    if action == "status" {
        let mut value = "on".to_owned();
        for line in response.lines() {
            let event: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
            if let Some(e) = event.get("error") {
                return Err(e.to_string());
            }
            if let Some(row) = event.get("row") {
                value = row[1][0].as_str().ok_or("invalid edges setting")?.to_owned();
            }
            if event.get("eoq").is_some() {
                println!("edges {value}");
                return Ok(());
            }
        }
        return Err("incomplete Corrosion query".into());
    }
    let result: Value = serde_json::from_str(&response).map_err(|e| e.to_string())?;
    if result["results"][0]["rows_affected"].as_u64().is_none() {
        return Err(format!("edges setting failed: {result}"));
    }
    println!("edges {action}");
    Ok(())
}

pub(crate) struct Switch {
    file: PathBuf,
    off: watch::Sender<bool>,
}

impl Switch {
    pub fn open(config: &CorrosionConfig, conn: &Connection) -> rusqlite::Result<Option<Self>> {
        if config.upstream.is_none() {
            return Ok(None);
        }
        let file =
            std::env::var_os("YO_EDGE_OFF_FILE").map_or_else(|| PathBuf::from("/run/yo-edge/off"), PathBuf::from);
        let global: Option<String> =
            conn.query_row("SELECT value FROM corro.hub_setting WHERE key='edges'", [], |r| r.get(0)).optional()?;
        let off = file.exists() || global.as_deref() == Some("off");
        Ok(Some(Self { file, off: watch::channel(off).0 }))
    }
}

impl Db {
    pub(crate) fn edge_off(&self) -> bool {
        self.agent.as_ref().and_then(|a| a.switch.as_ref()).is_some_and(|s| *s.off.borrow())
    }
}

async fn updates(api: &str, refresh: &Notify) {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("{}/v1/updates/hub_setting", api.trim_end_matches('/'));
    loop {
        if let Ok(mut response) = client.post(&url).send().await.and_then(|r| r.error_for_status()) {
            while let Ok(Some(_)) = response.chunk().await {
                refresh.notify_one();
            }
        }
        tokio::time::sleep(SECOND).await;
    }
}

async fn monitor(hub: &Hub, switch: &Switch, config: &CorrosionConfig, refresh: &Notify) {
    // A stalled delivery can hold the main DB mutex while waiting for the agent.
    let mut conn = None;
    let mut global = *switch.off.borrow();
    loop {
        let local = switch.file.exists();
        if !local {
            let result = (|| {
                if conn.is_none() {
                    conn = Some(Connection::open_with_flags(&config.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?);
                }
                conn.as_ref().unwrap().query_row(READ, [], |r| r.get::<_, String>(0)).optional()
            })();
            if let Ok(setting) = result {
                global = setting.as_deref() == Some("off");
            }
        }
        let off = local || global;
        if switch.off.send_if_modified(|v| {
            let changed = *v != off;
            *v = off;
            changed
        }) {
            if off {
                for waiter in hub.waiting.lock().unwrap().values() {
                    waiter.notify_waiters();
                }
                hub.pushes.lock().unwrap().clear();
            } else {
                eprintln!("yo-hub: EDGE-ON");
            }
        }
        tokio::select! {
            _ = refresh.notified() => {}
            _ = tokio::time::sleep(SECOND) => {}
        }
    }
}

fn accepted(hub: &Hub) -> rusqlite::Result<Vec<u64>> {
    // Locally minted ids encode creation time and hub index even after ack deleted the row.
    // Taking the writer's mutex here includes any delivery already committing at switch-off.
    let cutoff = hub.clock_ms().saturating_sub(30 * DAY_MS).saturating_sub(EPOCH) << 22;
    let conn = hub.db.conn();
    let mut query = conn.prepare("SELECT id FROM corro.message__crsql_pks WHERE id>=? AND ((id >> 18) & 15)=?")?;
    let rows = query.query_map(rusqlite::params![cutoff, hub.db.agent.as_ref().unwrap().config.index], |r| r.get(0))?;
    rows.collect()
}

async fn drain(hub: &Arc<Hub>) {
    let mut pending = loop {
        if let Ok(Ok(ids)) = hub.blocking(accepted).await {
            break ids;
        }
        eprintln!("yo-hub: EDGE-OFF cannot read accepted messages");
        tokio::time::sleep(2 * SECOND).await;
    };
    let total = pending.len();
    let mut next_log = tokio::time::Instant::now() + 10 * SECOND;
    while !pending.is_empty() {
        // Keep only ids the authority has yet to confirm. During an outage a single
        // failed probe suffices for this round, regardless of the size of the backlog.
        let mut missing = Vec::new();
        let mut batches = pending.chunks(512);
        while let Some(ids) = batches.next() {
            let answer = lifecycle::ask(hub, CENTRAL, ids).await;
            if tokio::time::Instant::now() >= next_log {
                eprintln!(
                    "yo-hub: EDGE-OFF waiting n={}",
                    missing.len() + ids.len() + batches.clone().map(|c| c.len()).sum::<usize>()
                );
                next_log += 10 * SECOND;
            }
            match answer {
                Some(answer) => missing.extend(ids.iter().copied().filter(|id| !answer.known(id))),
                None => {
                    missing.extend_from_slice(ids);
                    missing.extend(batches.flatten().copied());
                    break;
                }
            }
        }
        pending = missing;
        if !pending.is_empty() {
            tokio::time::sleep(2 * SECOND).await;
        }
    }
    eprintln!("yo-hub: EDGE-OFF drained n={total}");
}

pub(crate) async fn run(hub: Arc<Hub>) {
    let agent = hub.db.agent.as_ref().unwrap();
    let switch = agent.switch.as_ref().unwrap();
    let refresh = Notify::new();
    let mut state = switch.off.subscribe();
    let drains = async {
        loop {
            let off = *state.borrow_and_update();
            if off {
                tokio::select! {
                    _ = drain(&hub) => {}
                    _ = state.changed() => continue,
                }
            }
            let _ = state.changed().await;
        }
    };
    tokio::join!(monitor(&hub, switch, &agent.config, &refresh), updates(&agent.config.api, &refresh), drains);
}
