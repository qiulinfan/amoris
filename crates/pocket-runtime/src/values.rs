//! Values as commands and scenes write them (shared/contract/mcp.md 6.1): entity references (an id,
//! a name or `Name#id`) resolved to `EntityId`s, and project components read and written as JSON
//! objects of their fields through their schemas (vectors as `{x, y, z}`, enums by name, entities as
//! ids or `null`).

use std::sync::Arc;

use bevy_ecs::prelude::World;
use pocket_contract::codes::{
    ambiguous_ref, invalid_value, not_integer, num as int, unknown_field, wrong_type,
};
use pocket_contract::render::json_type;
use pocket_contract::{Candidate, Pointer, Problem, detail, suggest_names};
use pocket_sim::registry::{ComponentSchema, FieldType, ProjectValues};
use pocket_sim::{EntityId, Name, entity};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// An entity as a command names it: its id, or a name (`Sloop`, or `Sloop#3` to say which).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EntityRef {
    Id(u64),
    Name(String),
}

/// Every named live entity, ascending by id.
pub fn named(world: &World) -> Vec<(EntityId, String)> {
    let index = world.resource::<pocket_sim::EntityIndex>();
    index
        .iter()
        .filter_map(|(id, e)| world.get::<Name>(e).map(|n| (id, n.as_str().to_owned())))
        .collect()
}

fn not_found(reference: &str, world: &World) -> Problem {
    let names = named(world);
    let suggestions = suggest_names(reference, names.iter().map(|(_, n)| n.as_str()));
    Problem::new(
        "sim.entity_not_found",
        format!("No live entity is named '{reference}'; did you mean {suggestions:?}?"),
        detail([
            ("ref", json!(reference)),
            ("suggestions", json!(suggestions)),
        ]),
    )
}

/// The live entity a reference names: `sim.entity_not_found` when none does, `request.ambiguous_ref`
/// when a name is shared.
pub fn resolve(world: &World, r: &EntityRef, path: &Pointer) -> Result<EntityId, Problem> {
    let live = |id: EntityId| {
        entity::entity(world, id)
            .map(|_| id)
            .ok_or_else(|| entity::entity_not_found(id))
    };
    match r {
        EntityRef::Id(n) => {
            let id =
                EntityId::new(*n).ok_or_else(|| entity::entity_id_invalid(&n.to_string(), 0))?;
            live(id)
        }
        EntityRef::Name(s) => {
            let (name, id) = match s.rsplit_once('#') {
                Some((name, id)) if id.parse::<u64>().is_ok() => {
                    (name, id.parse::<u64>().ok().and_then(EntityId::new))
                }
                _ => (s.as_str(), None),
            };
            let found: Vec<(EntityId, String)> = named(world)
                .into_iter()
                .filter(|(i, n)| n == name && id.is_none_or(|id| id == *i))
                .collect();
            match found.as_slice() {
                [] => Err(not_found(s, world)),
                [(id, _)] => Ok(*id),
                many => {
                    let c: Vec<(u64, &str, &str)> = many
                        .iter()
                        .map(|(i, n)| (i.get(), n.as_str(), "entity"))
                        .collect();
                    Err(ambiguous_ref(path, s, &c))
                }
            }
        }
    }
}

/// A JSON value standing for an entity: an id, a name, or `null` for none.
pub fn resolve_json(world: &World, v: &Value, path: &Pointer) -> Result<Option<EntityId>, Problem> {
    match v {
        Value::Null => Ok(None),
        Value::Number(n) => match n.as_u64() {
            Some(n) => resolve(world, &EntityRef::Id(n), path).map(Some),
            None => Err(wrong_type(path, "an entity id, a name or null", "number")),
        },
        Value::String(s) => resolve(world, &EntityRef::Name(s.clone()), path).map(Some),
        other => Err(wrong_type(
            path,
            "an entity id, a name or null",
            json_type(other),
        )),
    }
}

fn comps(ty: &FieldType) -> &'static [&'static str] {
    match ty {
        FieldType::Vec2 => &["x", "y"],
        FieldType::Vec3 => &["x", "y", "z"],
        FieldType::Vec4 | FieldType::Quat => &["x", "y", "z", "w"],
        _ => &[],
    }
}

/// A project component's values as a JSON object of its fields.
pub fn to_json(schema: &ComponentSchema, v: &ProjectValues) -> Value {
    let mut out = Map::new();
    let mut strs = v.strs.iter();
    for f in schema.fields.iter() {
        let at = usize::from(f.first_slot);
        let x = v.nums.get(at).copied().unwrap_or(0.0);
        let value = match &f.ty {
            FieldType::F64 => json!(x),
            FieldType::I32 | FieldType::U32 | FieldType::Tick => int(x),
            FieldType::Bool => json!(x != 0.0),
            FieldType::Entity => {
                if x == 0.0 {
                    Value::Null
                } else {
                    int(x)
                }
            }
            FieldType::Str => json!(strs.next().map_or("", |s| &**s)),
            FieldType::Enum(names) => {
                let i = pocket_sim::num::to_u32(x).unwrap_or(0);
                json!(
                    names
                        .get(usize::try_from(i).unwrap_or(0))
                        .map_or("", |n| &**n)
                )
            }
            ty => {
                let names = comps(ty);
                let m: Map<String, Value> = names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| {
                        (
                            (*n).to_owned(),
                            json!(v.nums.get(at + i).copied().unwrap_or(0.0)),
                        )
                    })
                    .collect();
                Value::Object(m)
            }
        };
        out.insert(f.name.to_string(), value);
    }
    Value::Object(out)
}

