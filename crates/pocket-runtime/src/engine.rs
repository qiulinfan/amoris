//! Engine components by name (shared/contract/mcp.md 6.1; script-host.md 7.1): their JSON forms
//! for scenes and `world_edit`, and the slot accessors that let scripts read and write the ones a
//! game's rules need.
//!
//! `pocket-physics` registers its components without a script schema, since the `ScriptComponent`
//! derive that would generate their slot functions does not live in a crate it may depend on yet
//! (architecture.md 11, choice 7). The runtime, which links both physics and the script host,
//! therefore rebuilds the world's component registry with schemas for `Transform`, `Velocity`,
//! `Boat` and `Wind` and registers hand-written slot functions for them (architecture.md 11, Slice
//! 1). The rebuilt registry must name exactly the components the old one did, so a component
//! physics adds later fails the game's construction instead of vanishing from it.

use bevy_ecs::component::{Component, Mutable};
use bevy_ecs::prelude::World;
use bevy_ecs::world::{EntityRef, EntityWorldMut};
use pocket_assets::{Animator, Camera, Environment, Light, Model, Splat};
use pocket_contract::{CheckOptions, Problem, Shape, detail};
use pocket_physics::{
    Boat, Collider, ExternalForce, Floater, Hull, RigidBody, Sail, Sea, Transform, Trim, Velocity,
    Wind,
};
use pocket_script::{EngineFns, register_engine};
use pocket_sim::registry::{
    ComponentOrigin, ComponentSchema, FieldType, FieldValue, ProjectValues,
};
use pocket_sim::{ComponentRegistry, Name, Persisted};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

