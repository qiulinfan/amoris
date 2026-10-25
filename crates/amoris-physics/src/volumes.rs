//! Block volumes in the solver (docs/spec/voxels.md 5.1): each 16 x 16 x 16 chunk of a `Voxels`
//! holding a solid cell is one fixed collider with no body, whose shape is a parry `Voxels` of the
//! chunk's solid cells, so Rapier bodies land on volumes and the queries (`crate::query`) meet them.
//!
//! - **Built from content.** A chunk's shape depends only on `cell_m` and which of its 4096 cells
//!   are solid: its key is the BLAKE3 digest of those (`shapes::chunk_key`); it is built whole, from
//!   its cells in index order, and kept in the process's table of built shapes as a fixed
//!   `Collider`'s shape is. parry's `Voxels` keeps its edit history in its layout (the numbers of
//!   its own chunks, by which contact manifolds are keyed, and its BVH), so a shape edited in place
//!   with `set_voxel` would differ from the one a restore builds from the component, and contacts
//!   would diverge.
//! - **Checked by edit tick.** `physics.step` looks at a chunk again only when its `edited` tick is
//!   not before the tick it was last checked at, or when the volume's form (`cell_m` and which
//!   palette blocks are solid) changed; a check replaces the shape only when its key changed, and
//!   wakes the bodies touching it. A write that replaces a volume and leaves a changed chunk's
//!   `edited` behind is not seen until that chunk is edited again (voxels.md 3).
//! - **Persisted by reference.** Each collider's [`ChunkRecord`] (its chunk, the tick it was checked
//!   at, the volume's form and its shape's key) is its shape slot in the physics cache
//!   (`cache::slots`), which reads the records back with the solver; the shape itself is written
//!   only while the chunk has changed since the solver last saw it.
//! - The colliders' entity is the volume's (their `user_data`), so ray casts, overlaps and contact
//!   events name it; their friction and restitution are a `Collider`'s defaults (0.5 and 0).

use amoris_contract::{Problem, detail};
use amoris_sim::EntityId;
use amoris_sim::voxels::{CHUNK, CHUNK_CELLS, Cell, Chunk, Voxels};
use rapier3d::parry::math::IVector;
use rapier3d::prelude::{
    ActiveEvents, ColliderBuilder, ColliderHandle, PhysicsWorld, RigidBodyHandle, SharedShape,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::geom::{self, V3};
use crate::shapes::{self, ShapeKey};

/// A chunk's solid cells, one bit per cell in the chunk's index order.
pub type Mask = [u8; CHUNK_CELLS / 8];

/// What the solver knows of a chunk with a collider, written in the collider's shape slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRecord {
    /// The chunk's key in its volume.
    pub chunk: Cell,
    /// The tick of the step that last looked at the chunk.
    pub checked: u64,
    /// The volume's form then ([`form`]).
    pub form: u64,
    /// The key of the chunk's shape ([`shapes::chunk_key`]).
    pub key: ShapeKey,
}

/// A chunk the solver has looked at: its collider, or none when it holds no solid cell.
#[derive(Clone, Debug)]
pub struct ChunkCollider {
    pub entity: EntityId,
    pub handle: Option<ColliderHandle>,
    pub record: ChunkRecord,
}

/// A volume as `physics.step` syncs it: its entity, the component and its origin.
pub struct Volume<'a> {
    pub id: EntityId,
    pub voxels: &'a Voxels,
    pub origin: V3,
}

/// FNV-1a 64 over `cell_m`'s bits and each palette block's solidity: what every chunk's shape
/// depends on besides its cells.
pub fn form(v: &Voxels) -> u64 {
    let mut h =
        amoris_sim::rng::fnv1a64_extend(0xcbf2_9ce4_8422_2325, &v.cell_m.to_bits().to_le_bytes());
    for b in &v.palette {
        h = amoris_sim::rng::fnv1a64_extend(h, &[u8::from(b.solid)]);
    }
    h
}

/// The chunk's solid cells, or `None` when none is (or its cells are not a chunk's 4096).
pub fn solid_mask(v: &Voxels, c: &Chunk) -> Option<Mask> {
    if c.cells.0.len() != CHUNK_CELLS {
        return None;
    }
    let mut solid = [false; 256];
    for (s, b) in solid.iter_mut().zip(&v.palette) {
        *s = b.solid;
    }
    let mut mask = [0u8; CHUNK_CELLS / 8];
    let mut any = false;
    for (i, &b) in c.cells.0.iter().enumerate() {
        if solid[usize::from(b)] {
            mask[i / 8] |= 1 << (i % 8);
            any = true;
        }
    }
    any.then_some(mask)
}

