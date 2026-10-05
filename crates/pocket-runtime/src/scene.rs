//! Scenes (a project's `scene.json`) and spawning from data: an entity is a name, an optional
//! prefab from the engine's catalog (the showcase's Sloop, crates, seas and winds, which
//! `pocket_physics::sailing` builds) and components by name, engine or project, each a JSON value
//! that overrides the prefab's component field by field. A scene spawns its entities by name first
//! and then checks and inserts their components, so an entity field may name any entity of the
//! scene; a refused scene is rolled back whole. A `world_edit` spawn is checked before anything is
//! spawned, so a refused one changes nothing.

use std::collections::BTreeMap;
use std::sync::Arc;

use pocket_contract::{Pointer, Problem, detail};
use pocket_physics::sailing;
use pocket_script::{ComponentAccess, ScriptAccess};
use pocket_sim::registry::{ComponentOrigin, ProjectValues};
use pocket_sim::{Boundary, ComponentRegistry, EntityId, Name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::engine::{Inserter, engine_component, merge};
use crate::values;

/// The value of `Scene::format`.
pub const SCENE_FORMAT: &str = "pocket-scene";

/// A scene file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    /// Always "pocket-scene".
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
            let t = pocket_physics::Transform::at_yaw(*position, *yaw_deg);
            out.insert("Transform", to_json(&t));
            out.insert("Velocity", to_json(&pocket_physics::Velocity::default()));
            out.insert("RigidBody", to_json(&pocket_physics::RigidBody::dynamic()));
            let c =
                pocket_physics::Collider::new(pocket_physics::Shape::Cuboid { half_extents: h })
                    .with_density(350.0)
                    .with_friction(0.5, 0.1);
            out.insert("Collider", to_json(&c));
            out.insert(
                "ExternalForce",
                to_json(&pocket_physics::ExternalForce::default()),
            );
            out.insert("Floater", to_json(&pocket_physics::Floater::solid_box(h)));
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

/// A project component's schema and accessor, or `None` when `name` is not one.
pub fn project_component(
    world: &bevy_ecs::prelude::World,
    name: &str,
) -> Option<(
    Arc<pocket_sim::registry::ComponentSchema>,
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
            let defaults = pocket_script::access::defaults(&schema);
            let values = values::patch(world, &schema, &defaults, patch, &at)?;
            parts.push(Part::Project { access, values });
        } else {
            return Err(unknown_component(world, cname));
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
            .ok_or_else(|| pocket_sim::entity::entity_not_found(id))?;
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
        let r = self.spawn_marked(b);
        if r.is_err() {
            b.rollback(mark);
        }
        r
    }

    fn spawn_marked(&self, b: &mut Boundary<'_>) -> Result<Vec<EntityId>, Problem> {
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
            prepared.push(prepare(
                b.world(),
                None,
                e.prefab.as_ref(),
                &e.components,
                &path,
            )?);
        }
        for (p, id) in prepared.into_iter().zip(&ids) {
            p.insert(b, *id)?;
        }
        Ok(ids)
    }
}