/// An engine component's JSON operations.
pub struct EngineComponent {
    pub name: &'static str,
    /// The component of an entity as JSON, or `None`.
    pub get: fn(&EntityRef<'_>) -> Option<Value>,
    /// The component decoded from a whole JSON value, checked strictly (unknown fields refused with
    /// suggestions), ready to insert.
    pub decode: fn(&Value, &str) -> Result<Inserter, Problem>,
    pub remove: fn(&mut EntityWorldMut<'_>),
}

/// A decoded component waiting to be inserted (everything is decoded before anything changes).
pub struct Inserter(Box<dyn FnOnce(&mut EntityWorldMut<'_>) + Send>);

impl Inserter {
    pub fn insert(self, e: &mut EntityWorldMut<'_>) {
        (self.0)(e);
    }
}

fn get<C: Component + Serialize>(e: &EntityRef<'_>) -> Option<Value> {
    e.get::<C>().and_then(|c| serde_json::to_value(c).ok())
}

fn decode<C>(v: &Value, owner: &str) -> Result<Inserter, Problem>
where
    C: Component<Mutability = Mutable> + DeserializeOwned + JsonSchema,
{
    let c: C = Shape::of::<C>()
        .decode::<C>(v, &CheckOptions::new(owner))?
        .value;
    Ok(Inserter(Box::new(move |e: &mut EntityWorldMut<'_>| {
        e.insert(c);
    })))
}

fn remove<C: Component>(e: &mut EntityWorldMut<'_>) {
    e.remove::<C>();
}

fn entry<C>() -> EngineComponent
where
    C: Component<Mutability = Mutable> + Persisted + Serialize + DeserializeOwned + JsonSchema,
{
    EngineComponent {
        name: C::NAME,
        get: get::<C>,
        decode: decode::<C>,
        remove: remove::<C>,
    }
}

/// Every engine component a scene or `world_edit` can name (`Name` is the entity's own `name`).
pub fn engine_components() -> [EngineComponent; 17] {
    [
        entry::<Animator>(),
        entry::<Boat>(),
        entry::<Camera>(),
        entry::<Environment>(),
        entry::<Light>(),
        entry::<Model>(),
        entry::<Splat>(),
        entry::<Collider>(),
        entry::<ExternalForce>(),
        entry::<Floater>(),
        entry::<Hull>(),
        entry::<RigidBody>(),
        entry::<Sail>(),
        entry::<Sea>(),
        entry::<Transform>(),
        entry::<Velocity>(),
        entry::<Wind>(),
    ]
}

/// The engine component named `name`.
pub fn engine_component(name: &str) -> Option<EngineComponent> {
    engine_components().into_iter().find(|c| c.name == name)
}

/// Writes `patch` into `target`: each key a field, or a path into the value (`position.1`,
/// `layers.0.height`); other fields keep their values (mcp.md 6.1, `Set`).
pub fn merge(target: &mut Value, patch: &Map<String, Value>) -> Result<(), String> {
    for (key, v) in patch {
        let mut at = &mut *target;
        let parts: Vec<&str> = key.split('.').collect();
        for (i, part) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            let next = match at {
                Value::Object(m) => {
                    if last {
                        m.insert((*part).to_owned(), v.clone());
                        break;
                    }
                    m.entry((*part).to_owned())
                        .or_insert_with(|| Value::Object(Map::new()))
                }
                Value::Array(a) => {
                    let idx: usize = part
                        .parse()
                        .map_err(|_| format!("'{part}' in '{key}' is not an index"))?;
                    let len = a.len();
                    let slot = a
                        .get_mut(idx)
                        .ok_or_else(|| format!("index {idx} in '{key}' is past {len} items"))?;
                    if last {
                        *slot = v.clone();
                        break;
                    }
                    slot
                }
                _ => return Err(format!("'{key}' goes inside a value that is not an object")),
            };
            at = next;
        }
    }
    Ok(())
}

// --- Script schemas and slot functions -----------------------------------------------------------

fn f(name: &'static str, doc: &'static str) -> (&'static str, FieldType, FieldValue, &'static str) {
    (name, FieldType::F64, FieldValue::F64(0.0), doc)
}

fn schema(
    name: &str,
    version: u32,
    doc: &str,
    fields: Vec<(&str, FieldType, FieldValue, &str)>,
) -> Result<ComponentSchema, Problem> {
    ComponentSchema::new(name, ComponentOrigin::Engine, version, doc, fields)
}

const TRIMS: [Trim; 4] = [Trim::Furled, Trim::Luffing, Trim::Good, Trim::Overtrimmed];

fn trim_names() -> FieldType {
    FieldType::Enum(
        ["Furled", "Luffing", "Good", "Overtrimmed"]
            .map(Box::from)
            .into(),
    )
}

fn nums(values: &[f64]) -> ProjectValues {
    ProjectValues {
        nums: values.into(),
        strs: Box::new([]),
    }
}

fn v3(n: &[f64], at: usize) -> [f64; 3] {
    [n[at], n[at + 1], n[at + 2]]
}

fn transform_schema() -> Result<ComponentSchema, Problem> {
    schema(
        "Transform",
        Transform::VERSION,
        "Where a body is and how it is turned.",
        vec![
            (
                "position",
                FieldType::Vec3,
                FieldValue::Vec3([0.0; 3]),
                "The origin in the world, metres.",
            ),
            (
                "rotation",
                FieldType::Quat,
                FieldValue::Quat([0.0, 0.0, 0.0, 1.0]),
                "The orientation, a unit quaternion.",
            ),
        ],
    )
}

fn transform_fns() -> EngineFns<Transform> {
    EngineFns {
        read: |t| {
            let (p, r) = (t.position, t.rotation);
            nums(&[p[0], p[1], p[2], r[0], r[1], r[2], r[3]])
        },
        write: |t, v| {
            t.position = v3(&v.nums, 0);
            t.rotation = [v.nums[3], v.nums[4], v.nums[5], v.nums[6]];
        },
        make: |v| Transform {
            position: v3(&v.nums, 0),
            rotation: [v.nums[3], v.nums[4], v.nums[5], v.nums[6]],
        },
    }
}

fn velocity_schema() -> Result<ComponentSchema, Problem> {
    schema(
        "Velocity",
        Velocity::VERSION,
        "How a body moves, in the world frame.",
        vec![
            (
                "linear",
                FieldType::Vec3,
                FieldValue::Vec3([0.0; 3]),
                "Velocity of the centre of mass, m/s.",
            ),
            (
                "angular",
                FieldType::Vec3,
                FieldValue::Vec3([0.0; 3]),
                "Angular velocity, rad/s.",
            ),
        ],
    )
}

fn velocity_fns() -> EngineFns<Velocity> {
    EngineFns {
        read: |c| {
            let (l, a) = (c.linear, c.angular);
            nums(&[l[0], l[1], l[2], a[0], a[1], a[2]])
        },
        write: |c, v| {
            c.linear = v3(&v.nums, 0);
            c.angular = v3(&v.nums, 3);
        },
        make: |v| Velocity {
            linear: v3(&v.nums, 0),
            angular: v3(&v.nums, 3),
        },
    }
}

fn boat_schema() -> Result<ComponentSchema, Problem> {
    let b = |name, doc| (name, FieldType::Bool, FieldValue::Bool(false), doc);
    schema(
        "Boat",
        Boat::VERSION,
        "A boat: its controls, the actuators that follow them, and what it reads.",
        vec![
            f("hoist", "Control, 0..1: how much sail to set; 0 furled."),
            f("sheet", "Control, 0..1: 0 hard in, 1 eased right out."),
            f(
                "rudder",
                "Control, -1..1: positive turns the bow to starboard.",
            ),
            f("hoist_now", "The sail as set now."),
            f("sheet_now", "The sheet as eased now."),
            f("rudder_now", "The rudder blade now."),
            f("speed", "Speed through the water along the heading, m/s."),
            f(
                "heading_deg",
                "Where the bow points, degrees (0 toward -z, 90 toward +x).",
            ),
            f("heel_deg", "Positive heeled to starboard, degrees."),
            b("afloat", "The hull is in the water and not aground."),
            b("aground", "The boat touches land."),
            f("awa_deg", "Apparent wind angle off the bow, degrees."),
            f("aws", "Apparent wind speed, m/s."),
            f("boom_deg", "The boom's angle from the centreline, degrees."),
            f(
                "drive",
                "The sail's drive as a share of the best at this apparent wind.",
            ),
            (
                "trim",
                trim_names(),
                FieldValue::Enum(0),
                "How the sail is trimmed.",
            ),
        ],
    )
}

fn bit(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

fn boat_fns() -> EngineFns<Boat> {
    fn read(b: &Boat) -> ProjectValues {
        let trim = TRIMS.iter().position(|t| *t == b.trim).unwrap_or(0);
        nums(&[
            b.hoist,
            b.sheet,
            b.rudder,
            b.hoist_now,
            b.sheet_now,
            b.rudder_now,
            b.speed,
            b.heading_deg,
            b.heel_deg,
            bit(b.afloat),
            bit(b.aground),
            b.awa_deg,
            b.aws,
            b.boom_deg,
            b.drive,
            f64::from(u8::try_from(trim).unwrap_or(0)),
        ])
    }
    fn write(b: &mut Boat, v: &ProjectValues) {
        let n = &v.nums;
        (b.hoist, b.sheet, b.rudder) = (n[0], n[1], n[2]);
        (b.hoist_now, b.sheet_now, b.rudder_now) = (n[3], n[4], n[5]);
        (b.speed, b.heading_deg, b.heel_deg) = (n[6], n[7], n[8]);
        (b.afloat, b.aground) = (n[9] != 0.0, n[10] != 0.0);
        (b.awa_deg, b.aws, b.boom_deg, b.drive) = (n[11], n[12], n[13], n[14]);
        b.trim = pocket_sim::num::to_u32(n[15])
            .ok()
            .and_then(|i| TRIMS.get(usize::try_from(i).unwrap_or(usize::MAX)))
            .copied()
            .unwrap_or(Trim::Furled);
    }
    EngineFns {
        read,
        write,
        make: |v| {
            let mut b = Boat::default();
            write(&mut b, v);
            b
        },
    }
}

fn wind_schema() -> Result<ComponentSchema, Problem> {
    schema(
        "Wind",
        Wind::VERSION,
        "The wind over the sea.",
        vec![
            f(
                "from_deg",
                "Where the wind blows from, degrees (270 blows toward +x).",
            ),
            f("speed", "m/s."),
            f(
                "gust",
                "The gusts' share of the speed, 0 for a steady wind.",
            ),
            f("gust_period", "Seconds between gusts at a point."),
            f("gust_length", "Metres between gusts along the wind."),
        ],
    )
}

fn wind_fns() -> EngineFns<Wind> {
    EngineFns {
        read: |w| nums(&[w.from_deg, w.speed, w.gust, w.gust_period, w.gust_length]),
        write: |w, v| {
            let n = &v.nums;
            (w.from_deg, w.speed, w.gust, w.gust_period, w.gust_length) =
                (n[0], n[1], n[2], n[3], n[4]);
        },
        make: |v| {
            let n = &v.nums;
            Wind {
                from_deg: n[0],
                speed: n[1],
                gust: n[2],
                gust_period: n[3],
                gust_length: n[4],
            }
        },
    }
}

fn names(world: &World) -> Vec<String> {
    world
        .get_resource::<ComponentRegistry>()
        .map(|r| r.entries().iter().map(|e| e.name.to_string()).collect())
        .unwrap_or_default()
}

/// Rebuilds the world's component registry with script schemas for the engine components scripts
/// read and write, and registers their slot functions. Runs after `pocket_physics::plugin` and
/// before any project component is registered.
pub fn install_script_components(world: &mut World) -> Result<(), Problem> {
    let before = names(world);
    world.insert_resource(ComponentRegistry::default());
    ComponentRegistry::register::<Name>(world, None)?;
    ComponentRegistry::register::<Transform>(world, Some(transform_schema()?))?;
    ComponentRegistry::register::<Velocity>(world, Some(velocity_schema()?))?;
    ComponentRegistry::register::<Boat>(world, Some(boat_schema()?))?;
    ComponentRegistry::register::<Wind>(world, Some(wind_schema()?))?;
    ComponentRegistry::register::<RigidBody>(world, None)?;
    ComponentRegistry::register::<Collider>(world, None)?;
    ComponentRegistry::register::<ExternalForce>(world, None)?;
    ComponentRegistry::register::<Floater>(world, None)?;
    ComponentRegistry::register::<Sail>(world, None)?;
    ComponentRegistry::register::<Hull>(world, None)?;
    ComponentRegistry::register::<Sea>(world, None)?;
    ComponentRegistry::register::<Model>(world, None)?;
    ComponentRegistry::register::<Light>(world, None)?;
    ComponentRegistry::register::<Camera>(world, None)?;
    ComponentRegistry::register::<Environment>(world, None)?;
    ComponentRegistry::register::<Splat>(world, None)?;
    ComponentRegistry::register::<Animator>(world, None)?;
    let after = names(world);
    if before != after {
        return Err(Problem::new(
            "sim.internal",
            format!(
                "The runtime's component registry names {after:?}, the engine's {before:?}; \
                 pocket-runtime's engine.rs must list every engine component."
            ),
            detail([("before", json!(before)), ("after", json!(after))]),
        ));
    }
    register_engine::<Transform>(world, transform_fns())?;
    register_engine::<Velocity>(world, velocity_fns())?;
    register_engine::<Boat>(world, boat_fns())?;
    register_engine::<Wind>(world, wind_fns())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_reach_inside_values() {
        let mut v =
            json!({"position": [0.0, 1.0, 2.0], "rotation": [0, 0, 0, 1], "deep": {"a": 1}});
        let patch = json!({"position.1": 5.0, "deep.b": 2, "new": true});
        merge(&mut v, patch.as_object().unwrap()).unwrap();
        assert_eq!(v["position"], json!([0.0, 5.0, 2.0]));
        assert_eq!(v["deep"], json!({"a": 1, "b": 2}));
        assert_eq!(v["new"], json!(true));
        let bad = json!({"position.7": 1.0});
        assert!(merge(&mut v, bad.as_object().unwrap()).is_err());
    }

    #[test]
    fn boat_slots_round_trip() {
        let b = Boat {
            hoist: 1.0,
            rudder: -0.25,
            afloat: true,
            trim: Trim::Good,
            ..Boat::default()
        };
        let fns = boat_fns();
        let v = (fns.read)(&b);
        assert_eq!(v.nums.len(), usize::from(boat_schema().unwrap().slots()));
        assert_eq!((fns.make)(&v), b);
    }
}
