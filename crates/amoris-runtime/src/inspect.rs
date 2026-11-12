//! Reading the world as the editor and agents ask for it (docs/spec/host-protocol.md 4):
//! `world.tree` (entities with their names and component names, nested by `Parent`), `world.query`
//! (rows of the entities that have every listed component, with chosen fields) and `world.schema`
//! (the registry's component schemas). Reads only; entities in ascending id order (simulation.md
//! 8), a tree's roots and each entity's children alike.

use amoris_contract::{Pointer, Problem, detail};
use amoris_sim::entity::hierarchy::{listing, nest};
use amoris_sim::{ComponentRegistry, EntityId, Hierarchy, Name};
use bevy_ecs::prelude::{Entity, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::edit::component_json;
use crate::scene::unknown_component;
use crate::values::{EntityRef, resolve};

/// `world.tree`'s parameters.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorldTreeParams {
    /// Only this entity and what is under it by `Parent` (default: every root, an entity
    /// without a live parent).
    #[serde(default)]
    pub root: Option<EntityRef>,
    /// Levels below the roots to list: 0 lists the roots alone. Default and at most 48; an
    /// entity whose children the depth left out says how many in `more`.
    #[serde(default)]
    pub depth: Option<u32>,
    /// Only entities whose name contains this text (case-insensitive), each with the entities
    /// it is under.
    #[serde(default)]
    pub filter: Option<String>,
    /// Only entities that have every one of these components, each with the entities it is
    /// under.
    #[serde(default)]
    pub with: Vec<String>,
    /// At most this many entities, counted parent first and children in id order (default: all).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `world.query`'s parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorldQueryParams {
    /// The components every row's entity has.
    pub with: Vec<String>,
    /// Fields to read, as `Component.field` or a path into it (`Transform.position.1`, `.y`).
    #[serde(default)]
    pub fields: Vec<String>,
    /// Only entities whose name contains this text (case-insensitive).
    #[serde(default)]
    pub name: Option<String>,
    /// At most this many rows, default 100.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `world.schema`'s parameters.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorldSchemaParams {
    /// One component; default: every registered one.
    #[serde(default)]
    pub component: Option<String>,
}

/// The registry's names of the components `e` has, `Name` left out.
fn component_names(world: &World, e: Entity) -> Vec<String> {
    let ent = world.entity(e);
    world
        .resource::<ComponentRegistry>()
        .entries()
        .iter()
        .filter(|c| &*c.name != "Name" && ent.contains_id(c.id))
        .map(|c| c.name.to_string())
        .collect()
}

fn name_of(world: &World, e: Entity) -> Option<String> {
    world.get::<Name>(e).map(|n| n.as_str().to_owned())
}

fn check_components(world: &World, names: &[String]) -> Result<(), Problem> {
    let reg = world.resource::<ComponentRegistry>();
    for n in names {
        if reg.get(n).is_none() {
            return Err(unknown_component(world, n));
        }
    }
    Ok(())
}

fn has_all(world: &World, e: Entity, with: &[String]) -> bool {
    let reg = world.resource::<ComponentRegistry>();
    let ent = world.entity(e);
    with.iter()
        .all(|n| reg.get(n).is_some_and(|c| ent.contains_id(c.id)))
}

fn name_matches(name: Option<&str>, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(f) => name.is_some_and(|n| n.to_lowercase().contains(&f.to_lowercase())),
    }
}

/// The live entities in ascending id order.
fn live(world: &World) -> Vec<(EntityId, Entity)> {
    world.resource::<amoris_sim::EntityIndex>().iter().collect()
}

