//! Project components (persistence.md 6.2, open choice 8): dynamic components of layout
//! `ProjectValues` described by the world's component registry are snapshotted, hashed, restored,
//! forked and diffed without the script host.

mod common;

use std::alloc::Layout;
use std::sync::Arc;

use bevy_ecs::component::{ComponentCloneBehavior, ComponentDescriptor, ComponentId, StorageType};
use bevy_ecs::prelude::*;
use bevy_ecs::ptr::OwningPtr;
use pocket_persist::diff::diff_with;
use pocket_persist::{
    FormatTable, RestoreOptions, SectionKey, SectionKind, restore, snapshot, world_hash,
};
use pocket_sim::registry::{
    ComponentOrigin, ComponentRegistry, ComponentSchema, FieldType, FieldValue, ProjectValues,
};
use pocket_sim::{EntityId, Sim, Tick};
use serde_json::json;

unsafe fn drop_values(ptr: OwningPtr<'_>) {
    // SAFETY: the component is registered with the layout of ProjectValues.
    unsafe { ptr.drop_as::<ProjectValues>() }
}

fn plot_schema() -> ComponentSchema {
    let variants: Arc<[Box<str>]> = Arc::from(vec![Box::from("Wheat"), Box::from("Barley")]);
    ComponentSchema::new(
        "Plot",
        ComponentOrigin::Project,
        1,
        "A field plot.",
        vec![
            ("stage", FieldType::I32, FieldValue::I32(0), "growth stage"),
            ("water", FieldType::F64, FieldValue::F64(0.0), "litres"),
            (
                "crop",
                FieldType::Str,
                FieldValue::Str("wheat".into()),
                "what is sown",
            ),
            (
                "owner",
                FieldType::Entity,
                FieldValue::Entity(None),
                "who farms it",
            ),
            (
                "kind",
                FieldType::Enum(variants),
                FieldValue::Enum(0),
                "the grain",
            ),
            ("spot", FieldType::Vec3, FieldValue::Vec3([0.0; 3]), "where"),
            (
                "ripe",
                FieldType::Bool,
                FieldValue::Bool(false),
                "ready to harvest",
            ),
            (
                "planted",
                FieldType::Tick,
                FieldValue::Tick(Tick(0)),
                "when sown",
            ),
        ],
    )
    .unwrap()
}

/// Registers `Plot` as the script host would.
fn register(world: &mut World) -> ComponentId {
    // SAFETY: drop_values drops a ProjectValues, the layout given; ProjectValues is Send + Sync.
    let d = unsafe {
        ComponentDescriptor::new_with_layout(
            "Plot",
            StorageType::Table,
            Layout::new::<ProjectValues>(),
            Some(drop_values),
            true,
            ComponentCloneBehavior::Ignore,
            None,
        )
    };
    let id = world.register_component_with_descriptor(d);
    ComponentRegistry::register_project(world, plot_schema(), id).unwrap();
    id
}

fn put(world: &mut World, e: Entity, id: ComponentId, v: ProjectValues) {
    OwningPtr::make(v, |ptr| {
        // SAFETY: `id` is laid out as ProjectValues.
        unsafe {
            world.entity_mut(e).insert_by_id(id, ptr);
        }
    });
}

fn values(stage: f64, water: f64, crop: &str, owner: f64) -> ProjectValues {
    ProjectValues {
        nums: vec![stage, water, owner, 1.0, 1.5, -0.0, 2.25, 1.0, 17.0].into_boxed_slice(),
        strs: vec![Arc::from(crop)].into_boxed_slice(),
    }
}

fn farm() -> (Sim, ComponentId) {
    let mut sim = common::sim(3);
    let id = register(sim.world_mut());
    let ids: Vec<(EntityId, Entity)> = sim
        .world()
        .resource::<pocket_sim::EntityIndex>()
        .iter()
        .collect();
    put(
        sim.world_mut(),
        ids[0].1,
        id,
        values(2.0, 0.5, "barley", 3.0),
    );
    put(sim.world_mut(), ids[2].1, id, values(-1.0, 7.0, "", 0.0));
    (sim, id)
}

