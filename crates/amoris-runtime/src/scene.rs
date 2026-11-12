//! Scenes (a project's `scene.json`) and spawning from data: an entity is a name, an optional
//! prefab from the engine's catalog (the showcase's Sloop, crates, seas and winds, which
//! `amoris_physics::sailing` builds) and components by name, engine or project, each a JSON value
//! that overrides the prefab's component field by field. A scene spawns its entities by name first
//! and then checks and inserts their components, so an entity field may name any entity of the
//! scene (a `Parent` too: the scene's hierarchy is checked once every entity has its components);
//! a refused scene is rolled back whole. A `world_edit` spawn is checked before anything is
//! spawned, so a refused one changes nothing.

use std::collections::BTreeMap;
use std::sync::Arc;

use amoris_contract::{Pointer, Problem, detail};
use amoris_physics::sailing;
use amoris_script::{ComponentAccess, ScriptAccess};
use amoris_sim::registry::{ComponentOrigin, ProjectValues};
use amoris_sim::{Boundary, ComponentRegistry, EntityId, Name, Persisted};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::engine::{Inserter, engine_component, merge};
use crate::values;

/// The value of `Scene::format`.
pub const SCENE_FORMAT: &str = "amoris-scene";

/// A scene file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    /// Always "amoris-scene".
    pub format: String,
    /// 1.
    pub version: u32,
    /// Spawned in order, so their ids follow it.
    pub entities: Vec<SceneEntity>,
}

/// One entity of a scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneEntity {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prefab: Option<Prefab>,
    /// Components by name; each overrides the prefab's component of that name field by field.
    #[serde(default)]
    pub components: Map<String, Value>,
}

/// The engine's prefabs: the showcase's pieces (charter 2.4.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Prefab {
    /// The Sloop, its bow toward `heading_deg` (0 toward -z, 90 toward +x), sail furled.
    Sloop {
        position: [f64; 3],
        heading_deg: f64,
    },
    /// A floating crate, a 0.6 m cube of about 76 kg.
    Crate {
        position: [f64; 3],
        #[serde(default)]
        yaw_deg: f64,
    },
    /// Master's island sea: a swell and three smaller waves.
    IslandSea,
    /// A calm sea at level 0.
    CalmSea,
    /// A steady wind from `from_deg` (270 blows toward +x) at `speed` m/s.
    Breeze { from_deg: f64, speed: f64 },
}

fn to_json<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// A prefab's components as JSON, by name.
pub fn prefab_components(p: &Prefab) -> BTreeMap<&'static str, Value> {
    let mut out = BTreeMap::new();
    match p {
        Prefab::Sloop {
            position,
            heading_deg,
        } => {
            let (t, v, rb, c, ef, fl, boat, sail, hull) = sailing::sloop(*position, *heading_deg);
            out.insert("Transform", to_json(&t));
            out.insert("Velocity", to_json(&v));
            out.insert("RigidBody", to_json(&rb));
            out.insert("Collider", to_json(&c));
            out.insert("ExternalForce", to_json(&ef));
            out.insert("Floater", to_json(&fl));
            out.insert("Boat", to_json(&boat));
            out.insert("Sail", to_json(&sail));
            out.insert("Hull", to_json(&hull));
        }
        Prefab::Crate { position, yaw_deg } => {
            let h = [0.3; 3];
            let t = amoris_physics::Transform::at_yaw(*position, *yaw_deg);
            out.insert("Transform", to_json(&t));
            out.insert("Velocity", to_json(&amoris_physics::Velocity::default()));
            out.insert("RigidBody", to_json(&amoris_physics::RigidBody::dynamic()));
            let c =
                amoris_physics::Collider::new(amoris_physics::Shape::Cuboid { half_extents: h })
                    .with_density(350.0)
                    .with_friction(0.5, 0.1);
            out.insert("Collider", to_json(&c));
            out.insert(
                "ExternalForce",
                to_json(&amoris_physics::ExternalForce::default()),
            );
            out.insert("Floater", to_json(&amoris_physics::Floater::solid_box(h)));
        }
        Prefab::IslandSea => {
            out.insert("Sea", to_json(&sailing::island_sea()));
        }
        Prefab::CalmSea => {
            out.insert("Sea", to_json(&sailing::calm_sea()));
        }
        Prefab::Breeze { from_deg, speed } => {
            out.insert("Wind", to_json(&sailing::breeze(*from_deg, *speed)));
        }
    }
    out
}

