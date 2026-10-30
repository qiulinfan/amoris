//! The physics state as world state (docs/spec/persistence.md 2 and 8; versions.md 3.6 and 6.3):
//! the persisted classes of this crate's types, the solver as one Cache section, and the
//! persisted names and traces of its components.
//!
//! The Cache's bytes are Rapier's `PhysicsWorld` through its own `serde` output, written by
//! bincode 1.3.3 (fixed-width little-endian integers, `u64` lengths, floats by their bits; trailing
//! bytes refused on decode): the encoding the physics spike wrote and restored natively and in the
//! browser to the same bytes. `amoris-physics` cannot link `amoris-persist`'s PCE (architecture.md
//! 5), and a cache section is opaque bytes (persistence.md 4.1), so the encoding is named in the
//! cache's identity instead.
//!
//! Layout 2 writes each collider's shape in a slot ([`slots`]). A fixed body's own collider whose
//! shape `physics.step` built from the body's `RigidBody` and `Collider` (registered in the table
//! of built shapes, `crate::shapes`), while those components are unchanged (their spec hash is the
//! one the body carries), is written by reference: its shape, a level's triangle mesh say, is in
//! the `Collider` component already, and decoding puts back the shape those components build,
//! shared with any world that still holds it or built from the restored components. Every other
//! shape is written inline, as layout 1 wrote all of them.
//!
//! Layout 3 adds the block volumes' chunk colliders (`crate::volumes`, voxels.md 5.1): each one's
//! slot carries its `ChunkRecord`, from which decoding puts back `Physics::volumes`, and names its
//! shape by the key of its content while the volume's chunk still has that content (its `edited`
//! tick before the record's check, or else its content's key compared), so decoding takes the
//! shape from the table or builds it from the restored `Voxels`; a chunk changed since the solver
//! last looked at it has its shape inline.

mod slots;

use std::collections::BTreeMap;

use amoris_contract::{Problem, detail};
use amoris_sim::persisted::{Persisted, PersistedCache, RegisterPersisted, Staged};
use amoris_sim::voxels::{Cell, Voxels};
use amoris_sim::{EntityId, EntityIndex};
use bevy_ecs::prelude::World;
use bincode::Options;
use rapier3d::prelude::PhysicsWorld;
use serde_json::json;
use serde_reflection::{Samples, Tracer};

use crate::boat::{Boat, Floater, Hull, Sail, Trim};
use crate::body::{
    BodyKind, Collider, ExternalForce, PartShape, RigidBody, Shape, Transform, Velocity,
};
use crate::sea::{Rain, Sea, Wind};
use crate::shapes::{self, ShapeKey};
use crate::solver::{
    BodyIndex, ContactLog, Physics, RebuiltSolver, entity_of, rebuild_index, spec_hash, spec_of,
};
use crate::volumes::{self, ChunkCollider};
use crate::walker::Walker;
use crate::water::{
    BedSource, Edge, Style, WaterDrain, WaterRegion, WaterScratch, WaterSource, WaterSteps,
};
use slots::{In, Out, Reading, WorldIn, WorldOut, Writing};

/// The cache's identity (versions.md 3.6): the library, its exact versions, the features that
/// change its serialized layout, the encoding, this wrapper's layout number, and the version of
/// the shape builds a by-reference slot stands for (`shapes`: bumped with any change to
/// `solver::shared_shape`, to `volumes::build` or to parry's builders under them;
/// `probe::layout::SHAPE_BUILDS` pins them; 2 added the volume chunks). A test checks the versions
/// against `Cargo.lock`.
pub const IDENTITY: &str = "rapier3d 0.36.0 parry3d 0.31.1 dim3 f32 serde-serialize \
                            enhanced-determinism bincode 1.3.3 layout 3 shapes 2";

