//! Entities (docs/spec/simulation.md 7, 8 and 12): id allocation and despawn (`sim.entity_ids`),
//! iteration in id order, the storage shuffle (`sim.shuffle`) with its negative control, and the
//! component registry.

mod common;

use bevy_ecs::prelude::*;
use common::{Seen, Value, id, seen, sim, state_hash};
use pocket_sim::order::{by_id, by_id_mut, shuffle_storage};
use pocket_sim::registry::{ComponentOrigin, ComponentSchema, FieldType, FieldValue};
use pocket_sim::{
    ComponentRegistry, EntityAllocator, EntityId, Name, NoHooks, Persisted, RngTable, RunCondition,
    SimCommands, TickPhase, entity,
};

#[test]
fn ids_start_at_one_increase_and_are_never_reused() {
    let mut s = sim(1);
    let mut b = s.boundary();
    let ids: Vec<u64> = (0..3).map(|_| b.spawn(()).unwrap().get()).collect();
    assert_eq!(ids, [1, 2, 3]);
    assert_eq!(b.despawn(id(2)), Ok(true));
    assert_eq!(b.despawn(id(2)), Ok(false), "a second despawn is a no-op");
    let e = b.despawn(id(9)).unwrap_err();
    assert_eq!(e.code, "sim.entity_id_invalid");
    assert_eq!(e.detail["next"], serde_json::json!(4));
    assert_eq!(
        b.spawn(()).unwrap().get(),
        4,
        "a despawned id is not reused"
    );
    s.step(&mut NoHooks).unwrap();
    // After the tick compacted the index, the despawned id still answers false.
    let mut b = s.boundary();
    assert_eq!(b.despawn(id(2)), Ok(false));
    assert!(b.entity(id(2)).is_none());
    assert!(b.entity(id(3)).is_some());
    assert_eq!(
        entity::require(s.world(), id(2)).unwrap_err().code,
        "sim.entity_not_found"
    );
    assert_eq!(
        entity::require(s.world(), id(5)).unwrap_err().code,
        "sim.entity_id_invalid"
    );
    let ids: Vec<u64> = s
        .world()
        .resource::<pocket_sim::EntityIndex>()
        .ids()
        .map(EntityId::get)
        .collect();
    assert_eq!(ids, [1, 3, 4]);
}

#[test]
fn spawns_from_systems_land_when_the_system_returns() {
    let mut s = sim(1);
    s.add_system(
        "test.spawner",
        TickPhase::Update,
        RunCondition::Always,
        |mut cmd: SimCommands, q: Query<&EntityId>, mut seen: ResMut<Seen>| {
            let new = cmd.spawn(Value(1.0)).unwrap();
            // Visible to queries from the next system on, but its id is known at once.
            seen.0
                .push(format!("spawned {} while {} existed", new.get(), q.count()));
        },
    )
    .unwrap();
    s.add_system(
        "test.counter",
        TickPhase::Forces,
        RunCondition::Always,
        |q: Query<&EntityId>, mut seen: ResMut<Seen>| {
            seen.0.push(format!("then {}", q.count()));
        },
    )
    .unwrap();
    s.step(&mut NoHooks).unwrap();
    s.step(&mut NoHooks).unwrap();
    assert_eq!(
        seen(&s),
        [
            "spawned 1 while 0 existed",
            "then 1",
            "spawned 2 while 1 existed",
            "then 2"
        ]
    );
    // Despawn through the commands, twice in one system: the second is a no-op.
    s.add_system(
        "test.despawner",
        TickPhase::Physics,
        RunCondition::Always,
        |mut cmd: SimCommands, mut seen: ResMut<Seen>| {
            let a = cmd.despawn(EntityId::new(1).unwrap()).unwrap();
            let b = cmd.despawn(EntityId::new(1).unwrap()).unwrap();
            seen.0.push(format!("despawn {a} {b}"));
        },
    )
    .unwrap();
    s.step(&mut NoHooks).unwrap();
    assert!(seen(&s).contains(&"despawn true false".to_owned()));
    assert!(entity::entity(s.world(), id(1)).is_none());
}