/// The key of the chunk's shape, or `None` when it holds no solid cell.
pub fn chunk_key(v: &Voxels, c: &Chunk) -> Option<ShapeKey> {
    solid_mask(v, c).map(|m| shapes::chunk_key(v.cell_m, &m))
}

/// The chunk's shape: a parry `Voxels` of cell edge `cell_m` (in `f32`) holding its solid cells,
/// in index order, at their coordinates in the chunk.
pub fn build(cell_m: f64, mask: &Mask) -> SharedShape {
    let edge = CHUNK.unsigned_abs() as usize;
    let mut cells = Vec::new();
    for i in 0..CHUNK_CELLS {
        if mask[i / 8] & (1 << (i % 8)) != 0 {
            let at = |n: usize| i32::try_from(n).unwrap_or(0);
            cells.push(IVector::new(
                at(i % edge),
                at(i / (edge * edge)),
                at((i / edge) % edge),
            ));
        }
    }
    let s = geom::f32_of(cell_m);
    SharedShape::voxels(rapier3d::math::Vector::new(s, s, s), &cells)
}

/// The registered shape for `mask`, built if no world holds it.
fn shape(cell_m: f64, key: ShapeKey, mask: &Mask) -> SharedShape {
    shapes::chunk(key, || build(cell_m, mask))
}

/// The shape a by-reference chunk slot names, built from the volume's chunk if its content has
/// that key (a restore whose shape no world holds).
pub fn rebuilt(v: &Voxels, chunk: Cell, key: ShapeKey) -> Option<SharedShape> {
    let mask = solid_mask(v, v.chunk(chunk)?)?;
    (shapes::chunk_key(v.cell_m, &mask) == key).then(|| shape(v.cell_m, key, &mask))
}

fn volume_problem(entity: EntityId, reason: &str) -> Problem {
    Problem::new(
        "voxels.invalid",
        format!("Entity {entity}: {reason}; its volume does not collide with bodies."),
        detail([("entity", json!(entity.get())), ("reason", json!(reason))]),
    )
}

/// Wakes the bodies in a contact pair with collider `h`, in the narrow phase's order.
fn wake_touching(world: &mut PhysicsWorld, h: ColliderHandle) {
    let bodies: Vec<RigidBodyHandle> = world
        .contact_pairs_with(h)
        .filter_map(|p| {
            let other = if p.collider1 == h {
                p.collider2
            } else {
                p.collider1
            };
            world.colliders.get(other)?.parent()
        })
        .collect();
    for b in bodies {
        world.wake_up(b, true);
    }
}

/// Removes a chunk's collider, keeping its entity for the ends of its contacts.
fn remove(
    world: &mut PhysicsWorld,
    c: &ChunkCollider,
    removed: &mut Vec<(ColliderHandle, EntityId)>,
) {
    if let Some(h) = c.handle {
        wake_touching(world, h);
        world.remove_collider(h);
        removed.push((h, c.entity));
    }
}

