//! A block volume under Rapier bodies (docs/spec/voxels.md 5.1): a stone floor 40 x 3 x 40 cells
//! across nine 16-cell chunks, its top at y = 0, and a grid of boxes and balls 4 m apart dropped on
//! it from 4 m, one of them straddling a chunk border. At [`DIG_TICK`], when the bodies have come
//! to rest and gone to sleep, a scripted system digs a 5 x 3 x 5 hole through the floor under the
//! middle box, which falls through it.
//!
//! The web test `physics.volume_fork` (checks.md 7.2, `tests`) forks the scene at every tick and
//! restores it cold after every world holding its chunk shapes is gone; the report gives the
//! scene's hash at every tick, which the WebAssembly build must reproduce.

use amoris_sim::order::by_id_mut;
use amoris_sim::voxels::{Block, Voxels};
use amoris_sim::{EntityId, Name, NoHooks, RunCondition, Sim, SimClock, TickPhase};
use bevy_ecs::prelude::{Query, Res};

use super::{fork, ledger};
use crate::body::{Collider, RigidBody, Shape, Transform, Velocity};
use crate::shapes::ShapeKey;
use crate::{Physics, PhysicsCache};
use amoris_sim::persisted::PersistedCache;

/// The tick whose `Update` phase digs the hole.
pub const DIG_TICK: u64 = 150;
/// Ticks of the web test's scene.
pub const VOLUME_TICKS: u64 = 220;
/// The web test's cell edge: a size no other check uses, so its chunk shapes are its own.
const WEB_CELL: f64 = 0.875;

/// The floor's cells: x and z 0 to 39, y 0 to 2, the volume's origin putting its top at y = 0 and
/// its middle over the origin.
const FLOOR: ([i32; 3], [i32; 3]) = ([0, 0, 0], [39, 2, 39]);
/// The hole: under the middle box, through the floor.
const HOLE: ([i32; 3], [i32; 3]) = ([18, 0, 18], [22, 2, 22]);

/// The volume's entity in a world made by [`blocks_world`].
pub fn volume_id() -> EntityId {
    EntityId::FIRST
}

/// The volume: air and stone, the floor filled at tick 0.
pub fn floor(cell_m: f64) -> Voxels {
    let mut v = Voxels {
        cell_m,
        ..Voxels::default()
    };
    v.palette.push(Block {
        name: "stone".into(),
        solid: true,
        faces: ["stone".into(), "stone".into(), "stone".into()],
        ..Block::default()
    });
    v.fill(FLOOR.0, FLOOR.1, 1, 0).expect("a small fill");
    v
}

/// The volume's origin for cells of `cell_m`: the floor's top at y = 0, its middle over x = z = 0.
pub fn origin(cell_m: f64) -> [f64; 3] {
    [-20.0 * cell_m, -3.0 * cell_m, -20.0 * cell_m]
}

/// `probe.dig`: at [`DIG_TICK`], the hole, stamped with the tick.
pub fn dig(clock: Res<SimClock>, mut volumes: Query<(&EntityId, &mut Voxels)>) {
    let t = clock.tick.0;
    if t != DIG_TICK {
        return;
    }
    for (id, mut v) in by_id_mut(&mut volumes) {
        if *id == volume_id() {
            v.fill(HOLE.0, HOLE.1, 0, t).expect("a small fill");
        }
    }
}

/// A world with physics and the digger and no entities: what a fork restores into.
pub fn fresh_blocks() -> Sim {
    let mut sim = super::fresh();
    sim.add_system("probe.dig", TickPhase::Update, RunCondition::Always, dig)
        .expect("the digger's key is free");
    sim
}

/// The scene: the volume (entity 1) and `(2 * side + 1)^2` bodies 4 m apart at 4 m up, boxes and
/// balls in turn, the middle one (`side` away in each direction) a box over the hole. The body at
/// x = -4 m straddles the chunk border at cell 16 when `cell_m` is 1.
pub fn blocks_world(cell_m: f64, side: i32) -> Sim {
    let mut sim = fresh_blocks();
    let mut b = sim.boundary();
    b.spawn((
        Name::new("Floor").expect("a short name"),
        Transform::at(origin(cell_m)),
        floor(cell_m),
    ))
    .expect("ids remain");
    let mut n = 0;
    for x in -side..=side {
        for z in -side..=side {
            let shape = if n % 2 == 0 {
                Shape::Cuboid {
                    half_extents: [0.5, 0.5, 0.5],
                }
            } else {
                Shape::Ball { radius: 0.5 }
            };
            n += 1;
            b.spawn((
                Transform::at([4.0 * f64::from(x), 4.0, 4.0 * f64::from(z)]),
                Velocity::default(),
                RigidBody::dynamic(),
                Collider::new(shape).with_density(1000.0),
            ))
            .expect("ids remain");
        }
    }
    sim
}

/// The middle body of a world made by [`blocks_world`] with `side`.
pub fn middle_body(side: i32) -> EntityId {
    let n = 2 * side + 1;
    let middle = u64::try_from(n * n / 2).unwrap_or(0);
    EntityId::new(volume_id().get() + 1 + middle).expect("a non-zero id")
}

fn step(sim: &mut Sim) -> Result<(), String> {
    let r = sim.step(&mut NoHooks).map_err(|p| p.message)?;
    match r.errors.first() {
        Some(e) => Err(e.message.clone()),
        None => Ok(()),
    }
}

