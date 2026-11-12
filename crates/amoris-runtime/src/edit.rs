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
//! The hierarchy (docs/spec/server.md 3.1): a destroy takes the entity and every entity under it
//! by `Parent`, and the plan marks them all destroyed, so a later edit of the call naming one of
//! them is refused like one naming the destroyed entity. A `Parent` an edit writes names an entity
//! that is alive as the earlier edits leave the world, and not the entity itself; whether it
//! closes a loop is judged once, on the hierarchy the whole call leaves, so a call may pass
//! through a loop (swapping a parent with its child takes two sets) but never leave one
//! (`sim.parent_cycle`).
//! `Name` is the entity's name and the one component a `set` writes from a string
//! ([`SetValue::Name`]; docs/spec/server.md 3.1, Names): a `set` of it renames the entity or names
//! one that had no name, and a `remove` leaves it unnamed. Names are looked up as the edits before
//! leave them too, so after a rename a later edit of the call finds the entity by its new name and
//! no longer by the old one, as its `entity` or through an entity field.
//!
//! Every applied call also yields its inverse (docs/spec/server.md, history): the edits that bring
//! the world back, computed from the call's net effect on each entity it touched, in the order
//! that applies as one call: first the entities it destroyed are revived whole under their ids, by
//! one `revive` edit however many they are; then the components it set or removed on the others
//! get their values from before the call back; then the entities it spawned or revived are
//! destroyed, the topmost of them alone, since a destroy takes what is under it.
//! `world.edit` is the protocol's form of the same call (`{ops: [{set: {...}}], label}`).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use amoris_contract::codes::wrong_type;
use amoris_contract::{Pointer, Problem, detail};
use amoris_script::ComponentAccess;
use amoris_sim::entity::hierarchy;
use amoris_sim::registry::{ComponentSchema, FieldType, ProjectValues};
use amoris_sim::{Boundary, EntityId, Hierarchy, Name, Parent, Persisted};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::engine::{Inserter, engine_component, merge};
use crate::scene::{
    Prefab, PreparedSpawn, bad_value, not_editable, prepare_among, project_component,
    unknown_component,
};
use crate::values::{self, EntityRef, at_path, resolve};

/// The component that is an entity's name.
const NAME: &str = <Name as Persisted>::NAME;

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
    /// values. A key may be a path into the component (`position.1`). For `Name` the value is the
    /// name, a string.
    Set {
        entity: EntityRef,
        component: String,
        value: SetValue,
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
    /// Spawns a destroyed entity again under its id with these components (an undo of `destroy`),
    /// and with it the `others`: the entities the destroy took from under it. They are checked
    /// together, so each one's `Parent` may name another of them, in any order.
    Revive {
        entity: u64,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        components: Map<String, Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        others: Vec<Revived>,
    },
}

/// One more entity of a `revive`: a destroyed entity's id, its name and its components.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Revived {
    pub entity: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub components: Map<String, Value>,
}

/// What a `set` writes: an object of the component's fields or, for `Name` alone, the name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SetValue {
    /// The fields to write; a key may be a path into the component (`position.1`).
    Fields(Map<String, Value>),
    /// The entity's name, when the component is `Name`: at most 64 bytes of UTF-8.
    Name(String),
}

impl From<Map<String, Value>> for SetValue {
    fn from(fields: Map<String, Value>) -> SetValue {
        SetValue::Fields(fields)
    }
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
    /// Consecutive edits with the same group from one caller are one undo entry (a drag streams
    /// its values and undoes at once); every edit is still applied and recorded on its own.
    #[serde(default)]
    pub group: Option<String>,
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
    /// The fields to write, or for `Name` the name as a string (a rename).
    pub value: SetValue,
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
        Some(Edit::Revive {
            entity,
            name,
            others,
            ..
        }) => {
            let what = name.clone().unwrap_or_else(|| format!("#{entity}"));
            match others.len() {
                0 => format!("revive {what}"),
                n => format!("revive {what} and {n} under it"),
            }
        }
        None => "no edit".into(),
    };
    match edits.len() {
        0 | 1 => first,
        n => format!("{first} (+{})", n - 1),
    }
}

