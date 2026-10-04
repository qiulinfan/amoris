//! What the server pushes to editors (docs/spec/host-protocol.md 3; docs/spec/server.md): `status`
//! at most 10 times a second when it changed (and every second regardless), `world.changed`
//! coalesced over the publications between two pushes, `events` as the game emits them, and `log`
//! lines. Everything is read from `pocket-link`'s reader; nothing waits on the game thread.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pocket_link::{EventCursor, SnapshotView, WorldSnapshot};
use serde_json::{Value, json};

use crate::Host;

/// The most `(entity, component)` pairs one `world.changed` lists before it says `truncated`.
const MAX_CHANGED: usize = 2000;

/// The status as editors show it.
pub(crate) fn status_of(host: &Host, snap: &WorldSnapshot, tps: f64) -> Value {
    let reader = host.reader();
    let world = reader.world();
    let state = reader.status().state;
    let entities = SnapshotView::new(snap).entities().map_or(0, |e| e.len());
    let t = &snap.time;
    json!({
        "tick": t.tick,
        "t_s": t.t_s,
        "paused": t.paused,
        "pacing": t.pacing,
        "halted": t.halted,
        "behind_ms": t.behind_ms,
        "mode": world.mode.as_str(),
        "epoch": world.epoch,
        "world_hash": snap.snapshot.world_hash().to_string(),
        "entities": entities,
        "tps": (tps * 10.0).round() / 10.0,
        // The editor's transport shows the simulation's rate as fps.
        "fps": (tps * 10.0).round() / 10.0,
        "state": format!("{state:?}").to_lowercase(),
        "version": snap.version,
    })
}

fn ids(view: &SnapshotView<'_>) -> Vec<u64> {
    view.entities()
        .map(|e| e.into_iter().map(|i| i.get()).collect())
        .unwrap_or_default()
}

/// What changed from `old` to `new`: entities spawned and despawned, and the components whose
/// values differ on entities in both.
pub(crate) fn changes(old: &WorldSnapshot, new: &WorldSnapshot) -> Option<Value> {
    let (a, b) = (SnapshotView::new(old), SnapshotView::new(new));
    let (before, after) = (ids(&a), ids(&b));
    let spawned: Vec<u64> = after
        .iter()
        .filter(|i| before.binary_search(i).is_err())
        .copied()
        .collect();
    let despawned: Vec<u64> = before
        .iter()
        .filter(|i| after.binary_search(i).is_err())
        .copied()
        .collect();
    let da: BTreeMap<String, [u8; 16]> = a.component_digests().into_iter().collect();
    let db: BTreeMap<String, [u8; 16]> = b.component_digests().into_iter().collect();
    let mut names: Vec<&String> = da.keys().chain(db.keys()).collect();
    names.sort();
    names.dedup();
    let mut changed: Vec<(u64, String)> = Vec::new();
    let mut truncated = false;
    for name in names {
        if da.get(name) == db.get(name) {
            continue;
        }
        let rows = |v: &SnapshotView<'_>| -> BTreeMap<u64, Value> {
            v.rows_json(name).unwrap_or_default().into_iter().collect()
        };
        let (ra, rb) = (rows(&a), rows(&b));
        for (id, v) in &rb {
            if spawned.binary_search(id).is_ok() {
                continue;
            }
            if ra.get(id) != Some(v) {
                changed.push((*id, name.clone()));
            }
        }
        for id in ra.keys() {
            if !rb.contains_key(id) && despawned.binary_search(id).is_err() {
                changed.push((*id, name.clone()));
            }
        }
        if changed.len() > MAX_CHANGED {
            changed.truncate(MAX_CHANGED);
            truncated = true;
            break;
        }
    }
    if spawned.is_empty() && despawned.is_empty() && changed.is_empty() {
        return None;
    }
    changed.sort();
    let mut v = json!({
        "tick": new.state().0,
        "spawned": spawned,
        "despawned": despawned,
        "changed": changed.iter().map(|(i, c)| json!([i, c])).collect::<Vec<_>>(),
    });
    if truncated {
        v["truncated"] = json!(true);
    }
    Some(v)
}

/// The pushers' loop: events and log every 50 ms, status and world changes every 100 ms.
pub(crate) async fn run(host: Host) {
    let reader = host.reader().clone();
    let mut events = EventCursor {
        next: reader.latest().last_event + 1,
    };
    let mut log_next = 1u64;
    let mut last: Arc<WorldSnapshot> = reader.latest();
    let mut last_epoch = reader.world().epoch;
    let mut last_status = Value::Null;
    let mut last_status_at = Instant::now();
    let mut rate = (Instant::now(), last.state().0);
    let mut tps = 0.0;
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut n: u64 = 0;
    loop {
        tick.tick().await;
        n += 1;
        let batch = reader.events(&mut events, 500);
        if !batch.records.is_empty() {
            let list: Vec<Value> = batch
                .records
                .iter()
                .filter_map(|r| r.to_json().ok())
                .collect();
            host.push("events", Value::Array(list));
        }
        for line in reader.logs(&mut log_next, 200) {
            host.push("log", serde_json::to_value(line).unwrap_or(Value::Null));
        }
        if !n.is_multiple_of(2) {
            continue;
        }
        let now = reader.latest();
        let elapsed = rate.0.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            let ticks = now.state().0.saturating_sub(rate.1);
            tps = ticks as f64 / elapsed;
            rate = (Instant::now(), now.state().0);
        }
        let status = status_of(&host, &now, tps);
        let mut quiet = status.clone();
        quiet["tps"] = Value::Null;
        let mut was = last_status.clone();
        was["tps"] = Value::Null;
        if quiet != was || last_status_at.elapsed() >= Duration::from_secs(1) {
            host.push("status", status.clone());
            last_status = status;
            last_status_at = Instant::now();
        }
        let epoch = reader.world().epoch;
        if epoch != last_epoch {
            host.push(
                "world.changed",
                json!({"tick": now.state().0, "reset": true, "epoch": epoch}),
            );
            last_epoch = epoch;
            last = now;
        } else if now.version != last.version {
            if host.0.push.receiver_count() > 0
                && let Some(c) = changes(&last, &now)
            {
                host.push("world.changed", c);
            }
            last = now;
        }
    }
}