enum Part {
    Engine(Inserter),
    Project {
        access: Arc<dyn ComponentAccess>,
        values: ProjectValues,
    },
}

/// A spawn checked against the world, ready to apply.
pub struct PreparedSpawn {
    name: Option<Name>,
    parts: Vec<Part>,
}

/// `sim.component_unknown {component, suggestions}`.
pub fn unknown_component(world: &bevy_ecs::prelude::World, name: &str) -> Problem {
    let suggestions = world
        .get_resource::<ComponentRegistry>()
        .map(|r| r.suggest(name))
        .unwrap_or_default();
    Problem::new(
        "sim.component_unknown",
        format!("There is no component '{name}'; did you mean {suggestions:?}?"),
        detail([
            ("component", json!(name)),
            ("suggestions", json!(suggestions)),
        ]),
    )
}

/// The refusal of a component a scene or an edit names (at `path`) and cannot write, which is
/// every one that is neither an engine component with a JSON form nor a project component:
/// `sim.component_unknown` when the registry holds none of that name; when it holds one,
/// `request.invalid_value` saying why, since [`unknown_component`] would suggest the very name it
/// calls unknown. `Name` among `components` is the entity's `name`; the others are state the
/// engine's own systems keep (a seat's `Controls`, what an observer remembers).
pub fn not_editable(world: &bevy_ecs::prelude::World, name: &str, path: &Pointer) -> Problem {
    let registered = world
        .get_resource::<ComponentRegistry>()
        .is_some_and(|r| r.get(name).is_some());
    if !registered {
        return unknown_component(world, name);
    }
    if name == <Name as Persisted>::NAME {
        return bad_value(path, "an entity's name is its `name`, beside `components`");
    }
    bad_value(
        path,
        &format!("{name} is state the engine keeps itself, which a scene or an edit cannot write"),
    )
}

/// A project component's schema and accessor, or `None` when `name` is not one.
pub fn project_component(
    world: &bevy_ecs::prelude::World,
    name: &str,
) -> Option<(
    Arc<amoris_sim::registry::ComponentSchema>,
    Arc<dyn ComponentAccess>,
)> {
    let entry = world.get_resource::<ComponentRegistry>()?.get(name)?;
    if entry.origin != ComponentOrigin::Project {
        return None;
    }
    let access = world.get_resource::<ScriptAccess>()?;
    let schema = access
        .schema(entry.id)
        .cloned()
        .or_else(|| entry.script.clone())?;
    Some((schema, access.get(entry.id)?.clone()))
}

/// Checks a spawn: the prefab's components overridden by `components`, every value decoded.
pub fn prepare(
    world: &bevy_ecs::prelude::World,
    name: Option<&str>,
    prefab: Option<&Prefab>,
    components: &Map<String, Value>,
    path: &Pointer,
) -> Result<PreparedSpawn, Problem> {
    prepare_among(world, name, prefab, components, path, &values::none_else)
}

/// [`prepare`], where a project component's entity field may also name an entity `also` accepts:
/// one an earlier edit of the same `world_edit`, or the same `revive`, brings back.
pub fn prepare_among(
    world: &bevy_ecs::prelude::World,
    name: Option<&str>,
    prefab: Option<&Prefab>,
    components: &Map<String, Value>,
    path: &Pointer,
    also: values::AlsoAlive<'_>,
) -> Result<PreparedSpawn, Problem> {
    let name = name.map(Name::new).transpose()?;
    let mut base = prefab.map(prefab_components).unwrap_or_default();
    let mut parts = Vec::new();
    for (cname, value) in components {
        let at = path.key("components").key(cname);
        if let Some(c) = engine_component(cname) {
            let whole = match (base.remove(c.name), value) {
                (Some(mut b), Value::Object(patch)) => {
                    merge(&mut b, patch).map_err(|why| bad_value(&at, &why))?;
                    b
                }
                (_, v) => v.clone(),
            };
            parts.push(Part::Engine((c.decode)(&whole, cname)?));
        } else if let Some((schema, access)) = project_component(world, cname) {
            let patch = value
                .as_object()
                .ok_or_else(|| bad_value(&at, "a component's value is an object of its fields"))?;
            let defaults = amoris_script::access::defaults(&schema);
            let values = values::patch_among(world, &schema, &defaults, patch, &at, also)?;
            parts.push(Part::Project { access, values });
        } else {
            return Err(not_editable(world, cname, &at));
        }
    }
    for (cname, whole) in base {
        let c = engine_component(cname).ok_or_else(|| unknown_component(world, cname))?;
        parts.push(Part::Engine((c.decode)(&whole, cname)?));
    }
    Ok(PreparedSpawn { name, parts })
}