/// A refusal of `world.edit` with its paths as the caller wrote them: `/edits/0/value` (the
/// canonical form's) becomes `/ops/0/set/value`, and `edits[0].value` in the message
/// `ops[0].set.value`.
pub fn ops_paths(mut p: Problem, edits: &[Edit]) -> Problem {
    let op = |i: usize| match edits.get(i) {
        Some(Edit::Spawn { .. }) => "spawn",
        Some(Edit::Set { .. }) => "set",
        Some(Edit::Remove { .. }) => "remove",
        Some(Edit::Destroy { .. }) => "destroy",
        Some(Edit::Revive { .. }) | None => "revive",
    };
    for i in 0..edits.len() {
        let (slash, dots) = (format!("/edits/{i}"), format!("edits[{i}]"));
        let (to_slash, to_dots) = (format!("/ops/{i}/{}", op(i)), format!("ops[{i}].{}", op(i)));
        for v in p.detail.values_mut() {
            let under = |s: &str, head: &str| {
                s.strip_prefix(head)
                    .is_some_and(|r| r.is_empty() || r.starts_with(['/', '.', '[']))
            };
            if let Value::String(s) = v
                && (under(s, &slash) || under(s, &dots))
            {
                *s = s
                    .replacen(&slash, &to_slash, 1)
                    .replacen(&dots, &to_dots, 1);
            }
        }
        p.message = p
            .message
            .replace(&format!("{dots}."), &format!("{to_dots}."))
            .replace(&format!("{dots}'"), &format!("{to_dots}'"));
    }
    p
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
    /// Components to read; empty: every one the entity has (none when only fields are asked
    /// for). An entry with a dot is a field (`Boat.rudder`).
    #[serde(default)]
    pub components: Vec<String>,
    /// Fields to read alone, as `Component.field` or a path into it (`Transform.position.y`),
    /// answered under `fields`.
    #[serde(default)]
    pub fields: Vec<String>,
}

enum Checked {
    Spawn(PreparedSpawn),
    Set(EntityId, String, Setter),
    Remove(EntityId, String, Remover),
    /// The entity, and the entities under it as the edits before leave the hierarchy.
    Destroy(EntityId, Vec<EntityId>),
    /// The entities of one `revive`, the edit's own first.
    Revive(Vec<(EntityId, PreparedSpawn)>),
}

enum Setter {
    Engine(Inserter),
    Project(Arc<dyn ComponentAccess>, ProjectValues),
    Name(Name),
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
    /// The entity's name.
    Name(String),
    Removed,
}

/// The world as the edits checked so far leave it: the entities they destroy (with the destroying
/// edit's index), the components they set or remove, the entities they revive, and the hierarchy.
#[derive(Default)]
struct Plan {
    destroyed: BTreeMap<EntityId, usize>,
    pending: BTreeMap<(EntityId, String), Pending>,
    revived: BTreeSet<EntityId>,
    /// The hierarchy, once an edit needed it (see [`Plan::hierarchy`]).
    hierarchy: Option<Hierarchy>,
    /// The entities whose `Parent` an edit of the call wrote and that still have it, each with
    /// where the last such edit gave the parent: what the call's last check judges.
    parented: BTreeMap<EntityId, Pointer>,
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

    /// The hierarchy as the edits checked so far leave it. It is read from the world by the first
    /// edit that bears on it (a destroy, a revive, or an edit naming `Parent`) before that edit
    /// takes effect here: the world does not change while a call is checked and no earlier edit
    /// changed the hierarchy, so the world's rows are it. Every such edit then keeps it in step.
    fn hierarchy(&mut self, b: &Boundary<'_>) -> &mut Hierarchy {
        self.hierarchy
            .get_or_insert_with(|| Hierarchy::read(b.world()))
    }

    /// Refuses a `Parent` naming `parent` unless it is alive as the edits checked so far leave the
    /// world: live or revived by an earlier edit, and not destroyed by one.
    fn parent_ok(&self, b: &Boundary<'_>, parent: EntityId, at: &Pointer) -> Result<(), Problem> {
        if let Some(by) = self.destroyed.get(&parent) {
            return Err(destroyed_earlier(parent, *by, at));
        }
        if self.revived.contains(&parent) {
            return Ok(());
        }
        match amoris_sim::entity::require(b.world(), parent) {
            Ok(_) => Ok(()),
            Err(p) => Err(at_path(p, at)),
        }
    }

