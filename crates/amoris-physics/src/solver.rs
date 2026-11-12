//! The solver as world state (docs/spec/architecture.md 4.4; persistence.md 8; simulation.md 8.5):
//! Rapier's `PhysicsWorld` in the resource [`Physics`], a Cache, and the systems `physics.step` and
//! `physics.contacts`.
//!
//! `physics.step` syncs the bodies in from their components in `EntityId` order (new bodies made,
//! gone ones removed, changed ones rebuilt, moved ones teleported, a changed `CollisionFilter` set
//! on the built collider in place), applies each body's
//! `ExternalForce` after resetting Rapier's user forces, steps once, and writes `Transform`,
//! `Velocity` and the boats' readings back in `EntityId` order. Each body carries its entity in
//! the low 64 bits of its `user_data` and a hash of its `RigidBody` and `Collider` in the high 64
//! bits, both inside Rapier's serialized state, so the reverse map ([`BodyIndex`]) is Derived and
//! a component changed by any route (a script, a boundary write) is seen without change
//! detection, which simulation.md 4.5 forbids.

use std::sync::Mutex;

use amoris_contract::{Problem, detail};
use amoris_sim::order::{by_id, by_id_mut};
use amoris_sim::sim::system_failed;
use amoris_sim::voxels::Voxels;
use amoris_sim::{EntityId, SimClock, SystemKey, TickOutput, TickPhase};
use bevy_ecs::change_detection::DetectChanges;
use bevy_ecs::prelude::{Commands, Entity, Query, Ref, Res, ResMut, Resource, With, Without};
use rapier3d::prelude::{
    ActiveEvents, ColliderBuilder, ColliderHandle, ColliderSet, CollisionEvent,
    CollisionEventFlags, ContactPair, EventHandler, MassProperties, PhysicsWorld, RigidBodyBuilder,
    RigidBodyHandle, RigidBodySet, SharedShape, SoftBodySet, SoftBodyTearEvent,
};
use serde_json::json;

use crate::boat::{Boat, Floater};
use crate::body::{
    BodyKind, Collider, CollisionFilter, ExternalForce, PartShape, RigidBody, Shape, Transform,
    Velocity,
};
use crate::geom::{self, V3};
use crate::readings;
use crate::sea::{GRAVITY, Sea};
use crate::sensors::{self, SensorOverlaps};
use crate::shapes;
use crate::volumes::{self, ChunkCollider, Volume};
use crate::walker::Walker;

/// The solver's whole cross-tick state: bodies, colliders, broad and narrow phase with contact
/// manifolds and warm-start impulses, islands and sleep counters, joints, integration parameters,
/// and what it knows of the block volumes' chunks (`volumes`, each collider's record written in its
/// shape slot). A Cache (persistence.md 8), written by [`crate::PhysicsCache`].
#[derive(Resource)]
pub struct Physics {
    pub world: PhysicsWorld,
    /// The volumes' chunks the solver has looked at, ascending by entity and chunk
    /// (`crate::volumes`).
    pub volumes: Vec<ChunkCollider>,
}

impl Physics {
    /// An empty solver stepping `dt` seconds under standard gravity.
    pub fn new(dt: f64) -> Physics {
        let mut world = PhysicsWorld::new();
        world.gravity = geom::vec_in([0.0, -GRAVITY, 0.0]);
        world.integration_parameters.dt = geom::f32_of(dt);
        Physics {
            world,
            volumes: Vec::new(),
        }
    }
}

/// Each body's handle by entity, ascending by id. Derived: rebuilt from the bodies' `user_data`
/// after a restore ([`rebuild_index`]).
#[derive(Resource, Debug, Default)]
pub struct BodyIndex {
    entries: Vec<(EntityId, RigidBodyHandle)>,
}

impl BodyIndex {
    /// The body of `id`.
    pub fn get(&self, id: EntityId) -> Option<RigidBodyHandle> {
        self.entries
            .binary_search_by_key(&id, |e| e.0)
            .ok()
            .map(|i| self.entries[i].1)
    }

    /// Every (entity, body), ascending by id.
    pub fn entries(&self) -> &[(EntityId, RigidBodyHandle)] {
        &self.entries
    }
}

/// One begun or ended touch of the step: a contact between two solid colliders, or an overlap
/// with a sensor; `a` is the lower id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Touch {
    pub a: EntityId,
    pub b: EntityId,
    pub began: bool,
    /// Whether `a`'s and `b`'s colliders are sensors: a touch with a sensor is an overlap.
    pub sensors: (bool, bool),
}