fn read(world: &World, id: ComponentId, e: Entity) -> ProjectValues {
    let ptr = world.entity(e).get_by_id(id).unwrap();
    // SAFETY: `id` is laid out as ProjectValues.
    unsafe { ptr.deref::<ProjectValues>() }.clone()
}

#[test]
fn project_components_round_trip() {
    let reg = common::registry();
    let (sim, id) = farm();
    let s = snapshot(sim.world(), &reg).unwrap();
    let key = SectionKey::new(SectionKind::Component, "Plot");
    let plot = s.section(&key).expect("a Plot section");
    assert_eq!(plot.version, 1);
    // Restore into a fresh world whose script host registered Plot in its own order.
    let mut fresh = common::sim(9);
    let fid = register(fresh.world_mut());
    restore(fresh.world_mut(), &s, &reg, RestoreOptions::default()).unwrap();
    assert_eq!(
        snapshot(fresh.world(), &reg).unwrap().to_bytes(),
        s.to_bytes()
    );
    for n in [1u64, 3] {
        let (a, b) = (
            common::ent(&sim, n).unwrap(),
            pocket_sim::entity::entity(fresh.world(), EntityId::new(n).unwrap()).unwrap(),
        );
        let (va, vb) = (read(sim.world(), id, a), read(fresh.world(), fid, b));
        assert_eq!(va.strs, vb.strs);
        let bits = |v: &ProjectValues| v.nums.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&va), bits(&vb), "entity {n}, -0.0 kept");
    }
    // A world without the project component registered refuses the section.
    let mut bare = common::sim(9);
    assert_eq!(
        restore(bare.world_mut(), &s, &reg, RestoreOptions::default())
            .unwrap_err()
            .code,
        "persist.unknown_section"
    );
}

#[test]
fn project_values_are_checked_and_diffed() {
    let reg = common::registry();
    let (mut sim, id) = farm();
    let before = snapshot(sim.world(), &reg).unwrap();
    let e = common::ent(&sim, 1).unwrap();
    let mut v = read(sim.world(), id, e);
    v.nums[1] = 0.75;
    put(sim.world_mut(), e, id, v.clone());
    let after = snapshot(sim.world(), &reg).unwrap();
    let table = FormatTable::of(&reg, sim.world()).unwrap();
    let d = diff_with(&before, &after, &table, 50).unwrap();
    let f = d.fields.iter().find(|f| f.section.name == "Plot").unwrap();
    assert_eq!((f.entity.unwrap().get(), f.path.as_str()), (1, "/water"));
    assert_eq!(
        (f.expected.clone(), f.actual.clone()),
        (json!(0.5), json!(0.75))
    );
    // 2.5 in an i32 field is not a value of the field.
    v.nums[0] = 2.5;
    put(sim.world_mut(), e, id, v);
    let err = snapshot(sim.world(), &reg).unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.detail["entity"].clone()),
        ("persist.encode", json!(1))
    );
}

/// A project component on an entity without an `EntityId` is refused like an engine one: its row
/// would be in no section and no hash.
#[test]
fn a_project_row_without_an_entity_id_is_refused() {
    let reg = common::registry();
    let (mut sim, id) = farm();
    let good = snapshot(sim.world(), &reg).unwrap();
    let stray = sim.world_mut().spawn_empty().id();
    put(sim.world_mut(), stray, id, values(1.0, 0.0, "", 0.0));
    let err = world_hash(sim.world(), &reg).unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.detail["section"].clone()),
        ("persist.encode", json!("component:Plot"))
    );
    sim.world_mut().despawn(stray);
    assert_eq!(snapshot(sim.world(), &reg).unwrap(), good);
}