    /// Follows a `Parent` written on the live entity `id` (a set, or a revived entity's): its
    /// parent, read from the pending value, is not `id` itself and is alive.
    fn reparent(&mut self, b: &Boundary<'_>, id: EntityId, at: &Pointer) -> Result<(), Problem> {
        let key = (id, Parent::NAME.to_owned());
        let Some(Pending::Engine(whole)) = self.pending.get(&key) else {
            return Ok(());
        };
        let Some(parent) = parent_in(whole) else {
            return Ok(());
        };
        if parent == id {
            return Err(at_path(hierarchy::parent_cycle(id, parent, &[id]), at));
        }
        self.parent_ok(b, parent, at)?;
        self.hierarchy(b).set(id, parent);
        self.parented.insert(id, at.clone());
        Ok(())
    }

    /// The call's last check: no `Parent` it wrote closes a loop in the hierarchy it leaves.
    fn loops(&self) -> Result<(), Problem> {
        let Some(h) = &self.hierarchy else {
            return Ok(());
        };
        for (id, at) in &self.parented {
            if self.destroyed.contains_key(id) {
                continue;
            }
            if let Some(parent) = h.parent(*id)
                && let Some(chain) = h.loop_through(*id, parent)
            {
                let mut p = at_path(hierarchy::parent_cycle(*id, parent, &chain), at);
                p.message.push_str(" Nothing was applied.");
                return Err(p);
            }
        }
        Ok(())
    }

    /// The names the edits checked so far set (`Some`) or remove (`None`), by entity.
    fn renamed(&self) -> impl Iterator<Item = (EntityId, Option<&str>)> {
        self.pending
            .iter()
            .filter(|((_, component), _)| component == NAME)
            .filter_map(|((id, _), pending)| match pending {
                Pending::Name(name) => Some((*id, Some(name.as_str()))),
                Pending::Removed => Some((*id, None)),
                Pending::Engine(_) | Pending::Project(_) => None,
            })
    }

    /// The live entity `r` names in the world as the edits checked so far leave it (`at` is where
    /// the call names it): a name is looked up among the names those edits set and removed, so a
    /// renamed entity is found by its new name alone, and an entity one of them destroys is
    /// refused. An entity they destroy keeps its name until the call is applied: naming it says
    /// which edit destroys it, and a name it shares stays ambiguous.
    fn resolve(
        &self,
        world: &bevy_ecs::prelude::World,
        r: &EntityRef,
        at: &Pointer,
    ) -> Result<EntityId, Problem> {
        let found = match r {
            EntityRef::Name(s) if self.renamed().next().is_some() => {
                let mut names: BTreeMap<EntityId, String> =
                    values::named(world).into_iter().collect();
                for (id, name) in self.renamed() {
                    match name {
                        Some(name) => names.insert(id, name.to_owned()),
                        None => names.remove(&id),
                    };
                }
                let names: Vec<(EntityId, String)> = names.into_iter().collect();
                values::resolve_name(&names, s, at)
            }
            _ => resolve(world, r, at),
        };
        self.live(found.map_err(|p| at_path(p, at))?, at)
    }

