//! An edge keeps refused push tokens and device activity until the account writer acknowledges them.

use crate::{
    db::{to_the_hour, Db},
    error::Res,
    mail::authorize_peer,
    request::{b64_array, bad, Hubs},
    Hub,
};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use rusqlite::{params, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use yoip::{b64, unb64, DeviceId};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS main.peer_upkeep (
    kind INTEGER NOT NULL, device_id BLOB NOT NULL, token BLOB NOT NULL DEFAULT x'', at INTEGER NOT NULL DEFAULT 0,
    app TEXT, version TEXT, inside INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(kind,device_id,token))";
const MOST: usize = 500;
pub(crate) const BODY_LIMIT: usize = 256 * 1024;

#[derive(Serialize, Deserialize)]
struct Clear {
    device: String,
    token: String,
}
#[derive(Serialize, Deserialize, PartialEq)]
pub(crate) struct Touch {
    device: String,
    at: u64,
    app: Option<String>,
    version: Option<String>,
    inside: bool,
}
pub(crate) type Touches = HashMap<DeviceId, Touch>;

#[derive(Default, Serialize, Deserialize)]
struct Batch {
    clear: Vec<Clear>,
    touch: Vec<Touch>,
}

impl Db {
    pub(crate) fn queue_touch(
        &self,
        device: &DeviceId,
        now: u64,
        app: Option<(&str, Option<&str>)>,
        inside: bool,
    ) -> Result<()> {
        let touch = Touch {
            device: b64(device),
            at: to_the_hour(now),
            app: app.map(|(a, _)| a.into()),
            version: app.and_then(|(_, v)| v.map(str::to_owned)),
            inside,
        };
        let mut recent = self.agent.as_ref().unwrap().touches.lock().unwrap();
        if recent.get(device) == Some(&touch) {
            return Ok(());
        }
        self.conn().execute("INSERT INTO peer_upkeep(kind,device_id,at,app,version,inside) VALUES(1,?,?,?,?,?)
            ON CONFLICT(kind,device_id,token) DO UPDATE SET at=max(at,excluded.at),app=coalesce(excluded.app,app),
            version=CASE WHEN excluded.app IS NULL THEN version ELSE excluded.version END,inside=inside OR excluded.inside",
            params![device,touch.at,touch.app,touch.version,touch.inside])?;
        recent.insert(*device, touch);
        Ok(())
    }

    fn pending_upkeep(&self) -> Result<Batch> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT kind,device_id,token,at,app,version,inside FROM peer_upkeep ORDER BY rowid LIMIT ?",
        )?;
        let mut rows = stmt.query([MOST])?;
        let mut batch = Batch::default();
        let mut size = 32;
        while let Some(r) = rows.next()? {
            let device = b64(&r.get::<_, Vec<u8>>(1)?);
            if r.get::<_, u8>(0)? == 0 {
                let clear = Clear { device, token: b64(&r.get::<_, Vec<u8>>(2)?) };
                size += serde_json::to_vec(&clear).unwrap().len() + 1;
                if size > BODY_LIMIT {
                    break;
                }
                batch.clear.push(clear);
            } else {
                let touch = Touch { device, at: r.get(3)?, app: r.get(4)?, version: r.get(5)?, inside: r.get(6)? };
                size += serde_json::to_vec(&touch).unwrap().len() + 1;
                if size > BODY_LIMIT {
                    break;
                }
                batch.touch.push(touch);
            }
        }
        Ok(batch)
    }

    fn ack_upkeep(&self, batch: Batch) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for c in batch.clear {
            tx.execute(
                "DELETE FROM peer_upkeep WHERE kind=0 AND device_id=? AND token=?",
                params![unb64(&c.device).unwrap(), unb64(&c.token).unwrap()],
            )?;
        }
        for t in batch.touch {
            tx.execute("DELETE FROM peer_upkeep WHERE kind=1 AND device_id=? AND token=x'' AND at=? AND app IS ? AND version IS ? AND inside=?",
                params![unb64(&t.device).unwrap(),t.at,t.app,t.version,t.inside])?;
        }
        tx.commit()
    }
}

pub(crate) async fn receive(State(hub): Hubs, headers: HeaderMap, body: Bytes) -> Res<StatusCode> {
    authorize_peer(&hub, &headers)?;
    let batch: Batch = serde_json::from_slice(&body).map_err(|_| bad())?;
    if batch.clear.len() + batch.touch.len() > MOST {
        return Err(bad());
    }
    let clear = batch
        .clear
        .into_iter()
        .map(|c| Ok((b64_array(&c.device).ok_or_else(bad)?, unb64(&c.token)?)))
        .collect::<Res<Vec<_>>>()?;
    let touch = batch
        .touch
        .into_iter()
        .map(|t| {
            if t.at > i64::MAX as u64 {
                return Err(bad());
            }
            Ok((b64_array(&t.device).ok_or_else(bad)?, t))
        })
        .collect::<Res<Vec<_>>>()?;
    hub.blocking(move |h| -> Result<()> {
        for (device, token) in clear {
            h.db.clear_push(&device, &token)?;
        }
        for (device, t) in touch {
            h.db.touch_from_peer(&device, t.at, t.app.as_deref(), t.version.as_deref(), t.inside)?;
        }
        Ok(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

// TRANSITION(#15): remove when edges are equal hubs and the relay is gone.
async fn send(hub: &Arc<Hub>) -> Res<()> {
    let batch = hub.blocking(|h| h.db.pending_upkeep()).await??;
    if batch.clear.is_empty() && batch.touch.is_empty() {
        return Ok(());
    }
    hub.edge.as_ref().unwrap().post_upkeep(serde_json::to_vec(&batch).unwrap()).await?;
    hub.blocking(move |h| h.db.ack_upkeep(batch)).await??;
    Ok(())
}

// TRANSITION(#15): remove when edges are equal hubs and the relay is gone.
pub(crate) async fn run(hub: Arc<Hub>) {
    let mut failing = false;
    loop {
        tokio::time::sleep(Duration::from_secs(if failing { 5 } else { 1 })).await;
        match send(&hub).await {
            Ok(()) => failing = false,
            Err(error) => {
                if !failing {
                    eprintln!("yo-hub: peer upkeep: {error:?}");
                }
                failing = true;
            }
        }
    }
}