/// Declares this crate's types (persistence.md 2): every component persisted, the solver a Cache
/// whose state, the resource [`Physics`], is Derived (restore removes it and the section's staged
/// apply puts it back; persistence.md 14, choice 15), the body index and the contact log Derived
/// (rebuilt by `physics.index` after a restore), and the report of a solver rebuilt at a restore
/// ignored.
pub fn declare<R: RegisterPersisted>(r: &mut R) {
    r.component::<Transform>()
        .component::<Velocity>()
        .component::<RigidBody>()
        .component::<Collider>()
        .component::<ExternalForce>()
        .component::<Floater>()
        .component::<crate::vessel::Vessel>()
        .component::<Boat>()
        .component::<Sail>()
        .component::<Hull>()
        .component::<Sea>()
        .component::<Wind>()
        .component::<Rain>()
        .component::<Walker>()
        .component::<crate::avatar::Avatar>()
        .component::<crate::walker::Buttons>()
        .component::<crate::nav::NavGrid>()
        .ignore::<crate::nav::NavCache>(
            "baked navigation grids, rebuilt from the colliders at their next query",
        )
        .component::<crate::avatar::CameraRig>()
        .component::<crate::board::Board>()
        .component::<amoris_sim::voxels::Voxels>()
        .component::<WaterRegion>()
        .component::<crate::water::WaterColliderBed>()
        .component::<crate::water::resident::WaterResident>()
        .component::<crate::water::resident::WaterOutletStage>()
        .component::<crate::water::resident::WaterProbe>()
        .component::<crate::water::feedback::WaterFeedback>()
        .component::<crate::water::displacement::WaterDisplacement>()
        .resource::<crate::water::resident::WaterCheckpoints>()
        .component::<WaterSource>()
        .component::<crate::water::WaterFocus>()
        .component::<crate::terrain::Terrain>()
        .component::<crate::terrain::TerrainGround>()
        .component::<WaterDrain>()
        .cache::<PhysicsCache>()
        .derived::<Physics>("the state of cache physics.rapier, put back by its section")
        .derived::<BodyIndex>("rebuilt from the bodies' user_data, which name their entities")
        .derived::<ContactLog>("filled by physics.step and emptied by physics.contacts each tick")
        .ignore::<RebuiltSolver>("a report for the next physics.step, taken by it")
        .ignore::<WaterScratch>(
            "water working buffers/unfinished-tick continuation; reset on restore",
        )
        .ignore::<WaterSteps>("the tick each water tile was last stepped, for the feed only")
        .ignore::<crate::water::buoyancy::Samples>(
            "current-pose GPU buoyancy samples; rebuilt each tick and reset on restore",
        )
        .ignore::<crate::vessel::Hulls>(
            "vessels' hulls, built from their colliders whenever those change",
        )
        .ignore::<crate::water::swimming::Samples>(
            "current-pose GPU Walker samples; rebuilt each tick and reset on restore",
        )
        .ignore::<crate::water::compute::Backend>(
            "host compute service; all water state is in WaterRegion",
        )
        .rebuild("physics.index", rebuild_index)
        .rebuild("physics.water_compute", crate::water::compute::invalidate);
}

fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
}

/// The solver's section.
pub struct PhysicsCache;

/// A by-reference shape no world held at decoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Missing {
    /// A fixed body's: its entity, spec hash and key.
    Fixed(EntityId, u64, ShapeKey),
    /// A volume chunk's: its entity, chunk and key.
    Chunk(EntityId, Cell, ShapeKey),
}

impl Missing {
    fn entity(&self) -> EntityId {
        match *self {
            Missing::Fixed(id, ..) | Missing::Chunk(id, ..) => id,
        }
    }
}

/// A decoded solver.
enum StagedPhysics {
    /// An empty section: no solver.
    Absent,
    /// A solver with every shape in place.
    Ready(Box<Physics>),
    /// A solver read with placeholders for the by-reference shapes that no world held (`missing`):
    /// apply builds them from the restored components and reads the bytes again. `first` holds the
    /// shapes the first reading shared until then.
    Pending {
        bytes: Vec<u8>,
        first: Box<PhysicsWorld>,
        missing: Vec<Missing>,
        volumes: Vec<ChunkCollider>,
    },
}

impl Staged for StagedPhysics {
    fn apply(self: Box<Self>, world: &mut World) {
        match *self {
            StagedPhysics::Absent => {
                world.remove_resource::<Physics>();
            }
            StagedPhysics::Ready(p) => world.insert_resource(*p),
            StagedPhysics::Pending {
                bytes,
                first,
                missing,
                volumes,
            } => match complete(world, &bytes, &missing) {
                Some(w) => world.insert_resource(Physics { world: w, volumes }),
                // The components do not build the shapes the bytes refer to: a snapshot this
                // engine did not write. The solver is rebuilt from the components instead
                // (versions.md 6.3); the next step reports it, and a verified restore names the
                // section.
                None => {
                    let dt = world
                        .get_resource::<amoris_sim::SimClock>()
                        .map_or(f64::from(first.integration_parameters.dt), |c| c.dt());
                    world.insert_resource(Physics::new(dt));
                    let mut entities: Vec<EntityId> = missing.iter().map(Missing::entity).collect();
                    entities.sort_unstable();
                    entities.dedup();
                    world.insert_resource(RebuiltSolver { entities });
                }
            },
        }
    }
}

