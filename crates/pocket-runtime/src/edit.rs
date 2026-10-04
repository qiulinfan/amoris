//! `world_edit` and `world_get` (shared/contract/mcp.md 6.1): spawn, set, remove and destroy as one
//! Write at one boundary, every edit checked before any is applied, recorded in its canonical form
//! with every entity reference resolved to its id (threads.md 5.3), so a replay never resolves a
//! name again.
//!
//! The check simulates the call in order ([`Plan`]): each edit is checked against the world as the
//! edits before it leave it, so an edit naming an entity an earlier edit destroys is refused before
//! anything changes (charter 3.4), and a second `set` of a component starts from the first one's
//! result. Once the check passes, applying cannot fail but through an engine bug (`sim.internal`).
//! An edit cannot name an entity an earlier edit of the same call spawns: its id is not known until
//! it is applied (threads.md 13, Slice 1: `world_edit` as built).
//!
//! Every applied call also yields its inverse (docs/spec/server.md, history): the edits that bring
//! the world back, computed from the call's net effect on each entity it touched: the entities it
//! spawned or revived are destroyed, those it destroyed are revived whole under their ids, and the
//! components it set or removed on the others get their values from before the call back.
//! `world.edit` is the protocol's form of the same call (`{ops: [{set: {...}}], label}`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pocket_contract::{Pointer, Problem, detail};
use pocket_script::ComponentAccess;
use pocket_sim::registry::{ComponentSchema, FieldType, ProjectValues};
use pocket_sim::{Boundary, EntityId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::engine::{Inserter, engine_component, merge};
use crate::scene::{
    Prefab, PreparedSpawn, bad_value, prepare, project_component, unknown_component,
};
use crate::values::{self, EntityRef, resolve};

/// `world_edit`'s parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorldEditParams {
    /// 1 to 64 edits, applied in order at one boundary.
    pub edits: Vec<Edit>,
}

/// One edit.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    /// An entity with components, from a prefab or not.
    Spawn {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        prefab: Option<Prefab>,
        #[serde(default)]
        components: Map<String, Value>,
    },
    /// Writes the listed fields, adding the component when missing; other fields keep their
    /// values. A key may be a path into the component (`position.1`).
    Set {
        entity: EntityRef,
        component: String,
        value: Map<String, Value>,
        /// Write the value whole (fields not given take their defaults) instead of merging.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        replace: bool,
    },
    Remove {
        entity: EntityRef,
        component: String,
    },
    Destroy {
        entity: EntityRef,
    },
    /// Spawns a destroyed entity again under its id with these components (an undo of `destroy`).
    Revive {
        entity: u64,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        components: Map<String, Value>,
    },
}

/// `world.edit`'s parameters (docs/spec/host-protocol.md 4): one transaction.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorldEditOps {
    /// 1 to 64 edits, applied in order at one boundary, all or none.
    pub ops: Vec<EditOp>,
    /// The history's label; default: a summary of the edits.
    #[serde(default)]
    pub label: Option<String>,
}

