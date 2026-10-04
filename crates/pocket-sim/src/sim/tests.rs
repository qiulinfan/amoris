//! Unit tests of the step that need the crate's internals (docs/spec/simulation.md 12, items 4 and
//! 6; numeric.md 10, item 4): broken boundary invariants and a changed floating-point environment
//! must stop the tick and poison the world, in release builds too.

use bevy_ecs::prelude::World;
use bevy_ecs::system::SystemState;

use super::*;
use crate::entity::SimCommands;
use crate::event::{EventKind, NewEvent};
use crate::schedule::NoHooks;

fn sim() -> Sim {
    Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 7,
    })
    .unwrap()
}

#[test]
fn an_outbox_event_left_at_a_boundary_trips_the_invariants() {
    let mut s = sim();
    s.step(&mut NoHooks).unwrap();
    let kind = EventKind::new("test.leak").unwrap();
    let w = s.world_mut();
    let seq = w.resource_mut::<EventCounter>().next();
    w.resource_mut::<EventOutbox>().push(Event {
        seq: crate::event::EventSeq(seq),
        tick: Tick(1),
        kind,
        subject: None,
        cause: None,
        data: crate::data::PlainData::Null,
    });
    let e = s.step(&mut NoHooks).unwrap_err();
    assert_eq!(e.code, "sim.internal");
    assert!(e.message.contains("outbox"), "{}", e.message);
    assert_eq!(s.step(&mut NoHooks).unwrap_err().code, "sim.world_poisoned");
}

#[test]
fn a_staged_spawn_left_behind_trips_the_invariants() {
    let mut s = sim();
    s.add_exclusive(
        "test.leaky_spawn",
        TickPhase::Update,
        RunCondition::Always,
        |world: &mut World, _ctx: &mut SystemCtx<'_>| {
            // Commands that are never applied: the id and the index entry exist, the entity does not.
            let mut state = SystemState::<SimCommands>::new(world);
            let mut commands = state.get_mut(world).unwrap();
            commands.spawn(()).unwrap();
        },
    )
    .unwrap();
    let e = s.step(&mut NoHooks).unwrap_err();
    assert_eq!(e.code, "sim.internal");
    assert_eq!(e.detail["system"], serde_json::json!("sim.finish"));
    assert!(
        e.message.contains("spawned but is not in the world"),
        "{}",
        e.message
    );
    assert!(s.poisoned().is_some());
}

/// The walk of the feature `invariants` (on in the tests) finds an entity removed behind the
/// index's back, which the constant-time checks cannot see.
#[test]
fn the_index_walk_finds_a_despawn_that_bypassed_the_index() {
    let mut s = sim();
    let id = s.boundary().spawn(()).unwrap();
    s.step(&mut NoHooks).unwrap();
    let e = crate::entity::entity(s.world(), id).unwrap();
    s.world_mut().despawn(e);
    let err = s.step(&mut NoHooks).unwrap_err();
    assert_eq!(err.code, "sim.internal");
    assert!(
        err.message
            .contains("the index holds 1 live entities, the world 0"),
        "{}",
        err.message
    );
}

#[test]
fn an_unclosed_invocation_trips_the_invariants() {
    let mut s = sim();
    s.add_exclusive(
        "test.open",
        TickPhase::Update,
        RunCondition::Always,
        |world: &mut World, ctx: &mut SystemCtx<'_>| {
            let inv = begin_invocation(world, ctx.key());
            let _never_closed = inv;
        },
    )
    .unwrap();
    let e = s.step(&mut NoHooks).unwrap_err();
    assert!(e.message.contains("never committed"), "{}", e.message);
}