/// Builds the `missing` shapes from the restored components (each entity's components must have
/// the spec hash and key of its slot; each volume chunk the content of its key) and reads `bytes`
/// again with them.
fn complete(world: &mut World, bytes: &[u8], missing: &[Missing]) -> Option<PhysicsWorld> {
    let wanted: BTreeMap<EntityId, (u64, ShapeKey)> = missing
        .iter()
        .filter_map(|m| match *m {
            Missing::Fixed(id, spec, key) => Some((id, (spec, key))),
            Missing::Chunk(..) => None,
        })
        .collect();
    let mut chunks: BTreeMap<EntityId, Vec<(Cell, ShapeKey)>> = BTreeMap::new();
    for m in missing {
        if let Missing::Chunk(id, chunk, key) = *m {
            chunks.entry(id).or_default().push((chunk, key));
        }
    }
    let mut built = BTreeMap::new();
    let mut q = world.query::<(&EntityId, Option<&RigidBody>, &Collider)>();
    #[allow(clippy::disallowed_methods)] // looked up by id: the order changes nothing
    for (id, body, collider) in q.iter(world) {
        let Some(&(spec, key)) = wanted.get(id) else {
            continue;
        };
        if shapes::spec_and_key(body, collider) != (spec, key) {
            return None;
        }
        let shape = shapes::fixed(*id, collider, spec, key).ok()?;
        built.insert(key, shape);
    }
    if !wanted.values().all(|(_, k)| built.contains_key(k)) {
        return None;
    }
    // The entity index is rebuilt after the sections, so the volumes are found by their ids.
    let mut q = world.query::<(&EntityId, &Voxels)>();
    #[allow(clippy::disallowed_methods)] // looked up by id: the order changes nothing
    for (id, v) in q.iter(world) {
        for &(chunk, key) in chunks.get(id).map_or(&[][..], Vec::as_slice) {
            built.insert(key, volumes::rebuilt(v, chunk, key)?);
        }
    }
    if !chunks
        .values()
        .flatten()
        .all(|(_, k)| built.contains_key(k))
    {
        return None;
    }
    let reading = Reading::new(&built);
    let w = read(bytes, &reading).ok()?;
    reading.missing.borrow().is_empty().then_some(w)
}

fn read(bytes: &[u8], reading: &Reading) -> Result<PhysicsWorld, Problem> {
    options()
        .deserialize_seed(WorldIn { r: reading }, bytes)
        .map_err(|e| noncanonical(e.to_string()))
}

/// Whether the volume chunk of `c` still has the content its record names: its volume has the
/// record's form and the chunk is unedited since the record's check, or else its content has the
/// record's key. `forms` keeps each volume's form for the call.
fn chunk_in_sync(
    world: &World,
    index: Option<&EntityIndex>,
    c: &ChunkCollider,
    forms: &mut BTreeMap<EntityId, u64>,
) -> bool {
    let Some(v) = index
        .and_then(|i| i.get(c.entity))
        .and_then(|e| world.get::<Voxels>(e))
    else {
        return false;
    };
    let form = *forms.entry(c.entity).or_insert_with(|| volumes::form(v));
    let Some(chunk) = v.chunk(c.record.chunk) else {
        return false;
    };
    form == c.record.form
        && (chunk.edited < c.record.checked || volumes::chunk_key(v, chunk) == Some(c.record.key))
}

/// For each collider in Rapier's order, how its shape is written: a volume chunk's by its record,
/// by reference while its chunk is in sync ([`chunk_in_sync`]); a fixed body's own collider by
/// reference if its shape was built from that body's components (registered under the body's spec
/// hash) and its entity's `RigidBody` and `Collider` still have that spec hash, so they build this
/// very shape; every other inline.
fn by_reference(world: &World, p: &Physics) -> Vec<Out> {
    let index = world.get_resource::<EntityIndex>();
    let chunks: BTreeMap<(u32, u32), &ChunkCollider> = p
        .volumes
        .iter()
        .filter_map(|c| Some((c.handle?.into_raw_parts(), c)))
        .collect();
    let mut forms = BTreeMap::new();
    let p = &p.world;
    p.colliders
        .iter()
        .map(|(h, co)| {
            if let Some(c) = chunks.get(&h.into_raw_parts()) {
                return if chunk_in_sync(world, index, c, &mut forms) {
                    Out::Chunk(c.record)
                } else {
                    Out::ChunkInline(c.record)
                };
            }
            fixed_slot(world, index, p, co).map_or(Out::Inline, Out::ByRef)
        })
        .collect()
}

