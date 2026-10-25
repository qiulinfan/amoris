//! The shapes of fixed colliders, built once in a process for each content of their components
//! (docs/spec/persistence.md 8 and 14, choice 3).
//!
//! `physics.step` builds a fixed body's collider shape through [`fixed`], which registers it in a
//! process-wide table under its key: the BLAKE3 digest of the bincode bytes of the body's
//! `RigidBody` and `Collider`, the same bytes whose FNV-1a hash (the spec hash) the body carries in
//! its `user_data`. The table holds each shape weakly. While some world holds a shape, a body built
//! from the same components shares it by `Arc` instead of building it again; the physics cache
//! writes a registered shape by reference (its spec hash and key) and reads it back from the table,
//! or, once no world holds it, builds it again from the restored components through [`fixed`].
//!
//! Only shapes built here are registered, so a key names the shape its components build, whichever
//! world built it (barring a BLAKE3 collision): the table changes how long a build or a restore
//! takes, never its result, and holds no game state. A shape decoded inline from a snapshot is never
//! registered, so it never reaches another world. Rapier never changes a shape in place
//! (`Collider::shape_mut` copies a shared one first), so a world cannot see another's use of it.
//!
//! The shapes of block volumes' chunks (`crate::volumes`) are registered here too, under the key
//! [`chunk_key`] gives their content ([`chunk`]), with spec hash 0: a chunk's shape has no
//! components of its own.

use std::collections::BTreeMap;
use std::io::{self, BufWriter, Write};
use std::sync::{Arc, Mutex, Weak};

use amoris_contract::Problem;
use amoris_sim::EntityId;
use amoris_sim::persisted::{ContentHash, ContentHasher};
use rapier3d::prelude::{Shape as RapierShape, SharedShape};

use crate::body::{Collider, RigidBody};
use crate::solver::shared_shape;

/// A shape's key: the BLAKE3 digest of its components' bytes.
pub type ShapeKey = [u8; 32];

/// The key's BLAKE3 context (`derive_key`). A dated hash domain, not the crate's name: the
/// 2026-10-16 rename left it as it was, because the keys are written into the physics cache,
/// which the world hash covers (persistence.md 8, versions.md 3.6).
const CONTEXT: &str = "Amoris 2026-10-15 pocket-physics fixed shape v1";

/// The BLAKE3 context of a volume chunk's key.
const CHUNK_CONTEXT: &str = "Amoris 2026-10-17 voxel chunk shape v1";

/// FNV-1a 64 and BLAKE3 over the same bytes, as bincode writes them.
struct Both {
    fnv: u64,
    key: ContentHasher,
}

