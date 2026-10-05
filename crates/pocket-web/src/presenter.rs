//! The page's side of the web form (docs/spec/threads.md 7.1, 7.2 and 11): every `snap` rebuilt
//! into a [`WorldSnapshot`] from its transferred bytes, which `Snapshot::from_bytes` parses and
//! checks against its trailing hash, and compared with the worker's `world_hash`; the publication
//! order (versions one apart: `publication_order`, not the commands' canonical order, which the
//! check page's `ordering` run tests) and the hash stream (every tick once, in order) checked; the
//! latest two kept, as a presenter interpolating between them would; and a view of the named bodies
//! read through [`SnapshotView`] (the page's slice 1 display, before rendering in slice 3).

use std::collections::VecDeque;
use std::sync::Arc;

use pocket_check::Snapshot;
use pocket_contract::{Problem, detail};
use pocket_link::{RegistryInfo, SnapshotView, TimeStatus, WorldSnapshot};
use serde_json::{Value, json};

/// The components the view reads for each named entity.
const VIEWED: &[&str] = &["Transform", "Boat", "Wind", "Tally", "Log", "Cargo"];

/// What the page holds.
#[derive(Default)]
pub struct PresenterCore {
    latest: VecDeque<WorldSnapshot>,
    registry: Arc<RegistryInfo>,
    version: u64,
    /// The last tick of the hash stream.
    last_tick: Option<u64>,
    /// The stream's hashes since the last [`PresenterCore::take_hashes`].
    hashes: Vec<(u64, String)>,
    problems: Vec<Problem>,
    received: u64,
}

fn threads_failed(test: &str, message: String, more: Value) -> Problem {
    let mut d = detail([("test", json!(test)), ("message", json!(message))]);
    if let Value::Object(m) = more {
        d.extend(m);
    }
    Problem::new("web.threads_failed", message, d)
}

impl PresenterCore {
    pub fn new() -> PresenterCore {
        PresenterCore::default()
    }

    /// Snapshots rebuilt so far.
    pub fn received(&self) -> u64 {
        self.received
    }

    /// What the checks found wrong (`web.threads_failed {test, message}`).
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The hash stream's entries received since the last call, `(tick, hash)` in tick order.
    pub fn take_hashes(&mut self) -> Vec<(u64, String)> {
        std::mem::take(&mut self.hashes)
    }

    /// The latest snapshot rebuilt.
    pub fn latest(&self) -> Option<&WorldSnapshot> {
        self.latest.back()
    }

    /// Rebuilds one `snap` (its fields without the bytes, and the bytes); returns
    /// `{version, tick, writes, world_hash}` of the rebuilt snapshot, or the problem that stopped
    /// the rebuild (also kept in [`PresenterCore::problems`]).
    pub fn receive(&mut self, meta: &Value, bytes: &[u8]) -> Result<Value, Problem> {
        let r = self.rebuild(meta, bytes);
        if let Err(p) = &r {
            self.problems.push(p.clone());
        }
        r
    }

    fn rebuild(&mut self, meta: &Value, bytes: &[u8]) -> Result<Value, Problem> {
        let version = meta["version"].as_u64().unwrap_or(0);
        if version != self.version + 1 {
            self.problems.push(threads_failed(
                "publication_order",
                format!(
                    "snapshot version {version} followed {}; publications must arrive one apart",
                    self.version
                ),
                json!({"version": version, "previous": self.version}),
            ));
        }
        self.version = version;
        for item in meta["hashes"].as_array().into_iter().flatten() {
            let tick = item[0].as_u64().unwrap_or(u64::MAX);
            let hash = item[1].as_str().unwrap_or("").to_owned();
            if let Some(last) = self.last_tick
                && tick != last + 1
            {
                self.problems.push(threads_failed(
                    "hash_stream",
                    format!("the hash of tick {tick} followed tick {last}'s"),
                    json!({"tick": tick, "previous": last}),
                ));
            }
            self.last_tick = Some(tick);
            self.hashes.push((tick, hash));
        }
        if let Some(reg) = meta.get("registry") {
            self.registry = Arc::new(RegistryInfo {
                components: serde_json::from_value(reg["components"].clone()).map_err(|e| {
                    threads_failed(
                        "rebuild",
                        format!("the registry does not decode: {e}"),
                        json!({}),
                    )
                })?,
                formats: serde_json::from_value(reg["formats"].clone()).map_err(|e| {
                    threads_failed(
                        "rebuild",
                        format!("the formats do not decode: {e}"),
                        json!({}),
                    )
                })?,
            });
        }
        let snapshot = Snapshot::from_bytes(bytes)?;
        let hash = snapshot.world_hash().to_string();
        let header = snapshot.header();
        let (tick, writes) = (header.tick.0, header.writes);
        if Some(hash.as_str()) != meta["world_hash"].as_str()
            || Some(tick) != meta["header"]["tick"].as_u64()
            || Some(u64::from(writes)) != meta["header"]["writes"].as_u64()
        {
            return Err(threads_failed(
                "rebuild",
                format!(
                    "snapshot {version} rebuilt to tick {tick}, writes {writes}, hash {hash}; the worker posted {}",
                    meta["world_hash"]
                ),
                json!({"version": version, "rebuilt": hash, "posted": meta["world_hash"]}),
            ));
        }
        let time: TimeStatus = serde_json::from_value(meta["time"].clone()).map_err(|e| {
            threads_failed(
                "rebuild",
                format!("the time status does not decode: {e}"),
                json!({}),
            )
        })?;
        let ws = WorldSnapshot {
            version,
            snapshot,
            registry: self.registry.clone(),
            time,
            last_event: meta["last_event"].as_u64().unwrap_or(0),
            published_at_ms: meta["published_at_ms"].as_f64().unwrap_or(0.0),
        };
        if self.latest.len() == 2 {
            self.latest.pop_front();
        }
        self.latest.push_back(ws);
        self.received += 1;
        Ok(json!({"version": version, "tick": tick, "writes": writes, "world_hash": hash}))
    }

    /// Every named entity of the latest snapshot with the components of [`VIEWED`] it has, as
    /// JSON: what the page shows.
    pub fn view(&self) -> Result<Value, Problem> {
        let Some(ws) = self.latest() else {
            return Ok(json!({"entities": []}));
        };
        let view = SnapshotView::new(ws);
        let mut out = Vec::new();
        for id in view.entities()? {
            let Some(name) = view.get_json(id, "Name")? else {
                continue;
            };
            let mut e = json!({"id": id.get(), "name": name});
            for c in VIEWED {
                if let Some(v) = view.get_json(id, c)? {
                    e[*c] = v;
                }
            }
            out.push(e);
        }
        Ok(json!({"tick": ws.snapshot.header().tick.0, "time": ws.time, "entities": out}))
    }
}