    /// `patch` (a project component's fields, at `at`) with every entity field given by name
    /// resolved to its id as [`Plan::resolve`] does; `patch` itself while no edit checked so far
    /// changed a name, since the world's names then hold.
    fn entity_fields<'p>(
        &self,
        world: &bevy_ecs::prelude::World,
        schema: &ComponentSchema,
        patch: &'p Map<String, Value>,
        at: &Pointer,
    ) -> Result<Cow<'p, Map<String, Value>>, Problem> {
        if self.renamed().next().is_none() {
            return Ok(Cow::Borrowed(patch));
        }
        let mut resolved = patch.clone();
        for f in schema.fields.iter().filter(|f| f.ty == FieldType::Entity) {
            if let Some(Value::String(name)) = patch.get(&*f.name) {
                let r = EntityRef::Name(name.clone());
                let id = self.resolve(world, &r, &at.key(&f.name))?;
                resolved.insert(f.name.to_string(), json!(id.get()));
            }
        }
        Ok(Cow::Owned(resolved))
    }

    /// A spawn's `components` (at `at`) with the entity fields of its project components resolved
    /// as [`Plan::entity_fields`] does.
    fn spawn_fields<'p>(
        &self,
        world: &bevy_ecs::prelude::World,
        components: &'p Map<String, Value>,
        at: &Pointer,
    ) -> Result<Cow<'p, Map<String, Value>>, Problem> {
        if self.renamed().next().is_none() {
            return Ok(Cow::Borrowed(components));
        }
        let mut resolved = components.clone();
        for (cname, value) in components {
            if let (Some((schema, _)), Some(patch)) =
                (project_component(world, cname), value.as_object())
            {
                let patch = self.entity_fields(world, &schema, patch, &at.key(cname))?;
                resolved.insert(cname.clone(), Value::Object(patch.into_owned()));
            }
        }
        Ok(Cow::Owned(resolved))
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

/// The entity a JSON id names.
fn parent_of(id: &Value) -> Option<EntityId> {
    id.as_u64().and_then(EntityId::new)
}

/// The parent a `Parent`'s JSON value names.
fn parent_in(value: &Value) -> Option<EntityId> {
    value.get("parent").and_then(parent_of)
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
            .ok_or_else(|| amoris_sim::entity::entity_not_found(id))?,
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
    let (schema, access) = project_component(world, component)
        .ok_or_else(|| not_editable(world, component, &at.key("component")))?;
    let named = plan.entity_fields(world, &schema, value, &at.key("value"))?;
    let value = &*named;
    let base = match plan.pending.get(&key) {
        _ if replace => amoris_script::access::defaults(&schema),
        Some(Pending::Project(v)) => v.clone(),
        Some(Pending::Removed) => amoris_script::access::defaults(&schema),
        _ => access
            .read(&e)
            .unwrap_or_else(|| amoris_script::access::defaults(&schema)),
    };
    let revived = |id: EntityId| plan.revived.contains(&id);
    let at_value = at.key("value");
    let v = values::patch_among(world, &schema, &base, value, &at_value, &revived)?;
    let canonical = values::canonical_patch(world, &schema, value, &at_value, &revived)?;
    plan.references(&schema, &canonical, &at_value)?;
    plan.pending.insert(key, Pending::Project(v.clone()));
    Ok((Setter::Project(access, v), canonical))
}

/// A `set` of `Name`: the name as a string of at most 64 bytes of UTF-8 (`Name::new`), written
/// whole, on an entity with a name or without. Later edits of the call find the entity by it.
fn check_name(
    plan: &mut Plan,
    id: EntityId,
    value: &SetValue,
    at_value: &Pointer,
) -> Result<(Setter, SetValue), Problem> {
    let SetValue::Name(text) = value else {
        return Err(wrong_type(at_value, "string", "object"));
    };
    let name = Name::new(text.as_str()).map_err(|p| at_path(p, at_value))?;
    plan.pending
        .insert((id, NAME.to_owned()), Pending::Name(text.clone()));
    Ok((Setter::Name(name), value.clone()))
}

/// A `set` (at `at`) whose `value` is a string for a component other than `Name`: its fields go
/// in an object, or it is no component a `set` can write.
fn not_fields(world: &bevy_ecs::prelude::World, component: &str, at: &Pointer) -> Problem {
    if engine_component(component).is_some() || project_component(world, component).is_some() {
        wrong_type(&at.key("value"), "object", "string")
    } else {
        not_editable(world, component, &at.key("component"))
    }
}

