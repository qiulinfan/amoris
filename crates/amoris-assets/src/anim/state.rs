//! The components of an animated character (animation.md 4): the graph it plays (`AnimGraph`), the
//! graph's state (`AnimState`, persisted, so restores, forks and replays reproduce it), and the pose
//! the last tick evaluated (`AnimPose`, derived and never persisted).

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

/// The pose the last tick evaluated, the skeleton's joints in its order (derived each tick).
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct AnimPose {
    pub joints: Vec<Xform>,
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