/// The contacts and sensor overlaps that began and ended during this tick's step, for
/// `physics.contacts`. Derived: empty at every boundary.
#[derive(Resource, Debug, Default)]
pub struct ContactLog {
    pub(crate) events: Vec<Touch>,
}

/// Present after a restore whose physics cache referred to shapes that the restored components do
/// not build (a snapshot this engine did not write): the solver was rebuilt from the components
/// instead (versions.md 6.3), inexactly, and the next `physics.step` reports each entity named,
/// once. A report for the next tick, not game state: declared ignored.
#[derive(Resource, Debug, Default)]
pub struct RebuiltSolver {
    pub(crate) entities: Vec<EntityId>,
}

fn rebuilt_problem(entity: EntityId) -> Problem {
    let reason = "the snapshot's physics cache refers to a shape this entity's RigidBody and \
                  Collider do not build";
    Problem::new(
        "persist.noncanonical",
        format!(
            "Entity {entity}: {reason}; the solver was rebuilt from the components and continues \
             inexactly (versions.md 6.3)."
        ),
        detail([
            ("offset", json!(0)),
            ("entity", json!(entity.get())),
            ("reason", json!(reason)),
        ]),
    )
}

/// The entity in a body's or collider's `user_data`.
pub fn entity_of(user_data: u128) -> Option<EntityId> {
    EntityId::new(u64::try_from(user_data & u128::from(u64::MAX)).unwrap_or(0))
}

/// Whether the body built from spec `spec` had a `RigidBody`, as remembered when this process built
/// it: an entity that loses its `RigidBody` changes its body without a change on either component,
/// and the spec (which hashes the `RigidBody` or its absence) tells it apart. Kept outside the
/// solver's serialized state; a spec this process has not built (a restored solver) answers `None`
/// and the caller compares the hashes instead, which gives the same verdict.
fn bodied(spec: u64, built: Option<bool>) -> Option<bool> {
    static KNOWN: std::sync::Mutex<std::collections::BTreeMap<u64, bool>> =
        std::sync::Mutex::new(std::collections::BTreeMap::new());
    let mut known = KNOWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(has) = built {
        if known.len() >= 65_536 {
            known.clear();
        }
        known.insert(spec, has);
    }
    known.get(&spec).copied()
}

pub(crate) fn spec_of(user_data: u128) -> u64 {
    u64::try_from(user_data >> 64).unwrap_or(0)
}

/// A hash of what the body is made from, kept in its `user_data` to see a change.
pub(crate) fn spec_hash(body: Option<&RigidBody>, collider: &Collider) -> u64 {
    let bytes = bincode::serialize(&(body, collider)).unwrap_or_default();
    amoris_sim::rng::fnv1a64_extend(0xcbf2_9ce4_8422_2325, &bytes)
}

/// Rebuilds [`BodyIndex`] from the bodies' `user_data` (after a restore), and empties the
/// contact log.
pub fn rebuild_index(world: &mut bevy_ecs::prelude::World) {
    let mut entries: Vec<(EntityId, RigidBodyHandle)> = world
        .get_resource::<Physics>()
        .map(|p| {
            p.world
                .bodies
                .iter()
                .filter_map(|(h, b)| entity_of(b.user_data).map(|id| (id, h)))
                .collect()
        })
        .unwrap_or_default();
    entries.sort_unstable_by_key(|e| e.0);
    world.insert_resource(BodyIndex { entries });
    world.insert_resource(ContactLog::default());
}

fn shape_problem(entity: EntityId, reason: &str) -> Problem {
    Problem::new(
        "sim.collider_invalid",
        format!("Entity {entity}: {reason}"),
        detail([("entity", json!(entity.get())), ("reason", json!(reason))]),
    )
}

fn part_shape(entity: EntityId, s: &PartShape) -> Result<SharedShape, Problem> {
    Ok(match s {
        PartShape::Ball { radius } => SharedShape::ball(geom::f32_of(*radius)),
        PartShape::Cuboid { half_extents: h } => {
            SharedShape::cuboid(geom::f32_of(h[0]), geom::f32_of(h[1]), geom::f32_of(h[2]))
        }
        PartShape::Capsule {
            half_height,
            radius,
        } => SharedShape::capsule_y(geom::f32_of(*half_height), geom::f32_of(*radius)),
        PartShape::ConvexHull { points } => {
            let pts: Vec<_> = points.iter().map(|p| geom::vec_in(*p)).collect();
            SharedShape::convex_hull(&pts)
                .ok_or_else(|| shape_problem(entity, "the hull's points span no volume"))?
        }
    })
}

