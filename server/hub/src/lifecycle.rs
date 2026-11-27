// TRANSITION(#15): remove the delivery checker when replication has been validated.
use crate::{
    db::Db,
    error::{refuse, Res},
    mail, Hub,
};
use axum::{
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    Json,
};
use bytes::Bytes;
use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

const BATCH: usize = 512;
const WINDOW: u64 = 900_000;
const GRACE: u64 = 30_000;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Answer {
    hub: String,
    alive: HashSet<u64>,
    deleted: HashSet<u64>,
}

impl Answer {
    pub fn known(&self, id: &u64) -> bool {
        self.alive.contains(id) || self.deleted.contains(id)
    }
}

fn lookup(conn: &Connection, ids: &[u64], hub: String) -> Result<Answer> {
    let mut answer = Answer { hub, ..Answer::default() };
    let mut query = conn.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM corro.message WHERE id=?1),
        EXISTS(SELECT 1 FROM corro.message__crsql_pks p JOIN corro.message__crsql_clock c
        ON c.key=p.__crsql_key WHERE p.id=?1 AND c.col_name='-1' AND c.col_version % 2=0)",
    )?;
    for &id in ids {
        let (alive, deleted): (bool, bool) = query.query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        if alive {
            answer.alive.insert(id);
        } else if deleted {
            answer.deleted.insert(id);
        }
    }
    Ok(answer)
}

pub(crate) async fn check(State(hub): State<Arc<Hub>>, headers: HeaderMap, body: Bytes) -> Res<Json<Answer>> {
    mail::authorize_peer(&hub, &headers)?;
    let ids: Vec<u64> = serde_json::from_slice(&body).map_err(|_| refuse(StatusCode::BAD_REQUEST))?;
    if ids.len() > BATCH || ids.iter().any(|&id| id > i64::MAX as u64) {
        return Err(refuse(StatusCode::BAD_REQUEST));
    }
    Ok(Json(
        hub.blocking(move |h| {
            let mut conn = h.db.conn();
            let tx = conn.transaction()?;
            lookup(&tx, &ids, h.db.agent.as_ref().unwrap().config.hub.clone())
        })
        .await??,
    ))
}

pub(crate) async fn ask(hub: &Hub, peer: &str, ids: &[u64]) -> Option<Answer> {
    let client = hub.peers.as_ref()?;
    let token = hub.config.corrosion.as_ref()?.peer_token.as_ref()?;
    let mut answer = Answer::default();
    // Also probe empty batches so an unreachable peer is visible during quiet periods.
    for chunk in 0..ids.len().div_ceil(BATCH).max(1) {
        let ids = &ids[chunk * BATCH..ids.len().min((chunk + 1) * BATCH)];
        let request = Request::post("/v1/peer/check")
            .header("yo-peer-token", token)
            .body(Bytes::from(serde_json::to_vec(ids).ok()?))
            .ok()?;
        let response = client.send(peer, request, Duration::from_secs(2)).await.ok()?;
        if response.status() != StatusCode::OK {
            return None;
        }
        let part: Answer = serde_json::from_slice(response.body()).ok()?;
        // An edge in relay mode answers from the authority.
        if peer.trim_start_matches("yo-") != part.hub.trim_start_matches("yo-") {
            return None;
        }
        answer.alive.extend(part.alive);
        answer.deleted.extend(part.deleted);
    }
    Some(answer)
}

pub(crate) fn report(db: &Db, now: u64, kind: &'static str, id: u64, peer: &str, age: u64) {
    eprintln!("yo-hub: MESH-ERROR {kind} id={id} peer={peer} age_ms={age}");
    db.count_mesh_events(now, &[(kind, 1)]);
}

pub(crate) type Duplicates = HashMap<u64, Duration>;
pub(crate) fn duplicates(db: &Db, items: Duplicates) {
    let now = crate::now_ms();
    for (id, age) in items {
        report(db, now, "mesh-dup", id, &db.agent.as_ref().unwrap().config.hub, age.as_millis() as u64);
    }
}

#[derive(Default)]
pub(crate) struct Checks {
    acknowledged: HashMap<u64, (yoip::DeviceId, u64)>,
    expired: HashSet<u64>,
}

impl Checks {
    fn duplicate_age(&self, device: &yoip::DeviceId, id: u64, now: u64) -> Option<u64> {
        let (owner, at) = self.acknowledged.get(&id)?;
        let age = now.saturating_sub(*at);
        (owner == device && age <= WINDOW).then_some(age)
    }

    pub fn record(&mut self, db: &Db, device: &yoip::DeviceId, requested: &[u64], deleted: &[u64]) {
        let now = crate::now_ms();
        for id in requested.iter().copied().collect::<HashSet<_>>() {
            if let Some(age) = self.duplicate_age(device, id, now) {
                report(db, now, "mesh-dup", id, &db.agent.as_ref().unwrap().config.hub, age);
            }
        }
        self.acknowledged.extend(deleted.iter().map(|id| (*id, (*device, now))));
    }

    fn candidates(&mut self, now: u64) -> Vec<(u64, u64)> {
        self.acknowledged.retain(|_, (_, at)| now.saturating_sub(*at) <= WINDOW);
        self.acknowledged
            .iter()
            .filter(|(_, (_, at))| now.saturating_sub(*at) >= GRACE)
            .map(|(&id, &(_, at))| (id, at))
            .collect()
    }
}