/// Brings the chunk colliders (`colliders`, ascending by entity and chunk) to `volumes` (ascending
/// by id) at `tick`: new chunks get a collider, chunks edited or of a changed form are checked,
/// chunks gone lose theirs (listed in `removed`), and colliders follow their volume's origin. A
/// volume whose `cell_m` or origin the solver's `f32` cannot hold is reported and left out.
pub fn sync(
    world: &mut PhysicsWorld,
    colliders: &mut Vec<ChunkCollider>,
    tick: u64,
    volumes: &[Volume],
    removed: &mut Vec<(ColliderHandle, EntityId)>,
    report: &mut dyn FnMut(EntityId, Problem),
) {
    let old = std::mem::take(colliders);
    let mut old = old.into_iter().peekable();
    let mut next = Vec::with_capacity(old.len());
    for vol in volumes {
        while let Some(c) = old.next_if(|c| c.entity < vol.id) {
            remove(world, &c, removed);
        }
        let v = vol.voxels;
        let edge = geom::f32_of(v.cell_m);
        let usable =
            v.cell_m > 0.0 && edge.is_finite() && edge > 0.0 && geom::finite32(&vol.origin);
        if !usable {
            report(
                vol.id,
                volume_problem(
                    vol.id,
                    "its cell_m must be positive and its origin and cell_m finite in the solver's f32",
                ),
            );
        }
        let form = form(v);
        let span = f64::from(CHUNK) * v.cell_m;
        for chunk in v.chunks.iter().filter(|_| usable) {
            let mut had = None;
            while let Some(c) = old.next_if(|c| c.entity == vol.id && c.record.chunk <= chunk.key) {
                if c.record.chunk == chunk.key {
                    had = Some(c);
                } else {
                    remove(world, &c, removed);
                }
            }
            let corner = geom::add(vol.origin, chunk.key.map(|k| f64::from(k) * span));
            if !geom::finite32(&corner) {
                if let Some(c) = had {
                    remove(world, &c, removed);
                }
                continue;
            }
            let at = geom::vec_in(corner);
            let stale = had
                .as_ref()
                .is_none_or(|c| c.record.form != form || chunk.edited >= c.record.checked);
            let c = match had {
                Some(c) if !stale => c,
                had => {
                    let handle = had.as_ref().and_then(|c| c.handle);
                    let Some(mask) = solid_mask(v, chunk) else {
                        if let Some(c) = &had {
                            remove(world, c, removed);
                        }
                        next.push(ChunkCollider {
                            entity: vol.id,
                            handle: None,
                            record: ChunkRecord {
                                chunk: chunk.key,
                                checked: tick,
                                form,
                                key: [0; 32],
                            },
                        });
                        continue;
                    };
                    let key = shapes::chunk_key(v.cell_m, &mask);
                    let handle = match (handle, had.as_ref().map(|c| c.record.key)) {
                        (Some(h), Some(k)) if k == key => h,
                        (Some(h), _) => {
                            wake_touching(world, h);
                            if let Some(co) = world.colliders.get_mut(h) {
                                co.set_shape(shape(v.cell_m, key, &mask));
                            }
                            h
                        }
                        (None, _) => {
                            let co = ColliderBuilder::new(shape(v.cell_m, key, &mask))
                                .translation(at)
                                .friction(0.5)
                                .restitution(0.0)
                                .active_events(ActiveEvents::COLLISION_EVENTS)
                                .user_data(u128::from(vol.id.get()))
                                .build();
                            world.insert_collider(co, None)
                        }
                    };
                    ChunkCollider {
                        entity: vol.id,
                        handle: Some(handle),
                        record: ChunkRecord {
                            chunk: chunk.key,
                            checked: tick,
                            form,
                            key,
                        },
                    }
                }
            };
            if let Some(h) = c.handle
                && world
                    .colliders
                    .get(h)
                    .is_some_and(|co| co.translation() != at)
            {
                wake_touching(world, h);
                if let Some(co) = world.colliders.get_mut(h) {
                    co.set_translation(at);
                }
            }
            next.push(c);
        }
        while let Some(c) = old.next_if(|c| c.entity == vol.id) {
            remove(world, &c, removed);
        }
    }
    for c in old {
        remove(world, &c, removed);
    }
    *colliders = next;
}

#[cfg(test)]
mod tests {
    use super::*;
    use amoris_sim::voxels::Block;

    fn volume() -> Voxels {
        let mut v = Voxels::default();
        v.palette.push(Block {
            name: "stone".into(),
            solid: true,
            faces: ["s".into(), "s".into(), "s".into()],
            ..Block::default()
        });
        v.palette.push(Block {
            name: "water".into(),
            solid: false,
            faces: ["w".into(), String::new(), String::new()],
            ..Block::default()
        });
        v
    }

    /// A chunk's key follows its solid cells and `cell_m`, not its other blocks or edit ticks, and
    /// its shape holds exactly those cells where the volume has them.
    #[test]
    fn a_chunks_shape_follows_its_solid_cells() {
        let mut v = volume();
        v.set([1, 2, 3], 1, 5).unwrap();
        v.set([15, 0, 0], 1, 5).unwrap();
        let k = chunk_key(&v, &v.chunks[0]).unwrap();
        v.set([4, 4, 4], 2, 9).unwrap();
        assert_eq!(chunk_key(&v, &v.chunks[0]), Some(k));
        v.set([4, 4, 4], 1, 9).unwrap();
        assert_ne!(chunk_key(&v, &v.chunks[0]), Some(k));
        let mut half = v.clone();
        half.cell_m = 0.5;
        assert_ne!(
            chunk_key(&half, &half.chunks[0]),
            chunk_key(&v, &v.chunks[0])
        );

        let mask = solid_mask(&v, &v.chunks[0]).unwrap();
        let s = build(1.0, &mask);
        let vox = s.as_voxels().unwrap();
        let filled: Vec<[i32; 3]> = vox
            .voxels()
            .filter(|d| !d.state.is_empty())
            .map(|d| [d.grid_coords.x, d.grid_coords.y, d.grid_coords.z])
            .collect();
        let mut filled = filled;
        filled.sort_unstable();
        assert_eq!(filled, vec![[1, 2, 3], [4, 4, 4], [15, 0, 0]]);

        let mut wet = volume();
        wet.set([0, 0, 0], 2, 1).unwrap();
        assert!(solid_mask(&wet, &wet.chunks[0]).is_none());
    }
}
