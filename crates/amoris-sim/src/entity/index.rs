//! The entity index (docs/spec/simulation.md 7.5): `(EntityId, Entity)` pairs ascending by id.
//! Derived: rebuilt in one sorted pass on restore and fork, never hashed or serialized. Committed
//! ids only increase, so a spawn is a push that keeps the vector sorted; a despawn marks its slot and
//! `sim.finish` compacts the vector once per tick. Lookup by id is a binary search.

use bevy_ecs::prelude::{Entity, Resource, World};

use super::EntityId;

#[derive(Clone, Copy, Debug)]
struct Slot {
    id: EntityId,
    entity: Entity,
    live: bool,
}

/// The index, a Derived resource.
#[derive(Resource, Debug, Default)]
pub struct EntityIndex {
    slots: Vec<Slot>,
    dead: usize,
    /// Spawns (true) and despawns (false) since the last compaction, which the boundary checks
    /// confirm happened in the world (no staged structural change, simulation.md 4.1).
    recent: Vec<(EntityId, Entity, bool)>,
}

impl EntityIndex {
    fn find(&self, id: EntityId) -> Result<usize, usize> {
        self.slots.binary_search_by(|s| s.id.cmp(&id))
    }

    /// The live entity holding `id`.
    pub fn get(&self, id: EntityId) -> Option<Entity> {
        match self.find(id) {
            Ok(i) if self.slots[i].live => Some(self.slots[i].entity),
            _ => None,
        }
    }

    /// Whether `id` is in the index at all (live, or despawned since the last compaction).
    pub fn knows(&self, id: EntityId) -> bool {
        self.find(id).is_ok()
    }

    /// The number of live entities.
    pub fn len(&self) -> usize {
        self.slots.len() - self.dead
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Live `(id, entity)` pairs in ascending id order: the iteration order of simulation.md 8.
    pub fn iter(&self) -> impl Iterator<Item = (EntityId, Entity)> + '_ {
        self.slots
            .iter()
            .filter(|s| s.live)
            .map(|s| (s.id, s.entity))
    }

    /// Live ids, ascending.
    pub fn ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.iter().map(|(id, _)| id)
    }

    /// Records a spawn. Ids arrive ascending (allocation order); one that does not is inserted in
    /// place, so the order holds whatever the caller does.
    pub(crate) fn insert(&mut self, id: EntityId, entity: Entity) {
        let slot = Slot {
            id,
            entity,
            live: true,
        };
        match self.slots.last() {
            Some(last) if last.id >= id => match self.find(id) {
                Ok(i) => {
                    if !self.slots[i].live {
                        self.dead -= 1;
                    }
                    self.slots[i] = slot;
                }
                Err(i) => self.slots.insert(i, slot),
            },
            _ => self.slots.push(slot),
        }
        self.recent.push((id, entity, true));
    }

    /// Marks `id` despawned; returns its entity if it was live.
    pub(crate) fn remove(&mut self, id: EntityId) -> Option<Entity> {
        let i = self.find(id).ok()?;
        let slot = &mut self.slots[i];
        if !slot.live {
            return None;
        }
        slot.live = false;
        self.dead += 1;
        self.recent.push((id, slot.entity, false));
        Some(slot.entity)
    }

    /// Erases every slot of an id at least `first` (spawns being undone: their ids return to the
    /// allocator) and returns the live entities among them, for the caller to despawn.
    pub(crate) fn forget_from(&mut self, first: u64) -> Vec<Entity> {
        let at = self.slots.partition_point(|s| s.id.get() < first);
        let gone: Vec<Slot> = self.slots.drain(at..).collect();
        self.dead -= gone.iter().filter(|s| !s.live).count();
        self.recent.retain(|r| r.0.get() < first);
        gone.into_iter()
            .filter(|s| s.live)
            .map(|s| s.entity)
            .collect()
    }

    /// Drops despawned slots and forgets the recent changes (`sim.finish`).
    pub(crate) fn compact(&mut self) {
        if self.dead > 0 {
            self.slots.retain(|s| s.live);
            self.dead = 0;
        }
        self.recent.clear();
    }

    /// Confirms that every spawn and despawn recorded since the last compaction happened in the
    /// world: each spawned entity exists and carries its id, each despawned one is gone. Costs the
    /// number of changes, not the number of entities.
    pub(crate) fn staged_changes(&self, world: &World) -> Option<String> {
        for &(id, entity, spawned) in &self.recent {
            let carried = world.get::<EntityId>(entity).copied();
            if spawned && carried != Some(id) && self.get(id) == Some(entity) {
                return Some(format!(
                    "entity {} was spawned but is not in the world",
                    id.get()
                ));
            }
            if !spawned && carried == Some(id) {
                return Some(format!(
                    "entity {} was despawned but is still in the world",
                    id.get()
                ));
            }
        }
        None
    }

    /// Rebuilds the index from the world's `EntityId` components in one sorted pass (restore, fork,
    /// and the storage-shuffle check).
    pub fn rebuild(world: &mut World) {
        let mut q = world.query::<(Entity, &EntityId)>();
        #[allow(clippy::disallowed_methods)] // collected, then sorted by id
        let mut slots: Vec<Slot> = q
            .iter(world)
            .map(|(entity, &id)| Slot {
                id,
                entity,
                live: true,
            })
            .collect();
        slots.sort_unstable_by_key(|s| s.id);
        world.insert_resource(EntityIndex {
            slots,
            dead: 0,
            recent: Vec::new(),
        });
    }

    /// The full walk of simulation.md 4.1 (feature `invariants`): the index holds exactly the
    /// world's entities with an `EntityId`, each under its own id, ascending and unique.
    pub fn verify(&self, world: &mut World) -> Result<(), String> {
        for w in self.slots.windows(2) {
            if w[0].id >= w[1].id {
                return Err(format!("index not ascending at {}", w[1].id.get()));
            }
        }
        let mut q = world.query::<(Entity, &EntityId)>();
        let mut count = 0usize;
        #[allow(clippy::disallowed_methods)] // a membership check; the order is irrelevant
        for (entity, &id) in q.iter(world) {
            count += 1;
            if self.get(id) != Some(entity) {
                return Err(format!(
                    "entity {} is not in the index under its id",
                    id.get()
                ));
            }
        }
        if count != self.len() {
            return Err(format!(
                "the index holds {} live entities, the world {count}",
                self.len()
            ));
        }
        Ok(())
    }
}