fn number(v: &Value, path: &Pointer) -> Result<f64, Problem> {
    v.as_f64()
        .filter(|x| x.is_finite())
        .ok_or_else(|| wrong_type(path, "a finite number", json_type(v)))
}

fn integral(v: &Value, path: &Pointer, conv: fn(f64) -> bool) -> Result<f64, Problem> {
    let x = number(v, path)?;
    if !conv(x) {
        return Err(not_integer(path, v));
    }
    Ok(x)
}

/// Writes `patch`'s fields over `base` (a component's current values, or its defaults), checking
/// names, types and ranges; entity fields take an id, a name or `null`. `path` names the
/// component's object in the request.
pub fn patch(
    world: &World,
    schema: &ComponentSchema,
    base: &ProjectValues,
    patch: &Map<String, Value>,
    path: &Pointer,
) -> Result<ProjectValues, Problem> {
    let mut nums = base.nums.to_vec();
    let mut strs = base.strs.to_vec();
    for (key, v) in patch {
        let at_field = path.key(key);
        let Some((i, f)) = schema
            .fields
            .iter()
            .enumerate()
            .find(|(_, f)| &*f.name == key)
        else {
            let valid: Vec<Candidate<'_>> = schema
                .fields
                .iter()
                .map(|f| Candidate::new(&f.name))
                .collect();
            return Err(unknown_field(&at_field, &schema.name, &valid, None));
        };
        let slot = usize::from(f.first_slot);
        match &f.ty {
            FieldType::F64 => nums[slot] = number(v, &at_field)?,
            FieldType::I32 => {
                nums[slot] = integral(v, &at_field, |x| pocket_sim::num::to_i32(x).is_ok())?
            }
            FieldType::U32 => {
                nums[slot] = integral(v, &at_field, |x| pocket_sim::num::to_u32(x).is_ok())?
            }
            FieldType::Tick => {
                nums[slot] = integral(v, &at_field, |x| pocket_sim::num::to_tick(x).is_ok())?
            }
            FieldType::Bool => {
                let b = v
                    .as_bool()
                    .ok_or_else(|| wrong_type(&at_field, "a boolean", json_type(v)))?;
                nums[slot] = if b { 1.0 } else { 0.0 };
            }
            FieldType::Entity => {
                nums[slot] = resolve_json(world, v, &at_field)?.map_or(0.0, EntityId::to_f64)
            }
            FieldType::Str => {
                let s = v
                    .as_str()
                    .ok_or_else(|| wrong_type(&at_field, "a string", json_type(v)))?;
                let k = pocket_script::access::str_index(schema, i);
                strs[k] = Arc::from(s);
            }
            FieldType::Enum(names) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| wrong_type(&at_field, "a string", json_type(v)))?;
                let Some(idx) = names.iter().position(|n| &**n == s) else {
                    let valid: Vec<Candidate<'_>> =
                        names.iter().map(|n| Candidate::new(n)).collect();
                    return Err(invalid_value(&at_field, s, &valid));
                };
                nums[slot] = f64::from(u32::try_from(idx).unwrap_or(0));
            }
            ty => {
                let names = comps(ty);
                let obj = v
                    .as_object()
                    .ok_or_else(|| wrong_type(&at_field, "an object", json_type(v)))?;
                for (k, x) in obj {
                    let Some(c) = names.iter().position(|n| n == k) else {
                        let valid: Vec<Candidate<'_>> =
                            names.iter().map(|n| Candidate::new(n)).collect();
                        return Err(unknown_field(&at_field.key(k), key, &valid, None));
                    };
                    nums[slot + c] = number(x, &at_field.key(k))?;
                }
            }
        }
    }
    Ok(ProjectValues {
        nums: nums.into_boxed_slice(),
        strs: strs.into_boxed_slice(),
    })
}

/// A checked patch in its canonical recorded form (threads.md 5.3): entity fields given by name
/// become their ids, so a replay never resolves a name again.
pub fn canonical_patch(
    world: &World,
    schema: &ComponentSchema,
    patch: &Map<String, Value>,
    path: &Pointer,
) -> Result<Map<String, Value>, Problem> {
    let mut out = Map::new();
    for (key, v) in patch {
        let entity = schema
            .fields
            .iter()
            .any(|f| &*f.name == key && f.ty == FieldType::Entity);
        let v = if entity {
            match resolve_json(world, v, &path.key(key))? {
                Some(id) => json!(id.get()),
                None => Value::Null,
            }
        } else {
            v.clone()
        };
        out.insert(key.clone(), v);
    }
    Ok(out)
}
