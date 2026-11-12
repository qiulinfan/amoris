//! Values as commands and scenes write them (shared/contract/mcp.md 6.1): entity references (an id,
//! a name or `Name#id`) resolved to `EntityId`s, and project components read and written as JSON
//! objects of their fields through their schemas (vectors as `{x, y, z}`, enums by name, entities as
//! ids or `null`), and the places of an engine component's JSON that hold entity ids, read from
//! its JSON Schema ([`entity_refs`]).

use std::collections::BTreeSet;
use std::sync::Arc;

use amoris_contract::codes::{
    ambiguous_ref, invalid_value, not_integer, num as int, unknown_field, wrong_type,
};
use amoris_contract::render::json_type;
use amoris_contract::{Candidate, Pointer, Problem, detail, suggest_names};
use amoris_sim::registry::{ComponentSchema, FieldType, ProjectValues};
use amoris_sim::{EntityId, Name, entity};
use bevy_ecs::prelude::World;
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
    let index = world.resource::<amoris_sim::EntityIndex>();
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

/// Which entities a value may name besides the world's live ones: those an earlier edit of the
/// same `world_edit` revives, which are alive by the time the edit naming them is applied.
pub type AlsoAlive<'a> = &'a dyn Fn(EntityId) -> bool;

/// No entity besides the live ones.
pub fn none_else(_: EntityId) -> bool {
    false
}

/// A JSON value standing for an entity: an id, a name, or `null` for none. The entity is live, or
/// its id is one `also` accepts.
pub fn resolve_json(
    world: &World,
    v: &Value,
    path: &Pointer,
    also: AlsoAlive<'_>,
) -> Result<Option<EntityId>, Problem> {
    if let Some(id) = v.as_u64().and_then(EntityId::new)
        && also(id)
    {
        return Ok(Some(id));
    }
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
                let i = amoris_sim::num::to_u32(x).unwrap_or(0);
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
    patch_among(world, schema, base, patch, path, &none_else)
}

