//! Engine components by name (shared/contract/mcp.md 6.1; script-host.md 7.1): their JSON forms
//! for scenes and `world_edit`, and the slot accessors that let scripts read and write the ones a
//! game's rules need.
//!
//! `amoris-physics` registers its components without a script schema, since the `ScriptComponent`
//! derive that would generate their slot functions does not live in a crate it may depend on yet
//! (architecture.md 11, choice 7). The runtime, which links both physics and the script host,
//! therefore rebuilds the world's component registry with schemas for `Transform`, `Velocity`,
//! `Boat`, `Wind`, `Walker`, UI, the visual components a game animates (`Model`, `Light`,
//! `Environment`) and water request components, and registers hand-written slot functions for
//! them (architecture.md 11, Slice 1). The rebuilt registry must name exactly the components the old one did, so a component
//! physics adds later fails the game's construction instead of vanishing from it.

use amoris_assets::{
    Animator, AudioSource, Camera, Environment, Light, LightKind, Model, ParticleEmitter, SkyKind,
    Splat, UiBar, UiText,
};
use amoris_contract::{CheckOptions, Problem, Shape, detail};
use amoris_physics::water::displacement::WaterDisplacement;
use amoris_physics::water::feedback::WaterFeedback;
use amoris_physics::water::resident::{WaterOutletStage, WaterProbe, WaterResident};
use amoris_physics::{
    Board, Boat, Collider, ExternalForce, Floater, Hull, Rain, RigidBody, Sail, Sea, Terrain,
    Transform, Trim, Velocity, Vessel, Walker, WaterDrain, WaterRegion, WaterSource, Wind,
};
use amoris_script::{EngineFns, register_engine};
use amoris_sim::registry::{
    ComponentOrigin, ComponentSchema, FieldType, FieldValue, ProjectValues,
};
use amoris_sim::voxels::Voxels;
use amoris_sim::{ComponentRegistry, Name, Persisted};
use bevy_ecs::component::{Component, Mutable};
use bevy_ecs::prelude::World;
use bevy_ecs::world::{EntityRef, EntityWorldMut};
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

/// Every engine component a scene or `world_edit` can name (`Name` is the entity's own `name`);
/// the first three are the player layer's (perception.md: who observes, what can be perceived,
/// what blocks sight).
pub fn engine_components() -> [EngineComponent; 48] {
    [
        entry::<amoris_interface::perception::Observer>(),
        entry::<amoris_interface::perception::Perceivable>(),
        entry::<amoris_interface::perception::Occluder>(),
        entry::<Animator>(),
        entry::<amoris_assets::anim::AnimGraph>(),
        entry::<amoris_assets::anim::AnimState>(),
        entry::<amoris_assets::anim::Attach>(),
        entry::<AudioSource>(),
        entry::<Board>(),
        entry::<Boat>(),
        entry::<Camera>(),
        entry::<Environment>(),
        entry::<Light>(),
        entry::<Model>(),
        entry::<ParticleEmitter>(),
        entry::<Splat>(),
        entry::<UiBar>(),
        entry::<UiText>(),
        entry::<Collider>(),
        entry::<ExternalForce>(),
        entry::<Floater>(),
        entry::<Hull>(),
        entry::<RigidBody>(),
        entry::<Sail>(),
        entry::<Sea>(),
        entry::<Transform>(),
        entry::<Velocity>(),
        entry::<Vessel>(),
        entry::<Walker>(),
        entry::<amoris_physics::Avatar>(),
        entry::<amoris_physics::Buttons>(),
        entry::<amoris_physics::nav::NavGrid>(),
        entry::<amoris_physics::CameraRig>(),
        entry::<Wind>(),
        entry::<Rain>(),
        voxels_entry(),
        water_entry(),
        entry::<WaterResident>(),
        entry::<amoris_physics::water::WaterColliderBed>(),
        terrain_entry(),
        entry::<amoris_physics::terrain::TerrainGround>(),
        entry::<amoris_physics::water::WaterFocus>(),
        entry::<WaterOutletStage>(),
        probe_entry(),
        feedback_entry(),
        displacement_entry(),
        flow_entry::<WaterSource>(),
        flow_entry::<WaterDrain>(),
    ]
}

/// `Voxels`, decoded strictly and then validated (voxels.md 2.2).
fn voxels_entry() -> EngineComponent {
    fn decode_voxels(v: &Value, owner: &str) -> Result<Inserter, Problem> {
        let c: Voxels = Shape::of::<Voxels>()
            .decode::<Voxels>(v, &CheckOptions::new(owner))?
            .value;
        c.validate()?;
        Ok(Inserter(Box::new(move |e: &mut EntityWorldMut<'_>| {
            e.insert(c);
        })))
    }
    EngineComponent {
        name: Voxels::NAME,
        get: get::<Voxels>,
        decode: decode_voxels,
        remove: remove::<Voxels>,
    }
}

/// `Terrain`, decoded strictly and then validated (water.md 65.1).
fn terrain_entry() -> EngineComponent {
    fn decode_terrain(v: &Value, owner: &str) -> Result<Inserter, Problem> {
        let c: Terrain = Shape::of::<Terrain>()
            .decode::<Terrain>(v, &CheckOptions::new(owner))?
            .value;
        c.validate()?;
        Ok(Inserter(Box::new(move |e: &mut EntityWorldMut<'_>| {
            e.insert(c);
        })))
    }
    EngineComponent {
        name: Terrain::NAME,
        get: get::<Terrain>,
        decode: decode_terrain,
        remove: remove::<Terrain>,
    }
}

/// `WaterRegion`, decoded strictly and then validated on its own (water.md 3).
fn water_entry() -> EngineComponent {
    fn decode_water(v: &Value, owner: &str) -> Result<Inserter, Problem> {
        let c: WaterRegion = Shape::of::<WaterRegion>()
            .decode::<WaterRegion>(v, &CheckOptions::new(owner))?
            .value;
        c.validate()?;
        Ok(Inserter(Box::new(move |e: &mut EntityWorldMut<'_>| {
            e.insert(c);
        })))
    }
    EngineComponent {
        name: WaterRegion::NAME,
        get: get::<WaterRegion>,
        decode: decode_water,
        remove: remove::<WaterRegion>,
    }
}

/// A source's or a drain's point and rate, for their check (water.md 8).
trait FlowNumbers {
    fn numbers(&self) -> ([f64; 2], f64);
}

impl FlowNumbers for WaterSource {
    fn numbers(&self) -> ([f64; 2], f64) {
        (self.at, self.rate_m3s)
    }
}

impl FlowNumbers for WaterDrain {
    fn numbers(&self) -> ([f64; 2], f64) {
        (self.at, self.rate_m3s)
    }
}

fn decode_flow<C>(v: &Value, owner: &str) -> Result<Inserter, Problem>
where
    C: Component<Mutability = Mutable> + DeserializeOwned + JsonSchema + FlowNumbers,
{
    let c: C = Shape::of::<C>()
        .decode::<C>(v, &CheckOptions::new(owner))?
        .value;
    let (at, rate) = c.numbers();
    amoris_physics::water::validate_flow(at, rate)?;
    Ok(Inserter(Box::new(move |e: &mut EntityWorldMut<'_>| {
        e.insert(c);
    })))
}

