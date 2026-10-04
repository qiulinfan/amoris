//! Spawn and despawn (docs/spec/simulation.md 7.3). Every spawn allocates its id from the world's
//! allocator and records it in the index, so ids follow allocation order and the index stays
//! sorted; nothing else may spawn or despawn simulation entities.
//!
//! Two forms: functions over `&mut World` that apply at once (boundary writes, a script host
//! applying an invocation's staged commands at commit, exclusive systems), and [`SimCommands`], a
//! system parameter whose spawns and despawns are deferred through `bevy_ecs`'s `Commands` and
//! applied when the system returns (simulation.md 4.5). A script's spawn, whose id is known before
//! its entity exists, is [`reserve`] then [`spawn_allocated`] (simulation-slice1.md, 11).

use bevy_ecs::prelude::{Bundle, Commands, Entity, ResMut, Resource, World};
use bevy_ecs::system::SystemParam;
use pocket_contract::Problem;

use super::{EntityAllocator, EntityId, EntityIndex, entity_id_invalid, entity_not_found};

fn parts(world: &mut World) -> (EntityAllocator, &mut EntityIndex) {
    let alloc = *world.resource::<EntityAllocator>();
    (alloc, world.resource_mut::<EntityIndex>().into_inner())
}

/// Ids reserved by [`reserve`] and not spawned yet, ascending. Derived: a reservation lasts until
/// the invocation that made it commits or rolls back, or the tick ends (`sim.finish`); one made at
/// a boundary lasts until the next tick begins. An id that leaves the set unspawned stays
/// allocated and is never handed out again.
#[derive(Resource, Debug, Default)]
pub struct ReservedIds(Vec<EntityId>);

impl ReservedIds {
    /// Whether `id` is reserved and not spawned yet.
    pub fn contains(&self, id: EntityId) -> bool {
        self.0.binary_search(&id).is_ok()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    fn take(&mut self, id: EntityId) -> bool {
        match self.0.binary_search(&id) {
            Ok(i) => {
                self.0.remove(i);
                true
            }
            Err(_) => false,
        }
    }
}

/// Allocates an id and spawns the entity with its components, at once.
pub fn spawn(world: &mut World, bundle: impl Bundle) -> Result<EntityId, Problem> {
    let id = world.resource_mut::<EntityAllocator>().allocate()?;
    let entity = world.spawn((id, bundle)).id();
    world.resource_mut::<EntityIndex>().insert(id, entity);
    Ok(id)
}

/// Allocates an id for a spawn applied later through [`spawn_allocated`]: a script's `spawn`
/// returns its id at call time and the host applies the staged spawn before it commits the
/// invocation (script-host.md 5.4). `sim.entity_ids_exhausted` past the bound.
pub fn reserve(world: &mut World) -> Result<EntityId, Problem> {
    let id = world.resource_mut::<EntityAllocator>().allocate()?;
    // Ids arrive ascending, so a push keeps the set sorted.
    world.resource_mut::<ReservedIds>().0.push(id);
    Ok(id)
}

/// Spawns an entity under an id [`reserve`] handed out, before the reservation ends (see
/// [`ReservedIds`]). Any other id (never allocated, spawned already, or committed without a spawn)
/// is `sim.entity_id_invalid`: ids are never reused once committed (simulation.md 7.2).
pub fn spawn_allocated(
    world: &mut World,
    id: EntityId,
    bundle: impl Bundle,
) -> Result<Entity, Problem> {
    if !world.resource_mut::<ReservedIds>().take(id) {
        let next = world.resource::<EntityAllocator>().next();
        return Err(entity_id_invalid(&id.get().to_string(), next));
    }
    let entity = world.spawn((id, bundle)).id();
    world.resource_mut::<EntityIndex>().insert(id, entity);
    Ok(entity)
}

/// Despawns the entity holding `id` with all its components and returns `true`; for an id whose
/// entity is already despawned, does nothing and returns `false` (two rules removing the same
/// crate in one tick are not an error). An id never allocated is `sim.entity_id_invalid`.
/// Despawning never despawns other entities.
pub fn despawn(world: &mut World, id: EntityId) -> Result<bool, Problem> {
    let (alloc, index) = parts(world);
    if !alloc.allocated(id) {
        return Err(entity_id_invalid(&id.get().to_string(), alloc.next()));
    }
    match index.remove(id) {
        Some(entity) => {
            world.despawn(entity);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// The live entity holding `id`.
pub fn entity(world: &World, id: EntityId) -> Option<Entity> {
    world.resource::<EntityIndex>().get(id)
}

/// The live entity holding `id`, or `sim.entity_not_found` (despawned) or `sim.entity_id_invalid`
/// (never allocated).
pub fn require(world: &World, id: EntityId) -> Result<Entity, Problem> {
    if let Some(e) = entity(world, id) {
        return Ok(e);
    }
    let alloc = world.resource::<EntityAllocator>();
    if alloc.allocated(id) {
        Err(entity_not_found(id))
    } else {
        Err(entity_id_invalid(&id.get().to_string(), alloc.next()))
    }
}

/// Spawn and despawn for engine systems: ids are allocated at the call, the structural changes
/// apply when the system returns, and the entity becomes visible to queries from the next system.
#[derive(SystemParam)]
pub struct SimCommands<'w, 's> {
    commands: Commands<'w, 's>,
    alloc: ResMut<'w, EntityAllocator>,
    index: ResMut<'w, EntityIndex>,
}

impl<'w, 's> SimCommands<'w, 's> {
    /// Allocates an id and stages the entity with its components.
    pub fn spawn(&mut self, bundle: impl Bundle) -> Result<EntityId, Problem> {
        let id = self.alloc.allocate()?;
        let entity = self.commands.spawn((id, bundle)).id();
        self.index.insert(id, entity);
        Ok(id)
    }

    /// Stages the despawn of the entity holding `id` (see [`despawn`]).
    pub fn despawn(&mut self, id: EntityId) -> Result<bool, Problem> {
        if !self.alloc.allocated(id) {
            return Err(entity_id_invalid(&id.get().to_string(), self.alloc.next()));
        }
        match self.index.remove(id) {
            Some(entity) => {
                self.commands.entity(entity).despawn();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// The entity holding `id`, live or staged in this system.
    pub fn entity(&self, id: EntityId) -> Option<Entity> {
        self.index.get(id)
    }

    /// The underlying commands, for inserting and removing components.
    pub fn commands(&mut self) -> &mut Commands<'w, 's> {
        &mut self.commands
    }
}
