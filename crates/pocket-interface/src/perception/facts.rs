//! Evaluating facts and instruments (shared/contract/perception.md, Perceivable entities, kinds and
//! facts; Instruments): a component field by its registered name and a dotted path, or a derived
//! function registered by name. Engine components are read through their serde form, project
//! components (the game's own, declared in TypeScript) through the component registry's schema
//! over their `ProjectValues` (script-host.md 7.3), as `pocket-persist` reads them.
//!
//! A relative fact depends on the observer: its function reads the observer's body, the position
//! the observer knows the entity at and the facts it knows of it, never the live entity, so a
//! remembered entity shows what the observer could know (perception.md, The perception update).

use std::alloc::Layout;
use std::collections::BTreeMap;

use bevy_ecs::component::Component;
use bevy_ecs::prelude::{Entity, World};
use pocket_physics::geom::V3;
use pocket_physics::{
    Boat, Collider, ExternalForce, Floater, Hull, RigidBody, Sail, Sea, Transform, Velocity, Wind,
};
use pocket_sim::registry::{ComponentOrigin, FieldType, ProjectValues};
use pocket_sim::{ComponentRegistry, EntityId, EntityIndex, Name};
use serde::Serialize;
use serde_json::Value;

use super::defs::{FactSource, PerceptionDefs};
use super::state::{FactValue, Observer, Occluder, Perceivable};

/// A derived fact or instrument: a pure function of what [`FactCx`] gives.
pub type DerivedFn = fn(&FactCx<'_>) -> Option<FactValue>;

/// The observer's body, for relative facts and instruments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub id: EntityId,
    pub entity: Entity,
    pub position: V3,
}

/// What a derived function may read.
pub struct FactCx<'w> {
    pub world: &'w World,
    /// The entity the fact is of (the body itself for an instrument).
    pub id: EntityId,
    /// The live entity, for a fact evaluated at a sighting or an instrument; `None` when a
    /// relative fact is recomputed for a remembered or charted entity.
    pub entity: Option<Entity>,
    /// Where the observer knows the entity to be: live, remembered or charted.
    pub at: V3,
    /// The entity's facts as the observer knows them (stored at the last sighting).
    pub known: &'w BTreeMap<String, FactValue>,
    /// The observer's body.
    pub observer: Option<Body>,
}

impl<'w> FactCx<'w> {
    /// A component of the live entity.
    pub fn get<C: Component>(&self) -> Option<&'w C> {
        self.world.get::<C>(self.entity?)
    }

    /// A component of the observer's body.
    pub fn body<C: Component>(&self) -> Option<&'w C> {
        self.world.get::<C>(self.observer?.entity)
    }

    /// A field of a project component of `entity`.
    pub fn project(&self, entity: Entity, component: &str, path: &str) -> Option<FactValue> {
        project_field(self.world, entity, component, path)
    }
}

/// An empty fact map, for instruments and sightings.
pub static NO_FACTS: BTreeMap<String, FactValue> = BTreeMap::new();

/// Evaluates `source` for the entity in `cx` (`None`: absent for this observer and tick).
pub fn evaluate(defs: &PerceptionDefs, source: &FactSource, cx: &FactCx<'_>) -> Option<FactValue> {
    let v = match source {
        FactSource::Field { component, path } => read_field(cx.world, cx.entity?, component, path),
        FactSource::Derived { function } => (defs.derived(function)?)(cx),
        FactSource::Data { .. } => None,
    }?;
    finite(v)
}

/// Non-finite numbers never enter an answer or the world (README, Numbers).
fn finite(v: FactValue) -> Option<FactValue> {
    match &v {
        FactValue::Number(x) if !x.is_finite() => None,
        FactValue::Position(p) if !p.iter().all(|x| x.is_finite()) => None,
        _ => Some(v),
    }
}