/// `WaterSource` and `WaterDrain`, decoded strictly and their numbers checked.
fn flow_entry<C>() -> EngineComponent
where
    C: Component<Mutability = Mutable>
        + Persisted
        + Serialize
        + DeserializeOwned
        + JsonSchema
        + FlowNumbers,
{
    EngineComponent {
        name: C::NAME,
        get: get::<C>,
        decode: decode_flow::<C>,
        remove: remove::<C>,
    }
}

/// Editing a request invalidates prior output; external edits cannot forge a ready result.
fn probe_entry() -> EngineComponent {
    fn decode(v: &Value, owner: &str) -> Result<Inserter, Problem> {
        let mut probe: WaterProbe = Shape::of::<WaterProbe>()
            .decode::<WaterProbe>(v, &CheckOptions::new(owner))?
            .value;
        probe.ready = false;
        Ok(Inserter(Box::new(move |e: &mut EntityWorldMut<'_>| {
            e.insert(probe);
        })))
    }
    EngineComponent {
        name: WaterProbe::NAME,
        get: get::<WaterProbe>,
        decode,
        remove: remove::<WaterProbe>,
    }
}

/// Authoring opts in with an empty object. Queues and budgets are engine-owned, readable state.
fn feedback_entry() -> EngineComponent {
    fn decode(v: &Value, owner: &str) -> Result<Inserter, Problem> {
        if !v.as_object().is_some_and(|v| v.is_empty()) {
            return Err(Problem::new(
                "water.feedback_readonly",
                format!(
                    "{owner}: WaterFeedback is created with {{}}; its queues and budgets are read-only"
                ),
                detail([]),
            ));
        }
        Ok(Inserter(Box::new(|e: &mut EntityWorldMut<'_>| {
            e.insert(WaterFeedback::default());
        })))
    }
    EngineComponent {
        name: WaterFeedback::NAME,
        get: get::<WaterFeedback>,
        decode,
        remove: remove::<WaterFeedback>,
    }
}

/// Authoring opts in with an empty object. The queued volumes are engine-owned, readable state.
fn displacement_entry() -> EngineComponent {
    fn decode(v: &Value, owner: &str) -> Result<Inserter, Problem> {
        if !v.as_object().is_some_and(|v| v.is_empty()) {
            return Err(Problem::new(
                "water.displacement_readonly",
                format!(
                    "{owner}: WaterDisplacement is created with {{}}; its queued volumes are read-only"
                ),
                detail([]),
            ));
        }
        Ok(Inserter(Box::new(|e: &mut EntityWorldMut<'_>| {
            e.insert(WaterDisplacement::default());
        })))
    }
    EngineComponent {
        name: WaterDisplacement::NAME,
        get: get::<WaterDisplacement>,
        decode,
        remove: remove::<WaterDisplacement>,
    }
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
        b.trim = amoris_sim::num::to_u32(n[15])
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

fn walker_schema() -> Result<ComponentSchema, Problem> {
    let d = Walker::default();
    let n = |name, value, doc| (name, FieldType::F64, FieldValue::F64(value), doc);
    let b = |name, doc| (name, FieldType::Bool, FieldValue::Bool(false), doc);
    schema(
        "Walker",
        Walker::VERSION,
        "A first-person body (voxels.md 5.2); its entity's Transform is the eye.",
        vec![
            n("eye_m", d.eye_m, "The eye above the feet, metres."),
            n("height_m", d.height_m, "The body's height, metres."),
            n(
                "half_width_m",
                d.half_width_m,
                "Half the body's width, metres.",
            ),
            n("speed_mps", d.speed_mps, "Walking speed, m/s."),
            n("run_mps", d.run_mps, "Running speed, m/s."),
            n("jump_mps", d.jump_mps, "Take-off speed of a jump, m/s."),
            n("gravity_mps2", d.gravity_mps2, "Gravity, m/s2."),
            n("forward", 0.0, "Forward input, -1 to 1."),
            n("strafe", 0.0, "Rightward input, -1 to 1."),
            n(
                "yaw",
                0.0,
                "The look's turn about +y, radians; 0 looks along -z.",
            ),
            n("pitch", 0.0, "The look's tilt, radians, positive up."),
            b("run", "Running."),
            b("jump", "A one-tick pulse: jump if grounded."),
            b("primary", "A one-tick pulse for the game's rules."),
            b("secondary", "A one-tick pulse for the game's rules."),
            (
                "slot",
                FieldType::U32,
                FieldValue::U32(0),
                "The selected hotbar slot, 0 to 8.",
            ),
            n("vertical_mps", 0.0, "Vertical speed, m/s, positive up."),
            b("grounded", "Standing on a solid cell."),
            b("rise", "Held: swim up in a liquid."),
            n("swim_mps", d.swim_mps, "Swimming speed up, m/s."),
            b("in_liquid", "A liquid overlaps the body."),
            b("eye_in_liquid", "The eye is under a liquid's surface."),
            (
                "mount",
                FieldType::Entity,
                FieldValue::Entity(None),
                "The board ridden (water.md 50); none walks or swims. The engine clears it on a wipeout.",
            ),
        ],
    )
}

fn walker_fns() -> EngineFns<Walker> {
    fn read(w: &Walker) -> ProjectValues {
        nums(&[
            w.eye_m,
            w.height_m,
            w.half_width_m,
            w.speed_mps,
            w.run_mps,
            w.jump_mps,
            w.gravity_mps2,
            w.forward,
            w.strafe,
            w.yaw,
            w.pitch,
            bit(w.run),
            bit(w.jump),
            bit(w.primary),
            bit(w.secondary),
            f64::from(w.slot),
            w.vertical_mps,
            bit(w.grounded),
            bit(w.rise),
            w.swim_mps,
            bit(w.in_liquid),
            bit(w.eye_in_liquid),
            w.mount.map_or(0.0, |id| id.to_f64()),
        ])
    }
    fn write(w: &mut Walker, v: &ProjectValues) {
        let n = &v.nums;
        (w.eye_m, w.height_m, w.half_width_m) = (n[0], n[1], n[2]);
        (w.speed_mps, w.run_mps, w.jump_mps, w.gravity_mps2) = (n[3], n[4], n[5], n[6]);
        (w.forward, w.strafe, w.yaw, w.pitch) = (n[7], n[8], n[9], n[10]);
        (w.run, w.jump, w.primary, w.secondary) =
            (n[11] != 0.0, n[12] != 0.0, n[13] != 0.0, n[14] != 0.0);
        w.slot = amoris_sim::num::to_u32(n[15]).unwrap_or(0);
        (w.vertical_mps, w.grounded) = (n[16], n[17] != 0.0);
        (w.rise, w.swim_mps) = (n[18] != 0.0, n[19]);
        (w.in_liquid, w.eye_in_liquid) = (n[20] != 0.0, n[21] != 0.0);
        w.mount = amoris_sim::EntityId::from_f64(n[22]).ok();
    }
    EngineFns {
        read,
        write,
        make: |v| {
            let mut w = Walker::default();
            write(&mut w, v);
            w
        },
    }
}

