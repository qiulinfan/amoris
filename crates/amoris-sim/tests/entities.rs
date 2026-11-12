//! Entities (docs/spec/simulation.md 7, 8 and 12): id allocation and despawn (`sim.entity_ids`),
//! the hierarchy a despawn takes with it, iteration in id order, the storage shuffle
//! (`sim.shuffle`) with its negative control, and the component registry.

mod common;

use amoris_sim::order::{by_id, by_id_mut, shuffle_storage};
use amoris_sim::registry::{ComponentOrigin, ComponentSchema, FieldType, FieldValue};
use amoris_sim::{
    ComponentRegistry, EntityAllocator, EntityId, Hierarchy, Name, NoHooks, Parent, Persisted,
    RngTable, RunCondition, SimCommands, TickPhase, entity,
};
use bevy_ecs::prelude::*;
use common::{Seen, Value, id, seen, sim, state_hash};

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
        .resource::<amoris_sim::EntityIndex>()
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

/// The live ids, ascending.
fn live(s: &amoris_sim::Sim) -> Vec<u64> {
    let index = s.world().resource::<amoris_sim::EntityIndex>();
    index.ids().map(EntityId::get).collect()
}

fn under(n: u64) -> Parent {
    Parent { parent: id(n) }
}

/// 1 > (2 > (4, 5 > 7), 3), with 6 apart: ids as spawned.
fn family() -> amoris_sim::Sim {
    let mut s = sim(1);
    let mut b = s.boundary();
    b.spawn(Name::new("root").unwrap()).unwrap();
    b.spawn(under(1)).unwrap();
    b.spawn(under(1)).unwrap();
    b.spawn(under(2)).unwrap();
    b.spawn(under(2)).unwrap();
    b.spawn(()).unwrap();
    b.spawn(under(5)).unwrap();
    s
}

#[test]
fn despawning_a_parent_takes_everything_under_it() {
    let mut s = family();
    let next = s.world().resource::<EntityAllocator>().next();
    // A child goes with what is under it and leaves its parent and its sibling.
    let gone = s.boundary().despawn_tree(id(5)).unwrap();
    assert_eq!(gone, [id(5), id(7)]);
    assert_eq!(live(&s), [1, 2, 3, 4, 6]);
    // The parent: itself first, then each child before its own children, siblings ascending.
    let mut s = family();
    let gone = s.boundary().despawn_tree(id(1)).unwrap();
    assert_eq!(gone, [id(1), id(2), id(4), id(5), id(7), id(3)]);
    assert_eq!(live(&s), [6], "the entity apart stays");
    assert_eq!(s.world().resource::<EntityAllocator>().next(), next);
    assert_eq!(s.boundary().despawn(id(1)), Ok(false), "a second despawn");
    assert_eq!(
        s.boundary().despawn(id(7)),
        Ok(false),
        "taken with its parent"
    );
    assert_eq!(s.boundary().despawn_tree(id(4)), Ok(vec![]));
    assert_eq!(
        s.boundary().despawn_tree(id(40)).unwrap_err().code,
        "sim.entity_id_invalid"
    );
    // `despawn` answers whether the entity was live, as before, and takes the same entities.
    let mut s = family();
    assert_eq!(entity::despawn(s.world_mut(), id(2)), Ok(true));
    assert_eq!(live(&s), [1, 3, 6]);
    // The world steps on: the index holds what the world does.
    s.step(&mut NoHooks).unwrap();
    assert_eq!(live(&s), [1, 3, 6]);
    assert_eq!(
        s.boundary().spawn(()).unwrap().get(),
        8,
        "ids are not reused"
    );
}

/// What no checked write leaves but a world may hold (a component inserted from Rust, a snapshot
/// made by hand): a despawn ends, takes the loop it is in whole, and leaves what only names a
/// despawned entity.
#[test]
fn a_despawn_ends_on_loops_and_dangling_parents() {
    let mut s = sim(1);
    let mut b = s.boundary();
    b.spawn(under(2)).unwrap(); // 1 and 2 are each other's parent,
    b.spawn(under(1)).unwrap();
    b.spawn(under(2)).unwrap(); // 3 hangs under the loop,
    b.spawn(under(4)).unwrap(); // 4 is its own parent,
    b.spawn(under(9)).unwrap(); // 5 names an id never given out,
    b.spawn(()).unwrap();
    let h = Hierarchy::read(s.world());
    assert_eq!(h.parent(id(5)), Some(id(9)));
    assert_eq!(s.boundary().despawn_tree(id(4)), Ok(vec![id(4)]));
    assert_eq!(
        s.boundary().despawn_tree(id(1)),
        Ok(vec![id(1), id(2), id(3)])
    );
    assert_eq!(live(&s), [5, 6]);
    // 7 comes under 6; 6 is despawned behind it, so 7 names an entity that is gone and stays.
    let child = s.boundary().spawn(under(6)).unwrap();
    let e = entity::entity(s.world(), id(6)).unwrap();
    s.world_mut().despawn(e);
    amoris_sim::EntityIndex::rebuild(s.world_mut());
    assert_eq!(live(&s), [5, child.get()]);
    assert_eq!(s.boundary().despawn(id(6)), Ok(false));
    assert_eq!(live(&s), [5, 7]);
    s.step(&mut NoHooks).unwrap();
}