/// The solver's shape for a `Shape`.
///
/// A by-reference slot of the physics cache (layout 2, persistence.md 8) means "the shape this
/// function builds from the body's components", so a change to what it builds (another mesh flag,
/// another hull or compound handling) or to parry's builders under it (a parry upgrade, a patch
/// under `[patch.crates-io]`) changes what old snapshots mean: bump the `shapes` token of the
/// cache's identity (`crate::IDENTITY`; versions.md 3.6). `probe::layout::SHAPE_BUILDS` pins the
/// digest of what it builds, checked by the web test `physics.shape_builds` natively and in
/// WebAssembly, so such a change fails there until the token is bumped and the digest re-blessed.
pub(crate) fn shared_shape(entity: EntityId, shape: &Shape) -> Result<SharedShape, Problem> {
    Ok(match shape {
        Shape::Ball { radius } => part_shape(entity, &PartShape::Ball { radius: *radius })?,
        Shape::Cuboid { half_extents } => part_shape(
            entity,
            &PartShape::Cuboid {
                half_extents: *half_extents,
            },
        )?,
        Shape::Capsule {
            half_height,
            radius,
        } => part_shape(
            entity,
            &PartShape::Capsule {
                half_height: *half_height,
                radius: *radius,
            },
        )?,
        Shape::ConvexHull { points } => {
            let pts: Vec<_> = points.iter().map(|p| geom::vec_in(*p)).collect();
            SharedShape::convex_hull(&pts)
                .ok_or_else(|| shape_problem(entity, "the hull's points span no volume"))?
        }
        Shape::TriMesh { vertices, indices } => {
            let v: Vec<_> = vertices.iter().map(|p| geom::vec_in(*p)).collect();
            SharedShape::trimesh(v, indices.clone())
                .map_err(|e| shape_problem(entity, &format!("the mesh cannot be built: {e:?}")))?
        }
        Shape::HeightField {
            size,
            cell_m,
            heights,
        } => {
            // parry's rows run along z and its columns along x, the array column-major; its
            // field is centred on the shape's origin (`shape_offset` puts the corner back).
            let (nx, nz) = (size[0] as usize + 1, size[1] as usize + 1);
            let mut zx = vec![0.0f32; nx * nz];
            for k in 0..nz {
                for i in 0..nx {
                    zx[k + i * nz] = geom::f32_of(heights[i + k * nx]);
                }
            }
            let scale = geom::vec_in([
                f64::from(size[0]) * cell_m,
                1.0,
                f64::from(size[1]) * cell_m,
            ]);
            SharedShape::heightfield(rapier3d::parry::utils::Array2::new(nz, nx, zx), scale)
        }
        Shape::Compound { parts } => {
            let mut out = Vec::with_capacity(parts.len());
            for p in parts {
                let rotation = geom::normalized(p.rotation).unwrap_or(geom::IDENTITY);
                out.push((
                    geom::pose_in(p.position, rotation),
                    part_shape(entity, &p.shape)?,
                ));
            }
            SharedShape::compound(out)
        }
    })
}

fn body_problem(entity: EntityId, message: &str, reason: &str) -> Problem {
    Problem::new(
        "sim.body_invalid",
        format!("Entity {entity}: {message}"),
        detail([("entity", json!(entity.get())), ("reason", json!(reason))]),
    )
}

/// The solver's pose for a `Transform`: a position finite in `f32` (1e39 is not) and a finite,
/// non-zero rotation, normalized.
fn pose_of(entity: EntityId, tf: &Transform) -> Result<rapier3d::math::Pose, Problem> {
    match geom::normalized(tf.rotation) {
        Some(q) if geom::finite32(&tf.position) => Ok(geom::pose_in(tf.position, q)),
        _ => Err(body_problem(
            entity,
            "its Transform needs a position finite in the solver's f32 (below 3.4e38) and a \
             finite, non-zero rotation quaternion.",
            "Transform",
        )),
    }
}

