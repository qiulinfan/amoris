//! Pinned vectors (docs/spec/persistence.md 12, P4): published XXH3-128 and BLAKE3 test vectors, a
//! fixed tiny world's snapshot and world hash, and a short run with a fork whose hashes are pinned.
//! Native and WebAssembly builds must reproduce them exactly: [`web_report`] is what
//! `examples/web_vectors.rs` prints in a JavaScript engine and the `web` test natively.

use bevy_ecs::prelude::{Component, Query, ResMut};
use pocket_contract::Problem;
use pocket_sim::order::by_id_mut;
use pocket_sim::{
    ContentHash, EntityId, EventKind, Name, NewEvent, NoHooks, Persisted, PlainData,
    RegisterPersisted, RngTable, RunCondition, Sim, SimConfig, TickPhase, TickRate, math,
};
use serde::{Deserialize, Serialize};

use crate::hash::xxh3_128;
use crate::registry::Registry;
use crate::snapshot::{Snapshot, SnapshotHeader};
use crate::version::EngineVersion;
use crate::{RegistryBuilder, fork_into, sim_registry, snapshot, world_hash};

/// A body of the pinned run.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mover {
    pub x: f64,
    pub v: f64,
}

impl Persisted for Mover {
    const NAME: &'static str = "vectors.Mover";
    const VERSION: u32 = 1;
}

/// The registry of the vectors' worlds.
pub fn registry() -> Result<Registry, Problem> {
    let mut b: RegistryBuilder = sim_registry();
    b.component::<Mover>();
    b.build()
}

fn hex(b: &[u8]) -> String {
    crate::error::hex(b)
}

fn sway(mut q: Query<(&EntityId, &mut Mover)>, mut rng: ResMut<RngTable>) {
    for (id, mut m) in by_id_mut(&mut q) {
        let push = match rng.entity("vectors.sway", *id) {
            Ok(s) => s.range(-1.0, 1.0).unwrap_or(0.0),
            Err(_) => 0.0,
        };
        m.v = m.v * 0.99 + push * 0.1 + math::sin(m.x) * 0.01;
        m.x += m.v / 60.0;
    }
}

/// The tiny world: three named entities, one despawned, two events at the boundary, seed 42.
pub fn tiny_world() -> Result<Sim, Problem> {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 42,
    })?;
    sim.add_system(
        "vectors.sway",
        TickPhase::Update,
        RunCondition::Always,
        sway,
    )?;
    let mut b = sim.boundary();
    for (i, name) in ["sloop", "buoy", "crate"].iter().enumerate() {
        let x = f64::from(u8::try_from(i).unwrap_or(0));
        b.spawn((Name::new(*name)?, Mover { x, v: -0.0 }))?;
    }
    b.despawn(EntityId::FIRST)?;
    let kind = EventKind::new("vectors.ready")?;
    b.emit(NewEvent::new(kind.clone()).data(PlainData::object(vec![(
        "n".into(),
        PlainData::Number(2.5),
    )])?));
    b.emit(NewEvent::new(kind).subject(EntityId::new(2).unwrap_or(EntityId::FIRST)));
    Ok(sim)
}