fn anim_graph_schema() -> Result<ComponentSchema, Problem> {
    let d = amoris_assets::anim::AnimGraph::default();
    schema(
        "AnimGraph",
        amoris_assets::anim::AnimGraph::VERSION,
        "The animation graph an entity's model plays (animation.md 3); drive it with ctx.anim.",
        vec![
            (
                "graph",
                FieldType::Str,
                FieldValue::Str(d.graph.into()),
                "A graph file of the project's [animation] graphs.",
            ),
            (
                "playing",
                FieldType::Bool,
                FieldValue::Bool(true),
                "Whether the graph advances.",
            ),
            (
                "speed",
                FieldType::F64,
                FieldValue::F64(1.0),
                "Its playback rate (hit stop sets it low for a few ticks).",
            ),
        ],
    )
}

fn anim_graph_fns() -> EngineFns<amoris_assets::anim::AnimGraph> {
    fn write(g: &mut amoris_assets::anim::AnimGraph, v: &ProjectValues) {
        (g.playing, g.speed) = (v.nums[0] != 0.0, v.nums[1]);
        if let Some(s) = v.strs.first() {
            g.graph = s.to_string();
        }
    }
    EngineFns {
        read: |g| ProjectValues {
            nums: [bit(g.playing), g.speed].into(),
            strs: Box::new([g.graph.as_str().into()]),
        },
        write,
        make: |v| {
            let mut g = amoris_assets::anim::AnimGraph::default();
            write(&mut g, v);
            g
        },
    }
}

fn attach_schema() -> Result<ComponentSchema, Problem> {
    schema(
        "Attach",
        amoris_assets::anim::Attach::VERSION,
        "Held at another entity's joint (a sword in a hand); the engine places it each tick.",
        vec![
            (
                "to",
                FieldType::Entity,
                FieldValue::Entity(None),
                "The entity holding it.",
            ),
            (
                "joint",
                FieldType::Str,
                FieldValue::Str("".into()),
                "The holder's joint.",
            ),
            (
                "position",
                FieldType::Vec3,
                FieldValue::Vec3([0.0; 3]),
                "Offset in the joint's frame, metres.",
            ),
            (
                "rotation",
                FieldType::Quat,
                FieldValue::Quat([0.0, 0.0, 0.0, 1.0]),
                "Turn in the joint's frame.",
            ),
        ],
    )
}

fn attach_fns() -> EngineFns<amoris_assets::anim::Attach> {
    fn write(a: &mut amoris_assets::anim::Attach, v: &ProjectValues) {
        let n = &v.nums;
        a.to = amoris_sim::EntityId::from_f64(n[0]).ok();
        a.position = v3(n, 1);
        a.rotation = [n[4], n[5], n[6], n[7]];
        if let Some(s) = v.strs.first() {
            a.joint = s.to_string();
        }
    }
    EngineFns {
        read: |a| ProjectValues {
            nums: [
                a.to.map_or(0.0, |id| id.to_f64()),
                a.position[0],
                a.position[1],
                a.position[2],
                a.rotation[0],
                a.rotation[1],
                a.rotation[2],
                a.rotation[3],
            ]
            .into(),
            strs: Box::new([a.joint.as_str().into()]),
        },
        write,
        make: |v| {
            let mut a = amoris_assets::anim::Attach::default();
            write(&mut a, v);
            a
        },
    }
}

const FACINGS: [&str; 4] = ["motion", "look", "target", "fixed"];

fn avatar_schema() -> Result<ComponentSchema, Problem> {
    let d = amoris_physics::Avatar::default();
    let n = |name, value, doc| (name, FieldType::F64, FieldValue::F64(value), doc);
    schema(
        "Avatar",
        amoris_physics::Avatar::VERSION,
        "A visible body at a walker's feet that turns toward where it goes (animation.md 6).",
        vec![
            (
                "walker",
                FieldType::Entity,
                FieldValue::Entity(None),
                "The walker it stands for.",
            ),
            (
                "facing",
                FieldType::Enum(FACINGS.map(Box::from).into()),
                FieldValue::Enum(0),
                "How it turns: toward its motion, its look, its target, or not at all.",
            ),
            (
                "target",
                FieldType::Entity,
                FieldValue::Entity(None),
                "What it faces when facing is target.",
            ),
            n(
                "turn_rate",
                d.turn_rate,
                "The fastest it turns, radians a second.",
            ),
            (
                "root_motion",
                FieldType::Bool,
                FieldValue::Bool(true),
                "Whether its graph's root motion moves the walker while it has any.",
            ),
            n(
                "yaw",
                0.0,
                "Which way it faces, as a walker's yaw. Engine-written.",
            ),
            n(
                "speed",
                0.0,
                "Its horizontal speed over the last tick, m/s. Engine-written.",
            ),
            (
                "local",
                FieldType::Vec2,
                FieldValue::Vec2([0.0; 2]),
                "That motion in its own frame: x to its right, y ahead, m/s. Engine-written.",
            ),
            (
                "last",
                FieldType::Vec3,
                FieldValue::Vec3([0.0; 3]),
                "Where it stood after the last tick. Engine-written.",
            ),
        ],
    )
}

fn avatar_fns() -> EngineFns<amoris_physics::Avatar> {
    fn write(a: &mut amoris_physics::Avatar, v: &ProjectValues) {
        let n = &v.nums;
        a.walker = amoris_sim::EntityId::from_f64(n[0]).ok();
        a.facing = if n[1] == 1.0 {
            amoris_physics::Facing::Look
        } else if n[1] == 2.0 {
            amoris_physics::Facing::Target
        } else if n[1] == 3.0 {
            amoris_physics::Facing::Fixed
        } else {
            amoris_physics::Facing::Motion
        };
        a.target = amoris_sim::EntityId::from_f64(n[2]).ok();
        (a.turn_rate, a.root_motion, a.yaw, a.speed) = (n[3], n[4] != 0.0, n[5], n[6]);
        a.local = [n[7], n[8]];
        a.last = v3(n, 9);
    }
    EngineFns {
        read: |a| {
            let facing = match a.facing {
                amoris_physics::Facing::Motion => 0.0,
                amoris_physics::Facing::Look => 1.0,
                amoris_physics::Facing::Target => 2.0,
                amoris_physics::Facing::Fixed => 3.0,
            };
            nums(&[
                a.walker.map_or(0.0, |id| id.to_f64()),
                facing,
                a.target.map_or(0.0, |id| id.to_f64()),
                a.turn_rate,
                bit(a.root_motion),
                a.yaw,
                a.speed,
                a.local[0],
                a.local[1],
                a.last[0],
                a.last[1],
                a.last[2],
            ])
        },
        write,
        make: |v| {
            let mut a = amoris_physics::Avatar::default();
            write(&mut a, v);
            a
        },
    }
}

