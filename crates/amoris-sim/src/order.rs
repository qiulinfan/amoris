//! Iteration in `EntityId` order (docs/spec/simulation.md 8): every loop whose result reaches the
//! world, the events, the RNG draws, the allocation order or the step's report visits entities in
//! ascending id order, never in the ECS's storage order, which differs between a world and its
//! restored copy. `Query::iter` and `iter_mut` are disallowed in the tick-code crates (clippy);
//! these are the sorted forms, and `shuffle_storage` is the check that proves a loop needs them.

use bevy_ecs::prelude::{Query, World};
use bevy_ecs::query::{IterQueryData, QueryData, QueryFilter, QueryItem, ROQueryItem};

use crate::entity::{EntityId, EntityIndex};
use crate::rng::Pcg32;

/// The query's items in ascending `EntityId` order. The query must read `&EntityId` (the sort's
/// key, `QueryIter::sort_unstable::<&EntityId>()`); ids are unique, so the unstable sort yields
/// the one total order.
#[allow(clippy::disallowed_methods)] // the sorted form the lint points to
pub fn by_id<'a, 's, D: QueryData, F: QueryFilter>(
    query: &'a Query<'_, 's, D, F>,
) -> impl Iterator<Item = ROQueryItem<'a, 's, D>> + 'a
where
    D::ReadOnly: IterQueryData,
{
    query.iter().sort_unstable::<&EntityId>()
}

/// The query's items, mutably, in ascending `EntityId` order (see [`by_id`]).
#[allow(clippy::disallowed_methods)] // the sorted form the lint points to
pub fn by_id_mut<'a, 's, D: IterQueryData, F: QueryFilter>(
    query: &'a mut Query<'_, 's, D, F>,
) -> impl Iterator<Item = QueryItem<'a, 's, D>> + 'a {
    query.iter_mut().sort_unstable::<&EntityId>()
}

/// Permutes the world's storage (the storage-shuffle check, simulation.md 12, item 2): every
/// simulation entity is cloned into a new entity and the original despawned, in an order drawn from
/// `seed` by a generator outside the simulation, keeping each `EntityId` and component; then the
/// index is rebuilt. Two runs, one shuffled before every tick, must give the same hash chain.
pub fn shuffle_storage(world: &mut World, seed: u64) {
    let mut order: Vec<_> = world.resource::<EntityIndex>().iter().collect();
    let mut g = Pcg32::new(seed, 0x5348_5546_4c45);
    g.shuffle(&mut order);
    // Every clone first, in the drawn order, then the originals: removing an original right after
    // cloning it would swap the clone back into the original's row.
    for (_, entity) in &order {
        world.entity_mut(*entity).clone_and_spawn();
    }
    g.shuffle(&mut order);
    for (_, entity) in order {
        world.despawn(entity);
    }
    EntityIndex::rebuild(world);
}