/// The web test's scene's hash at tick 0 and after each of `ticks` ticks.
pub fn volume_chain(ticks: u64) -> Result<Vec<u64>, String> {
    let l = ledger();
    let mut sim = blocks_world(WEB_CELL, 1);
    let mut out = vec![l.snapshot(sim.world())?.hash()];
    for _ in 0..ticks {
        step(&mut sim)?;
        out.push(l.snapshot(sim.world())?.hash());
    }
    Ok(out)
}

/// The keys of the solver's chunk shapes.
pub fn chunk_keys(sim: &Sim) -> Vec<ShapeKey> {
    sim.world()
        .get_resource::<Physics>()
        .map(|p| {
            p.volumes
                .iter()
                .filter(|c| c.handle.is_some())
                .map(|c| c.record.key)
                .collect()
        })
        .unwrap_or_default()
}

/// Bytes of the solver: the physics cache's, the chunk shapes' as Rapier writes them, and the whole
/// `PhysicsWorld`'s as Rapier writes it (every shape inline, as layout 1 did).
#[derive(Clone, Copy, Debug)]
pub struct CacheBytes {
    pub cache: usize,
    pub chunks: usize,
    pub inline: usize,
}

impl CacheBytes {
    /// Whether the cache leaves out the chunk shapes: smaller than Rapier's own bytes by at least
    /// all but 128 bytes a collider of the chunk shapes.
    pub fn by_reference(&self, colliders: usize) -> bool {
        self.cache + self.chunks <= self.inline + 128 * colliders
    }
}

/// The solver's [`CacheBytes`].
pub fn cache_bytes(sim: &Sim) -> Result<CacheBytes, String> {
    let mut cache = Vec::new();
    PhysicsCache::encode(sim.world(), &mut cache).map_err(|p| p.message)?;
    let p = sim.world().get_resource::<Physics>().ok_or("no solver")?;
    let mut chunks = 0;
    for c in &p.volumes {
        if let Some(co) = c.handle.and_then(|h| p.world.colliders.get(h)) {
            chunks += bincode::serialize(co.shared_shape())
                .map_err(|e| e.to_string())?
                .len();
        }
    }
    let inline = bincode::serialize(&p.world)
        .map_err(|e| e.to_string())?
        .len();
    Ok(CacheBytes {
        cache: cache.len(),
        chunks,
        inline,
    })
}

/// `physics.volume_fork`: the chunk shapes are not in the cache; a fork at every tick 0 to
/// `DIG_TICK + 10` of the scene continues 3 ticks with the original's hashes (the forks around the
/// dig carry the hole into their runs); a snapshot at tick `DIG_TICK - 1` restored after every
/// world holding its chunk shapes is dropped has the snapshot's bytes and continues with the
/// original's hashes to [`VOLUME_TICKS`], through the dig.
pub fn check_volume_forks() -> Result<(), String> {
    let (l, forks, run) = (
        ledger(),
        usize::try_from(DIG_TICK).unwrap_or(0) + 10,
        3usize,
    );
    let expected = volume_chain(VOLUME_TICKS)?;
    let mut sim = blocks_world(WEB_CELL, 1);
    let mut cold_at = None;
    for t in 0..forks {
        if t + 1 == usize::try_from(DIG_TICK).unwrap_or(0) {
            cold_at = Some(l.snapshot(sim.world())?);
        }
        let mut f = fork(&l, &sim, fresh_blocks)?;
        if l.snapshot(f.world())?.hash() != expected[t] {
            return Err(format!("the fork at tick {t} differs from its source"));
        }
        for k in 1..=run {
            step(&mut f)?;
            if l.snapshot(f.world())?.hash() != expected[t + k] {
                return Err(format!(
                    "the fork made at tick {t} diverged at tick {}",
                    t + k
                ));
            }
        }
        step(&mut sim)?;
    }
    let bytes = cache_bytes(&sim)?;
    let keys = chunk_keys(&sim);
    if bytes.chunks == 0 || !bytes.by_reference(keys.len()) {
        return Err(format!("the cache holds the chunk shapes: {bytes:?}"));
    }
    let snap = cold_at.ok_or("no snapshot before the dig")?;
    drop(sim);
    if keys.iter().any(|k| crate::shapes::recall(k).is_some()) {
        return Err("a chunk shape is still held: the restore would not be cold".into());
    }
    let mut cold = fresh_blocks();
    l.restore(&mut cold, &snap)?;
    if l.snapshot(cold.world())? != snap {
        return Err("a cold restore changed the snapshot's bytes".into());
    }
    let start = usize::try_from(DIG_TICK).unwrap_or(0);
    for (t, want) in expected.iter().enumerate().skip(start) {
        step(&mut cold)?;
        if l.snapshot(cold.world())?.hash() != *want {
            return Err(format!("the cold restore diverged at tick {t}"));
        }
    }
    Ok(())
}

/// The report's volume lines: `volume tick hash` for every tick of the scene.
pub fn volume_report() -> String {
    match volume_chain(VOLUME_TICKS) {
        Ok(h) => h
            .iter()
            .enumerate()
            .map(|(t, x)| format!("volume {t} {x:016x}\n"))
            .collect(),
        Err(e) => format!("FAIL volume chain: {e}\n"),
    }
}