#[test]
fn allocation_follows_the_allocator_resource() {
    let mut s = sim(1);
    s.world_mut()
        .insert_resource(EntityAllocator::starting_at(100).unwrap());
    assert_eq!(s.boundary().spawn(()).unwrap().get(), 100);
    let e = entity::spawn_allocated(s.world_mut(), id(100), ()).unwrap_err();
    assert_eq!(e.code, "sim.entity_id_invalid", "already spawned");
    let reserved = entity::reserve(s.world_mut()).unwrap();
    assert_eq!(reserved.get(), 101);
    entity::spawn_allocated(s.world_mut(), reserved, ()).unwrap();
    assert!(
        entity::spawn_allocated(s.world_mut(), reserved, ()).is_err(),
        "spawned already"
    );
    assert!(
        entity::spawn_allocated(s.world_mut(), id(500), ()).is_err(),
        "never allocated"
    );
}

/// Only a reserved, unspawned id can be spawned later, and only until its reservation ends: a
/// committed id is never reused (simulation.md 7.2), even after `sim.finish` compacted the index.
#[test]
fn spawn_allocated_never_brings_back_a_committed_id() {
    let mut s = sim(1);
    let one = s.boundary().spawn(()).unwrap();
    assert_eq!(s.boundary().despawn(one), Ok(true));
    let e = entity::spawn_allocated(s.world_mut(), one, ()).unwrap_err();
    assert_eq!(
        e.code, "sim.entity_id_invalid",
        "despawned, index not compacted"
    );
    s.step(&mut NoHooks).unwrap();
    let e = entity::spawn_allocated(s.world_mut(), one, ()).unwrap_err();
    assert_eq!(e.code, "sim.entity_id_invalid", "despawned and compacted");
    assert!(entity::entity(s.world(), one).is_none());
    // A reservation made at a boundary ends when the next tick begins.
    let late = entity::reserve(s.world_mut()).unwrap();
    s.step(&mut NoHooks).unwrap();
    assert!(entity::spawn_allocated(s.world_mut(), late, ()).is_err());
    // Inside a tick: an invocation's reservation ends at its commit or rollback.
    s.add_exclusive(
        "test.reserver",
        TickPhase::Update,
        RunCondition::Always,
        |world: &mut World, ctx: &mut pocket_sim::SystemCtx<'_>| {
            let inv = pocket_sim::sim::begin_invocation(world, ctx.key());
            let kept = entity::reserve(world).unwrap();
            let dropped = entity::reserve(world).unwrap();
            entity::spawn_allocated(world, kept, Value(1.0)).unwrap();
            pocket_sim::sim::commit_invocation(world, inv);
            let refused = entity::spawn_allocated(world, dropped, ()).unwrap_err();
            let mut seen = world.resource_mut::<Seen>();
            seen.0.push(format!("{} {}", kept.get(), refused.code));
            let inv = pocket_sim::sim::begin_invocation(world, ctx.key());
            let undone = entity::reserve(world).unwrap();
            pocket_sim::sim::rollback_invocation(world, inv);
            let refused = entity::spawn_allocated(world, undone, ()).unwrap_err();
            world.resource_mut::<Seen>().0.push(refused.code);
        },
    )
    .unwrap();
    s.step(&mut NoHooks).unwrap();
    assert_eq!(
        seen(&s),
        ["3 sim.entity_id_invalid", "sim.entity_id_invalid"]
    );
    // The rolled-back id returned to the allocator; the dropped one (4) stays used.
    assert_eq!(s.world().resource::<EntityAllocator>().next(), 5);
}

/// A world of `n` entities with values whose updates depend on the visiting order; spawns and
/// despawns every few ticks fragment the storage.
fn order_world(storage_order_sum: bool) -> pocket_sim::Sim {
    let mut s = sim(3);
    let mut b = s.boundary();
    for i in 0..40 {
        b.spawn(Value(1.0 + f64::from(i) * 1e-3)).unwrap();
    }
    // Order-dependent, in id order: each value takes a share of the running sum before it.
    s.add_system(
        "test.mix",
        TickPhase::Update,
        RunCondition::Always,
        |mut q: Query<(&EntityId, &mut Value)>| {
            let mut acc = 0.0;
            for (_, mut v) in by_id_mut(&mut q) {
                acc = acc * 0.5 + v.0;
                v.0 = v.0 * 0.999 + acc * 1e-3;
            }
        },
    )
    .unwrap();
    s.add_system(
        "test.churn",
        TickPhase::Forces,
        RunCondition::every(3, 0).unwrap(),
        |mut cmd: SimCommands, q: Query<(&EntityId, &Value)>, mut rng: ResMut<RngTable>| {
            let ids: Vec<EntityId> = by_id(&q).map(|(id, _)| *id).collect();
            let r = rng.system("test.churn").unwrap();
            let victim = ids[r.pick(ids.len()).unwrap()];
            cmd.despawn(victim).unwrap();
            cmd.spawn(Value(r.range(0.5, 2.0).unwrap())).unwrap();
        },
    )
    .unwrap();
    if storage_order_sum {
        // The negative control: a float summed over the query in storage order (the lint's
        // `IntoIterator` gap), folded into the world.
        s.add_system(
            "test.bad_sum",
            TickPhase::Physics,
            RunCondition::Always,
            |mut q: Query<&mut Value>| {
                let mut acc = 0.0;
                for mut v in &mut q {
                    acc = acc * 0.5 + v.0;
                    v.0 += acc * 1e-12;
                }
            },
        )
        .unwrap();
    }
    s
}

