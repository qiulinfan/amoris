//! What perception's declarations refuse and how the sailing course reads (docs/spec/
//! perception-slice2.md 3 and 8): an event's data field is `chart`, `coarse` or `hidden` and never
//! `relative`; `Course.next` is the `order` of the mark to round next, never an entity id.

use std::alloc::Layout;
use std::sync::Arc;

use bevy_ecs::component::{ComponentCloneBehavior, ComponentDescriptor, ComponentId, StorageType};
use bevy_ecs::prelude::World;
use bevy_ecs::ptr::OwningPtr;
use pocket_interface::perception::probe::{self, Ids};
use pocket_interface::perception::{FactValue, PerceptionView, load};
use pocket_sim::registry::{
    ComponentOrigin, ComponentRegistry, ComponentSchema, FieldType, FieldValue, ProjectValues,
};
use pocket_sim::{EntityId, EntityIndex, Name, NoHooks};
use serde_json::json;

/// Declarations with one sight-scope event whose one data field has this exposure.
fn event_field(exposure: &str, relative: bool) -> String {
    let field = json!({
        "name": "n", "doc": "A count.", "unit": {"unit": "count"}, "exposure": exposure,
        "relative": relative, "source": {"from": "data", "path": "n"}
    });
    json!({
        "observers": [],
        "events": [{"kind": "ev.one", "doc": "An event.", "scope": {"scope": "sight"},
                    "data": [field]}]
    })
    .to_string()
}

#[test]
fn event_data_is_chart_coarse_or_hidden_and_never_relative() {
    for exposure in ["chart", "coarse", "hidden"] {
        assert!(load(&event_field(exposure, false)).is_ok(), "{exposure}");
    }
    for (exposure, relative, member) in [
        ("full", false, "exposure"),
        ("owner", false, "exposure"),
        ("coarse", true, "relative"),
    ] {
        let e = load(&event_field(exposure, relative)).expect_err("refused");
        assert_eq!(e.code, "definition.invalid", "{exposure}");
        assert_eq!(
            e.detail["path"],
            json!(format!("/events/0/data/0/{member}")),
            "{exposure} {relative}"
        );
    }
}

unsafe fn drop_values(ptr: OwningPtr<'_>) {
    // SAFETY: the component is registered with the layout of ProjectValues.
    unsafe { ptr.drop_as::<ProjectValues>() }
}

/// Registers a project component as the script host would.
fn register(world: &mut World, schema: ComponentSchema) -> ComponentId {
    // SAFETY: drop_values drops a ProjectValues, the layout given; ProjectValues is Send + Sync.
    let d = unsafe {
        ComponentDescriptor::new_with_layout(
            schema.name.to_string(),
            StorageType::Table,
            Layout::new::<ProjectValues>(),
            Some(drop_values),
            true,
            ComponentCloneBehavior::Ignore,
            None,
        )
    };
    let id = world.register_component_with_descriptor(d);
    ComponentRegistry::register_project(world, schema, id).expect("registers");
    id
}

fn put(world: &mut World, id: EntityId, c: ComponentId, nums: &[f64], strs: &[&str]) {
    let e = world.resource::<EntityIndex>().get(id).expect("live");
    let v = ProjectValues {
        nums: nums.into(),
        strs: strs.iter().map(|s| Arc::from(*s)).collect(),
    };
    OwningPtr::make(v, |ptr| {
        // SAFETY: `c` is laid out as ProjectValues.
        unsafe {
            world.entity_mut(e).insert_by_id(c, ptr);
        }
    });
}

/// The sample's `Mark {color, round_to, order}` and `Course {next}` (sailing.md, The skipper's
/// perception; perception-slice2.md 8).
fn schemas() -> (ComponentSchema, ComponentSchema) {
    let mark = ComponentSchema::new(
        "Mark",
        ComponentOrigin::Project,
        1,
        "A course mark.",
        vec![
            (
                "color",
                FieldType::Str,
                FieldValue::Str("".into()),
                "Its colour.",
            ),
            (
                "round_to",
                FieldType::Str,
                FieldValue::Str("".into()),
                "Its side.",
            ),
            (
                "order",
                FieldType::U32,
                FieldValue::U32(0),
                "Its place in the course.",
            ),
        ],
    )
    .expect("a schema");
    let course = ComponentSchema::new(
        "Course",
        ComponentOrigin::Project,
        1,
        "The course.",
        vec![(
            "next",
            FieldType::U32,
            FieldValue::U32(0),
            "The order to round next.",
        )],
    )
    .expect("a schema");
    (mark, course)
}

/// The skipper's `next_mark`, and the `next` fact of Mark1 (seen) and Mark2 (charted), with the
/// marks' orders and the course's `next` as given.
fn course(
    orders: [f64; 2],
    next: f64,
) -> (Option<FactValue>, Option<FactValue>, Option<FactValue>) {
    let (mut sim, ids): (_, Ids) = probe::world(20);
    let (mark, course) = schemas();
    let w = sim.world_mut();
    let mark = register(w, mark);
    let course = register(w, course);
    put(w, ids.mark1, mark, &[orders[0]], &["yellow", "port"]);
    put(w, ids.far_mark, mark, &[orders[1]], &["red", "starboard"]);
    let c = sim
        .boundary()
        .spawn((Name::new("Course").expect("a name"),))
        .expect("spawns");
    put(sim.world_mut(), c, course, &[next], &[]);
    // Mark1 is seen: its facts are stored at the next update.
    sim.step(&mut NoHooks).expect("a tick");
    let v = PerceptionView::for_seat(sim.world(), "skipper").expect("the skipper");
    let fact = |id: EntityId| {
        v.percept(id)?
            .facts
            .into_iter()
            .find(|r| r.name == "next")
            .map(|r| r.value)
    };
    (
        v.instrument("next_mark"),
        fact(ids.mark1),
        fact(ids.far_mark),
    )
}

fn text(s: &str) -> Option<FactValue> {
    Some(FactValue::Text(s.to_owned()))
}

const YES: Option<FactValue> = Some(FactValue::Bool(true));
const NO: Option<FactValue> = Some(FactValue::Bool(false));

#[test]
fn the_course_names_its_next_mark_by_order() {
    let (_, ids) = probe::world(0);
    let (m1, m2) = (ids.mark1.to_f64(), ids.far_mark.to_f64());
    assert_eq!(course([1.0, 2.0], 1.0), (text("Mark1"), YES, NO));
    assert_eq!(course([1.0, 2.0], 2.0), (text("Mark2"), NO, YES));
    // `next` equal to an entity's id that is no mark's order names nothing.
    assert_eq!(
        course([1.0, 2.0], ids.isle.to_f64()),
        (text("none"), NO, NO)
    );
    // Each mark's order is the other's id: `next` is matched against the orders only.
    assert_eq!(course([m2, m1], m1), (text("Mark2"), NO, YES));
}