impl Write for Both {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.fnv = amoris_sim::rng::fnv1a64_extend(self.fnv, b);
        self.key.update(b);
        Ok(b.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The spec hash of a body's components (`solver::spec_hash`) and their key, in one pass over their
/// bytes, which are streamed rather than gathered in a buffer.
pub fn spec_and_key(body: Option<&RigidBody>, collider: &Collider) -> (u64, ShapeKey) {
    let both = Both {
        fnv: 0xcbf2_9ce4_8422_2325,
        key: ContentHash::hasher(CONTEXT),
    };
    let mut w = BufWriter::with_capacity(1 << 16, both);
    // Writing to memory cannot fail; a failure would leave the bytes short, which only makes the
    // body's spec and key differ from any other's.
    let _ = bincode::serialize_into(&mut w, &(body, collider));
    let both = w
        .into_inner()
        .unwrap_or_else(|e| e.into_parts().1.into_parts().0);
    (both.fnv, both.key.finish().0)
}

struct Entry {
    shape: Weak<dyn RapierShape>,
    spec: u64,
}

struct Table {
    by_key: BTreeMap<ShapeKey, Entry>,
    /// The key of each registered shape by its address. An entry's `Weak` keeps the allocation,
    /// so an address names one shape while its entry exists.
    by_address: BTreeMap<usize, ShapeKey>,
    /// The size at which dead entries are next swept.
    sweep_at: usize,
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    by_key: BTreeMap::new(),
    by_address: BTreeMap::new(),
    sweep_at: 64,
});

fn with<R>(f: impl FnOnce(&mut Table) -> R) -> R {
    f(&mut TABLE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

fn address(shape: &SharedShape) -> usize {
    Arc::as_ptr(&shape.0).cast::<()>().addr()
}

/// The shape registered under `key` while a world holds it, and the spec hash of the components
/// that built it.
pub fn recall(key: &ShapeKey) -> Option<(SharedShape, u64)> {
    with(|t| {
        let e = t.by_key.get(key)?;
        Some((SharedShape(e.shape.upgrade()?), e.spec))
    })
}

/// The key and spec hash of `shape` if [`fixed`] built it.
pub fn registered(shape: &SharedShape) -> Option<(ShapeKey, u64)> {
    let at = address(shape);
    with(|t| {
        let key = t.by_address.get(&at)?;
        let e = t.by_key.get(key)?;
        (e.shape.as_ptr().cast::<()>().addr() == at).then_some((*key, e.spec))
    })
}

/// Registers `shape` under `key` unless a world holds one registered there already, which is
/// returned instead.
fn register(key: ShapeKey, spec: u64, shape: SharedShape) -> SharedShape {
    with(|t| {
        if let Some(e) = t.by_key.get(&key)
            && let Some(live) = e.shape.upgrade()
        {
            return SharedShape(live);
        }
        let old = t.by_key.insert(
            key,
            Entry {
                shape: Arc::downgrade(&shape.0),
                spec,
            },
        );
        if let Some(old) = old {
            t.by_address.remove(&old.shape.as_ptr().cast::<()>().addr());
        }
        t.by_address.insert(address(&shape), key);
        if t.by_key.len() >= t.sweep_at {
            t.by_key.retain(|_, e| e.shape.strong_count() > 0);
            let Table {
                by_key, by_address, ..
            } = t;
            by_address.retain(|_, k| by_key.contains_key(k));
            t.sweep_at = (2 * t.by_key.len()).max(64);
        }
        shape
    })
}

/// The shape of a fixed body whose components have the spec hash `spec` and the key `key`
/// ([`spec_and_key`]): the one registered under `key` while a world holds it, or else built from
/// `collider` and registered.
pub fn fixed(
    entity: EntityId,
    collider: &Collider,
    spec: u64,
    key: ShapeKey,
) -> Result<SharedShape, Problem> {
    if let Some((shape, _)) = recall(&key) {
        return Ok(shape);
    }
    Ok(register(key, spec, shared_shape(entity, &collider.shape)?))
}

/// The key of a volume chunk's shape: the BLAKE3 digest of the cell edge's `f64` bits and the
/// chunk's solid mask (`crate::volumes`).
pub fn chunk_key(cell_m: f64, mask: &[u8]) -> ShapeKey {
    let mut h = ContentHash::hasher(CHUNK_CONTEXT);
    h.update(&cell_m.to_bits().to_le_bytes());
    h.update(mask);
    h.finish().0
}

/// The shape of a volume chunk whose key is `key`: the one registered there while a world holds it,
/// or else `build`'s, registered.
pub fn chunk(key: ShapeKey, build: impl FnOnce() -> SharedShape) -> SharedShape {
    if let Some((shape, _)) = recall(&key) {
        return shape;
    }
    register(key, 0, build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::spec_hash;

    /// The streamed spec hash is the one `physics.step` computes from the gathered bytes.
    #[test]
    fn the_streamed_spec_hash_is_the_steps() {
        let terrain = crate::probe::level::terrain(30);
        let fixed = RigidBody::fixed();
        for body in [None, Some(&fixed)] {
            let (spec, key) = spec_and_key(body, &terrain);
            assert_eq!(spec, spec_hash(body, &terrain));
            assert_ne!(key, [0; 32]);
        }
        assert_ne!(
            spec_and_key(None, &terrain).1,
            spec_and_key(Some(&fixed), &terrain).1
        );
    }

    /// A shape built for a key is shared while a world holds it and registered by its address; a
    /// shape built elsewhere, equal or not, is not registered.
    #[test]
    fn built_shapes_are_shared_while_held() {
        let id = EntityId::FIRST;
        let collider = crate::probe::level::terrain(7);
        let (spec, key) = spec_and_key(None, &collider);
        let a = fixed(id, &collider, spec, key).unwrap();
        let b = fixed(id, &collider, spec, key).unwrap();
        assert!(Arc::ptr_eq(&a.0, &b.0));
        assert_eq!(registered(&a), Some((key, spec)));
        let other = shared_shape(id, &collider.shape).unwrap();
        assert_eq!(registered(&other), None);
        drop((a, b));
        assert!(recall(&key).is_none());
        let c = fixed(id, &collider, spec, key).unwrap();
        assert_eq!(registered(&c), Some((key, spec)));
    }
}
