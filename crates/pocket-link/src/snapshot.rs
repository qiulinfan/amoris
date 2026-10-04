//! What a publication carries (docs/spec/threads.md 4.1 and 4.2): spec-persist's snapshot of the
//! world with what a presenter needs beside it, and a view that decodes the columns or the single
//! components a presenter asks for, leaving every other section undecoded.

use std::sync::Arc;

use pocket_contract::{Problem, detail};
use pocket_persist::format::json::to_json;
use pocket_persist::format::rows_format;
use pocket_persist::pce::Decoder;
use pocket_persist::{FormatTable, SectionKey, SectionKind, Snapshot};
use pocket_sim::registry::ComponentInfo;
use pocket_sim::{EntityId, Persisted};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How the game paces its ticks (spec-contract's `Pacing`, the slice 1 subset).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PacingStatus {
    /// Ticks run when a `step` asks for them.
    Stepped,
    /// Ticks run on the loop's clock at `speed` times real time.
    RealTime { speed: f64 },
}

/// spec-contract's `TimeStatus` (shared/contract/time.md, Requests): the slice 1 subset, with
/// `behind_ms` (threads.md 3.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimeStatus {
    pub tick: u64,
    pub t_s: f64,
    pub pacing: PacingStatus,
    pub paused: bool,
    /// The world is poisoned: ticks are refused until a restore.
    pub halted: bool,
    /// Real time: how far the game is behind its clock.
    pub behind_ms: Option<f64>,
}

/// What presenters need of the component registry: names, versions, docs and JSON Schemas, and the
/// formats that decode every section (project components included).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegistryInfo {
    pub components: Vec<ComponentInfo>,
    pub formats: FormatTable,
}

/// One publication (threads.md 4.1).
#[derive(Clone, Debug)]
pub struct WorldSnapshot {
    /// Publication counter of this run, from 1.
    pub version: u64,
    /// spec-persist's: header (tick, writes, engine, bundle), sections, world hash.
    pub snapshot: Snapshot,
    /// The same `Arc` until it changes.
    pub registry: Arc<RegistryInfo>,
    pub time: TimeStatus,
    /// Stream number of the last event emitted up to this state.
    pub last_event: u64,
    /// The loop's clock at publication; presentation only, never hashed.
    pub published_at_ms: f64,
}

impl WorldSnapshot {
    /// `(tick, writes)`: the world state this publication shows.
    pub fn state(&self) -> (u64, u32) {
        (self.snapshot.header().tick.0, self.snapshot.header().writes)
    }
}

fn decode_failed(section: &str, why: &str) -> Problem {
    Problem::new(
        "persist.noncanonical",
        format!("The snapshot's section {section} does not decode: {why}."),
        detail([("section", json!(section))]),
    )
}

/// Reads a snapshot's sections on demand (threads.md 4.2).
pub struct SnapshotView<'a> {
    snap: &'a WorldSnapshot,
}

impl<'a> SnapshotView<'a> {
    pub fn new(snap: &'a WorldSnapshot) -> SnapshotView<'a> {
        SnapshotView { snap }
    }

    /// Values of one engine component type in ascending `EntityId` order, decoded from its section
    /// with the canonical encoding. A type with no rows has no section: an empty column.
    pub fn column<T: Persisted + DeserializeOwned>(&self) -> Result<Vec<(EntityId, T)>, Problem> {
        let key = SectionKey::new(SectionKind::Component, T::NAME);
        let Some(s) = self.snap.snapshot.section(&key) else {
            return Ok(Vec::new());
        };
        let fail = |e: pocket_persist::pce::PceError| decode_failed(T::NAME, &e.reason());
        let mut d = Decoder::new(&s.bytes, true);
        let n = d.uleb().map_err(fail)?;
        let mut out = Vec::new();
        for _ in 0..n {
            let raw = d.take(8).map_err(fail)?;
            let mut le = [0u8; 8];
            le.copy_from_slice(raw);
            let id = EntityId::new(u64::from_le_bytes(le))
                .ok_or_else(|| decode_failed(T::NAME, "a row's id is 0"))?;
            out.push((id, d.value::<T>().map_err(fail)?));
        }
        d.finish().map_err(fail)?;
        Ok(out)
    }

    /// One component of one entity as JSON, through the format the registry gives its section:
    /// the editor's inspector, and project components, which have no Rust type. `None` when the
    /// entity has no such component.
    pub fn get_json(&self, entity: EntityId, component: &str) -> Result<Option<Value>, Problem> {
        let key = SectionKey::new(SectionKind::Component, component);
        let Some(entry) = self.snap.registry.formats.get(&key) else {
            return Err(Problem::new(
                "sim.component_unknown",
                format!("There is no component '{component}'."),
                detail([("component", json!(component))]),
            ));
        };
        let Some(s) = self.snap.snapshot.section(&key) else {
            return Ok(None);
        };
        let Some(format) = &entry.format else {
            return Err(decode_failed(component, "it has no format"));
        };
        let rows = to_json(&rows_format(format), &s.bytes, true)
            .map_err(|e| decode_failed(component, &format!("{e:?}")))?;
        let found = rows.as_array().and_then(|rows| {
            rows.iter().find_map(|row| {
                let pair = row.as_array()?;
                (pair.first()?.as_u64()? == entity.get()).then(|| pair.get(1).cloned())?
            })
        });
        Ok(found)
    }

    /// Every live entity, ascending.
    pub fn entities(&self) -> Result<Vec<EntityId>, Problem> {
        let Some(s) = self.snap.snapshot.section(&SectionKey::entities()) else {
            return Ok(Vec::new());
        };
        let v = to_json(&pocket_persist::format::entities_format(), &s.bytes, true)
            .map_err(|e| decode_failed("entities", &format!("{e:?}")))?;
        Ok(v.get(1)
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(|x| x.as_u64().and_then(EntityId::new))
                    .collect()
            })
            .unwrap_or_default())
    }
}