fn camera_rig_schema() -> Result<ComponentSchema, Problem> {
    let d = amoris_physics::CameraRig::default();
    let n = |name, value, doc| (name, FieldType::F64, FieldValue::F64(value), doc);
    schema(
        "CameraRig",
        amoris_physics::CameraRig::VERSION,
        "A camera on a spring arm behind a walker's look, pulled in short of walls (animation.md 6).",
        vec![
            (
                "target",
                FieldType::Entity,
                FieldValue::Entity(None),
                "The walker whose look it follows.",
            ),
            n(
                "pivot_m",
                d.pivot_m,
                "The arm's pivot over the walker's feet, metres.",
            ),
            n(
                "shoulder_m",
                d.shoulder_m,
                "The camera's offset to the right, metres.",
            ),
            n("distance_m", d.distance_m, "The arm's length, metres."),
            n(
                "min_distance_m",
                d.min_distance_m,
                "The shortest it is pulled to, metres.",
            ),
            n(
                "probe_m",
                d.probe_m,
                "How far from what it meets the camera stays, metres.",
            ),
            n(
                "return_mps",
                d.return_mps,
                "How fast it lengthens again, m/s.",
            ),
            (
                "look_at",
                FieldType::Entity,
                FieldValue::Entity(None),
                "An entity it turns toward (lock-on).",
            ),
            n("length_m", d.length_m, "Its length now. Engine-written."),
            n(
                "shake",
                d.shake,
                "Shake, 0 to 1: raise it on a blow; it falls by shake_decay a second.",
            ),
            n(
                "shake_decay",
                d.shake_decay,
                "How fast the shake falls, a second.",
            ),
            n("shake_m", d.shake_m, "The full shake's tremble, metres."),
            n(
                "shake_rad",
                d.shake_rad,
                "The full shake's turn of the look, radians.",
            ),
        ],
    )
}

fn camera_rig_fns() -> EngineFns<amoris_physics::CameraRig> {
    fn write(r: &mut amoris_physics::CameraRig, v: &ProjectValues) {
        let n = &v.nums;
        r.target = amoris_sim::EntityId::from_f64(n[0]).ok();
        (r.pivot_m, r.shoulder_m, r.distance_m, r.min_distance_m) = (n[1], n[2], n[3], n[4]);
        (r.probe_m, r.return_mps) = (n[5], n[6]);
        r.look_at = amoris_sim::EntityId::from_f64(n[7]).ok();
        r.length_m = n[8];
        (r.shake, r.shake_decay, r.shake_m, r.shake_rad) = (n[9], n[10], n[11], n[12]);
    }
    EngineFns {
        read: |r| {
            nums(&[
                r.target.map_or(0.0, |id| id.to_f64()),
                r.pivot_m,
                r.shoulder_m,
                r.distance_m,
                r.min_distance_m,
                r.probe_m,
                r.return_mps,
                r.look_at.map_or(0.0, |id| id.to_f64()),
                r.length_m,
                r.shake,
                r.shake_decay,
                r.shake_m,
                r.shake_rad,
            ])
        },
        write,
        make: |v| {
            let mut r = amoris_physics::CameraRig::default();
            write(&mut r, v);
            r
        },
    }
}

fn board_schema() -> Result<ComponentSchema, Problem> {
    let d = Board::default();
    let n = |name, value, doc| (name, FieldType::F64, FieldValue::F64(value), doc);
    schema(
        "Board",
        Board::VERSION,
        "A board a Walker rides (water.md 50): planing, rails and fin on its Floater's points; the rider paddles lying down, carves standing up.",
        vec![
            n("area_m2", d.area_m2, "Bottom area, square metres."),
            n(
                "planing",
                d.planing,
                "Normal-force coefficient of the bottom.",
            ),
            n("glide", d.glide, "Friction coefficient along the board."),
            n("rail", d.rail, "Grip coefficient across the board."),
            (
                "fin_at",
                FieldType::Vec3,
                FieldValue::Vec3(d.fin_at),
                "The fin's position in the board's frame (forward -z).",
            ),
            n("fin_m2", d.fin_m2, "Fin area, square metres; 0 for none."),
            n(
                "paddle_n",
                d.paddle_n,
                "Paddling thrust lying down, newtons.",
            ),
            n(
                "carve_nm",
                d.carve_nm,
                "Yaw torque per unit of lean standing, at speed, N m.",
            ),
            n("roll_nm", d.roll_nm, "Roll torque per unit of lean, N m."),
            (
                "seat",
                FieldType::Vec3,
                FieldValue::Vec3(d.seat),
                "The rider's feet in the board's frame.",
            ),
            (
                "standing",
                FieldType::Bool,
                FieldValue::Bool(false),
                "Standing up (the rider's jump toggles it); else lying down to paddle.",
            ),
            (
                "rider",
                FieldType::Entity,
                FieldValue::Entity(None),
                "The Walker riding it, written by the engine.",
            ),
            n(
                "speed",
                0.0,
                "Horizontal speed, m/s, written by the engine.",
            ),
            n(
                "through_water",
                0.0,
                "Speed through the water along the board, m/s, written by the engine.",
            ),
            (
                "wipeouts",
                FieldType::U32,
                FieldValue::U32(0),
                "Times the board has thrown its rider, written by the engine.",
            ),
        ],
    )
}

fn board_fns() -> EngineFns<Board> {
    fn read(b: &Board) -> ProjectValues {
        nums(&[
            b.area_m2,
            b.planing,
            b.glide,
            b.rail,
            b.fin_at[0],
            b.fin_at[1],
            b.fin_at[2],
            b.fin_m2,
            b.paddle_n,
            b.carve_nm,
            b.roll_nm,
            b.seat[0],
            b.seat[1],
            b.seat[2],
            bit(b.standing),
            b.rider.map_or(0.0, |id| id.to_f64()),
            b.speed,
            b.through_water,
            f64::from(b.wipeouts),
        ])
    }
    fn write(b: &mut Board, v: &ProjectValues) {
        let n = &v.nums;
        (b.area_m2, b.planing, b.glide, b.rail) = (n[0], n[1], n[2], n[3]);
        b.fin_at = [n[4], n[5], n[6]];
        (b.fin_m2, b.paddle_n, b.carve_nm, b.roll_nm) = (n[7], n[8], n[9], n[10]);
        b.seat = [n[11], n[12], n[13]];
        b.standing = n[14] != 0.0;
        b.rider = amoris_sim::EntityId::from_f64(n[15]).ok();
        (b.speed, b.through_water) = (n[16], n[17]);
        b.wipeouts = amoris_sim::num::to_u32(n[18]).unwrap_or(0);
    }
    EngineFns {
        read,
        write,
        make: |v| {
            let mut b = Board::default();
            write(&mut b, v);
            b
        },
    }
}

