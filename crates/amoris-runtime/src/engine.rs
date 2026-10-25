//! Engine components by name (shared/contract/mcp.md 6.1; script-host.md 7.1): their JSON forms
//! for scenes and `world_edit`, and the slot accessors that let scripts read and write the ones a
//! game's rules need.
//!
//! `amoris-physics` registers its components without a script schema, since the `ScriptComponent`
//! derive that would generate their slot functions does not live in a crate it may depend on yet
//! (architecture.md 11, choice 7). The runtime, which links both physics and the script host,
//! therefore rebuilds the world's component registry with schemas for `Transform`, `Velocity`,
//! `Boat`, `Wind`, `Walker`, UI and water request components, and registers
//! hand-written slot functions for them (architecture.md 11, Slice
//! 1). The rebuilt registry must name exactly the components the old one did, so a component
//! physics adds later fails the game's construction instead of vanishing from it.

use amoris_assets::{
    Animator, AudioSource, Camera, Environment, Light, Model, ParticleEmitter, Splat, UiBar, UiText,
};
use amoris_contract::{CheckOptions, Problem, Shape, detail};
use amoris_physics::water::displacement::WaterDisplacement;
use amoris_physics::water::feedback::WaterFeedback;
use amoris_physics::water::resident::{WaterOutletStage, WaterProbe, WaterResident};
use amoris_physics::{
    Board, Boat, Collider, ExternalForce, Floater, Hull, RigidBody, Sail, Sea, Transform, Trim,
    Velocity, Vessel, Walker, WaterDrain, WaterRegion, WaterSource, Wind,
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
pub fn engine_components() -> [EngineComponent; 37] {
    [
        entry::<amoris_interface::perception::Observer>(),
        entry::<amoris_interface::perception::Perceivable>(),
        entry::<amoris_interface::perception::Occluder>(),
        entry::<Animator>(),
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
        entry::<Wind>(),
        voxels_entry(),
        water_entry(),
        entry::<WaterResident>(),
        entry::<amoris_physics::water::WaterColliderBed>(),
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
    ComponentRegistry::register::<Walker>(world, Some(walker_schema()?))?;
    ComponentRegistry::register::<Voxels>(world, None)?;
    ComponentRegistry::register::<WaterRegion>(world, None)?;
    ComponentRegistry::register::<WaterResident>(world, None)?;
    ComponentRegistry::register::<amoris_physics::water::WaterColliderBed>(world, None)?;
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
    ComponentRegistry::register::<Model>(world, None)?;
    ComponentRegistry::register::<Light>(world, None)?;
    ComponentRegistry::register::<Camera>(world, None)?;
    ComponentRegistry::register::<Environment>(world, None)?;
    ComponentRegistry::register::<Splat>(world, None)?;
    ComponentRegistry::register::<Animator>(world, None)?;
    ComponentRegistry::register::<UiText>(world, Some(ui_text_schema()?))?;
    ComponentRegistry::register::<UiBar>(world, None)?;
    ComponentRegistry::register::<AudioSource>(world, None)?;
    ComponentRegistry::register::<ParticleEmitter>(world, None)?;
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
    register_engine::<Walker>(world, walker_fns())?;
    register_engine::<UiText>(world, ui_text_fns())?;
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
}