/// A fixed body's own collider's slot by reference, if it has one ([`by_reference`]).
fn fixed_slot(
    world: &World,
    index: Option<&EntityIndex>,
    p: &PhysicsWorld,
    co: &rapier3d::prelude::Collider,
) -> Option<(u64, ShapeKey)> {
    let body = p.bodies.get(co.parent()?)?;
    let id = entity_of(body.user_data)?;
    if !body.is_fixed() || entity_of(co.user_data) != Some(id) {
        return None;
    }
    let spec = spec_of(body.user_data);
    let (key, built_from) = shapes::registered(co.shared_shape())?;
    if built_from != spec {
        return None;
    }
    let e = index?.get(id)?;
    let same = verified(world, e, spec)?;
    same.then_some((spec, key))
}

/// Whether entity `e`'s `RigidBody` and `Collider` hash to `spec`. Hashing a large fixed shape (a
/// terrain's ground) for every snapshot cost more than the rest of the tick, so a verdict is kept
/// per world and entity with the components' change ticks and holds while neither changes.
fn verified(world: &World, e: bevy_ecs::entity::Entity, spec: u64) -> Option<bool> {
    type Verdicts =
        std::collections::BTreeMap<bevy_ecs::entity::Entity, ([u32; 2], bool, u64, bool)>;
    // By world (a few: the game's and its forks'), then by entity.
    static KEPT: std::sync::Mutex<Vec<(bevy_ecs::world::WorldId, Verdicts)>> =
        std::sync::Mutex::new(Vec::new());
    let entity = world.get_entity(e).ok()?;
    let collider = entity.get_change_ticks::<Collider>()?.changed.get();
    let body = entity.get_change_ticks::<RigidBody>();
    let ticks = (
        [collider, body.map_or(0, |t| t.changed.get())],
        body.is_some(),
    );
    let mut kept = KEPT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let id = world.id();
    let at = match kept.iter().position(|(w, _)| *w == id) {
        Some(at) => at,
        None => {
            if kept.len() >= 16 {
                kept.clear();
            }
            kept.push((id, Verdicts::new()));
            kept.len() - 1
        }
    };
    let kept = &mut kept[at].1;
    if let Some(&(t, has_body, s, same)) = kept.get(&e)
        && (t, has_body, s) == (ticks.0, ticks.1, spec)
    {
        return Some(same);
    }
    let same = spec_hash(world.get::<RigidBody>(e), world.get::<Collider>(e)?) == spec;
    if kept.len() >= 4096 {
        kept.clear();
    }
    kept.insert(e, (ticks.0, ticks.1, spec, same));
    Some(same)
}

fn encode_problem(reason: String) -> Problem {
    Problem::new(
        "persist.encode",
        format!("The physics cache could not be written: {reason}"),
        detail([
            ("section", json!(PhysicsCache::NAME)),
            ("entity", json!(null)),
            ("reason", json!(reason)),
        ]),
    )
}

fn noncanonical(reason: String) -> Problem {
    Problem::new(
        "persist.noncanonical",
        format!("The physics cache's bytes are not a solver this engine wrote: {reason}"),
        detail([("offset", json!(0)), ("reason", json!(reason))]),
    )
}

impl PersistedCache for PhysicsCache {
    const NAME: &'static str = "physics.rapier";