type Messages = Vec<(u64, u64)>;
fn candidates(hub: &Hub, now: u64) -> Result<Messages> {
    let agent = hub.db.agent.as_ref().unwrap();
    let conn = hub.db.conn();
    let live = conn
        .prepare_cached("SELECT id,received_at FROM corro.message WHERE hub=? AND received_at BETWEEN ? AND ?")?
        .query_map(params![agent.config.hub, now.saturating_sub(WINDOW), now.saturating_sub(GRACE)], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<Result<_>>()?;
    if agent.config.upstream.is_none() {
        expired(&hub.db, &conn, now, now.saturating_sub(hub.config.limits.envelope_ttl_days * crate::DAY_MS))?;
    }
    Ok(live)
}

/// The pruner also calls this while holding the connection, so a sweep between checker ticks is visible.
pub(crate) fn expired(db: &Db, conn: &Connection, now: u64, before: u64) -> Result<()> {
    let agent = db.agent.as_ref().unwrap();
    let mut query =
        conn.prepare_cached("SELECT id,received_at FROM corro.message WHERE received_at<? ORDER BY received_at")?;
    let mut rows = query.query([before])?;
    let mut checks = agent.checks.lock().unwrap();
    let mut present = HashSet::new();
    let mut first = None;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let id = row.get::<_, u64>(0)?;
        present.insert(id);
        if !checks.expired.contains(&id) {
            first.get_or_insert((id, row.get::<_, u64>(1)?));
            count += 1;
        }
    }
    checks.expired = present;
    drop(checks);
    if let Some((id, at)) = first {
        let age = now.saturating_sub(at);
        eprintln!("yo-hub: MESH-ERROR mesh-expired id={id} peer={} age_ms={age}", agent.config.hub);
        db.count_mesh_events(now, &[("mesh-expired", count)]);
    }
    Ok(())
}

#[derive(Default)]
struct Reported(HashSet<(&'static str, String, u64)>);

impl Reported {
    fn findings(
        &mut self,
        peer: &str,
        answer: &Answer,
        live: &Messages,
        acked: &Messages,
    ) -> Vec<(&'static str, u64, u64)> {
        let mut found = vec![];
        for (kind, rows) in [("mesh-stuck", live), ("mesh-ack-lost", acked)] {
            for &(id, at) in rows {
                let failed = if kind == "mesh-stuck" { !answer.known(&id) } else { answer.alive.contains(&id) };
                if failed && self.0.insert((kind, peer.to_owned(), id)) {
                    found.push((kind, id, at))
                }
            }
        }
        found
    }
}

pub(crate) async fn run(hub: Arc<Hub>) {
    let mut timer = tokio::time::interval(Duration::from_millis(GRACE));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut reported = Reported::default();
    loop {
        timer.tick().await;
        let now = hub.clock_ms();
        let agent = hub.db.agent.as_ref().unwrap();
        let snapshot = hub.blocking(move |h| candidates(h, now)).await;
        let live = match snapshot {
            Ok(Ok(s)) => s,
            e => {
                eprintln!("yo-hub: lifecycle snapshot: {e:?}");
                continue;
            }
        };
        let acked = agent.checks.lock().unwrap().candidates(now);
        let ids: Vec<_> = live.iter().chain(&acked).map(|r| r.0).collect();
        let active: HashSet<_> = ids.iter().copied().collect();
        reported.0.retain(|(_, _, id)| active.contains(id));
        if let Some(client) = &hub.peers {
            let answers = futures_util::future::join_all(client.peers().map(|peer| {
                let request = ask(&hub, peer, &ids);
                async move { (peer, request.await) }
            }))
            .await;
            for (peer, answer) in answers {
                if let Some(answer) = answer {
                    for (kind, id, at) in reported.findings(peer, &answer, &live, &acked) {
                        report(&hub.db, now, kind, id, peer, now.saturating_sub(at));
                    }
                } else {
                    hub.db.count_mesh_events(now, &[("mesh-check-skip", 1)]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ack_history_is_scoped_to_device_and_check_window() {
        let history = Checks { acknowledged: [(7, ([1; 16], 100))].into(), ..Default::default() };
        assert_eq!(history.duplicate_age(&[1; 16], 7, 100 + WINDOW), Some(WINDOW));
        assert_eq!(history.duplicate_age(&[2; 16], 7, 110), None);
        assert_eq!(history.duplicate_age(&[1; 16], 7, 101 + WINDOW), None);
        let mut history = history;
        assert!(history.candidates(100 + GRACE - 1).is_empty());
        assert_eq!(history.candidates(100 + GRACE), [(7, 100)]);
        assert!(history.candidates(101 + WINDOW).is_empty());
    }

    #[test]
    fn only_unknown_deliveries_and_live_acks_fail_once_per_peer() {
        let answer = Answer { alive: [2, 4].into(), deleted: [3, 5].into(), ..Default::default() };
        let mut reported = Reported::default();
        let live = vec![(1, 10), (2, 10), (3, 10)];
        let acked = vec![(4, 20), (5, 20), (6, 20)];
        for peer in ["east", "west"] {
            assert_eq!(
                reported.findings(peer, &answer, &live, &acked),
                [("mesh-stuck", 1, 10), ("mesh-ack-lost", 4, 20)]
            );
            assert!(reported.findings(peer, &answer, &live, &acked).is_empty());
        }
    }
}