fn check_remove(b: &Boundary<'_>, component: &str, at: &Pointer) -> Result<Remover, Problem> {
    // The entity goes on unnamed, as one spawned without a name.
    if component == NAME {
        return Ok(Remover::Engine(|e| {
            e.remove::<Name>();
        }));
    }
    if let Some(c) = engine_component(component) {
        return Ok(Remover::Engine(c.remove));
    }
    let (_, access) = project_component(b.world(), component)
        .ok_or_else(|| not_editable(b.world(), component, &at.key("component")))?;
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
        let r = |plan: &Plan, r: &EntityRef| plan.resolve(b.world(), r, &at_entity);
        match edit {
            Edit::Spawn {
                name,
                prefab,
                components,
            } => {
                let named = plan.spawn_fields(b.world(), components, &at.key("components"))?;
                let components = &*named;
                let revived = |id: EntityId| plan.revived.contains(&id);
                checked.push(Checked::Spawn(prepare_among(
                    b.world(),
                    name.as_deref(),
                    prefab.as_ref(),
                    components,
                    &at,
                    &revived,
                )?));
                let mut resolved = Map::new();
                for (cname, v) in components {
                    let v = match (project_component(b.world(), cname), v.as_object()) {
                        (Some((schema, _)), Some(patch)) => {
                            let at = at.key("components").key(cname);
                            let c =
                                values::canonical_patch(b.world(), &schema, patch, &at, &revived)?;
                            plan.references(&schema, &c, &at)?;
                            Value::Object(c)
                        }
                        _ => v.clone(),
                    };
                    resolved.insert(cname.clone(), v);
                }
                // Its id is not known yet, so it has nothing under it and closes no loop.
                if let Some(parent) = components.get(Parent::NAME).and_then(parent_in) {
                    let at = at.key("components").key(Parent::NAME).key("parent");
                    plan.parent_ok(b, parent, &at)?;
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
                let at_value = at.key("value");
                let (setter, value) = if component == NAME {
                    check_name(&mut plan, id, value, &at_value)?
                } else {
                    match value {
                        SetValue::Fields(fields) => {
                            let (setter, fields) =
                                check_set(b, &mut plan, id, component, fields, *replace, &at)?;
                            (setter, SetValue::Fields(fields))
                        }
                        SetValue::Name(_) => return Err(not_fields(b.world(), component, &at)),
                    }
                };
                if component == Parent::NAME {
                    plan.reparent(b, id, &at_value.key("parent"))?;
                }
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
                    check_remove(b, component, &at)?,
                ));
                plan.pending
                    .insert((id, component.clone()), Pending::Removed);
                if component == Parent::NAME {
                    plan.hierarchy(b).clear(id);
                    plan.parented.remove(&id);
                }
                canonical.push(json!({"op": "remove", "entity": id.get(), "component": component}));
            }
            Edit::Destroy { entity } => {
                let id = r(&plan, entity)?;
                // The entities under it go with it: none of them can be named after this edit.
                let h = plan.hierarchy(b);
                let under = h.descendants(id);
                h.clear(id);
                for gone in &under {
                    h.clear(*gone);
                }
                plan.destroyed.insert(id, i);
                for gone in &under {
                    plan.destroyed.entry(*gone).or_insert(i);
                }
                checked.push(Checked::Destroy(id, under));
                canonical.push(json!({"op": "destroy", "entity": id.get()}));
            }
            Edit::Revive {
                entity,
                name,
                components,
                others,
            } => {
                // Every entity of the edit is marked revived first, then each one's components are
                // checked, then each one's parent: an entity field or a `Parent` may name another
                // entity of the edit, wherever that one comes in the list.
                let first = (*entity, name, components, at.clone());
                let rest = others.iter().enumerate().map(|(k, o)| {
                    let at = at.key("others").index(k);
                    (o.entity, &o.name, &o.components, at)
                });
                let mut members = Vec::with_capacity(others.len() + 1);
                for (n, name, components, at) in std::iter::once(first).chain(rest) {
                    let id = check_revive(b, &plan, n, &at.key("entity"))?;
                    plan.revived.insert(id);
                    members.push((id, name, components, at));
                }
                let revived = |id: EntityId| plan.revived.contains(&id);
                let mut spawns = Vec::with_capacity(members.len());
                for (id, name, components, at) in &members {
                    let spawn =
                        prepare_among(b.world(), name.as_deref(), None, components, at, &revived)?;
                    spawns.push((*id, spawn));
                }
                for (id, _, components, _) in &members {
                    if let Some(parent) = components.get(Parent::NAME) {
                        let key = (*id, Parent::NAME.to_owned());
                        plan.pending.insert(key, Pending::Engine(parent.clone()));
                    }
                }
                for (id, _, components, at) in &members {
                    if components.contains_key(Parent::NAME) {
                        let at = at.key("components").key(Parent::NAME).key("parent");
                        plan.reparent(b, *id, &at)?;
                    }
                }
                let mut c = json!({"op": "revive", "entity": entity, "name": name,
                                   "components": components});
                if !others.is_empty() {
                    c["others"] = json!(others);
                }
                canonical.push(c);
                checked.push(Checked::Revive(spawns));
            }
        }
    }
    plan.loops()?;
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
    let next = b.world().resource::<amoris_sim::EntityAllocator>().next();
    let id = EntityId::new(n)
        .filter(|id| id.get() < next)
        .ok_or_else(|| amoris_sim::entity::entity_id_invalid(&n.to_string(), next))?;
    if b.entity(id).is_some() || plan.revived.contains(&id) {
        let mut p = amoris_sim::entity::entity_alive(id);
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
    for entry in world.resource::<amoris_sim::ComponentRegistry>().entries() {
        if let Some(v) = component_json(world, e, &entry.name) {
            out.insert(entry.name.to_string(), v);
        }
    }
    out
}