    fn identity() -> &'static str {
        IDENTITY
    }

    /// The solver's bytes; nothing when the world has no solver.
    fn encode(world: &World, out: &mut Vec<u8>) -> Result<(), Problem> {
        let Some(p) = world.get_resource::<Physics>() else {
            return Ok(());
        };
        let slots = by_reference(world, p);
        let writing = Writing::new(&slots);
        let value = WorldOut {
            world: &p.world,
            writing: &writing,
        };
        options()
            .serialize_into(&mut *out, &value)
            .map_err(|e| encode_problem(e.to_string()))?;
        if writing.written() != slots.len() {
            return Err(encode_problem(format!(
                "{} shape slots written for {} colliders",
                writing.written(),
                slots.len()
            )));
        }
        Ok(())
    }

    /// Decodes and checks that every body and collider names an entity, that there is one shape
    /// slot per collider, that a slot by reference belongs to a fixed body's own collider and
    /// names that body's spec hash, and that a volume chunk's slot belongs to a collider with no
    /// body, one per chunk.
    fn decode(bytes: &[u8]) -> Result<Box<dyn Staged>, Problem> {
        if bytes.is_empty() {
            return Ok(Box::new(StagedPhysics::Absent));
        }
        let none = BTreeMap::new();
        let reading = Reading::new(&none);
        let w = read(bytes, &reading)?;
        let bodies = w.bodies.iter().map(|(_, b)| b.user_data);
        let colliders = w.colliders.iter().map(|(_, c)| c.user_data);
        if let Some(bad) = bodies.chain(colliders).find(|u| entity_of(*u).is_none()) {
            return Err(noncanonical(format!(
                "a body or collider names no entity (user data {bad:#x})"
            )));
        }
        let slots = reading.slots.into_inner();
        if slots.len() != w.colliders.len() {
            return Err(noncanonical(format!(
                "{} shape slots for {} colliders",
                slots.len(),
                w.colliders.len()
            )));
        }
        // The slots no world held, in ascending order, walked beside the colliders.
        let mut gaps = reading.missing.into_inner().into_iter().peekable();
        let mut missing = Vec::new();
        let mut chunks = Vec::new();
        for (k, ((h, co), slot)) in w.colliders.iter().zip(&slots).enumerate() {
            let (spec, key) = match *slot {
                In::Inline => continue,
                In::ByRef(r) => r,
                In::Chunk(record) => {
                    let (None, Some(id)) = (co.parent(), entity_of(co.user_data)) else {
                        return Err(noncanonical(format!(
                            "collider {k} is a volume chunk's, but it has a body"
                        )));
                    };
                    if gaps.next_if_eq(&k).is_some() {
                        missing.push(Missing::Chunk(id, record.chunk, record.key));
                    }
                    chunks.push(ChunkCollider {
                        entity: id,
                        handle: Some(h),
                        record,
                    });
                    continue;
                }
            };
            let entity = entity_of(co.user_data);
            let owner = co.parent().and_then(|h| w.bodies.get(h)).filter(|b| {
                b.is_fixed() && spec_of(b.user_data) == spec && entity_of(b.user_data) == entity
            });
            let (Some(_), Some(id)) = (owner, entity) else {
                return Err(noncanonical(format!(
                    "collider {k}'s shape is by reference, but it is not a fixed body's own \
                     collider or names another spec"
                )));
            };
            if gaps.next_if_eq(&k).is_some() {
                missing.push(Missing::Fixed(id, spec, key));
            }
        }
        chunks.sort_unstable_by_key(|c| (c.entity, c.record.chunk));
        if let Some(d) = chunks
            .windows(2)
            .find(|d| (d[0].entity, d[0].record.chunk) == (d[1].entity, d[1].record.chunk))
        {
            return Err(noncanonical(format!(
                "two colliders for chunk {:?} of entity {}",
                d[0].record.chunk, d[0].entity
            )));
        }
        if missing.is_empty() {
            return Ok(Box::new(StagedPhysics::Ready(Box::new(Physics {
                world: w,
                volumes: chunks,
            }))));
        }
        Ok(Box::new(StagedPhysics::Pending {
            bytes: bytes.to_vec(),
            first: Box::new(w),
            missing,
            volumes: chunks,
        }))
    }

    /// Rebuilds the solver from the components (versions.md 6.3): a new one, every body made at the
    /// next step from its `RigidBody`, `Collider`, `Transform` and `Velocity`, in id order. Contacts
    /// restart without warm-start impulses, so it continues close to, not exactly as, the old one.
    fn rebuild(world: &mut World) -> Result<(), Problem> {
        let rate = world.resource::<amoris_sim::SimClock>().dt();
        world.insert_resource(Physics::new(rate));
        rebuild_index(world);
        Ok(())
    }
}

macro_rules! persisted {
    ($($t:ty => $name:literal),* $(,)?) => {
        $(impl Persisted for $t {
            const NAME: &'static str = $name;
            const VERSION: u32 = 1;
        })*
    };
}

persisted! {
    Transform => "Transform",
    Velocity => "Velocity",
    ExternalForce => "ExternalForce",
    Sail => "Sail",
    Hull => "Hull",
    Wind => "Wind",
    Rain => "Rain",
    WaterSource => "WaterSource",
    WaterDrain => "WaterDrain",
}