#[test]
fn a_system_despawn_takes_the_descendants_when_the_system_returns() {
    let mut s = family();
    s.add_system(
        "test.feller",
        TickPhase::Update,
        RunCondition::every(2, 1).unwrap(),
        |mut cmd: SimCommands, q: Query<&EntityId>, mut seen: ResMut<Seen>| {
            // A child spawned before the call goes with its parent; one staged after it does not.
            let early = cmd.spawn(Parent { parent: id(2) }).unwrap();
            let first = cmd.despawn(id(2)).unwrap();
            let again = cmd.despawn(id(2)).unwrap();
            // A descendant despawned by hand in the same system is no error.
            let child = cmd.despawn(id(5)).unwrap();
            let late = cmd.spawn(Parent { parent: id(2) }).unwrap();
            seen.0.push(format!(
                "despawn {first} {again} {child}; {} live, 4 known {}, spawned {} and {}",
                q.count(),
                cmd.entity(id(4)).is_some(),
                early.get(),
                late.get()
            ));
        },
    )
    .unwrap();
    s.add_system(
        "test.counter",
        TickPhase::Forces,
        RunCondition::Always,
        |q: Query<&EntityId>, mut seen: ResMut<Seen>| {
            let mut ids: Vec<u64> = by_id(&q).map(|id| id.get()).collect();
            ids.sort_unstable();
            seen.0.push(format!("then {ids:?}"));
        },
    )
    .unwrap();
    s.step(&mut NoHooks).unwrap();
    assert_eq!(
        seen(&s),
        [
            "despawn true false true; 7 live, 4 known true, spawned 8 and 9",
            "then [1, 3, 6, 9]"
        ]
    );
    assert_eq!(live(&s), [1, 3, 6, 9]);
    // The late child names a parent that is gone: it lists nowhere under it, and nothing loops.
    let h = Hierarchy::read(s.world());
    assert_eq!(h.parent(id(9)), Some(id(2)));
    assert!(h.descendants(id(1)).contains(&id(3)));
    s.step(&mut NoHooks).unwrap();
    assert_eq!(live(&s), [1, 3, 6, 9]);
}

/// The hierarchy is read from the rows by id, so it is the same whatever the storage's order.
#[test]
fn the_hierarchy_does_not_depend_on_storage_order() {
    let mut s = family();
    let before = Hierarchy::read(s.world());
    let children =
        |h: &Hierarchy, n: u64| -> Vec<u64> { h.children(id(n)).map(EntityId::get).collect() };
    assert_eq!(children(&before, 1), [2, 3]);
    assert_eq!(children(&before, 2), [4, 5]);
    for seed in 0..5 {
        shuffle_storage(s.world_mut(), seed);
        assert_eq!(Hierarchy::read(s.world()), before);
    }
    assert_eq!(
        s.boundary().despawn_tree(id(1)).unwrap(),
        [id(1), id(2), id(4), id(5), id(7), id(3)]
    );
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
        |world: &mut World, ctx: &mut amoris_sim::SystemCtx<'_>| {
            let inv = amoris_sim::sim::begin_invocation(world, ctx.key());
            let kept = entity::reserve(world).unwrap();
            let dropped = entity::reserve(world).unwrap();
            entity::spawn_allocated(world, kept, Value(1.0)).unwrap();
            amoris_sim::sim::commit_invocation(world, inv);
            let refused = entity::spawn_allocated(world, dropped, ()).unwrap_err();
            let mut seen = world.resource_mut::<Seen>();
            seen.0.push(format!("{} {}", kept.get(), refused.code));
            let inv = amoris_sim::sim::begin_invocation(world, ctx.key());
            let undone = entity::reserve(world).unwrap();
            amoris_sim::sim::rollback_invocation(world, inv);
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
fn order_world(storage_order_sum: bool) -> amoris_sim::Sim {
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

fn chain(mut s: amoris_sim::Sim, shuffle: bool, ticks: u64) -> Vec<u64> {
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
        .resource::<amoris_sim::EntityIndex>()
        .iter()
        .collect();
    shuffle_storage(s.world_mut(), 5);
    let after: Vec<(EntityId, Entity)> = s
        .world()
        .resource::<amoris_sim::EntityIndex>()
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
    // `Parent` is in every world: one field, an entity id that is never none.
    let parent = reg_of(&s, "Parent");
    assert_eq!(parent.version, Parent::VERSION);
    assert_eq!(parent.origin, ComponentOrigin::Engine);
    assert!(parent.doc.contains("under another"), "{}", parent.doc);
    assert_eq!(
        parent.json_schema["required"],
        serde_json::json!(["parent"])
    );
    assert_eq!(
        parent.json_schema["properties"]["parent"]["$ref"],
        serde_json::json!("#/$defs/EntityId")
    );
    assert_eq!(
        parent.json_schema["additionalProperties"],
        serde_json::json!(false)
    );
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
    assert_eq!(names, ["Name", "Parent", "Tally"]);
    let tally = reg.get("Tally").unwrap();
    assert_eq!(
        tally.json_schema["properties"]["at"]["required"],
        serde_json::json!(["x", "y", "z"])
    );
    assert_eq!(reg.by_id(cid).map(|e| &*e.name), Some("Tally"));
    assert_eq!(reg.info()[2].version, 2);
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

/// A copy of the registry's entry for `name`.
fn reg_of(s: &amoris_sim::Sim, name: &str) -> amoris_sim::registry::ComponentEntry {
    let reg = s.world().resource::<ComponentRegistry>();
    reg.get(name)
        .unwrap_or_else(|| panic!("{name} is registered"))
        .clone()
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
        (FieldType::Tick, FieldValue::Tick(amoris_sim::Tick(1 << 53))),
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
                FieldValue::Tick(amoris_sim::Tick(amoris_sim::MAX_ENTITY_ID)),
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