fn rain_schema() -> Result<ComponentSchema, Problem> {
    schema(
        "Rain",
        Rain::VERSION,
        "Rain falling on the scene: water regions whose physics take it gain its depth.",
        vec![
            f(
                "intensity_mm_h",
                "mm of water an hour: 2.5 light, 10 heavy, 50 a cloudburst.",
            ),
            f("drop_mm", "The drops' median diameter, mm."),
        ],
    )
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

fn ui_text_schema() -> Result<ComponentSchema, Problem> {
    let d = UiText::default();
    schema(
        "UiText",
        UiText::VERSION,
        "Text on the screen (its anchor and world offset stay as the scene sets them).",
        vec![
            (
                "text",
                FieldType::Str,
                FieldValue::Str("".into()),
                "The text.",
            ),
            (
                "size",
                FieldType::F64,
                FieldValue::F64(d.size),
                "Height of a line, logical pixels.",
            ),
            (
                "color",
                FieldType::Vec4,
                FieldValue::Vec4(d.color),
                "sRGB colour with alpha.",
            ),
            (
                "offset",
                FieldType::Vec2,
                FieldValue::Vec2(d.offset),
                "Logical pixels from the anchor (+x right, +y down).",
            ),
            (
                "visible",
                FieldType::Bool,
                FieldValue::Bool(true),
                "Whether it is drawn.",
            ),
        ],
    )
}

fn ui_bar_schema() -> Result<ComponentSchema, Problem> {
    let d = UiBar::default();
    let v4 = |name, value: [f64; 4], doc| (name, FieldType::Vec4, FieldValue::Vec4(value), doc);
    schema(
        "UiBar",
        UiBar::VERSION,
        "A bar on the screen (a health or stamina bar); its anchor stays as the scene sets it.",
        vec![
            (
                "value",
                FieldType::F64,
                FieldValue::F64(d.value),
                "How full, out of max.",
            ),
            (
                "max",
                FieldType::F64,
                FieldValue::F64(d.max),
                "A full bar's value.",
            ),
            (
                "size",
                FieldType::Vec2,
                FieldValue::Vec2(d.size),
                "Logical pixels.",
            ),
            v4("color", d.color, "sRGB with alpha: the filled part."),
            v4("back", d.back, "sRGB with alpha: the background."),
            (
                "offset",
                FieldType::Vec2,
                FieldValue::Vec2(d.offset),
                "Logical pixels from the anchor.",
            ),
            (
                "world_offset",
                FieldType::Vec3,
                FieldValue::Vec3(d.world_offset),
                "Metres above the entity, for an entity-anchored bar.",
            ),
            (
                "visible",
                FieldType::Bool,
                FieldValue::Bool(true),
                "Whether it is drawn.",
            ),
        ],
    )
}

fn ui_bar_fns() -> EngineFns<UiBar> {
    fn write(u: &mut UiBar, v: &ProjectValues) {
        let n = &v.nums;
        (u.value, u.max) = (n[0], n[1]);
        u.size = [n[2], n[3]];
        u.color = [n[4], n[5], n[6], n[7]];
        u.back = [n[8], n[9], n[10], n[11]];
        u.offset = [n[12], n[13]];
        u.world_offset = v3(n, 14);
        u.visible = n[17] != 0.0;
    }
    EngineFns {
        read: |u| {
            nums(&[
                u.value,
                u.max,
                u.size[0],
                u.size[1],
                u.color[0],
                u.color[1],
                u.color[2],
                u.color[3],
                u.back[0],
                u.back[1],
                u.back[2],
                u.back[3],
                u.offset[0],
                u.offset[1],
                u.world_offset[0],
                u.world_offset[1],
                u.world_offset[2],
                bit(u.visible),
            ])
        },
        write,
        make: |v| {
            let mut u = UiBar::default();
            write(&mut u, v);
            u
        },
    }
}

fn ui_text_fns() -> EngineFns<UiText> {
    fn read(u: &UiText) -> ProjectValues {
        let c = u.color;
        ProjectValues {
            nums: [
                u.size,
                c[0],
                c[1],
                c[2],
                c[3],
                u.offset[0],
                u.offset[1],
                bit(u.visible),
            ]
            .into(),
            strs: Box::new([u.text.as_str().into()]),
        }
    }
    fn write(u: &mut UiText, v: &ProjectValues) {
        let n = &v.nums;
        u.size = n[0];
        u.color = [n[1], n[2], n[3], n[4]];
        u.offset = [n[5], n[6]];
        u.visible = n[7] != 0.0;
        if let Some(t) = v.strs.first() {
            u.text = t.to_string();
        }
    }
    EngineFns {
        read,
        write,
        make: |v| {
            let mut u = UiText::default();
            write(&mut u, v);
            u
        },
    }
}

fn model_schema() -> Result<ComponentSchema, Problem> {
    let d = Model::default();
    schema(
        "Model",
        Model::VERSION,
        "What an entity looks like: a mesh with a material, scaled at its Transform. A game \
         changes scale, colour, emission and visibility to animate a look; the mesh and material \
         name the asset it was built with.",
        vec![
            (
                "mesh",
                FieldType::Str,
                FieldValue::Str(d.mesh.into()),
                "A primitive or an asset mesh path.",
            ),
            (
                "material",
                FieldType::Str,
                FieldValue::Str(d.material.into()),
                "An asset material path, or empty for the inline material.",
            ),
            (
                "scale",
                FieldType::Vec3,
                FieldValue::Vec3(d.scale),
                "Scale along the entity's own axes.",
            ),
            (
                "color",
                FieldType::Vec4,
                FieldValue::Vec4(d.color),
                "Base colour, linear RGBA; alpha below 1 draws it blended.",
            ),
            (
                "metallic",
                FieldType::F64,
                FieldValue::F64(d.metallic),
                "0 dielectric to 1 metal.",
            ),
            (
                "roughness",
                FieldType::F64,
                FieldValue::F64(d.roughness),
                "0 mirror to 1 rough.",
            ),
            (
                "emissive",
                FieldType::Vec3,
                FieldValue::Vec3(d.emissive),
                "Emitted light, linear RGB.",
            ),
            (
                "cast_shadows",
                FieldType::Bool,
                FieldValue::Bool(d.cast_shadows),
                "Whether it casts shadows.",
            ),
            (
                "visible",
                FieldType::Bool,
                FieldValue::Bool(d.visible),
                "Whether it is drawn.",
            ),
        ],
    )
}

fn model_fns() -> EngineFns<Model> {
    fn read(m: &Model) -> ProjectValues {
        let c = m.color;
        ProjectValues {
            nums: [
                m.scale[0],
                m.scale[1],
                m.scale[2],
                c[0],
                c[1],
                c[2],
                c[3],
                m.metallic,
                m.roughness,
                m.emissive[0],
                m.emissive[1],
                m.emissive[2],
                bit(m.cast_shadows),
                bit(m.visible),
            ]
            .into(),
            strs: Box::new([m.mesh.as_str().into(), m.material.as_str().into()]),
        }
    }
    fn write(m: &mut Model, v: &ProjectValues) {
        let n = &v.nums;
        m.scale = v3(n, 0);
        m.color = [n[3], n[4], n[5], n[6]];
        m.metallic = n[7];
        m.roughness = n[8];
        m.emissive = v3(n, 9);
        m.cast_shadows = n[12] != 0.0;
        m.visible = n[13] != 0.0;
        if let Some(s) = v.strs.first() {
            m.mesh = s.to_string();
        }
        if let Some(s) = v.strs.get(1) {
            m.material = s.to_string();
        }
    }
    EngineFns {
        read,
        write,
        make: |v| {
            let mut m = Model::default();
            write(&mut m, v);
            m
        },
    }
}

fn light_schema() -> Result<ComponentSchema, Problem> {
    let d = Light::default();
    schema(
        "Light",
        Light::VERSION,
        "A light at an entity's Transform: the sun (directional), a lamp (point) or a beam (spot).",
        vec![
            (
                "kind",
                FieldType::Enum(["directional", "point", "spot"].map(Box::from).into()),
                FieldValue::Enum(0),
                "directional, point or spot.",
            ),
            (
                "color",
                FieldType::Vec3,
                FieldValue::Vec3(d.color),
                "Linear RGB.",
            ),
            (
                "intensity",
                FieldType::F64,
                FieldValue::F64(d.intensity),
                "Lux-like for the sun, candela-like for point and spot.",
            ),
            (
                "range",
                FieldType::F64,
                FieldValue::F64(d.range),
                "Point and spot: where it fades out, metres.",
            ),
            (
                "inner_deg",
                FieldType::F64,
                FieldValue::F64(d.inner_deg),
                "Spot: the full-intensity cone's half angle.",
            ),
            (
                "outer_deg",
                FieldType::F64,
                FieldValue::F64(d.outer_deg),
                "Spot: the cone's half angle.",
            ),
            (
                "shadows",
                FieldType::Bool,
                FieldValue::Bool(d.shadows),
                "Whether it casts shadows.",
            ),
        ],
    )
}

fn light_kind_index(kind: LightKind) -> f64 {
    match kind {
        LightKind::Directional => 0.0,
        LightKind::Point => 1.0,
        LightKind::Spot => 2.0,
    }
}

fn light_fns() -> EngineFns<Light> {
    fn read(l: &Light) -> ProjectValues {
        let c = l.color;
        nums(&[
            light_kind_index(l.kind),
            c[0],
            c[1],
            c[2],
            l.intensity,
            l.range,
            l.inner_deg,
            l.outer_deg,
            bit(l.shadows),
        ])
    }
    fn write(l: &mut Light, v: &ProjectValues) {
        let n = &v.nums;
        l.kind = if n[0] == 1.0 {
            LightKind::Point
        } else if n[0] == 2.0 {
            LightKind::Spot
        } else {
            LightKind::Directional
        };
        l.color = v3(n, 1);
        l.intensity = n[4];
        l.range = n[5];
        l.inner_deg = n[6];
        l.outer_deg = n[7];
        l.shadows = n[8] != 0.0;
    }
    EngineFns {
        read,
        write,
        make: |v| {
            let mut l = Light::default();
            write(&mut l, v);
            l
        },
    }
}

fn particle_emitter_schema() -> Result<ComponentSchema, Problem> {
    let d = ParticleEmitter::default();
    let f = |name, value: f64, doc| (name, FieldType::F64, FieldValue::F64(value), doc);
    schema(
        "ParticleEmitter",
        ParticleEmitter::VERSION,
        "GPU particles at an entity (spray, smoke, sparks, a falling stream): presentation only.",
        vec![
            f("rate", d.rate, "Particles per second while emitting."),
            (
                "burst",
                FieldType::U32,
                FieldValue::U32(d.burst),
                "Particles released at once when burst_id changes.",
            ),
            (
                "burst_id",
                FieldType::U32,
                FieldValue::U32(d.burst_id),
                "Change it (any value) to fire the burst again.",
            ),
            (
                "emitting",
                FieldType::Bool,
                FieldValue::Bool(d.emitting),
                "Whether it emits at `rate`.",
            ),
            f("lifetime", d.lifetime, "Seconds a particle lives."),
            f(
                "speed",
                d.speed,
                "Initial speed, m/s, along the entity's +y.",
            ),
            f(
                "spread_deg",
                d.spread_deg,
                "The cone about +y particles leave in.",
            ),
            (
                "acceleration",
                FieldType::Vec3,
                FieldValue::Vec3(d.acceleration),
                "m/s^2 (gravity is [0, -9.8, 0]; smoke rises).",
            ),
            f("drag", d.drag, "Fraction of velocity lost per second."),
            (
                "size",
                FieldType::Vec2,
                FieldValue::Vec2(d.size),
                "Size at birth and at death, metres.",
            ),
            (
                "color_start",
                FieldType::Vec4,
                FieldValue::Vec4(d.color_start),
                "Linear colour and alpha at birth.",
            ),
            (
                "color_end",
                FieldType::Vec4,
                FieldValue::Vec4(d.color_end),
                "Linear colour and alpha at death.",
            ),
            f("radius", d.radius, "Emit from a sphere of this radius."),
        ],
    )
}

fn particle_emitter_fns() -> EngineFns<ParticleEmitter> {
    fn read(p: &ParticleEmitter) -> ProjectValues {
        let (a, s, c, e) = (p.acceleration, p.size, p.color_start, p.color_end);
        nums(&[
            p.rate,
            f64::from(p.burst),
            f64::from(p.burst_id),
            bit(p.emitting),
            p.lifetime,
            p.speed,
            p.spread_deg,
            a[0],
            a[1],
            a[2],
            p.drag,
            s[0],
            s[1],
            c[0],
            c[1],
            c[2],
            c[3],
            e[0],
            e[1],
            e[2],
            e[3],
            p.radius,
        ])
    }
    fn write(p: &mut ParticleEmitter, v: &ProjectValues) {
        let n = &v.nums;
        // Slots of a u32 field hold whole numbers in range (the schema checks writes).
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let u = |x: f64| x as u32;
        p.rate = n[0];
        p.burst = u(n[1]);
        p.burst_id = u(n[2]);
        p.emitting = n[3] != 0.0;
        p.lifetime = n[4];
        p.speed = n[5];
        p.spread_deg = n[6];
        p.acceleration = v3(n, 7);
        p.drag = n[10];
        p.size = [n[11], n[12]];
        p.color_start = [n[13], n[14], n[15], n[16]];
        p.color_end = [n[17], n[18], n[19], n[20]];
        p.radius = n[21];
    }
    EngineFns {
        read,
        write,
        make: |v| {
            let mut p = ParticleEmitter::default();
            write(&mut p, v);
            p
        },
    }
}

fn environment_schema() -> Result<ComponentSchema, Problem> {
    let d = Environment::default();
    schema(
        "Environment",
        Environment::VERSION,
        "The world's sky, ambient light, fog and image settings; the lowest-id entity with it wins.",
        vec![
            (
                "sky",
                FieldType::Enum(["atmosphere", "color"].map(Box::from).into()),
                FieldValue::Enum(0),
                "atmosphere or color.",
            ),
            (
                "sky_color",
                FieldType::Vec3,
                FieldValue::Vec3(d.sky_color),
                "The flat sky's linear RGB.",
            ),
            (
                "ambient",
                FieldType::F64,
                FieldValue::F64(d.ambient),
                "How much the sky lights the scene, 0 to 2.",
            ),
            (
                "gi_intensity",
                FieldType::F64,
                FieldValue::F64(d.gi_intensity),
                "Multiplier for baked or learned diffuse light.",
            ),
            (
                "fog_density",
                FieldType::F64,
                FieldValue::F64(d.fog_density),
                "Exponential height fog per metre; 0 for none.",
            ),
            (
                "fog_color",
                FieldType::Vec3,
                FieldValue::Vec3(d.fog_color),
                "Linear RGB.",
            ),
            (
                "exposure_ev",
                FieldType::F64,
                FieldValue::F64(d.exposure_ev),
                "Exposure, stops.",
            ),
            (
                "bloom",
                FieldType::F64,
                FieldValue::F64(d.bloom),
                "Bloom strength, 0 to 1.",
            ),
        ],
    )
}

fn environment_fns() -> EngineFns<Environment> {
    fn read(e: &Environment) -> ProjectValues {
        nums(&[
            if e.sky == SkyKind::Color { 1.0 } else { 0.0 },
            e.sky_color[0],
            e.sky_color[1],
            e.sky_color[2],
            e.ambient,
            e.gi_intensity,
            e.fog_density,
            e.fog_color[0],
            e.fog_color[1],
            e.fog_color[2],
            e.exposure_ev,
            e.bloom,
        ])
    }
    fn write(e: &mut Environment, v: &ProjectValues) {
        let n = &v.nums;
        e.sky = if n[0] == 1.0 {
            SkyKind::Color
        } else {
            SkyKind::Atmosphere
        };
        e.sky_color = v3(n, 1);
        e.ambient = n[4];
        e.gi_intensity = n[5];
        e.fog_density = n[6];
        e.fog_color = v3(n, 7);
        e.exposure_ev = n[10];
        e.bloom = n[11];
    }
    EngineFns {
        read,
        write,
        make: |v| {
            let mut e = Environment::default();
            write(&mut e, v);
            e
        },
    }
}

fn rain_fns() -> EngineFns<Rain> {
    EngineFns {
        read: |r| nums(&[r.intensity_mm_h, r.drop_mm]),
        write: |r, v| {
            let n = &v.nums;
            (r.intensity_mm_h, r.drop_mm) = (n[0], n[1]);
        },
        make: |v| {
            let n = &v.nums;
            Rain {
                intensity_mm_h: n[0],
                drop_mm: n[1],
            }
        },
    }
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

fn flow_schema(name: &str, version: u32, doc: &str) -> Result<ComponentSchema, Problem> {
    schema(
        name,
        version,
        doc,
        vec![
            (
                "region",
                FieldType::Entity,
                FieldValue::Entity(None),
                "The water region; none: nowhere.",
            ),
            (
                "at",
                FieldType::Vec2,
                FieldValue::Vec2([0.0; 2]),
                "The column under this world point (x, z), metres.",
            ),
            f("rate_m3s", "Cubic metres a second."),
        ],
    )
}

/// The slot functions of a source or a drain: `region` (an id, 0 for none), `at`, `rate_m3s`.
macro_rules! flow_fns {
    ($t:ident) => {
        EngineFns {
            read: |c: &$t| {
                nums(&[
                    c.region.map_or(0.0, |r| r.to_f64()),
                    c.at[0],
                    c.at[1],
                    c.rate_m3s,
                ])
            },
            write: |c: &mut $t, v| {
                let n = &v.nums;
                c.region = amoris_sim::EntityId::from_f64(n[0]).ok();
                c.at = [n[1], n[2]];
                c.rate_m3s = n[3];
            },
            make: |v| {
                let n = &v.nums;
                $t {
                    region: amoris_sim::EntityId::from_f64(n[0]).ok(),
                    at: [n[1], n[2]],
                    rate_m3s: n[3],
                }
            },
        }
    };
}

fn names(world: &World) -> Vec<String> {
    world
        .get_resource::<ComponentRegistry>()
        .map(|r| r.entries().iter().map(|e| e.name.to_string()).collect())
        .unwrap_or_default()
}

/// Rebuilds the world's component registry with script schemas for the engine components scripts
/// read and write, and registers their slot functions. Runs after `amoris_physics::plugin` and
/// before any project component is registered.
pub fn install_script_components(world: &mut World) -> Result<(), Problem> {
    let before = names(world);
    world.insert_resource(ComponentRegistry::default());
    ComponentRegistry::register::<Name>(world, None)?;
    ComponentRegistry::register::<Transform>(world, Some(transform_schema()?))?;
    ComponentRegistry::register::<Velocity>(world, Some(velocity_schema()?))?;
    ComponentRegistry::register::<Boat>(world, Some(boat_schema()?))?;
    ComponentRegistry::register::<Board>(world, Some(board_schema()?))?;
    ComponentRegistry::register::<Wind>(world, Some(wind_schema()?))?;
    ComponentRegistry::register::<Rain>(world, Some(rain_schema()?))?;
    ComponentRegistry::register::<Walker>(world, Some(walker_schema()?))?;
    ComponentRegistry::register::<amoris_physics::Avatar>(world, Some(avatar_schema()?))?;
    ComponentRegistry::register::<amoris_physics::nav::NavGrid>(world, None)?;
    ComponentRegistry::register::<amoris_physics::Buttons>(
        world,
        Some(schema(
            "Buttons",
            amoris_physics::Buttons::VERSION,
            "The game's buttons on a seat's body, a bit each in the order of [player] buttons.",
            vec![
                (
                    "held",
                    FieldType::U32,
                    FieldValue::U32(0),
                    "The buttons down now.",
                ),
                (
                    "pressed",
                    FieldType::U32,
                    FieldValue::U32(0),
                    "Those pressed since the last tick.",
                ),
            ],
        )?),
    )?;
    ComponentRegistry::register::<amoris_physics::CameraRig>(world, Some(camera_rig_schema()?))?;
    ComponentRegistry::register::<Voxels>(world, None)?;
    ComponentRegistry::register::<WaterRegion>(world, None)?;
    ComponentRegistry::register::<WaterResident>(world, None)?;
    ComponentRegistry::register::<amoris_physics::water::WaterColliderBed>(world, None)?;
    ComponentRegistry::register::<Terrain>(world, None)?;
    ComponentRegistry::register::<amoris_physics::terrain::TerrainGround>(world, None)?;
    ComponentRegistry::register::<amoris_physics::water::WaterFocus>(world, None)?;
    ComponentRegistry::register::<WaterOutletStage>(world, None)?;
    ComponentRegistry::register::<WaterFeedback>(
        world,
        Some(schema(
            "WaterFeedback",
            WaterFeedback::VERSION,
            "Opt-in to next-tick horizontal body reactions. Create with {}; observe engine-owned budgets through world.get.",
            vec![],
        )?),
    )?;
    ComponentRegistry::register::<WaterDisplacement>(
        world,
        Some(schema(
            "WaterDisplacement",
            WaterDisplacement::VERSION,
            "Opt-in: Floater points and Walkers push this resident region's water aside. Create with {}; the queued volumes are engine-owned.",
            vec![],
        )?),
    )?;
    ComponentRegistry::register::<WaterProbe>(
        world,
        Some(schema(
            "WaterProbe",
            WaterProbe::VERSION,
            "A resident water query's request. Read completed results with ctx.water.observe.",
            vec![
                (
                    "region",
                    FieldType::Entity,
                    FieldValue::Entity(None),
                    "The resident region; none disables the request.",
                ),
                (
                    "at",
                    FieldType::Vec2,
                    FieldValue::Vec2([0.0; 2]),
                    "World x/z position: Vec2.x and Vec2.y, metres.",
                ),
            ],
        )?),
    )?;
    ComponentRegistry::register::<WaterSource>(
        world,
        Some(flow_schema(
            "WaterSource",
            WaterSource::VERSION,
            "A spring or a pipe: water added at a region's column each tick.",
        )?),
    )?;
    ComponentRegistry::register::<WaterDrain>(
        world,
        Some(flow_schema(
            "WaterDrain",
            WaterDrain::VERSION,
            "A sink: water taken at a region's column each tick, never more than is there.",
        )?),
    )?;
    ComponentRegistry::register::<RigidBody>(world, None)?;
    ComponentRegistry::register::<Collider>(world, None)?;
    ComponentRegistry::register::<ExternalForce>(world, None)?;
    ComponentRegistry::register::<Floater>(world, None)?;
    ComponentRegistry::register::<Vessel>(world, None)?;
    ComponentRegistry::register::<Sail>(world, None)?;
    ComponentRegistry::register::<Hull>(world, None)?;
    ComponentRegistry::register::<Sea>(world, None)?;
    ComponentRegistry::register::<Model>(world, Some(model_schema()?))?;
    ComponentRegistry::register::<Light>(world, Some(light_schema()?))?;
    ComponentRegistry::register::<Camera>(world, None)?;
    ComponentRegistry::register::<Environment>(world, Some(environment_schema()?))?;
    ComponentRegistry::register::<Splat>(world, None)?;
    ComponentRegistry::register::<Animator>(world, None)?;
    ComponentRegistry::register::<amoris_assets::anim::AnimGraph>(
        world,
        Some(anim_graph_schema()?),
    )?;
    ComponentRegistry::register::<amoris_assets::anim::AnimState>(world, None)?;
    ComponentRegistry::register::<amoris_assets::anim::Attach>(world, Some(attach_schema()?))?;
    ComponentRegistry::register::<UiText>(world, Some(ui_text_schema()?))?;
    ComponentRegistry::register::<UiBar>(world, Some(ui_bar_schema()?))?;
    ComponentRegistry::register::<AudioSource>(world, None)?;
    ComponentRegistry::register::<ParticleEmitter>(world, Some(particle_emitter_schema()?))?;
    let after = names(world);
    if before != after {
        return Err(Problem::new(
            "sim.internal",
            format!(
                "The runtime's component registry names {after:?}, the engine's {before:?}; \
                 amoris-runtime's engine.rs must list every engine component."
            ),
            detail([("before", json!(before)), ("after", json!(after))]),
        ));
    }
    register_engine::<Transform>(world, transform_fns())?;
    register_engine::<Velocity>(world, velocity_fns())?;
    register_engine::<Boat>(world, boat_fns())?;
    register_engine::<Board>(world, board_fns())?;
    register_engine::<Wind>(world, wind_fns())?;
    register_engine::<Rain>(world, rain_fns())?;
    register_engine::<Walker>(world, walker_fns())?;
    register_engine::<amoris_physics::Avatar>(world, avatar_fns())?;
    register_engine::<amoris_physics::Buttons>(
        world,
        EngineFns {
            read: |b| nums(&[f64::from(b.held), f64::from(b.pressed)]),
            write: |b, v| {
                b.held = amoris_sim::num::to_u32(v.nums[0]).unwrap_or(0);
                b.pressed = amoris_sim::num::to_u32(v.nums[1]).unwrap_or(0);
            },
            make: |v| amoris_physics::Buttons {
                held: amoris_sim::num::to_u32(v.nums[0]).unwrap_or(0),
                pressed: amoris_sim::num::to_u32(v.nums[1]).unwrap_or(0),
            },
        },
    )?;
    register_engine::<amoris_physics::CameraRig>(world, camera_rig_fns())?;
    register_engine::<amoris_assets::anim::AnimGraph>(world, anim_graph_fns())?;
    register_engine::<amoris_assets::anim::Attach>(world, attach_fns())?;
    register_engine::<UiText>(world, ui_text_fns())?;
    register_engine::<UiBar>(world, ui_bar_fns())?;
    register_engine::<Model>(world, model_fns())?;
    register_engine::<Light>(world, light_fns())?;
    register_engine::<ParticleEmitter>(world, particle_emitter_fns())?;
    register_engine::<Environment>(world, environment_fns())?;
    register_engine::<WaterSource>(world, flow_fns!(WaterSource))?;
    register_engine::<WaterDrain>(world, flow_fns!(WaterDrain))?;
    register_engine::<WaterProbe>(
        world,
        EngineFns {
            read: |p: &WaterProbe| {
                nums(&[p.region.map_or(0.0, |id| id.to_f64()), p.at[0], p.at[1]])
            },
            write: |p: &mut WaterProbe, v| {
                let region = amoris_sim::EntityId::from_f64(v.nums[0]).ok();
                let at = [v.nums[1], v.nums[2]];
                if p.region != region || p.at != at {
                    p.ready = false;
                }
                p.region = region;
                p.at = at;
            },
            make: |v| WaterProbe {
                region: amoris_sim::EntityId::from_f64(v.nums[0]).ok(),
                at: [v.nums[1], v.nums[2]],
                ..WaterProbe::default()
            },
        },
    )?;
    register_engine::<WaterFeedback>(
        world,
        EngineFns {
            read: |_| nums(&[]),
            write: |_, _| {},
            make: |_| WaterFeedback::default(),
        },
    )?;
    register_engine::<WaterDisplacement>(
        world,
        EngineFns {
            read: |_| nums(&[]),
            write: |_, _| {},
            make: |_| WaterDisplacement::default(),
        },
    )?;
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

    #[test]
    fn a_model_slots_round_trip_keeps_its_asset_and_optional_material() {
        let m = Model {
            mesh: "models/mirror.glb#Mirror".into(),
            material: "models/mirror.glb#Silver".into(),
            scale: [1.0, 2.0, 3.0],
            color: [0.1, 0.2, 0.3, 0.9],
            metallic: 0.7,
            roughness: 0.2,
            emissive: [1.0, 0.5, 0.25],
            transmission: Some(0.4),
            ior: Some(1.3),
            cast_shadows: false,
            visible: true,
        };
        let fns = model_fns();
        let v = (fns.read)(&m);
        assert_eq!(v.nums.len(), usize::from(model_schema().unwrap().slots()));
        let mut out = m.clone();
        (fns.write)(&mut out, &v);
        assert_eq!(out, m);
    }

    #[test]
    fn a_light_slots_round_trip_keeps_its_kind_and_cone() {
        let l = Light {
            kind: LightKind::Spot,
            color: [0.2, 0.4, 0.6],
            intensity: 12.0,
            range: 9.0,
            inner_deg: 15.0,
            outer_deg: 31.0,
            shadows: false,
        };
        let fns = light_fns();
        let v = (fns.read)(&l);
        assert_eq!(v.nums.len(), usize::from(light_schema().unwrap().slots()));
        let mut out = Light::default();
        (fns.write)(&mut out, &v);
        assert_eq!(out, l);
    }

    #[test]
    fn a_particle_emitter_slots_round_trip() {
        let p = ParticleEmitter {
            rate: 240.0,
            burst: 12,
            burst_id: 3,
            emitting: false,
            lifetime: 2.5,
            speed: 4.0,
            spread_deg: 8.0,
            acceleration: [0.5, -9.8, 0.0],
            drag: 0.1,
            size: [0.3, 0.9],
            color_start: [0.8, 0.9, 1.0, 0.7],
            color_end: [0.9, 0.95, 1.0, 0.0],
            radius: 0.4,
        };
        let fns = particle_emitter_fns();
        let v = (fns.read)(&p);
        assert_eq!(
            v.nums.len(),
            usize::from(particle_emitter_schema().unwrap().slots())
        );
        let mut out = ParticleEmitter::default();
        (fns.write)(&mut out, &v);
        assert_eq!(out, p);
    }

    #[test]
    fn an_environment_slots_round_trip_keeps_its_sky_fog_and_look() {
        let e = Environment {
            sky: SkyKind::Color,
            sky_color: [0.5, 0.3, 0.2],
            ambient: 0.8,
            gi_intensity: 1.2,
            fog_density: 0.004,
            fog_color: [0.4, 0.3, 0.2],
            exposure_ev: 0.25,
            bloom: 0.3,
            ..Environment::default()
        };
        let fns = environment_fns();
        let v = (fns.read)(&e);
        assert_eq!(
            v.nums.len(),
            usize::from(environment_schema().unwrap().slots())
        );
        let mut out = e.clone();
        (fns.write)(&mut out, &v);
        assert_eq!(out, e);
    }
}