impl Persisted for RigidBody {
    const NAME: &'static str = "RigidBody";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<BodyKind>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for Collider {
    const NAME: &'static str = "Collider";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<PartShape>(s)?;
        t.trace_type::<Shape>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for crate::avatar::Avatar {
    const NAME: &'static str = "Avatar";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<crate::avatar::Facing>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for crate::nav::NavGrid {
    const NAME: &'static str = "NavGrid";
    const VERSION: u32 = 1;
}

impl Persisted for crate::walker::Buttons {
    const NAME: &'static str = "Buttons";
    const VERSION: u32 = 1;
}

impl Persisted for crate::avatar::CameraRig {
    const NAME: &'static str = "CameraRig";
    const VERSION: u32 = 1;
}

impl Persisted for Walker {
    const NAME: &'static str = "Walker";
    const VERSION: u32 = 2;
}

impl Persisted for Floater {
    const NAME: &'static str = "Floater";
    const VERSION: u32 = 2;
}

impl Persisted for crate::vessel::Vessel {
    const NAME: &'static str = "Vessel";
    const VERSION: u32 = 1;
}

impl Persisted for Sea {
    const NAME: &'static str = "Sea";
    const VERSION: u32 = 2;
}

impl Persisted for WaterRegion {
    const NAME: &'static str = "WaterRegion";
    const VERSION: u32 = 10;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<crate::water::Solver>(s)?;
        t.trace_type::<BedSource>(s)?;
        t.trace_type::<Edge>(s)?;
        t.trace_type::<Style>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for Boat {
    const NAME: &'static str = "Boat";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<Trim>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amoris_sim::NoHooks;
    use amoris_sim::persisted::{record_samples, tracer_config};

    /// The wrappers write Rapier's own bytes: with no slots, the `PhysicsWorld` written field by
    /// field and through the layers is what its derived `Serialize` writes, for the sailing scene
    /// and for a level scene with a fixed terrain.
    #[test]
    fn the_wrappers_write_rapiers_own_bytes() {
        let mut sailing = crate::probe::sailing_world();
        let mut level = crate::probe::level::level_world(6, 1);
        for sim in [&mut sailing, &mut level] {
            for _ in 0..30 {
                sim.step(&mut NoHooks).unwrap();
            }
            let p = &sim.world().resource::<Physics>().world;
            assert!(!p.colliders.is_empty());
            let writing = Writing::transparent();
            let mut ours = Vec::new();
            let world = WorldOut {
                world: p,
                writing: &writing,
            };
            options().serialize_into(&mut ours, &world).unwrap();
            assert_eq!(writing.written(), p.colliders.len());
            assert!(ours == options().serialize(p).unwrap());
        }
    }

    /// Every persisted type traces with `serde-reflection` and the registry resolves completely
    /// (persistence.md 3.5 and 6.2): what `amoris-persist` does to fingerprint them.
    #[test]
    fn every_persisted_type_traces() {
        let mut t = Tracer::new(tracer_config());
        let mut s = Samples::new();
        record_samples(&mut t, &mut s).unwrap();
        Transform::trace(&mut t, &s).unwrap();
        Velocity::trace(&mut t, &s).unwrap();
        RigidBody::trace(&mut t, &s).unwrap();
        Collider::trace(&mut t, &s).unwrap();
        ExternalForce::trace(&mut t, &s).unwrap();
        Floater::trace(&mut t, &s).unwrap();
        crate::vessel::Vessel::trace(&mut t, &s).unwrap();
        Boat::trace(&mut t, &s).unwrap();
        Sail::trace(&mut t, &s).unwrap();
        Hull::trace(&mut t, &s).unwrap();
        Sea::trace(&mut t, &s).unwrap();
        Wind::trace(&mut t, &s).unwrap();
        Rain::trace(&mut t, &s).unwrap();
        let reg = t.registry().unwrap();
        for name in [
            "Transform",
            "Velocity",
            "RigidBody",
            "BodyKind",
            "MassProps",
            "Collider",
            "Shape",
            "PartShape",
            "Part",
            "ExternalForce",
            "Floater",
            "BuoyPoint",
            "Vessel",
            "Hole",
            "Boat",
            "Trim",
            "Sail",
            "Hull",
            "Sea",
            "Wave",
            "Wind",
            "Rain",
        ] {
            assert!(
                reg.contains_key(name),
                "{name} missing from {:?}",
                reg.keys()
            );
        }
    }
}