/// Whether a collider's mass properties as Rapier computes them in `f32` (from its density) are
/// usable: a mass and principal inertias positive, normal and finite. A ball of radius 1e-30 is a
/// size `f32` holds, but its mass rounds to 0.
fn usable_mass(co: &rapier3d::prelude::Collider) -> bool {
    let mp = co.mass_properties();
    let ok = |x: f32| x.is_finite() && x >= f32::MIN_POSITIVE;
    let i = mp.principal_inertia();
    ok(mp.mass()) && ok(i.x) && ok(i.y) && ok(i.z)
}

/// Where a shape's own origin sits in its body's frame: a heightfield's centre, since parry centres
/// the field and the component names its corner; the origin for every other shape.
pub(crate) fn shape_offset(shape: &Shape) -> rapier3d::math::Vector {
    match shape {
        Shape::HeightField { size, cell_m, .. } => geom::vec_in([
            0.5 * f64::from(size[0]) * cell_m,
            0.0,
            0.5 * f64::from(size[1]) * cell_m,
        ]),
        _ => geom::vec_in([0.0; 3]),
    }
}

/// Builds the body and collider of one entity into the solver.
fn insert(
    physics: &mut PhysicsWorld,
    entity: EntityId,
    tf: &Transform,
    vel: Option<&Velocity>,
    body: Option<&RigidBody>,
    collider: &Collider,
    filter: Option<&CollisionFilter>,
) -> Result<RigidBodyHandle, Problem> {
    crate::body::validate(entity, body, collider)?;
    let filter = filter.copied().unwrap_or_default();
    let pose = pose_of(entity, tf)?;
    let dynamic = body.is_some_and(|b| b.kind == BodyKind::Dynamic);
    // A fixed body's shape comes from the process's table of built shapes (`shapes`), which the
    // physics cache writes by reference.
    let (hash, shape) = if dynamic {
        (
            spec_hash(body, collider),
            shared_shape(entity, &collider.shape)?,
        )
    } else {
        let (spec, key) = shapes::spec_and_key(body, collider);
        (spec, shapes::fixed(entity, collider, spec, key)?)
    };
    let mut rb = if dynamic {
        RigidBodyBuilder::dynamic()
    } else {
        RigidBodyBuilder::fixed()
    }
    .pose(pose)
    .user_data(u128::from(entity.get()) | (u128::from(hash) << 64));
    bodied(hash, Some(body.is_some()));
    if let Some(b) = body {
        rb = rb
            .linear_damping(geom::f32_of(b.linear_damping))
            .angular_damping(geom::f32_of(b.angular_damping))
            .gravity_scale(geom::f32_of(b.gravity_scale));
        if let Some(m) = b.mass {
            rb = rb.additional_mass_properties(MassProperties::new(
                geom::vec_in(m.center),
                geom::f32_of(m.mass),
                geom::vec_in(m.inertia),
            ));
        }
    }
    if dynamic && let Some(v) = vel {
        rb = rb
            .linvel(geom::vec_in(v.linear))
            .angvel(geom::vec_in(v.angular));
    }
    // No density means no mass from the shape: Rapier's density 0 skips the shape's mass
    // properties entirely (numeric.md 5).
    let co = ColliderBuilder::new(shape)
        .translation(shape_offset(&collider.shape))
        .density(geom::f32_of(collider.density.unwrap_or(0.0)))
        .friction(geom::f32_of(collider.friction))
        .restitution(geom::f32_of(collider.restitution))
        .sensor(filter.sensor)
        .collision_groups(sensors::groups_of(&filter))
        .active_events(ActiveEvents::COLLISION_EVENTS)
        .user_data(u128::from(entity.get()))
        .build();
    if collider.density.is_some() && !usable_mass(&co) {
        return Err(shape_problem(
            entity,
            "the mass or inertia its density gives is 0 or infinite in the solver's f32",
        ));
    }
    let (handle, _) = physics.insert(rb, co);
    Ok(handle)
}

/// Collects the step's contact and sensor events by entity. A collider removed before the step
/// (its body despawned or rebuilt) is no longer in the set when Rapier reports its pairs' ends, so
/// `removed` names its entity. Rapier marks an overlap's events with `SENSOR`; which side is the
/// sensor is read from the collider, except that an ended overlap was the one before the step's
/// changes: `was_sensor` lists the colliders that were sensors and are gone or solid now, and
/// `now_sensor` those that became sensors in this step's sync.
struct Collector {
    events: Mutex<Vec<Touch>>,
    removed: Vec<(ColliderHandle, EntityId)>,
    was_sensor: Vec<ColliderHandle>,
    now_sensor: Vec<ColliderHandle>,
}