/// `request.invalid_value` for a value that cannot be used as given.
pub fn bad_value(path: &Pointer, why: &str) -> Problem {
    Problem::new(
        "request.invalid_value",
        format!("'{}' cannot be used: {why}.", path.dotted()),
        detail([("path", json!(path.dotted()))]),
    )
}

impl PreparedSpawn {
    /// Spawns the entity at a boundary.
    pub fn spawn(mut self, b: &mut Boundary<'_>) -> Result<EntityId, Problem> {
        let id = match self.name.take() {
            Some(n) => b.spawn(n)?,
            None => b.spawn(())?,
        };
        self.insert(b, id)?;
        Ok(id)
    }

    /// Spawns the entity again under the destroyed entity's id `id` (an undo of a destroy).
    pub fn revive(mut self, b: &mut Boundary<'_>, id: EntityId) -> Result<(), Problem> {
        match self.name.take() {
            Some(n) => b.revive(id, n)?,
            None => b.revive(id, ())?,
        };
        self.insert(b, id)
    }

    /// Inserts the components into the live entity `id` (its name, if any, is left out).
    pub fn insert(self, b: &mut Boundary<'_>, id: EntityId) -> Result<(), Problem> {
        let e = b
            .entity(id)
            .ok_or_else(|| amoris_sim::entity::entity_not_found(id))?;
        let world = b.world_mut();
        let mut ent = world.entity_mut(e);
        for p in self.parts {
            match p {
                Part::Engine(i) => i.insert(&mut ent),
                Part::Project { access, values } => access.insert(&mut ent, values),
            }
        }
        Ok(())
    }
}

/// `scene.place_invalid {path, place, entities}`: an entity number in a loaded scene that is no
/// place in its file.
fn place_invalid(path: &Pointer, place: &Value, entities: usize) -> Problem {
    Problem::new(
        "scene.place_invalid",
        format!(
            "{path}: {place} is no entity of the scene, whose entities are its places 1 to \
             {entities}."
        ),
        detail([
            ("path", json!(path.to_string())),
            ("place", place.clone()),
            ("entities", json!(entities)),
        ]),
    )
}

/// An entity's components with every entity number (a place in the file, from 1) rewritten to the
/// id spawned for that place: a project component's entity fields by its schema, an engine
/// component's by its JSON Schema (`values::entity_refs`, as `project.save` writes them). A name
/// stays a name.
fn renumbered(
    world: &bevy_ecs::prelude::World,
    components: &Map<String, Value>,
    ids: &[EntityId],
    path: &Pointer,
) -> Result<Map<String, Value>, Problem> {
    let mut out = components.clone();
    let place = |v: &mut Value, at: &Pointer| -> Result<(), Problem> {
        let Some(n) = v.as_u64() else { return Ok(()) };
        let id = usize::try_from(n)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| ids.get(i))
            .ok_or_else(|| place_invalid(at, v, ids.len()))?;
        *v = json!(id.get());
        Ok(())
    };
    for (cname, value) in out.iter_mut() {
        let at = path.key("components").key(cname);
        if let Some((schema, _)) = project_component(world, cname) {
            for f in schema
                .fields
                .iter()
                .filter(|f| f.ty == amoris_sim::registry::FieldType::Entity)
            {
                if let Some(slot) = value.get_mut(&*f.name) {
                    place(slot, &at.key(&f.name))?;
                }
            }
        } else if let Some(entry) = world
            .get_resource::<ComponentRegistry>()
            .and_then(|r| r.get(cname))
        {
            for pointer in values::entity_refs(&entry.json_schema, value) {
                let mut slot_at = at.clone();
                for part in pointer.split('/').skip(1) {
                    slot_at = slot_at.key(&part.replace("~1", "/").replace("~0", "~"));
                }
                if let Some(slot) = value.pointer_mut(&pointer) {
                    place(slot, &slot_at)?;
                }
            }
        }
    }
    Ok(out)
}

