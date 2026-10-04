//! Writes at a boundary (docs/spec/simulation.md 4.2): player actions, editor and developer
//! commands, hot updates and reseeds change the world only between ticks, one at a time, each
//! seeing the world as the writes before it left it. Boundary writes never draw random numbers;
//! their events go to the inbox, which the next tick reads.

use bevy_ecs::prelude::{Bundle, Entity, World};
use pocket_contract::Problem;

use crate::entity::{self, EntityAllocator, EntityId, EntityIndex, ReservedIds};
use crate::event::{self, EventCounter, EventInbox, EventSeq, NewEvent};
use crate::rng::WorldSeed;
use crate::time::{SimClock, Tick};

/// Mutable access to the world between ticks.
pub struct Boundary<'a> {
    world: &'a mut World,
}

/// Where a boundary write began, to undo what a refused write allocated, spawned and emitted.
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct BoundaryMark {
    next_id: u64,
    next_event: u64,
    inbox_len: usize,
}

impl<'a> Boundary<'a> {
    pub(crate) fn new(world: &'a mut World) -> Boundary<'a> {
        Boundary { world }
    }

    /// The tick the world shows (the boundary after it).
    pub fn tick(&self) -> Tick {
        self.world.resource::<SimClock>().tick
    }

    pub fn world(&self) -> &World {
        self.world
    }

    /// The world, for component writes. Spawns, despawns and events go through this type's
    /// methods, which keep ids, the index and the sequence numbers right.
    pub fn world_mut(&mut self) -> &mut World {
        self.world
    }

    /// Allocates an id and spawns an entity at once.
    pub fn spawn(&mut self, bundle: impl Bundle) -> Result<EntityId, Problem> {
        entity::spawn(self.world, bundle)
    }

    /// Despawns the entity holding `id`: `true`, or `false` if it already was despawned.
    pub fn despawn(&mut self, id: EntityId) -> Result<bool, Problem> {
        entity::despawn(self.world, id)
    }

    /// The live entity holding `id`.
    pub fn entity(&self, id: EntityId) -> Option<Entity> {
        entity::entity(self.world, id)
    }

    /// Appends an event to the inbox; the next tick reads it.
    pub fn emit(&mut self, event: NewEvent) -> EventSeq {
        event::emit(self.world, event)
    }

    /// Replaces the world seed from the next tick on (rng.md 5.4; `rng.seed_invalid` outside
    /// 0..=2^53 - 1).
    pub fn reseed(&mut self, seed: u64) -> Result<(), Problem> {
        let seed = WorldSeed::new(seed)?;
        self.world.insert_resource(seed);
        Ok(())
    }

    /// Marks the start of a write.
    pub fn mark(&self) -> BoundaryMark {
        BoundaryMark {
            next_id: self.world.resource::<EntityAllocator>().next(),
            next_event: self.world.resource::<EventCounter>().next(),
            inbox_len: self.world.resource::<EventInbox>().events().len(),
        }
    }

    /// Undoes a refused write's allocations, spawns and events: its ids return to the allocator
    /// (they were never visible outside it), its entities are removed, its events dropped. A
    /// write validates before it changes components or despawns, so nothing else needs undoing.
    pub fn rollback(&mut self, mark: BoundaryMark) {
        let spawned = self
            .world
            .resource_mut::<EntityIndex>()
            .forget_from(mark.next_id);
        for e in spawned {
            self.world.despawn(e);
        }
        if let Ok(alloc) = EntityAllocator::starting_at(mark.next_id) {
            self.world.insert_resource(alloc);
        }
        self.world.resource_mut::<ReservedIds>().clear();
        self.world
            .resource_mut::<EventInbox>()
            .truncate(mark.inbox_len);
        self.world
            .resource_mut::<EventCounter>()
            .reset_to(mark.next_event);
    }
}