fn chain(mut s: pocket_sim::Sim, shuffle: bool, ticks: u64) -> Vec<u64> {
    (0..ticks)
        .map(|t| {
            if shuffle {
                shuffle_storage(s.world_mut(), 1000 + t);
            }
            s.step(&mut NoHooks).unwrap();
            state_hash(&s)
        })
        .collect()
}

#[test]
fn storage_shuffle_gives_the_same_hash_chain() {
    let plain = chain(order_world(false), false, 30);
    let shuffled = chain(order_world(false), true, 30);
    assert_eq!(plain, shuffled);
    // The shuffle really permutes storage: entity ids survive, bevy entities do not.
    let mut s = order_world(false);
    let before: Vec<(EntityId, Entity)> = s
        .world()
        .resource::<pocket_sim::EntityIndex>()
        .iter()
        .collect();
    shuffle_storage(s.world_mut(), 5);
    let after: Vec<(EntityId, Entity)> = s
        .world()
        .resource::<pocket_sim::EntityIndex>()
        .iter()
        .collect();
    assert_eq!(before.len(), after.len());
    assert!(
        before
            .iter()
            .zip(&after)
            .all(|(a, b)| a.0 == b.0 && a.1 != b.1)
    );
}

#[test]
fn storage_order_iteration_diverges_under_the_shuffle() {
    let plain = chain(order_world(true), false, 30);
    let shuffled = chain(order_world(true), true, 30);
    assert_ne!(plain, shuffled, "the negative control must diverge");
}

#[test]
fn the_registry_names_versions_and_schemas() {
    let mut s = sim(1);
    let reg = s.world().resource::<ComponentRegistry>();
    let name = reg.get("Name").expect("Name is registered");
    assert_eq!(name.version, Name::VERSION);
    assert_eq!(name.origin, ComponentOrigin::Engine);
    assert_eq!(name.json_schema["type"], serde_json::json!("string"));
    assert!(name.doc.contains("plain-data name"), "{}", name.doc);
    assert_eq!(reg.suggest("Nmae"), ["Name"]);
    let e = ComponentRegistry::register::<Name>(s.world_mut(), None).unwrap_err();
    assert_eq!(e.code, "sim.component_duplicate");
    // A project component: fields in order, slots assigned, a JSON Schema of an object.
    let schema = ComponentSchema::new(
        "Tally",
        ComponentOrigin::Project,
        2,
        "Crates taken.",
        vec![
            ("taken", FieldType::U32, FieldValue::U32(0), "How many."),
            (
                "at",
                FieldType::Vec3,
                FieldValue::Vec3([0.0; 3]),
                "Where the last was.",
            ),
            (
                "by",
                FieldType::Entity,
                FieldValue::Entity(None),
                "Who took it.",
            ),
            (
                "label",
                FieldType::Str,
                FieldValue::Str("".into()),
                "A label.",
            ),
        ],
    )
    .unwrap();
    assert_eq!(schema.slots(), 5);
    assert_eq!(schema.fields[2].first_slot, 4);
    let cid = s.world_mut().register_component::<Value>();
    ComponentRegistry::register_project(s.world_mut(), schema, cid).unwrap();
    let reg = s.world().resource::<ComponentRegistry>();
    let names: Vec<&str> = reg.entries().iter().map(|e| &*e.name).collect();
    assert_eq!(names, ["Name", "Tally"]);
    let tally = reg.get("Tally").unwrap();
    assert_eq!(
        tally.json_schema["properties"]["at"]["required"],
        serde_json::json!(["x", "y", "z"])
    );
    assert_eq!(reg.by_id(cid).map(|e| &*e.name), Some("Tally"));
    assert_eq!(reg.info()[1].version, 2);
    // Names and docs are checked.
    let bad = ComponentSchema::new("tally", ComponentOrigin::Project, 1, "", vec![]);
    assert_eq!(bad.unwrap_err().code, "sim.component_name_invalid");
    let undocumented = ComponentSchema::new(
        "Tally",
        ComponentOrigin::Project,
        1,
        "",
        vec![("taken", FieldType::U32, FieldValue::U32(0), " ")],
    );
    assert_eq!(
        undocumented.unwrap_err().code,
        "sim.component_field_invalid"
    );
}

