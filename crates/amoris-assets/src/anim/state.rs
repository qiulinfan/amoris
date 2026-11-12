//! The components of an animated character (animation.md 4): the graph it plays (`AnimGraph`), the
//! graph's state (`AnimState`, persisted, so restores, forks and replays reproduce it), and the pose
//! the last tick evaluated (`AnimPose`, derived and never persisted), and its morph target weights
//! (`MorphWeights`, persisted).

use bevy_ecs::prelude::Component;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::math::Xform;

/// The graph an entity's model plays.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AnimGraph {
    /// A graph file of the project's `[animation] graphs`.
    pub graph: String,
    pub playing: bool,
    /// Playback rate of the whole graph (hit stop sets it low for a few ticks).
    pub speed: f64,
}

impl Default for AnimGraph {
    fn default() -> Self {
        AnimGraph {
            graph: String::new(),
            playing: true,
            speed: 1.0,
        }
    }
}

/// A parameter's value (a boolean is 0 or 1).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AnimParam {
    pub name: String,
    pub value: f64,
}

/// A state machine layer: its state and how far into it (seconds; cycles for a blend space), and
/// the state it fades from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct LayerState {
    pub state: String,
    pub time: f64,
    /// The state faded from ("" when none) and its time, frozen.
    pub from: String,
    pub from_time: f64,
    pub fade: f64,
    pub fade_len: f64,
}

/// A slot and the montage it plays ("" when none).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SlotState {
    pub slot: String,
    pub montage: String,
    pub section: String,
    /// Seconds into the section's clip.
    pub time: f64,
    /// The section after this one ("" ends the montage).
    pub next: String,
    pub rate: f64,
    /// Seconds since the montage began.
    pub age: f64,
    /// Seconds into its fade-out, -1 while it plays.
    pub out: f64,
    pub fade_out: f64,
    /// The interrupted montage's clip, its frozen time and the slot's weight then.
    pub from_clip: String,
    pub from_time: f64,
    pub from_weight: f64,
}

impl Default for SlotState {
    fn default() -> Self {
        SlotState {
            slot: String::new(),
            montage: String::new(),
            section: String::new(),
            time: 0.0,
            next: String::new(),
            rate: 1.0,
            age: 0.0,
            out: -1.0,
            fade_out: 0.0,
            from_clip: String::new(),
            from_time: 0.0,
            from_weight: 0.0,
        }
    }
}

/// A request a script queued (`ctx.anim.play` and the like); the next evaluation applies it.
/// `op` is `play` (`montage`, `section` optional, `rate`), `next` (`section`), `jump` (`section`)
/// or `stop` (`fade`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AnimCommand {
    pub op: String,
    pub montage: String,
    pub section: String,
    pub rate: f64,
    pub fade: f64,
}

/// A socket's place in the world after the last evaluation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Socket {
    pub joint: String,
    pub position: [f64; 3],
    pub rotation: [f64; 4],
}

/// The state of an entity's graph.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AnimState {
    /// The graph this state is of; a different `AnimGraph.graph` starts it afresh.
    pub graph: String,
    pub params: Vec<AnimParam>,
    pub layers: Vec<LayerState>,
    pub slots: Vec<SlotState>,
    pub commands: Vec<AnimCommand>,
    pub sockets: Vec<Socket>,
    /// The body's motion from root-motion clips in the last tick, in the world, metres.
    pub root_motion: [f64; 3],
}

/// The pose the last tick evaluated, the skeleton's joints in its order, and its morph weights,
/// flat in the same order (derived each tick).
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct AnimPose {
    pub joints: Vec<Xform>,
    pub morph: Vec<f32>,
}

/// A morph target's weight, by the target's name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct MorphWeight {
    pub name: String,
    pub weight: f64,
}

/// An animated model's morph target weights (animation.md 5a): scripts' overrides by target name,
/// and each named target's weight after the last evaluation.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct MorphWeights {
    /// Weights set by name: every target of that name takes it in place of the graph's.
    pub overrides: Vec<MorphWeight>,
    /// Each named target's weight after the last evaluation, overrides included (engine-written).
    pub weights: Vec<MorphWeight>,
}

/// A thing held by another's joint (a sword in a hand): its place is the parent's, then the
/// joint's, then this offset.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Attach {
    /// The entity holding it.
    pub to: Option<amoris_sim::EntityId>,
    pub joint: String,
    pub position: [f64; 3],
    pub rotation: [f64; 4],
}

impl Default for Attach {
    fn default() -> Self {
        Attach {
            to: None,
            joint: String::new(),
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

/// A script's goal for one IK node (animation.md 6a): points in the world and a weight.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct IkGoal {
    /// The IK node's name in the graph.
    pub node: String,
    /// Where a two-bone tip reaches or a look-at looks; none leaves those nodes idle.
    pub target: Option<[f64; 3]>,
    /// A point the two-bone bend turns toward.
    pub pole: Option<[f64; 3]>,
    /// Multiplies the node's own weight, 0 to 1.
    pub weight: f64,
}

impl Default for IkGoal {
    fn default() -> Self {
        IkGoal {
            node: String::new(),
            target: None,
            pole: None,
            weight: 1.0,
        }
    }
}

/// An animated body's inverse kinematics (animation.md 6a): scripts' goals by node, and the feet
/// node's state (engine-written).
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AnimIk {
    pub goals: Vec<IkGoal>,
    /// The feet node's weight now, moving toward what its speed and grounding want.
    pub weight: f64,
    /// Each leg's ground over the body's origin, metres, as the feet follow it.
    pub ground_m: Vec<f64>,
}