/// One edit in the protocol's form.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EditOp {
    /// An entity with components, from a prefab or not.
    Spawn(SpawnOp),
    /// Writes the listed fields (keys may be paths: `position.1`), adding the component if missing.
    Set(SetOp),
    Remove(RemoveOp),
    Destroy(DestroyOp),
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpawnOp {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prefab: Option<Prefab>,
    /// Components by name; each overrides the prefab's field by field.
    #[serde(default)]
    pub components: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetOp {
    /// An id, a name, or `Name#id`.
    pub entity: EntityRef,
    pub component: String,
    pub value: Map<String, Value>,
    /// Write the value whole instead of merging fields.
    #[serde(default)]
    pub replace: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveOp {
    pub entity: EntityRef,
    pub component: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DestroyOp {
    pub entity: EntityRef,
}

impl From<EditOp> for Edit {
    fn from(op: EditOp) -> Edit {
        match op {
            EditOp::Spawn(o) => Edit::Spawn {
                name: o.name,
                prefab: o.prefab,
                components: o.components,
            },
            EditOp::Set(o) => Edit::Set {
                entity: o.entity,
                component: o.component,
                value: o.value,
                replace: o.replace,
            },
            EditOp::Remove(o) => Edit::Remove {
                entity: o.entity,
                component: o.component,
            },
            EditOp::Destroy(o) => Edit::Destroy { entity: o.entity },
        }
    }
}

fn entity_text(r: &EntityRef) -> String {
    match r {
        EntityRef::Id(n) => format!("#{n}"),
        EntityRef::Name(s) => s.clone(),
    }
}

/// A short label for a call's edits: the first edit, and how many more.
pub fn label_of(edits: &[Edit]) -> String {
    let first = match edits.first() {
        Some(Edit::Spawn { name, prefab, .. }) => {
            let what = name.clone().or_else(|| {
                prefab.as_ref().and_then(|p| {
                    serde_json::to_value(p)
                        .ok()
                        .and_then(|v| v.get("kind").and_then(Value::as_str).map(str::to_owned))
                })
            });
            format!("spawn {}", what.unwrap_or_else(|| "entity".into()))
        }
        Some(Edit::Set {
            entity, component, ..
        }) => format!("set {component} on {}", entity_text(entity)),
        Some(Edit::Remove { entity, component }) => {
            format!("remove {component} from {}", entity_text(entity))
        }
        Some(Edit::Destroy { entity }) => format!("destroy {}", entity_text(entity)),
        Some(Edit::Revive { entity, name, .. }) => {
            format!(
                "revive {}",
                name.clone().unwrap_or_else(|| format!("#{entity}"))
            )
        }
        None => "no edit".into(),
    };
    match edits.len() {
        0 | 1 => first,
        n => format!("{first} (+{})", n - 1),
    }
}

/// What an applied call gives: the result, the canonical recorded parameters, the inverse edits
/// and the ids it spawned.
pub struct Applied {
    pub result: Value,
    pub canonical: Value,
    pub inverse: Vec<Edit>,
}

/// `world_get`'s parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorldGetParams {
    pub entity: EntityRef,
    /// Components to read; empty: every one the entity has.
    #[serde(default)]
    pub components: Vec<String>,
}

enum Checked {
    Spawn(PreparedSpawn),
    Set(EntityId, String, Setter),
    Remove(EntityId, String, Remover),
    Destroy(EntityId),
    Revive(EntityId, PreparedSpawn),
}

enum Setter {
    Engine(Inserter),
    Project(Arc<dyn ComponentAccess>, ProjectValues),
}

enum Remover {
    Engine(fn(&mut bevy_ecs::world::EntityWorldMut<'_>)),
    Project(Arc<dyn ComponentAccess>),
}

/// A component as the edits checked so far leave it.
enum Pending {
    /// An engine component's whole JSON value.
    Engine(Value),
    Project(ProjectValues),
    Removed,
}

/// The world as the edits checked so far leave it: the entities they destroy (with the destroying
/// edit's index) and the components they set or remove.
#[derive(Default)]
struct Plan {
    destroyed: BTreeMap<EntityId, usize>,
    pending: BTreeMap<(EntityId, String), Pending>,
    revived: BTreeSet<EntityId>,
}

/// `sim.entity_not_found` for an entity an earlier edit of the same call destroys.
fn destroyed_earlier(id: EntityId, by: usize, at: &Pointer) -> Problem {
    Problem::new(
        "sim.entity_not_found",
        format!(
            "Entity {} is destroyed by edit {by} of this call, so '{}' cannot name it; nothing was \
             applied.",
            id.get(),
            at.dotted()
        ),
        detail([
            ("id", json!(id.get())),
            ("path", json!(at.to_string())),
            ("destroyed_by", json!(by)),
        ]),
    )
}

impl Plan {
    /// `id`, unless an earlier edit destroys it.
    fn live(&self, id: EntityId, at: &Pointer) -> Result<EntityId, Problem> {
        match self.destroyed.get(&id) {
            Some(by) => Err(destroyed_earlier(id, *by, at)),
            None => Ok(id),
        }
    }

    /// Refuses a project component's checked patch whose entity fields name an entity an earlier
    /// edit destroys (`canonical` holds ids).
    fn references(
        &self,
        schema: &ComponentSchema,
        canonical: &Map<String, Value>,
        at: &Pointer,
    ) -> Result<(), Problem> {
        for f in schema.fields.iter().filter(|f| f.ty == FieldType::Entity) {
            if let Some(id) = canonical
                .get(&*f.name)
                .and_then(Value::as_u64)
                .and_then(EntityId::new)
            {
                self.live(id, &at.key(&f.name))?;
            }
        }
        Ok(())
    }
}

fn check_set(
    b: &Boundary<'_>,
    plan: &mut Plan,
    id: EntityId,
    component: &str,
    value: &Map<String, Value>,
    replace: bool,
    at: &Pointer,
) -> Result<(Setter, Map<String, Value>), Problem> {
    let world = b.world();
    let e = world.entity(
        b.entity(id)
            .ok_or_else(|| pocket_sim::entity::entity_not_found(id))?,
    );
    let key = (id, component.to_owned());
    if let Some(c) = engine_component(component) {
        let mut whole = match plan.pending.get(&key) {
            _ if replace => Value::Object(Map::new()),
            Some(Pending::Engine(v)) => v.clone(),
            Some(Pending::Removed) => Value::Object(Map::new()),
            _ => (c.get)(&e).unwrap_or_else(|| Value::Object(Map::new())),
        };
        merge(&mut whole, value).map_err(|why| bad_value(&at.key("value"), &why))?;
        let inserter = (c.decode)(&whole, component)?;
        plan.pending.insert(key, Pending::Engine(whole));
        return Ok((Setter::Engine(inserter), value.clone()));
    }
    let (schema, access) =
        project_component(world, component).ok_or_else(|| unknown_component(world, component))?;
    let base = match plan.pending.get(&key) {
        _ if replace => pocket_script::access::defaults(&schema),
        Some(Pending::Project(v)) => v.clone(),
        Some(Pending::Removed) => pocket_script::access::defaults(&schema),
        _ => access
            .read(&e)
            .unwrap_or_else(|| pocket_script::access::defaults(&schema)),
    };
    let v = values::patch(world, &schema, &base, value, &at.key("value"))?;
    let canonical = values::canonical_patch(world, &schema, value, &at.key("value"))?;
    plan.references(&schema, &canonical, &at.key("value"))?;
    plan.pending.insert(key, Pending::Project(v.clone()));
    Ok((Setter::Project(access, v), canonical))
}

fn check_remove(b: &Boundary<'_>, component: &str) -> Result<Remover, Problem> {
    if let Some(c) = engine_component(component) {
        return Ok(Remover::Engine(c.remove));
    }
    let (_, access) = project_component(b.world(), component)
        .ok_or_else(|| unknown_component(b.world(), component))?;
    Ok(Remover::Project(access))
}

/// Applies `world_edit` at a boundary: checks every edit against the world as the edits before it
/// leave it, then applies them in order. Returns the result, the canonical recorded form of the
/// parameters and the inverse edits. A refused call changes nothing.
pub fn world_edit(b: &mut Boundary<'_>, params: &WorldEditParams) -> Result<Applied, Problem> {
    if params.edits.is_empty() || params.edits.len() > 64 {
        return Err(bad_value(
            &Pointer::root().key("edits"),
            "give 1 to 64 edits",
        ));
    }
    let mut plan = Plan::default();
    let mut checked = Vec::with_capacity(params.edits.len());
    let mut canonical = Vec::with_capacity(params.edits.len());
    for (i, edit) in params.edits.iter().enumerate() {
        let at = Pointer::root().key("edits").index(i);
        let at_entity = at.key("entity");
        let r = |plan: &Plan, r: &EntityRef| {
            let id = resolve(b.world(), r, &at_entity)?;
            plan.live(id, &at_entity)
        };
        match edit {
            Edit::Spawn {
                name,
                prefab,
                components,
            } => {
                checked.push(Checked::Spawn(prepare(
                    b.world(),
                    name.as_deref(),
                    prefab.as_ref(),
                    components,
                    &at,
                )?));
                let mut resolved = Map::new();
                for (cname, v) in components {
                    let v = match (project_component(b.world(), cname), v.as_object()) {
                        (Some((schema, _)), Some(patch)) => {
                            let at = at.key("components").key(cname);
                            let c = values::canonical_patch(b.world(), &schema, patch, &at)?;
                            plan.references(&schema, &c, &at)?;
                            Value::Object(c)
                        }
                        _ => v.clone(),
                    };
                    resolved.insert(cname.clone(), v);
                }
                canonical.push(json!({"op": "spawn", "name": name, "prefab": prefab,
                                      "components": resolved}));
            }
            Edit::Set {
                entity,
                component,
                value,
                replace,
            } => {
                let id = r(&plan, entity)?;
                let (setter, value) = check_set(b, &mut plan, id, component, value, *replace, &at)?;
                checked.push(Checked::Set(id, component.clone(), setter));
                let mut c = json!({"op": "set", "entity": id.get(), "component": component,
                                   "value": value});
                if *replace {
                    c["replace"] = json!(true);
                }
                canonical.push(c);
            }
            Edit::Remove { entity, component } => {
                let id = r(&plan, entity)?;
                checked.push(Checked::Remove(
                    id,
                    component.clone(),
                    check_remove(b, component)?,
                ));
                plan.pending
                    .insert((id, component.clone()), Pending::Removed);
                canonical.push(json!({"op": "remove", "entity": id.get(), "component": component}));
            }
            Edit::Destroy { entity } => {
                let id = r(&plan, entity)?;
                plan.destroyed.insert(id, i);
                checked.push(Checked::Destroy(id));
                canonical.push(json!({"op": "destroy", "entity": id.get()}));
            }
            Edit::Revive {
                entity,
                name,
                components,
            } => {
                let id = check_revive(b, &plan, *entity, &at_entity)?;
                plan.revived.insert(id);
                checked.push(Checked::Revive(
                    id,
                    prepare(b.world(), name.as_deref(), None, components, &at)?,
                ));
                canonical.push(json!({"op": "revive", "entity": id.get(), "name": name,
                                      "components": components}));
            }
        }
    }
    let before = Before::capture(b.world(), &checked);
    let mut results = Vec::with_capacity(checked.len());
    let mut spawned = Vec::new();
    for c in checked {
        let r = apply(b, c).map_err(|p| applying_failed(b, &p))?;
        if r["op"] == "spawned"
            && let Some(id) = r["id"].as_u64().and_then(EntityId::new)
        {
            spawned.push(id);
        }
        results.push(r);
    }
    let inverse = before.inverse(b.world(), &spawned);
    let ids: Vec<u64> = spawned.iter().map(|i| i.get()).collect();
    Ok(Applied {
        result: json!({"tick": b.tick().0, "applied": results.len(), "spawned": ids,
                       "results": results}),
        canonical: json!({"edits": canonical}),
        inverse,
    })
}

/// A revival's id: allocated, not live, not revived earlier in the call.
fn check_revive(b: &Boundary<'_>, plan: &Plan, n: u64, at: &Pointer) -> Result<EntityId, Problem> {
    let next = b.world().resource::<pocket_sim::EntityAllocator>().next();
    let id = EntityId::new(n)
        .filter(|id| id.get() < next)
        .ok_or_else(|| pocket_sim::entity::entity_id_invalid(&n.to_string(), next))?;
    if b.entity(id).is_some() || plan.revived.contains(&id) {
        let mut p = pocket_sim::entity::entity_alive(id);
        p.detail.insert("path".into(), json!(at.to_string()));
        return Err(p);
    }
    Ok(id)
}

/// One component of a live entity as JSON, engine or project; `None` when it has none.
pub fn component_json(
    world: &bevy_ecs::prelude::World,
    e: bevy_ecs::prelude::Entity,
    name: &str,
) -> Option<Value> {
    let ent = world.entity(e);
    if let Some(c) = engine_component(name) {
        (c.get)(&ent)
    } else if let Some((schema, access)) = project_component(world, name) {
        access.read(&ent).map(|v| values::to_json(&schema, &v))
    } else {
        None
    }
}

/// Every component of a live entity as JSON by name (its `Name` left out).
pub fn components_json(
    world: &bevy_ecs::prelude::World,
    e: bevy_ecs::prelude::Entity,
) -> Map<String, Value> {
    let mut out = Map::new();
    for entry in world.resource::<pocket_sim::ComponentRegistry>().entries() {
        if let Some(v) = component_json(world, e, &entry.name) {
            out.insert(entry.name.to_string(), v);
        }
    }
    out
}

/// What a call's inverse needs from the world before it: the entities it destroys whole, the
/// components it sets or removes on live entities, and the ids it revives.
struct Before {
    destroyed: Vec<(EntityId, Option<String>, Map<String, Value>)>,
    touched: BTreeMap<(EntityId, String), Option<Value>>,
    revived: Vec<EntityId>,
}

impl Before {
    fn capture(world: &bevy_ecs::prelude::World, checked: &[Checked]) -> Before {
        let mut out = Before {
            destroyed: Vec::new(),
            touched: BTreeMap::new(),
            revived: Vec::new(),
        };
        let live = |id: EntityId| pocket_sim::entity::entity(world, id);
        for c in checked {
            match c {
                Checked::Set(id, comp, _) | Checked::Remove(id, comp, _) => {
                    if let Some(e) = live(*id) {
                        out.touched
                            .entry((*id, comp.clone()))
                            .or_insert_with(|| component_json(world, e, comp));
                    }
                }
                Checked::Destroy(id) => {
                    if let Some(e) = live(*id)
                        && !out.destroyed.iter().any(|(d, _, _)| d == id)
                    {
                        let name = world
                            .get::<pocket_sim::Name>(e)
                            .map(|n| n.as_str().to_owned());
                        out.destroyed.push((*id, name, components_json(world, e)));
                    }
                }
                Checked::Revive(id, _) => out.revived.push(*id),
                Checked::Spawn(_) => {}
            }
        }
        out
    }

    /// The edits that undo the call, against the world it left.
    fn inverse(self, world: &bevy_ecs::prelude::World, spawned: &[EntityId]) -> Vec<Edit> {
        let live = |id: EntityId| pocket_sim::entity::entity(world, id);
        let mut inv = Vec::new();
        for id in spawned.iter().chain(&self.revived) {
            if live(*id).is_some() {
                inv.push(Edit::Destroy {
                    entity: EntityRef::Id(id.get()),
                });
            }
        }
        let gone: BTreeSet<EntityId> = self.destroyed.iter().map(|(id, _, _)| *id).collect();
        for (id, name, components) in self.destroyed {
            inv.push(Edit::Revive {
                entity: id.get(),
                name,
                components,
            });
        }
        for ((id, component), prev) in self.touched {
            if gone.contains(&id) {
                continue;
            }
            let Some(e) = live(id) else { continue };
            match prev {
                Some(Value::Object(value)) => inv.push(Edit::Set {
                    entity: EntityRef::Id(id.get()),
                    component,
                    value,
                    replace: true,
                }),
                Some(_) => {}
                None => {
                    if component_json(world, e, &component).is_some() {
                        inv.push(Edit::Remove {
                            entity: EntityRef::Id(id.get()),
                            component,
                        });
                    }
                }
            }
        }
        inv
    }
}

/// `sim.internal`: an edit that passed the check failed to apply, which the check rules out; the
/// edits before it stay applied. An engine bug, never a refusal.
fn applying_failed(b: &Boundary<'_>, cause: &Problem) -> Problem {
    Problem::new(
        "sim.internal",
        format!(
            "world_edit passed its check but failed to apply at boundary {}: {}. This is an \
             engine bug.",
            b.tick().0,
            cause.message
        ),
        detail([
            ("tick", json!(b.tick().0)),
            ("phase", json!("boundary")),
            ("system", json!("world_edit")),
            ("message", json!(cause.message)),
        ]),
    )
}

fn apply(b: &mut Boundary<'_>, c: Checked) -> Result<Value, Problem> {
    let live = |b: &Boundary<'_>, id| {
        b.entity(id)
            .ok_or_else(|| pocket_sim::entity::entity_not_found(id))
    };
    Ok(match c {
        Checked::Spawn(p) => json!({"op": "spawned", "id": p.spawn(b)?.get()}),
        Checked::Set(id, _, s) => {
            let e = live(b, id)?;
            let mut ent = b.world_mut().entity_mut(e);
            match s {
                Setter::Engine(i) => i.insert(&mut ent),
                Setter::Project(access, v) => {
                    if !access.write(&mut ent, &v) {
                        access.insert(&mut ent, v);
                    }
                }
            }
            json!({"op": "set", "entity": id.get()})
        }
        Checked::Remove(id, _, r) => {
            let e = live(b, id)?;
            let mut ent = b.world_mut().entity_mut(e);
            match r {
                Remover::Engine(f) => f(&mut ent),
                Remover::Project(access) => access.remove(&mut ent),
            }
            json!({"op": "removed", "entity": id.get()})
        }
        Checked::Destroy(id) => {
            let gone = b.despawn(id)?;
            json!({"op": "destroyed", "count": u32::from(gone)})
        }
        Checked::Revive(id, p) => {
            p.revive(b, id)?;
            json!({"op": "revived", "id": id.get()})
        }
    })
}

/// `world_get`: the entity's components as JSON, engine and project alike.
pub fn world_get(
    world: &bevy_ecs::prelude::World,
    params: &WorldGetParams,
) -> Result<Value, Problem> {
    let id = resolve(world, &params.entity, &Pointer::root().key("entity"))?;
    let e = pocket_sim::entity::require(world, id)?;
    let ent = world.entity(e);
    let mut out = Map::new();
    let wanted: Vec<String> = if params.components.is_empty() {
        world
            .resource::<pocket_sim::ComponentRegistry>()
            .entries()
            .iter()
            .map(|e| e.name.to_string())
            .collect()
    } else {
        params.components.clone()
    };
    for name in &wanted {
        let v = if let Some(c) = engine_component(name) {
            (c.get)(&ent)
        } else if let Some((schema, access)) = project_component(world, name) {
            access.read(&ent).map(|v| values::to_json(&schema, &v))
        } else if name == "Name" {
            ent.get::<pocket_sim::Name>().map(|n| json!(n.as_str()))
        } else if params.components.is_empty() {
            None
        } else {
            return Err(unknown_component(world, name));
        };
        if let Some(v) = v {
            out.insert(name.clone(), v);
        }
    }
    let name = ent.get::<pocket_sim::Name>().map(|n| n.as_str().to_owned());
    Ok(json!({"id": id.get(), "name": name, "components": out}))
}