/// Published vectors: XXH3-128 of the empty input with seed 0 (xxHash's sanity vectors), BLAKE3
/// of the empty input and of "abc" (the BLAKE3 reference).
pub fn check_hash_vectors() -> Result<(), String> {
    let x = xxh3_128(0, b"");
    let mut be = x;
    be.reverse();
    let want = [
        (hex(&be), "99aa06d3014798d86001c324468d497f", "xxh3_128('')"),
        (
            blake3::hash(b"").to_hex().to_string(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
            "blake3('')",
        ),
        (
            blake3::hash(b"abc").to_hex().to_string(),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85",
            "blake3('abc')",
        ),
    ];
    for (got, expected, what) in want {
        if got != expected {
            return Err(format!("{what} is {got}, not {expected}"));
        }
    }
    Ok(())
}

/// The tiny world's world hash and the BLAKE3 of its snapshot's bytes, pinned.
pub const TINY_WORLD_HASH: &str = "be5c1391f6a928100f74ef5bbe9653a3";
pub const TINY_SNAPSHOT_BLAKE3: &str =
    "60c564abf39f9fc3b9382ba139f34d4bea7706d8ea44afbc9055156e04fc755a";
/// The world hash after 120 ticks of the tiny world, and of a fork made at tick 60 and run on.
pub const RUN_WORLD_HASH: &str = "37d0990d0a1d99cc2cd0a7fcc1cfd7f6";

/// The tiny world's pinned values.
pub fn tiny_values() -> Result<(String, String), String> {
    let reg = registry().map_err(|p| p.message)?;
    let sim = tiny_world().map_err(|p| p.message)?;
    let s = snapshot(sim.world(), &reg).map_err(|p| p.message)?;
    // The header names the build (target, profile), which the world hash leaves out; the pinned
    // bytes are the snapshot under a fixed header.
    let header = SnapshotHeader {
        engine: EngineVersion {
            semver: "0.0.0".into(),
            commit: "pinned".into(),
            source: ContentHash([0; 32]),
            target: "any".into(),
            profile: "any".into(),
            contract: String::new(),
            c_compiler: String::new(),
        },
        ..s.header().clone()
    };
    let fixed = Snapshot::from_parts(header, s.sections().to_vec());
    Ok((
        s.world_hash().to_string(),
        blake3::hash(&fixed.to_bytes()).to_hex().to_string(),
    ))
}

/// The run's pinned value, checking on the way that a fork at tick 60 continues identically.
pub fn run_value() -> Result<String, String> {
    let m = |p: Problem| p.message;
    let reg = registry().map_err(m)?;
    let mut a = tiny_world().map_err(m)?;
    let mut b = tiny_world().map_err(m)?;
    for t in 1..=120u64 {
        a.step(&mut NoHooks).map_err(m)?;
        if t == 60 {
            fork_into(a.world(), b.world_mut(), &reg).map_err(m)?;
        }
        if t > 60 {
            b.step(&mut NoHooks).map_err(m)?;
            let (ha, hb) = (
                world_hash(a.world(), &reg).map_err(m)?,
                world_hash(b.world(), &reg).map_err(m)?,
            );
            if ha != hb {
                return Err(format!("the fork at 60 diverged at {t}"));
            }
        }
    }
    Ok(world_hash(a.world(), &reg).map_err(m)?.to_string())
}

fn pinned(what: &str, got: &str, want: &str) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what} is {got}, pinned {want}"))
    }
}

pub fn check_tiny_world() -> Result<(), String> {
    let (hash, bytes) = tiny_values()?;
    pinned("the tiny world's hash", &hash, TINY_WORLD_HASH)?;
    pinned(
        "the tiny world's snapshot BLAKE3",
        &bytes,
        TINY_SNAPSHOT_BLAKE3,
    )
}

pub fn check_run() -> Result<(), String> {
    pinned("the run's hash at 120", &run_value()?, RUN_WORLD_HASH)
}

/// A web test: its name and the function that runs it.
pub type WebTest = (&'static str, fn() -> Result<(), String>);

pub const WEB_TESTS: &[WebTest] = &[
    ("persist.hash_vectors", check_hash_vectors),
    ("persist.tiny_world", check_tiny_world),
    ("persist.run_and_fork", check_run),
];

/// One `ok name` or `FAIL name: why` line per web test.
pub fn web_report() -> String {
    let mut out = String::new();
    for (name, test) in WEB_TESTS {
        match test() {
            Ok(()) => out.push_str(&format!("ok {name}\n")),
            Err(e) => out.push_str(&format!("FAIL {name}: {e}\n")),
        }
    }
    out
}