/// A project declaration is checked whole before anything registers: defaults must fit their type
/// (finite numbers, a tick within 2^53 - 1, an enum index below the variant count), enum variants
/// must be present and distinct, and the slots must fit in a u16.
#[test]
fn project_declarations_are_validated() {
    use std::sync::Arc;
    let refuse = |fields: Vec<(&str, FieldType, FieldValue, &str)>| {
        let e = ComponentSchema::new("Probe", ComponentOrigin::Project, 1, "A probe.", fields)
            .unwrap_err();
        assert_eq!(e.code, "sim.component_field_invalid", "{}", e.message);
        assert_eq!(e.detail["component"], serde_json::json!("Probe"));
        e
    };
    let one = |ty: FieldType, default: FieldValue| vec![("f", ty, default, "A field.")];
    let modes = || FieldType::Enum(Arc::from(vec![Box::<str>::from("calm")]));
    for (ty, default) in [
        (FieldType::F64, FieldValue::F64(f64::NAN)),
        (FieldType::F64, FieldValue::F64(f64::NEG_INFINITY)),
        (FieldType::Vec3, FieldValue::Vec3([0.0, f64::INFINITY, 0.0])),
        (FieldType::Quat, FieldValue::Quat([0.0, 0.0, 0.0, f64::NAN])),
        (FieldType::U32, FieldValue::Str("3".into())),
        (FieldType::I32, FieldValue::U32(3)),
        (FieldType::Vec4, FieldValue::Quat([0.0; 4])),
        (FieldType::Tick, FieldValue::Tick(pocket_sim::Tick(1 << 53))),
        (modes(), FieldValue::Enum(1)),
        (modes(), FieldValue::Enum(u32::MAX)),
        (
            FieldType::Enum(Arc::from(Vec::<Box<str>>::new())),
            FieldValue::Enum(0),
        ),
        (
            FieldType::Enum(Arc::from(vec![Box::<str>::from("a"), "a".into()])),
            FieldValue::Enum(0),
        ),
        (
            FieldType::Enum(Arc::from(vec![Box::<str>::from("")])),
            FieldValue::Enum(0),
        ),
    ] {
        let e = refuse(one(ty, default));
        assert_eq!(e.detail["field"], serde_json::json!("f"));
    }
    // What fits is accepted, -0 and subnormals included (numeric.md 7).
    let ok = ComponentSchema::new(
        "Probe",
        ComponentOrigin::Project,
        1,
        "A probe.",
        vec![
            ("a", FieldType::F64, FieldValue::F64(-0.0), "A."),
            ("b", modes(), FieldValue::Enum(0), "B."),
            (
                "c",
                FieldType::Tick,
                FieldValue::Tick(pocket_sim::Tick(pocket_sim::MAX_ENTITY_ID)),
                "C.",
            ),
            (
                "d",
                FieldType::F64,
                FieldValue::F64(f64::from_bits(1)),
                "D.",
            ),
        ],
    )
    .unwrap();
    assert_eq!(ok.slots(), 4);
    // 16,383 Vec4 fields fill 65,532 slots; one more takes it past 65,535.
    let names: Vec<String> = (0..16_400).map(|i| format!("v{i}")).collect();
    let many = |n: usize| {
        names[..n]
            .iter()
            .map(|f| {
                (
                    f.as_str(),
                    FieldType::Vec4,
                    FieldValue::Vec4([0.0; 4]),
                    "V.",
                )
            })
            .collect::<Vec<_>>()
    };
    let full = ComponentSchema::new(
        "Probe",
        ComponentOrigin::Project,
        1,
        "A probe.",
        many(16_383),
    );
    assert_eq!(full.unwrap().slots(), 65_532);
    let e = refuse(many(16_400));
    assert_eq!(e.detail["field"], serde_json::json!("v16383"));
}