/// A component a `set` or `remove` touches, as the inverse restores it: its JSON value, and for
/// `Name` the name as a string (a destroyed entity's name travels beside its components instead,
/// so [`components_json`] leaves it out).
fn touched_json(
    world: &bevy_ecs::prelude::World,
    e: bevy_ecs::prelude::Entity,
    name: &str,
) -> Option<Value> {
    component_of(world, &world.entity(e), name).ok().flatten()
}

/// What a call's inverse needs from the world before it: the entities it destroys whole (each
/// destroy's own entity, then those under it), the components it sets or removes on live entities,
/// and the ids it revives.
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
        let live = |id: EntityId| amoris_sim::entity::entity(world, id);
        let mut recorded = BTreeSet::new();
        for c in checked {
            match c {
                Checked::Set(id, comp, _) | Checked::Remove(id, comp, _) => {
                    if let Some(e) = live(*id) {
                        out.touched
                            .entry((*id, comp.clone()))
                            .or_insert_with(|| touched_json(world, e, comp));
                    }
                }
                Checked::Destroy(id, under) => {
                    // Those live before the call; one an earlier edit revived is not among them.
                    for id in std::iter::once(id).chain(under) {
                        if let Some(e) = live(*id)
                            && recorded.insert(*id)
                        {
                            let name = world
                                .get::<amoris_sim::Name>(e)
                                .map(|n| n.as_str().to_owned());
                            out.destroyed.push((*id, name, components_json(world, e)));
                        }
                    }
                }
                Checked::Revive(members) => out.revived.extend(members.iter().map(|(id, _)| *id)),
                Checked::Spawn(_) => {}
            }
        }
        out
    }

    /// The edits that undo the call, against the world it left, in the order that applies as one
    /// call: the revive, the restores, the destroys. A restore may put an entity back under one
    /// the revive brings back, and must take an entity out from under one a destroy removes
    /// before that destroy, which would take it too.
    fn inverse(self, world: &bevy_ecs::prelude::World, spawned: &[EntityId]) -> Vec<Edit> {
        let live = |id: EntityId| amoris_sim::entity::entity(world, id);
        let mut inv = Vec::new();
        let gone: BTreeSet<EntityId> = self.destroyed.iter().map(|(id, _, _)| *id).collect();
        let mut destroyed = self.destroyed.into_iter();
        if let Some((id, name, components)) = destroyed.next() {
            inv.push(Edit::Revive {
                entity: id.get(),
                name,
                components,
                others: destroyed
                    .map(|(id, name, components)| Revived {
                        entity: id.get(),
                        name,
                        components,
                    })
                    .collect(),
            });
        }
        // The entities the call made and left alive, and the hierarchy as the restores below
        // leave it: what the destroys after them will find. Most calls make nothing (a drag sets
        // one component), and then no row is read.
        let made: BTreeSet<EntityId> = spawned
            .iter()
            .chain(&self.revived)
            .copied()
            .filter(|id| live(*id).is_some())
            .collect();
        let mut h = match made.is_empty() {
            true => Hierarchy::default(),
            false => Hierarchy::read(world),
        };
        for ((id, component), prev) in self.touched {
            if gone.contains(&id) {
                continue;
            }
            let Some(e) = live(id) else { continue };
            match prev {
                Some(Value::Object(value)) => {
                    if component == Parent::NAME
                        && let Some(parent) = value.get("parent").and_then(parent_of)
                    {
                        h.set(id, parent);
                    }
                    inv.push(Edit::Set {
                        entity: EntityRef::Id(id.get()),
                        component,
                        value: SetValue::Fields(value),
                        replace: true,
                    });
                }
                Some(Value::String(name)) if component == NAME => inv.push(Edit::Set {
                    entity: EntityRef::Id(id.get()),
                    component,
                    value: SetValue::Name(name),
                    replace: false,
                }),
                Some(_) => {}
                None => {
                    if touched_json(world, e, &component).is_some() {
                        if component == Parent::NAME {
                            h.clear(id);
                        }
                        inv.push(Edit::Remove {
                            entity: EntityRef::Id(id.get()),
                            component,
                        });
                    }
                }
            }
        }
        // Of the entities the call made, the topmost alone: one under another of them goes with
        // it, and naming it after that destroy would be refused. One that a restore moves out
        // from under the others, with the entity it is under, is topmost again.
        for id in &made {
            let mut seen = BTreeSet::from([*id]);
            let mut at = h.parent(*id);
            let mut top = true;
            while let Some(up) = at {
                if made.contains(&up) {
                    top = false;
                    break;
                }
                if !seen.insert(up) {
                    break;
                }
                at = h.parent(up);
            }
            if top {
                inv.push(Edit::Destroy {
                    entity: EntityRef::Id(id.get()),
                });
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
            .ok_or_else(|| amoris_sim::entity::entity_not_found(id))
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
                Setter::Name(name) => {
                    ent.insert(name);
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
        Checked::Destroy(id, _) => {
            let gone = b.despawn_tree(id)?;
            json!({"op": "destroyed", "count": gone.len()})
        }
        Checked::Revive(members) => {
            let (first, count) = (members[0].0, members.len());
            for (id, p) in members {
                p.revive(b, id)?;
            }
            json!({"op": "revived", "id": first.get(), "count": count})
        }
    })
}

/// One component of an entity as JSON (`Name` too), `None` when the entity has none; an unknown
/// component is refused with suggestions.
fn component_of(
    world: &bevy_ecs::prelude::World,
    ent: &bevy_ecs::world::EntityRef<'_>,
    name: &str,
) -> Result<Option<Value>, Problem> {
    Ok(if let Some(c) = engine_component(name) {
        (c.get)(ent)
    } else if let Some((schema, access)) = project_component(world, name) {
        access.read(ent).map(|v| values::to_json(&schema, &v))
    } else if name == "Name" {
        ent.get::<amoris_sim::Name>().map(|n| json!(n.as_str()))
    } else {
        return Err(unknown_component(world, name));
    })
}

/// `request.invalid_value`: a field path the component's value does not have, with its fields.
pub fn no_field(spec: &str, component: &str, path: &str, value: &Value) -> Problem {
    let keys: Vec<&str> = value
        .as_object()
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let first = path.split('.').next().unwrap_or(path);
    let suggestions = amoris_contract::suggest_names(first, keys.iter().copied());
    Problem::new(
        "request.invalid_value",
        format!(
            "{component} has no field '{path}'; it has {}.",
            if keys.is_empty() {
                "no fields".to_owned()
            } else {
                keys.join(", ")
            }
        ),
        detail([
            ("path", json!("/fields")),
            ("field", json!(spec)),
            ("suggestions", json!(suggestions)),
            ("allowed", json!(keys)),
        ]),
    )
}

/// `world_get`: the entity's components as JSON, engine and project alike, and the fields asked
/// for alone (`fields`: `{"Boat.rudder": 0.3}`; `null` where the entity lacks the component).
pub fn world_get(
    world: &bevy_ecs::prelude::World,
    params: &WorldGetParams,
) -> Result<Value, Problem> {
    let id = resolve(world, &params.entity, &Pointer::root().key("entity"))?;
    let e = amoris_sim::entity::require(world, id)?;
    let ent = world.entity(e);
    let (named, dotted): (Vec<String>, Vec<String>) = params
        .components
        .iter()
        .cloned()
        .partition(|c| !c.contains('.'));
    let fields: Vec<String> = dotted.into_iter().chain(params.fields.clone()).collect();
    let all = named.is_empty() && fields.is_empty();
    let mut out = Map::new();
    let wanted: Vec<String> = if all {
        world
            .resource::<amoris_sim::ComponentRegistry>()
            .entries()
            .iter()
            .map(|e| e.name.to_string())
            .collect()
    } else {
        named
    };
    for name in &wanted {
        let v = match component_of(world, &ent, name) {
            Ok(v) => v,
            Err(_) if all => None,
            Err(p) => return Err(p),
        };
        if let Some(v) = v {
            out.insert(name.clone(), v);
        }
    }
    let name = ent.get::<amoris_sim::Name>().map(|n| n.as_str().to_owned());
    let mut answer = json!({"id": id.get(), "name": name, "components": out});
    if !fields.is_empty() {
        let mut got = Map::new();
        for spec in &fields {
            let (c, path) = crate::inspect::split_field(spec);
            let v = match component_of(world, &ent, c)? {
                None => Value::Null,
                Some(v) => crate::inspect::field_at(&v, path)
                    .cloned()
                    .ok_or_else(|| no_field(spec, c, path, &v))?,
            };
            got.insert(spec.clone(), v);
        }
        answer["fields"] = Value::Object(got);
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recording made before `Name` could be set holds `set`s with object values only: its
    /// canonical `world_edit` decodes as it did and writes back the same, and a rename's `value`
    /// is the name.
    #[test]
    fn the_canonical_form_recorded_before_names_still_decodes() {
        let recorded = json!({"edits": [
            {"op": "set", "entity": 3, "component": "Boat", "value": {"rudder": 0.5},
             "replace": true},
            {"op": "set", "entity": 4, "component": "Cargo", "value": {"value": 9}},
            {"op": "remove", "entity": 3, "component": "Log"},
            {"op": "destroy", "entity": 7},
            {"op": "spawn", "name": "Mate", "prefab": null, "components": {"Crew": {"take": 4}}},
            {"op": "revive", "entity": 7, "name": "Crate4",
             "components": {"Cargo": {"value": 3}}}
        ]});
        let p: WorldEditParams = crate::decode(&recorded, "world_edit").unwrap();
        match &p.edits[0] {
            Edit::Set {
                value: SetValue::Fields(fields),
                replace: true,
                ..
            } => assert_eq!(fields.get("rudder"), Some(&json!(0.5))),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            &p.edits[1],
            Edit::Set {
                value: SetValue::Fields(_),
                replace: false,
                ..
            }
        ));
        assert_eq!(serde_json::to_value(&p).unwrap(), recorded);

        let rename = json!({"edits": [
            {"op": "set", "entity": 4, "component": "Name", "value": "CrateA"}
        ]});
        let p: WorldEditParams = crate::decode(&rename, "world_edit").unwrap();
        match &p.edits[0] {
            Edit::Set {
                value: SetValue::Name(name),
                ..
            } => assert_eq!(name, "CrateA"),
            other => panic!("{other:?}"),
        }
        assert_eq!(serde_json::to_value(&p).unwrap(), rename);

        // Neither form: refused by the shape, before any edit is looked at.
        for bad in [json!(7), json!([1]), json!(null), json!(true)] {
            let mut call = rename.clone();
            call["edits"][0]["value"] = bad;
            let e = crate::decode::<WorldEditParams>(&call, "world_edit").unwrap_err();
            assert_eq!(e.code, "request.wrong_type", "{e:#?}");
            assert_eq!(e.detail["expected"], json!("object or string"), "{e:#?}");
            assert_eq!(e.detail["path"], json!("/edits/0/value"), "{e:#?}");
        }
    }
}