/// [`patch`], where an entity field may also name an entity `also` accepts.
pub fn patch_among(
    world: &World,
    schema: &ComponentSchema,
    base: &ProjectValues,
    patch: &Map<String, Value>,
    path: &Pointer,
    also: AlsoAlive<'_>,
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
                nums[slot] = integral(v, &at_field, |x| amoris_sim::num::to_i32(x).is_ok())?
            }
            FieldType::U32 => {
                nums[slot] = integral(v, &at_field, |x| amoris_sim::num::to_u32(x).is_ok())?
            }
            FieldType::Tick => {
                nums[slot] = integral(v, &at_field, |x| amoris_sim::num::to_tick(x).is_ok())?
            }
            FieldType::Bool => {
                let b = v
                    .as_bool()
                    .ok_or_else(|| wrong_type(&at_field, "a boolean", json_type(v)))?;
                nums[slot] = if b { 1.0 } else { 0.0 };
            }
            FieldType::Entity => {
                nums[slot] = resolve_json(world, v, &at_field, also)?.map_or(0.0, EntityId::to_f64)
            }
            FieldType::Str => {
                let s = v
                    .as_str()
                    .ok_or_else(|| wrong_type(&at_field, "a string", json_type(v)))?;
                let k = amoris_script::access::str_index(schema, i);
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
/// become their ids, so a replay never resolves a name again. An id may also be one `also`
/// accepts, as in [`patch_among`].
pub fn canonical_patch(
    world: &World,
    schema: &ComponentSchema,
    patch: &Map<String, Value>,
    path: &Pointer,
    also: AlsoAlive<'_>,
) -> Result<Map<String, Value>, Problem> {
    let mut out = Map::new();
    for (key, v) in patch {
        let entity = schema
            .fields
            .iter()
            .any(|f| &*f.name == key && f.ty == FieldType::Entity);
        let v = if entity {
            match resolve_json(world, v, &path.key(key), also)? {
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

/// How many `$ref`s deep [`entity_refs`] follows a schema: a recursive type ends the walk there.
const MAX_REFS: u32 = 32;

/// The JSON pointers (RFC 6901) of `value` that hold an entity id according to `schema`, a
/// component's JSON Schema as `schemars` writes it: every place whose schema is a `$ref` to
/// `EntityId` and whose value is an integer. The walk follows the value and the schema together:
/// other `$ref`s into the root's `$defs`, the alternatives of `anyOf`, `oneOf` and `allOf` (one
/// whose tag, a property with a `const`, the value does not carry is passed over), `properties`,
/// `additionalProperties`, `prefixItems` and `items`. Map keys are not places. `project.save`
/// renumbers engine components through it (files.rs), since an engine component's decoding sees
/// no world and its entity fields have no `FieldType`.
pub fn entity_refs(schema: &Value, value: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    walk_refs(schema, schema, value, &mut String::new(), 0, &mut out);
    out
}

/// Whether `alt` is an alternative tagged for another value: it gives a property a `const` that
/// `value` does not hold there.
fn other_tag(alt: &Value, value: &Value) -> bool {
    let Some(props) = alt.get("properties").and_then(Value::as_object) else {
        return false;
    };
    props.iter().any(|(k, p)| {
        p.get("const")
            .is_some_and(|c| value.as_object().and_then(|m| m.get(k)) != Some(c))
    })
}

fn walk_refs(
    root: &Value,
    schema: &Value,
    value: &Value,
    at: &mut String,
    refs: u32,
    out: &mut BTreeSet<String>,
) {
    let Some(s) = schema.as_object() else {
        return;
    };
    if let Some(r) = s.get("$ref").and_then(Value::as_str) {
        let name = r.rsplit('/').next().unwrap_or(r);
        if name == "EntityId" {
            if value.as_u64().is_some() {
                out.insert(at.clone());
            }
        } else if refs < MAX_REFS
            && let Some(target) = root
                .get("$defs")
                .or_else(|| root.get("definitions"))
                .and_then(|d| d.get(name))
        {
            walk_refs(root, target, value, at, refs + 1, out);
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        for alt in s.get(key).and_then(Value::as_array).into_iter().flatten() {
            if !other_tag(alt, value) {
                walk_refs(root, alt, value, at, refs, out);
            }
        }
    }
    let len = at.len();
    match value {
        Value::Object(m) => {
            let props = s.get("properties").and_then(Value::as_object);
            let rest = s.get("additionalProperties");
            for (k, v) in m {
                if let Some(sub) = props.and_then(|p| p.get(k)).or(rest) {
                    at.push('/');
                    at.push_str(&k.replace('~', "~0").replace('/', "~1"));
                    walk_refs(root, sub, v, at, refs, out);
                    at.truncate(len);
                }
            }
        }
        Value::Array(a) => {
            let prefix = s.get("prefixItems").and_then(Value::as_array);
            let items = s.get("items");
            for (i, v) in a.iter().enumerate() {
                if let Some(sub) = prefix.and_then(|p| p.get(i)).or(items) {
                    at.push('/');
                    at.push_str(&i.to_string());
                    walk_refs(root, sub, v, at, refs, out);
                    at.truncate(len);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(schema: &Value, value: &Value) -> Vec<String> {
        entity_refs(schema, value).into_iter().collect()
    }

    fn id_def() -> Value {
        json!({"type": "integer", "minimum": 1})
    }

    /// A plain field, an optional one (`anyOf` with null) that is set or not, and a field that
    /// only looks like an id.
    #[test]
    fn fields_typed_as_entity_ids_are_found() {
        let schema = json!({
            "type": "object",
            "properties": {
                "to": {"description": "Who.", "$ref": "#/$defs/EntityId"},
                "mount": {"anyOf": [{"$ref": "#/$defs/EntityId"}, {"type": "null"}]},
                "count": {"type": "integer"},
            },
            "$defs": {"EntityId": id_def()},
        });
        let v = json!({"to": 4, "mount": 9, "count": 4});
        assert_eq!(refs(&schema, &v), ["/mount", "/to"]);
        let v = json!({"to": 4, "mount": null, "count": 4});
        assert_eq!(refs(&schema, &v), ["/to"]);
        assert!(refs(&schema, &json!({"count": 1})).is_empty());
    }

    /// Through another definition, a tuple (`prefixItems`), a list (`items`) and a map's values
    /// (`additionalProperties`); a key that needs escaping is escaped as a JSON pointer asks.
    #[test]
    fn nested_places_are_followed() {
        let schema = json!({
            "type": "object",
            "properties": {
                "cover": {"anyOf": [{"$ref": "#/$defs/Cover"}, {"type": "null"}]},
                "targets": {"type": "array", "items": {"$ref": "#/$defs/EntityId"}},
                "by_name": {"type": "object",
                            "additionalProperties": {"$ref": "#/$defs/EntityId"}},
            },
            "$defs": {
                "EntityId": id_def(),
                "Cover": {"type": "array",
                          "prefixItems": [{"$ref": "#/$defs/EntityId"}, {"type": "number"}]},
            },
        });
        let v = json!({"cover": [3, 5], "targets": [1, 2], "by_name": {"a/b": 7, "c~d": 8}});
        assert_eq!(
            refs(&schema, &v),
            [
                "/by_name/a~1b",
                "/by_name/c~0d",
                "/cover/0",
                "/targets/0",
                "/targets/1"
            ]
        );
        for p in refs(&schema, &v) {
            assert!(v.pointer(&p).is_some_and(Value::is_u64), "{p}");
        }
    }

    /// Enums: an externally tagged variant is found under its key, and of two internally tagged
    /// alternatives only the one the value's tag names is followed.
    #[test]
    fn enum_alternatives_follow_the_value() {
        let schema = json!({
            "oneOf": [
                {"type": "string", "enum": ["Free"]},
                {"type": "object", "required": ["Follow"],
                 "properties": {"Follow": {"$ref": "#/$defs/EntityId"}}},
                {"type": "object", "required": ["Orbit"],
                 "properties": {"Orbit": {"type": "object", "properties": {
                     "around": {"$ref": "#/$defs/EntityId"}, "turns": {"type": "integer"}}}}},
            ],
            "$defs": {"EntityId": id_def()},
        });
        assert!(refs(&schema, &json!("Free")).is_empty());
        assert_eq!(refs(&schema, &json!({"Follow": 2})), ["/Follow"]);
        assert_eq!(
            refs(&schema, &json!({"Orbit": {"around": 2, "turns": 2}})),
            ["/Orbit/around"]
        );
        let tagged = json!({
            "oneOf": [
                {"type": "object", "properties": {
                    "kind": {"type": "string", "const": "seat"},
                    "who": {"$ref": "#/$defs/EntityId"}}},
                {"type": "object", "properties": {
                    "kind": {"type": "string", "const": "count"},
                    "who": {"type": "integer"}}},
            ],
            "$defs": {"EntityId": id_def()},
        });
        assert_eq!(refs(&tagged, &json!({"kind": "seat", "who": 5})), ["/who"]);
        assert!(refs(&tagged, &json!({"kind": "count", "who": 5})).is_empty());
    }

    /// A type that refers to itself is followed as deep as the value goes, up to [`MAX_REFS`]
    /// `$ref`s, where the walk ends instead of looping.
    #[test]
    fn a_recursive_schema_ends() {
        let schema = json!({
            "$ref": "#/$defs/Node",
            "$defs": {
                "EntityId": id_def(),
                "Node": {"type": "object", "properties": {
                    "id": {"$ref": "#/$defs/EntityId"},
                    "next": {"anyOf": [{"$ref": "#/$defs/Node"}, {"type": "null"}]}}},
                "Loop": {"$ref": "#/$defs/Loop"},
            },
        });
        assert_eq!(
            refs(&schema, &json!({"id": 1, "next": {"id": 2, "next": null}})),
            ["/id", "/next/id"]
        );
        let mut deep = json!({"id": 1, "next": null});
        for _ in 0..40 {
            deep = json!({"id": 1, "next": deep});
        }
        assert_eq!(refs(&schema, &deep).len(), 32, "one id per `$ref` followed");
        let looping = json!({"$ref": "#/$defs/Loop", "$defs": schema["$defs"].clone()});
        assert!(refs(&looping, &json!(3)).is_empty());
    }
}