/// `world.tree`: the entities as a tree, `[{id, name, components, children, more?}]`. Children
/// are the entities whose `Parent` names the entity; roots and children are in ascending id
/// order. The rules (roots, `depth` and `more`, what `filter`, `with` and `limit` keep) are
/// `amoris_sim::entity::hierarchy::listing`'s, which the host answering for a held game follows
/// too (amoris-server `paused.rs`).
pub fn tree(world: &World, p: &WorldTreeParams) -> Result<Value, Problem> {
    check_components(world, &p.with)?;
    let root = match &p.root {
        Some(r) => {
            let id = resolve(world, r, &Pointer::root().key("root"))?;
            amoris_sim::entity::require(world, id)?;
            Some(id)
        }
        None => None,
    };
    let index = world.resource::<amoris_sim::EntityIndex>();
    let ids: Vec<EntityId> = index.ids().collect();
    let keep = |id: EntityId| -> Result<bool, Problem> {
        Ok(index.get(id).is_some_and(|e| {
            has_all(world, e, &p.with)
                && name_matches(name_of(world, e).as_deref(), p.filter.as_deref())
        }))
    };
    let listed = listing(&ids, &Hierarchy::read(world), root, p.depth, p.limit, keep)?;
    let nodes = nest(&listed, &mut |l, children: Vec<Value>| {
        let (name, components) = match index.get(l.id) {
            Some(e) => (name_of(world, e), component_names(world, e)),
            None => (None, Vec::new()),
        };
        let mut node = json!({"id": l.id.get(), "name": name, "components": components,
                              "children": children});
        if l.more > 0 {
            node["more"] = json!(l.more);
        }
        node
    });
    Ok(Value::Array(nodes))
}

/// A field inside a component's JSON: `speed`, `position.1`, `position.y` (`x`, `y`, `z`, `w` also
/// index an array).
pub fn field_at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut at = value;
    for part in path.split('.').filter(|p| !p.is_empty()) {
        at = match at {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => {
                let i = match part {
                    "x" => 0,
                    "y" => 1,
                    "z" => 2,
                    "w" => 3,
                    n => n.parse().ok()?,
                };
                a.get(i)?
            }
            _ => return None,
        };
    }
    Some(at)
}

/// Splits `Component.field.path` into the component and the path.
pub fn split_field(spec: &str) -> (&str, &str) {
    spec.split_once('.').unwrap_or((spec, ""))
}

/// `world.query`.
pub fn query(world: &World, p: &WorldQueryParams) -> Result<Value, Problem> {
    if p.with.is_empty() {
        return Err(Problem::new(
            "request.invalid_value",
            "world.query needs at least one component in 'with'.",
            detail([("path", json!("/with"))]),
        ));
    }
    check_components(world, &p.with)?;
    let fields: Vec<(&str, &str, &String)> = p
        .fields
        .iter()
        .map(|f| {
            let (c, path) = split_field(f);
            (c, path, f)
        })
        .collect();
    let comps: Vec<String> = fields.iter().map(|(c, _, _)| (*c).to_owned()).collect();
    check_components(world, &comps)?;
    let limit = usize::try_from(p.limit.unwrap_or(100)).unwrap_or(usize::MAX);
    let mut rows = Vec::new();
    for (id, e) in live(world) {
        if rows.len() >= limit {
            break;
        }
        if !has_all(world, e, &p.with) {
            continue;
        }
        let name = name_of(world, e);
        if !name_matches(name.as_deref(), p.name.as_deref()) {
            continue;
        }
        let mut row = Map::new();
        row.insert("id".into(), json!(id.get()));
        row.insert("name".into(), json!(name));
        for (c, path, spec) in &fields {
            let v = component_json(world, e, c)
                .and_then(|v| field_at(&v, path).cloned())
                .unwrap_or(Value::Null);
            row.insert((*spec).clone(), v);
        }
        rows.push(Value::Object(row));
    }
    Ok(Value::Array(rows))
}

/// `world.schema`.
pub fn schema(world: &World, p: &WorldSchemaParams) -> Result<Value, Problem> {
    let reg = world.resource::<ComponentRegistry>();
    let info = reg.info();
    match &p.component {
        Some(name) => info
            .into_iter()
            .find(|c| &c.name == name)
            .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
            .ok_or_else(|| unknown_component(world, name)),
        None => Ok(serde_json::to_value(info).unwrap_or(Value::Null)),
    }
}