impl EventHandler for Collector {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        colliders: &ColliderSet,
        event: CollisionEvent,
        _pair: Option<&ContactPair>,
    ) {
        let (a, b, began, flags) = match event {
            CollisionEvent::Started(a, b, f) => (a, b, true, f),
            CollisionEvent::Stopped(a, b, f) => (a, b, false, f),
        };
        let overlap = flags.contains(CollisionEventFlags::SENSOR);
        // Handles carry their generation, so a rebuilt body's new collider in a reused slot is not
        // its removed one.
        let id = |h: ColliderHandle| {
            colliders
                .get(h)
                .and_then(|c| entity_of(c.user_data))
                .or_else(|| self.removed.iter().find(|r| r.0 == h).map(|r| r.1))
        };
        let sensor = |h: ColliderHandle| {
            overlap
                && if began {
                    colliders.get(h).is_some_and(|c| c.is_sensor())
                } else if self.was_sensor.contains(&h) {
                    true
                } else {
                    !self.now_sensor.contains(&h) && colliders.get(h).is_some_and(|c| c.is_sensor())
                }
        };
        if let (Some(ia), Some(ib)) = (id(a), id(b))
            && let Ok(mut v) = self.events.lock()
        {
            let (sa, sb) = (sensor(a), sensor(b));
            v.push(if ia <= ib {
                Touch {
                    a: ia,
                    b: ib,
                    began,
                    sensors: (sa, sb),
                }
            } else {
                Touch {
                    a: ib,
                    b: ia,
                    began,
                    sensors: (sb, sa),
                }
            });
        }
    }

    fn handle_contact_force_event(
        &self,
        _dt: f32,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _pair: &ContactPair,
        _total: f32,
    ) {
    }

    fn handle_soft_body_tear_event(&self, _soft: &SoftBodySet, _event: &SoftBodyTearEvent) {}
}

/// Whether a body touches land: an active contact between one of its colliders and a fixed body's
/// (or a collider of no body).
fn touches_land(world: &PhysicsWorld, handle: RigidBodyHandle) -> bool {
    let fixed = |c: ColliderHandle| {
        world.colliders.get(c).is_some_and(|c| {
            c.parent()
                .and_then(|p| world.bodies.get(p))
                .is_none_or(|b| b.is_fixed())
        })
    };
    world.bodies.get(handle).is_some_and(|b| {
        b.colliders().iter().any(|&own| {
            world.contact_pairs_with(own).any(|pair| {
                let other = if pair.collider1 == own {
                    pair.collider2
                } else {
                    pair.collider1
                };
                pair.has_any_active_contact() && fixed(other)
            })
        })
    })
}

type BodyItem<'a> = (
    &'a EntityId,
    Option<&'a mut Transform>,
    Option<&'a mut Velocity>,
    Option<Ref<'a, RigidBody>>,
    Ref<'a, Collider>,
    Option<&'a mut ExternalForce>,
    Option<&'a mut Boat>,
    Option<&'a Floater>,
    Option<&'a crate::vessel::Vessel>,
    Option<&'a CollisionFilter>,
);

fn not_finite(entity: EntityId, what: &str) -> Problem {
    Problem::new(
        "number.not_finite",
        format!(
            "The solver gave entity {entity} a non-finite {what}; its components were kept and \
             its body reset to them at rest (numeric.md 7)."
        ),
        detail([
            ("field", json!(what)),
            ("value", json!("NaN")),
            ("entity", json!(entity.get())),
        ]),
    )
}

