//! Invocations as transactions (docs/spec/simulation.md 4.5): an invocation's effects (component
//! writes, structural changes, events, RNG draws, entity ids) take effect when it returns normally,
//! together; when it fails, none of them do. The script host stages its writes and applies them
//! at commit (script-host.md 5.4); what it cannot stage, the ids `spawn` reserves at call time and
//! the RNG draws, rolls back to the marks taken here, and so do events, decisions and entities
//! applied before a failure.

use bevy_ecs::prelude::World;

use super::state::{TickOutput, TickState};
use crate::entity::{AllocMark, EntityAllocator, EntityIndex, ReservedIds};
use crate::event::{EventCounter, EventOutbox};
use crate::rng::{RngMark, RngTable};
use crate::schedule::SystemKey;

/// The marks of an open invocation.
#[derive(Debug)]
#[must_use]
pub struct Invocation {
    alloc: AllocMark,
    rng: RngMark,
    outbox_len: usize,
    next_event: u64,
    decisions: usize,
}

/// Opens an invocation of `system`: marks the allocator, the RNG table, the outbox, the event
/// counter and the decisions. Invocations do not nest; opening a second one is an engine bug.
pub fn begin_invocation(world: &mut World, system: &SystemKey) -> Invocation {
    let mut state = world.resource_mut::<TickState>();
    assert!(
        state.invocation.is_none(),
        "an invocation is already open when {system} begins"
    );
    state.invocation = Some(system.clone());
    Invocation {
        alloc: world.resource::<EntityAllocator>().mark(),
        rng: world.resource_mut::<RngTable>().mark(),
        outbox_len: world.resource::<EventOutbox>().len(),
        next_event: world.resource::<EventCounter>().next(),
        decisions: world.resource::<TickOutput>().decisions.len(),
    }
}

/// Keeps everything the invocation did. Its reserved ids not spawned by now stay allocated and are
/// never spawned (`entity::spawn_allocated` refuses them from here on).
pub fn commit_invocation(world: &mut World, inv: Invocation) {
    world.resource_mut::<RngTable>().commit(inv.rng);
    world.resource_mut::<ReservedIds>().clear();
    world.resource_mut::<TickState>().invocation = None;
}

/// Undoes the invocation: its ids return to the allocator, its draws are as if they never
/// happened, its events and decisions are dropped and any entity it spawned is removed. The world
/// is as if the system had not run; the caller reports the failure (`system_failed`).
pub fn rollback_invocation(world: &mut World, inv: Invocation) {
    let spawned = world
        .resource_mut::<EntityIndex>()
        .forget_from(inv.alloc.next());
    for e in spawned {
        world.despawn(e);
    }
    world.resource_mut::<EntityAllocator>().rollback(inv.alloc);
    world.resource_mut::<ReservedIds>().clear();
    world.resource_mut::<RngTable>().rollback(inv.rng);
    world.resource_mut::<EventOutbox>().truncate(inv.outbox_len);
    world
        .resource_mut::<EventCounter>()
        .reset_to(inv.next_event);
    world
        .resource_mut::<TickOutput>()
        .decisions
        .truncate(inv.decisions);
    world.resource_mut::<TickState>().invocation = None;
}