impl Scene {
    /// A scene with no entities.
    pub fn empty() -> Scene {
        Scene {
            format: SCENE_FORMAT.to_owned(),
            version: 1,
            entities: Vec::new(),
        }
    }

    /// A scene from its JSON text, checked strictly.
    pub fn from_json(text: &str) -> Result<Scene, Problem> {
        let v: Value = serde_json::from_str(text).map_err(|e| {
            Problem::new(
                "request.malformed",
                format!("The scene is not JSON: {e}."),
                detail([("line", json!(e.line())), ("column", json!(e.column()))]),
            )
        })?;
        let scene: Scene = crate::decode(&v, "the scene")?;
        if scene.format != SCENE_FORMAT || scene.version != 1 {
            return Err(Problem::new(
                "version.format",
                format!(
                    "The scene is '{}' version {}; this engine reads '{SCENE_FORMAT}' version 1.",
                    scene.format, scene.version
                ),
                detail([
                    ("format", json!(scene.format)),
                    ("version", json!(scene.version)),
                ]),
            ));
        }
        Ok(scene)
    }

    /// Spawns every entity at the boundary the world is at, in order; nothing when one is refused.
    /// Every entity is spawned with its name first, so the ids follow the file and an entity field
    /// can name any entity of the scene (charter 3.2); then each entity's components are checked
    /// against that world and inserted.
    pub fn spawn_into(&self, b: &mut Boundary<'_>) -> Result<Vec<EntityId>, Problem> {
        let mark = b.mark();
        let r = self.spawn_marked(b, false);
        if r.is_err() {
            b.rollback(mark);
        }
        r
    }

    /// [`Scene::spawn_into`] for a scene loaded into a world that may hold entities already
    /// (`scene.load`, simulation.md 7.4): an entity number inside a component is the place of an
    /// entity in the file, from 1, rewritten to the id that entity was spawned with; a number past
    /// the file is refused with `scene.place_invalid`. In a new world the two agree.
    pub fn spawn_placed(&self, b: &mut Boundary<'_>) -> Result<Vec<EntityId>, Problem> {
        let mark = b.mark();
        let r = self.spawn_marked(b, true);
        if r.is_err() {
            b.rollback(mark);
        }
        r
    }

    fn spawn_marked(&self, b: &mut Boundary<'_>, placed: bool) -> Result<Vec<EntityId>, Problem> {
        let mut ids = Vec::with_capacity(self.entities.len());
        for e in &self.entities {
            ids.push(match &e.name {
                Some(n) => b.spawn(Name::new(n)?)?,
                None => b.spawn(())?,
            });
        }
        let mut prepared = Vec::with_capacity(self.entities.len());
        for (i, e) in self.entities.iter().enumerate() {
            let path = Pointer::root().key("entities").index(i);
            let components = if placed {
                std::borrow::Cow::Owned(renumbered(b.world(), &e.components, &ids, &path)?)
            } else {
                std::borrow::Cow::Borrowed(&e.components)
            };
            prepared.push(prepare(
                b.world(),
                None,
                e.prefab.as_ref(),
                &components,
                &path,
            )?);
        }
        for (p, id) in prepared.into_iter().zip(&ids) {
            p.insert(b, *id)?;
        }
        // Checks across entities: a water region sits on its volume's grid (water.md 5), and
        // every `Parent` names a live entity and closes no loop (simulation.md 7.4).
        amoris_physics::water::check_world(b.world())?;
        crate::hierarchy::check_scene(b.world(), &ids)?;
        Ok(ids)
    }
}