/// `physics.step`: sync in, forces, one step of `dt`, write back.
///
/// Block volumes sync after the bodies (`crate::volumes::sync`), in id order: a volume's origin is
/// its `Transform` (the origin without one), read from the bodies' rows for a volume that also has
/// a `Collider`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn step(
    clock: Res<SimClock>,
    mut physics: ResMut<Physics>,
    mut index: ResMut<BodyIndex>,
    mut log: ResMut<ContactLog>,
    mut out: ResMut<TickOutput>,
    seas: Query<(&EntityId, &Sea)>,
    mut bodies: Query<BodyItem<'static>>,
    mut loose: Query<(&EntityId, &mut ExternalForce), Without<Collider>>,
    volumes: Query<(&EntityId, &Voxels, Option<&Transform>), Without<Collider>>,
    clad: Query<(&EntityId, &Voxels), With<Collider>>,
    rebuilt: Option<ResMut<RebuiltSolver>>,
) {
    let key = SystemKey::new("physics.step").ok();
    let mut report = |entity: EntityId, p: Problem| {
        if let Some(k) = &key {
            out.report(system_failed(
                clock.tick,
                TickPhase::Physics,
                k,
                Some(entity),
                &p,
            ));
        }
    };
    if let Some(mut r) = rebuilt {
        for id in std::mem::take(&mut r.entities) {
            report(id, rebuilt_problem(id));
        }
    }
    let Physics {
        world,
        volumes: chunks,
    } = &mut *physics;
    let mut items: Vec<_> = by_id_mut(&mut bodies).collect();

    // 1. Remove the bodies whose entity is gone, lost its Collider or Transform, or whose RigidBody
    //    or Collider changed, in id order, before any insertion, so the arena's free slots are
    //    reused the same way always. Their colliders' entities are kept for the ends of their
    //    contacts, which Rapier reports during the step.
    let mut kept: Vec<Option<RigidBodyHandle>> = vec![None; items.len()];
    let mut removed = Vec::new();
    let mut was_sensor = Vec::new();
    let mut now_sensor = Vec::new();
    let mut i = 0;
    for &(id, handle) in &index.entries {
        while i < items.len() && *items[i].0 < id {
            i += 1;
        }
        let here = i < items.len() && *items[i].0 == id && items[i].1.is_some();
        // A body's spec is hashed again only when its components changed: hashing a large
        // TriMesh (a terrain's ground) every tick cost most of the step.
        let same = match world.bodies.get(handle) {
            Some(b) if here => {
                let (_, _, _, body, collider, ..) = &items[i];
                // A cache, not a rule: an unchanged component hashes to the spec it was built from,
                // so skipping the hash gives the verdict hashing would (repeated snapshots trust
                // change ticks the same way, persistence.md 4.3).
                #[allow(clippy::disallowed_methods)]
                let changed = collider.is_changed()
                    || body.as_ref().is_some_and(|b| b.is_changed())
                    || bodied(spec_of(b.user_data), None) != Some(body.is_some());
                !changed || spec_of(b.user_data) == spec_hash(body.as_deref(), collider)
            }
            _ => false,
        };
        if same {
            kept[i] = Some(handle);
            continue;
        }
        if let Some(b) = world.bodies.get(handle) {
            for &c in b.colliders() {
                if let Some(co) = world.colliders.get(c)
                    && let Some(e) = entity_of(co.user_data)
                {
                    removed.push((c, e));
                    if co.is_sensor() {
                        was_sensor.push(c);
                    }
                }
            }
        }
        world.remove_body(handle);
    }

    // 2. Make the new bodies, teleport or set the velocity of the kept ones whose components were
    //    written from outside, and apply each body's gathered force.
    let mut entries = Vec::with_capacity(items.len());
    for (slot, item) in kept.iter().zip(items.iter_mut()) {
        let (id, tf, vel, body, collider, ext, _, _, _, filter) = item;
        let id = **id;
        // The tick's gathered force goes to the solver once, whatever becomes of the body.
        let force = ext.as_deref_mut().map(std::mem::take).unwrap_or_default();
        let Some(tf) = tf.as_deref() else {
            report(
                id,
                body_problem(
                    id,
                    "it has a Collider but no Transform to place its body.",
                    "no Transform",
                ),
            );
            continue;
        };
        let handle = match slot {
            Some(h) => *h,
            None => match insert(
                world,
                id,
                tf,
                vel.as_deref(),
                body.as_deref(),
                collider,
                filter.as_deref(),
            ) {
                Ok(h) => h,
                Err(p) => {
                    report(id, p);
                    continue;
                }
            },
        };
        entries.push((id, handle));
        // The filter is set on the built collider in place, compared as the solver holds it:
        // a body without a `CollisionFilter` is solid and in every group.
        let want = filter.as_deref().copied().unwrap_or_default();
        let groups = sensors::groups_of(&want);
        // `get_mut` marks a collider modified, which Rapier acts on: it is taken only to change.
        let differ: Vec<ColliderHandle> = world.bodies.get(handle).map_or(Vec::new(), |b| {
            b.colliders()
                .iter()
                .copied()
                .filter(|c| {
                    world.colliders.get(*c).is_some_and(|co| {
                        co.is_sensor() != want.sensor || co.collision_groups() != groups
                    })
                })
                .collect()
        });
        for c in differ {
            if let Some(co) = world.colliders.get_mut(c) {
                if co.is_sensor() != want.sensor {
                    if want.sensor {
                        now_sensor.push(c);
                    } else {
                        was_sensor.push(c);
                    }
                }
                co.set_sensor(want.sensor);
                co.set_collision_groups(groups);
            }
        }
        let Some(b) = world.bodies.get_mut(handle) else {
            continue;
        };
        // Compared as the solver holds them: a component written back from the solver rounds to
        // the same f32, so only a write from outside moves the body.
        if geom::vec_in(tf.position) != b.translation()
            || geom::quat_in(tf.rotation) != *b.rotation()
        {
            match pose_of(id, tf) {
                Ok(pose) => b.set_position(pose, true),
                Err(p) => report(id, p),
            }
        }
        if !b.is_dynamic() {
            continue;
        }
        if let Some(v) = vel.as_deref() {
            if geom::vec_in(v.linear) != b.linvel() {
                b.set_linvel(geom::vec_in(v.linear), true);
            }
            if geom::vec_in(v.angular) != b.angvel() {
                b.set_angvel(geom::vec_in(v.angular), true);
            }
        }
        b.reset_forces(false);
        b.reset_torques(false);
        if force != ExternalForce::default() {
            b.add_force(geom::vec_in(force.force), true);
            b.add_torque(geom::vec_in(force.torque), true);
        }
    }
    index.entries = entries;
    // A force gathered on an entity without a collider has no body to act on; it does not carry
    // over to the next tick either.
    for (_, mut f) in by_id_mut(&mut loose) {
        *f = ExternalForce::default();
    }

    // 2a. The block volumes' chunks.
    let mut list: Vec<Volume> = by_id(&volumes)
        .map(|(id, voxels, tf)| Volume {
            id: *id,
            voxels,
            origin: tf.map_or(geom::ZERO, |t| t.position),
        })
        .collect();
    for (id, voxels) in by_id(&clad) {
        let origin = items
            .binary_search_by_key(id, |item| *item.0)
            .ok()
            .and_then(|i| items[i].1.as_deref())
            .map_or(geom::ZERO, |t| t.position);
        list.push(Volume {
            id: *id,
            voxels,
            origin,
        });
    }
    list.sort_unstable_by_key(|v| v.id);
    volumes::sync(
        world,
        chunks,
        clock.tick.0,
        &list,
        &mut removed,
        &mut report,
    );

    // 3. One step.
    let collector = Collector {
        events: Mutex::new(Vec::new()),
        removed,
        was_sensor,
        now_sensor,
    };
    world.step_with_events(&(), &collector);
    log.events = collector.events.into_inner().unwrap_or_default();
    let quarantined: Vec<RigidBodyHandle> = world.quarantine().bodies().to_vec();

    // 4. Write back, in id order.
    let sea = by_id(&seas).next().map(|(_, s)| s.field());
    let time = clock.time();
    let mut e = 0;
    for item in &mut items {
        let (id, tf, vel, _, _, _, boat, floater, vessel, _) = item;
        let id = **id;
        let Some(tf) = tf.as_deref_mut() else {
            continue;
        };
        while e < index.entries.len() && index.entries[e].0 < id {
            e += 1;
        }
        let Some(&(_, handle)) = index.entries.get(e).filter(|x| x.0 == id) else {
            continue;
        };
        let aground = boat.is_some() && touches_land(world, handle);
        let Some(b) = world.bodies.get_mut(handle) else {
            continue;
        };
        if b.is_dynamic() {
            let pos = geom::vec_out(b.translation());
            let rot = geom::quat_out(*b.rotation());
            let lin = geom::vec_out(b.linvel());
            let ang = geom::vec_out(b.angvel());
            let finite = geom::finite(&pos) && geom::finite(&rot) && geom::finite(&lin);
            if quarantined.contains(&handle) || !(finite && geom::finite(&ang)) {
                report(id, not_finite(id, "pose or velocity"));
                b.set_enabled(true);
                if let Ok(pose) = pose_of(id, tf) {
                    b.set_position(pose, true);
                }
                b.set_linvel(geom::vec_in(geom::ZERO), true);
                b.set_angvel(geom::vec_in(geom::ZERO), true);
                if let Some(v) = vel.as_deref_mut() {
                    *v = Velocity::default();
                }
                continue;
            }
            tf.position = pos;
            tf.rotation = rot;
            if let Some(v) = vel.as_deref_mut() {
                v.linear = lin;
                v.angular = ang;
            }
        }
        if let Some(boat) = boat.as_deref_mut() {
            let v = vel.as_deref().copied().unwrap_or_default();
            let submerged = floater
                .as_deref()
                .map(|f| f.submerged)
                .or(vessel.as_deref().map(|v| v.submerged));
            readings::after_step(boat, tf, &v, submerged, sea.as_ref(), time, aground);
        }
    }
}