#[test]
fn a_panic_poisons_until_the_derived_state_is_rebuilt() {
    let mut s = sim();
    s.add_exclusive(
        "test.boom",
        TickPhase::Forces,
        RunCondition::Start,
        |_: &mut World, _: &mut SystemCtx<'_>| {
            panic!("boom");
        },
    )
    .unwrap();
    let e = s.step(&mut NoHooks).unwrap_err();
    assert_eq!(e.code, "sim.internal");
    assert_eq!(e.detail["system"], serde_json::json!("test.boom"));
    assert_eq!(e.detail["phase"], serde_json::json!("Forces"));
    assert_eq!(e.detail["message"], serde_json::json!("boom"));
    let p = s.step(&mut NoHooks).unwrap_err();
    assert_eq!(p.code, "sim.world_poisoned");
    assert_eq!(p.detail["tick"], serde_json::json!(1));
    // What a restore does after applying the sections: the derived state comes back clean. (A
    // real restore also brings back the snapshot's clock; this world keeps tick 1, so the Start
    // system does not run again.)
    crate::persisted::rebuild_derived(s.world_mut());
    assert!(s.poisoned().is_none());
    assert_eq!(s.step(&mut NoHooks).unwrap().tick, Tick(2));
    s.step(&mut NoHooks).unwrap();
}

#[test]
fn a_fault_poisons_with_its_own_code() {
    let mut s = sim();
    s.add_exclusive(
        "script.update",
        TickPhase::Update,
        RunCondition::Always,
        |_: &mut World, ctx: &mut SystemCtx<'_>| {
            ctx.fault(Problem::new(
                "script.out_of_memory",
                "out of memory.",
                Default::default(),
            ));
        },
    )
    .unwrap();
    let e = s.step(&mut NoHooks).unwrap_err();
    assert_eq!(e.code, "script.out_of_memory");
    assert_eq!(s.step(&mut NoHooks).unwrap_err().code, "sim.world_poisoned");
}

/// A `Single` with nothing to match skips its system (bevy_ecs's `RunSystemError::Skipped`): the
/// system did not run that tick, and the world is not poisoned. A parameter that fails for real (a
/// missing resource) still poisons.
#[test]
fn a_system_its_parameters_skip_does_not_poison() {
    use bevy_ecs::prelude::{Component, ResMut, Resource, Single, With};
    #[derive(Component)]
    struct Boat;
    #[derive(Resource, Default)]
    struct Runs(u32);
    #[derive(Resource)]
    struct Missing;
    let mut s = sim();
    s.world_mut().init_resource::<Runs>();
    s.add_system(
        "physics.hull",
        TickPhase::Forces,
        RunCondition::Always,
        |_boat: Single<&crate::entity::EntityId, With<Boat>>, mut runs: ResMut<Runs>| {
            runs.0 += 1;
        },
    )
    .unwrap();
    s.step(&mut NoHooks).unwrap();
    assert!(s.poisoned().is_none());
    assert_eq!(s.world().resource::<Runs>().0, 0);
    s.boundary().spawn(Boat).unwrap();
    s.step(&mut NoHooks).unwrap();
    assert_eq!(s.world().resource::<Runs>().0, 1);
    s.add_system(
        "test.needs",
        TickPhase::Update,
        RunCondition::Always,
        |_: bevy_ecs::prelude::Res<Missing>| {},
    )
    .unwrap();
    let e = s.step(&mut NoHooks).unwrap_err();
    assert_eq!(e.code, "sim.internal");
    assert_eq!(e.detail["system"], serde_json::json!("test.needs"));
}

#[test]
fn boundary_events_need_no_tick() {
    let mut s = sim();
    let seq = s
        .boundary()
        .emit(NewEvent::new(EventKind::new("test.hello").unwrap()));
    assert_eq!(seq.0, 1);
    assert_eq!(
        s.world().resource::<EventInbox>().boundary_events().len(),
        1
    );
    s.step(&mut NoHooks).unwrap();
    assert!(s.world().resource::<EventInbox>().events().is_empty());
}

#[cfg(target_arch = "x86_64")]
#[test]
fn flush_to_zero_on_the_game_thread_trips_the_float_check() {
    let mut s = sim();
    s.step(&mut NoHooks).unwrap();
    let before = float_env::read();
    let before32 = u32::try_from(before).unwrap();
    float_env::write(before32 | 0x8040); // flush-to-zero and denormals-are-zero
    let r = s.step(&mut NoHooks);
    float_env::write(before32);
    let e = r.unwrap_err();
    assert_eq!(e.code, "number.float_env_changed");
    assert_eq!(e.detail["register"], serde_json::json!("MXCSR"));
    assert!(s.poisoned().is_some());
    assert!(float_env::check().is_ok());
}