/// A member of event data by a dotted path, as a fact value.
pub fn data_field(data: &pocket_sim::PlainData, path: &str) -> Option<FactValue> {
    let mut at = data;
    for part in path.split('.') {
        at = match at {
            pocket_sim::PlainData::Object(_) => at.get(part)?,
            pocket_sim::PlainData::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    finite(json_fact(&at.to_json())?)
}

fn json_of<C: Component + Serialize>(world: &World, e: Entity) -> Option<Value> {
    world.get::<C>(e).and_then(|c| serde_json::to_value(c).ok())
}

/// The serde form of an engine component this crate knows, by its registered name.
pub fn engine_json(world: &World, e: Entity, component: &str) -> Option<Value> {
    match component {
        "Transform" => json_of::<Transform>(world, e),
        "Velocity" => json_of::<Velocity>(world, e),
        "RigidBody" => json_of::<RigidBody>(world, e),
        "Collider" => json_of::<Collider>(world, e),
        "ExternalForce" => json_of::<ExternalForce>(world, e),
        "Floater" => json_of::<Floater>(world, e),
        "Boat" => json_of::<Boat>(world, e),
        "Sail" => json_of::<Sail>(world, e),
        "Hull" => json_of::<Hull>(world, e),
        "Sea" => json_of::<Sea>(world, e),
        "Wind" => json_of::<Wind>(world, e),
        "Name" => json_of::<Name>(world, e),
        "Observer" => json_of::<Observer>(world, e),
        "Perceivable" => json_of::<Perceivable>(world, e),
        "Occluder" => json_of::<Occluder>(world, e),
        _ => None,
    }
}

/// A JSON value as a fact: a number, a boolean, a string, or three numbers as a position.
pub fn json_fact(v: &Value) -> Option<FactValue> {
    match v {
        Value::Bool(b) => Some(FactValue::Bool(*b)),
        Value::Number(n) => n.as_f64().map(FactValue::Number),
        Value::String(s) => Some(FactValue::Text(s.clone())),
        Value::Array(a) if a.len() == 3 => {
            let mut p = [0.0; 3];
            for (i, x) in a.iter().enumerate() {
                p[i] = x.as_f64()?;
            }
            Some(FactValue::Position(p))
        }
        Value::Object(m) if m.len() == 3 => {
            let c = |k: &str| m.get(k).and_then(Value::as_f64);
            Some(FactValue::Position([c("x")?, c("y")?, c("z")?]))
        }
        _ => None,
    }
}

/// Follows a dotted path into a JSON value (`position`, `position.1`, `trim`).
fn walk<'v>(mut v: &'v Value, path: &str) -> Option<&'v Value> {
    if path.is_empty() {
        return Some(v);
    }
    for part in path.split('.') {
        v = match v {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(v)
}

/// A component field of `e` by the component's registered name and a dotted path.
pub fn read_field(world: &World, e: Entity, component: &str, path: &str) -> Option<FactValue> {
    // The transform's position, the most read field, without its serde form.
    if component == "Transform" {
        let p = world.get::<Transform>(e)?.position;
        return match path {
            "position" => Some(FactValue::Position(p)),
            "position.0" => Some(FactValue::Number(p[0])),
            "position.1" => Some(FactValue::Number(p[1])),
            "position.2" => Some(FactValue::Number(p[2])),
            _ => json_fact(walk(&engine_json(world, e, component)?, path)?),
        };
    }
    if let Some(v) = engine_json(world, e, component) {
        return json_fact(walk(&v, path)?);
    }
    project_field(world, e, component, path)
}

/// The values of a project component of `e`, read by its registered layout.
fn project_values<'w>(
    world: &'w World,
    e: Entity,
    component: &str,
) -> Option<(&'w pocket_sim::registry::ComponentSchema, &'w ProjectValues)> {
    let entry = world.get_resource::<ComponentRegistry>()?.get(component)?;
    if entry.origin != ComponentOrigin::Project {
        return None;
    }
    let schema = entry.script.as_deref()?;
    let layout = world.components().get_info(entry.id).map(|i| i.layout());
    if layout != Some(Layout::new::<ProjectValues>()) {
        return None;
    }
    let ptr = world.get_entity(e).ok()?.get_by_id(entry.id).ok()?;
    // SAFETY: the registry names this component a project component and the world registered it
    // with the layout of ProjectValues, the one layout every project component shares
    // (script-host.md 7.3), as pocket-persist checks before it reads one the same way.
    let values = unsafe { ptr.deref::<ProjectValues>() };
    Some((schema, values))
}

/// A field of a project component: `field`, or `field.x` (or `.0`) into a vector.
pub fn project_field(world: &World, e: Entity, component: &str, path: &str) -> Option<FactValue> {
    let (schema, values) = project_values(world, e, component)?;
    let (name, sub) = match path.split_once('.') {
        Some((n, s)) => (n, Some(s)),
        None => (path, None),
    };
    let mut strings = 0usize;
    for f in schema.fields.iter() {
        if &*f.name != name {
            if f.ty == FieldType::Str {
                strings += 1;
            }
            continue;
        }
        let slot = usize::from(f.first_slot);
        let num = |i: usize| values.nums.get(slot + i).copied();
        let v = match (&f.ty, sub) {
            (FieldType::Str, None) => FactValue::Text(values.strs.get(strings)?.to_string()),
            (FieldType::Bool, None) => FactValue::Bool(num(0)? != 0.0),
            (FieldType::Enum(variants), None) => {
                let i = num(0)?;
                let i = variants
                    .iter()
                    .enumerate()
                    .find(|(k, _)| index_is(*k, i))?
                    .1;
                FactValue::Text(i.to_string())
            }
            (FieldType::Vec3, None) => FactValue::Position([num(0)?, num(1)?, num(2)?]),
            (FieldType::Vec2 | FieldType::Vec3 | FieldType::Vec4 | FieldType::Quat, Some(s)) => {
                let i = match s {
                    "x" | "0" => 0,
                    "y" | "1" => 1,
                    "z" | "2" => 2,
                    "w" | "3" => 3,
                    _ => return None,
                };
                if i >= usize::from(f.ty.slots()) {
                    return None;
                }
                FactValue::Number(num(i)?)
            }
            (_, None) => FactValue::Number(num(0)?),
            _ => return None,
        };
        return Some(v);
    }
    None
}

/// Whether the slot value `x` is the variant index `k`.
fn index_is(k: usize, x: f64) -> bool {
    u32::try_from(k).is_ok_and(|k| f64::from(k) == x)
}

/// Every component of `e` as `Component.field` facts, for the raw omniscient view (perception.md,
/// The omniscient view): engine components this crate knows by their serde form's top-level
/// fields, project components by their schema. Names ascend.
pub fn component_facts(world: &World, e: Entity) -> Vec<(String, FactValue)> {
    let mut out = Vec::new();
    let Some(reg) = world.get_resource::<ComponentRegistry>() else {
        return out;
    };
    for entry in reg.entries() {
        if entry.origin == ComponentOrigin::Project {
            let Some(schema) = entry.script.as_deref() else {
                continue;
            };
            if project_values(world, e, &entry.name).is_none() {
                continue;
            }
            for f in schema.fields.iter() {
                if let Some(v) = project_field(world, e, &entry.name, &f.name) {
                    out.push((format!("{}.{}", entry.name, f.name), v));
                }
            }
            continue;
        }
        let Some(v) = engine_json(world, e, &entry.name) else {
            continue;
        };
        match &v {
            Value::Object(m) => {
                for (k, x) in m {
                    let fact = json_fact(x).unwrap_or_else(|| FactValue::Text(x.to_string()));
                    out.push((format!("{}.{k}", entry.name), fact));
                }
            }
            other => {
                let fact = json_fact(other).unwrap_or_else(|| FactValue::Text(other.to_string()));
                out.push((entry.name.to_string(), fact));
            }
        }
    }
    out
}

/// The live entity of an id.
pub fn live(world: &World, id: EntityId) -> Option<Entity> {
    world.get_resource::<EntityIndex>()?.get(id)
}

/// The name of an entity, if it has one.
pub fn name_of(world: &World, e: Entity) -> Option<String> {
    world.get::<Name>(e).map(|n| n.as_str().to_owned())
}

/// An entity's position (its `Transform`), if it has one.
pub fn position(world: &World, e: Entity) -> Option<V3> {
    world.get::<Transform>(e).map(|t| t.position)
}