/// `physics.contacts`: the step's touches as events (simulation.md 8.4 and 8.5), by pair
/// `(min id, max id)` and then in the order the step reported them. A contact between two solid
/// colliders is `physics.contact_began` or `physics.contact_ended`, subject the lower id; an
/// overlap with a sensor is `physics.sensor_entered` or `physics.sensor_left`, subject the sensor
/// (once for each sensor of the pair), data `{other}` in both.
///
/// Before them, each `Walker` without a `Collider` of its own is tested against the sensors where
/// the step left them ([`sensors::overlapped`]); a sensor it overlaps now and not in its
/// [`SensorOverlaps`], or the other way round, is a touch of the pair, and the list is written
/// (removed when it empties).
pub(crate) fn contacts(
    mut log: ResMut<ContactLog>,
    mut emit: amoris_sim::Emit,
    physics: Option<Res<Physics>>,
    mut walkers: Query<WalkerItem<'static>, Without<Collider>>,
    mut commands: Commands,
) {
    let mut events = std::mem::take(&mut log.events);
    if let Some(physics) = physics.as_deref() {
        for (entity, id, walker, tf, filter, overlaps) in by_id_mut(&mut walkers) {
            let now = sensors::overlapped(physics, *id, walker, tf.position, filter);
            let before: &[EntityId] = overlaps.as_deref().map_or(&[], |o| &o.sensors);
            if before == now.as_slice() {
                continue;
            }
            let (entered, left) = sensors::changes(before, &now);
            for (sensor, began) in left
                .into_iter()
                .map(|s| (s, false))
                .chain(entered.into_iter().map(|s| (s, true)))
            {
                events.push(Touch {
                    a: sensor.min(*id),
                    b: sensor.max(*id),
                    began,
                    sensors: (sensor < *id, sensor > *id),
                });
            }
            match overlaps {
                _ if now.is_empty() => {
                    commands.entity(entity).remove::<SensorOverlaps>();
                }
                Some(mut o) => o.sensors = now,
                None => {
                    commands
                        .entity(entity)
                        .insert(SensorOverlaps { sensors: now });
                }
            }
        }
    }
    events.sort_by_key(|e| (e.a, e.b));
    for t in events {
        let mut out = Vec::with_capacity(2);
        match t.sensors {
            (false, false) => out.push((
                if t.began {
                    "physics.contact_began"
                } else {
                    "physics.contact_ended"
                },
                t.a,
                t.b,
            )),
            (sa, sb) => {
                let kind = if t.began {
                    "physics.sensor_entered"
                } else {
                    "physics.sensor_left"
                };
                if sa {
                    out.push((kind, t.a, t.b));
                }
                if sb {
                    out.push((kind, t.b, t.a));
                }
            }
        }
        for (kind, subject, other) in out {
            let (Ok(kind), Ok(data)) = (
                amoris_sim::EventKind::new(kind),
                amoris_sim::PlainData::object(vec![(
                    "other".into(),
                    amoris_sim::PlainData::Number(other.to_f64()),
                )]),
            ) else {
                continue;
            };
            emit.emit(amoris_sim::NewEvent::new(kind).subject(subject).data(data));
        }
    }
}

type WalkerItem<'a> = (
    Entity,
    &'a EntityId,
    &'a Walker,
    &'a Transform,
    Option<&'a CollisionFilter>,
    Option<bevy_ecs::prelude::Mut<'a, SensorOverlaps>>,
);

/// The velocity of the body's material point `p` (world), for a body whose centre of mass is
/// `com` and whose velocity is `v`.
pub fn velocity_at(v: &Velocity, com: V3, p: V3) -> V3 {
    geom::add(v.linear, geom::cross(v.angular, geom::sub(p, com)))
}
