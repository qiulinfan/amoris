//! The visual components (charter 4.4): what an entity looks like, the lights, the cameras and the
//! environment. They are plain data in the world like every other component, so they are
//! persisted, hashed, edited through `world.edit`, seen by agents in `world.get`, and forked with
//! the rest of the world; the renderer only reads them through the render feed (`frame`).
//!
//! Numbers are `f64` like every simulation component (numeric.md 3.1); the feed converts them to
//! `f32` for the GPU. A pose comes from the entity's `Transform` (pocket-physics); a visual
//! component adds only what the pose does not say.

use bevy_ecs::prelude::Component;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What an entity looks like: a mesh drawn with a material, at its `Transform`, scaled.
///
/// `mesh` names a built-in primitive (`cube`, `sphere`, `plane`, `cylinder`, `capsule`, `cone`,
/// `torus`) or a mesh of an asset: `models/boat.glb` (every mesh of the file's default scene) or
/// `models/boat.glb#Hull` (the mesh named Hull; `#2` is the third). `material` is empty for the
/// inline parameters below, or names an asset's material (`models/boat.glb#Sail`), whose values
/// the inline `color` multiplies.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Model {
    /// A primitive name or an asset mesh path.
    pub mesh: String,
    /// Empty for the inline material, or an asset material path.
    pub material: String,
    /// Scale along the entity's own axes.
    pub scale: [f64; 3],
    /// Base color, linear RGBA; multiplies the material's.
    pub color: [f64; 4],
    /// 0 dielectric to 1 metal.
    pub metallic: f64,
    /// 0 mirror to 1 rough.
    pub roughness: f64,
    /// Emitted light, linear RGB in nits-like units (1 = as bright as a lit white surface).
    pub emissive: [f64; 3],
    /// Whether it casts shadows.
    pub cast_shadows: bool,
    /// Whether it is drawn.
    pub visible: bool,
}

impl Default for Model {
    fn default() -> Self {
        Model {
            mesh: "cube".into(),
            material: String::new(),
            scale: [1.0; 3],
            color: [1.0; 4],
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0; 3],
            cast_shadows: true,
            visible: true,
        }
    }
}

/// What kind of light.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LightKind {
    /// Parallel light from infinitely far along the entity's -z (the sun).
    #[default]
    Directional,
    /// From the entity's origin in every direction.
    Point,
    /// From the entity's origin in a cone along its -z.
    Spot,
}

/// A light at the entity's `Transform`.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Light {
    pub kind: LightKind,
    /// Linear RGB.
    pub color: [f64; 3],
    /// Directional: illuminance in lux-like units (the sun at noon is about 10). Point and spot:
    /// luminous intensity, candela-like.
    pub intensity: f64,
    /// Point and spot: where the light fades to nothing, metres.
    pub range: f64,
    /// Spot: the full-intensity cone's half angle, degrees.
    pub inner_deg: f64,
    /// Spot: the cone's half angle, degrees.
    pub outer_deg: f64,
    /// Whether it casts shadows (directional: cascaded shadow maps).
    pub shadows: bool,
}

impl Default for Light {
    fn default() -> Self {
        Light {
            kind: LightKind::Directional,
            color: [1.0, 0.96, 0.9],
            intensity: 6.0,
            range: 20.0,
            inner_deg: 25.0,
            outer_deg: 35.0,
            shadows: true,
        }
    }
}

/// A camera at the entity's `Transform`, looking along its -z with +y up.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Camera {
    /// Vertical field of view, degrees.
    pub fov_deg: f64,
    /// Near plane, metres.
    pub near: f64,
    /// Far plane, metres; 0 for infinitely far (reversed-Z makes that free).
    pub far: f64,
    /// The camera the game is shown through; the lowest-id active camera wins.
    pub active: bool,
    /// Exposure compensation, stops.
    pub exposure_ev: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            fov_deg: 60.0,
            near: 0.1,
            far: 0.0,
            active: true,
            exposure_ev: 0.0,
        }
    }
}

/// How the sky is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkyKind {
    /// A physically based atmosphere lit by the directional light.
    #[default]
    Atmosphere,
    /// A flat color.
    Color,
}

/// The world's sky, ambient light, fog and image settings; one per world (the lowest id wins).
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Environment {
    pub sky: SkyKind,
    /// `sky: color`'s color, linear RGB.
    pub sky_color: [f64; 3],
    /// How much the sky lights the scene (image-based light), 0 to 2.
    pub ambient: f64,
    /// Project-relative baked GI JSON asset; empty disables world-space baked probes.
    pub baked_gi: String,
    /// Project-relative neural GI JSON asset; takes precedence over `baked_gi` when nonempty.
    pub neural_gi: String,
    /// Multiplier for diffuse light reconstructed from baked probes or a learned field.
    pub gi_intensity: f64,
    /// Exponential height fog density per metre; 0 for none.
    pub fog_density: f64,
    /// Linear RGB.
    pub fog_color: [f64; 3],
    /// Exposure, stops (0 = the default exposure for a sunlit scene).
    pub exposure_ev: f64,
    /// Bloom strength, 0 to 1.
    pub bloom: f64,
}

impl Default for Environment {
    fn default() -> Self {
        Environment {
            sky: SkyKind::Atmosphere,
            sky_color: [0.35, 0.5, 0.75],
            ambient: 1.0,
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.0,
            fog_color: [0.6, 0.7, 0.8],
            exposure_ev: 0.0,
            bloom: 0.15,
        }
    }
}

/// A 3D Gaussian Splatting cloud (charter 4.4, neural rendering) at the entity's `Transform`:
/// `asset` names a `.ply` (the 3DGS training output layout) or `.splat` file.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Splat {
    pub asset: String,
    /// Uniform scale.
    pub scale: f64,
    pub visible: bool,
}

impl Default for Splat {
    fn default() -> Self {
        Splat {
            asset: String::new(),
            scale: 1.0,
            visible: true,
        }
    }
}

/// A skeletal animation playing on the entity's model (a skinned glTF mesh): which clip, how far
/// into it, how fast. The simulation advances `time` every tick (deterministically), so a fork, a
/// replay and an agent reading the world all see the same pose; the renderer evaluates the clip at
/// that time.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Animator {
    /// The clip's name in the model's file (`idle`, `run`), or its index (`0`).
    pub clip: String,
    /// Seconds into the clip.
    pub time: f64,
    /// Playback rate; 1 is real time, negative plays backwards.
    pub speed: f64,
    /// Wrap at the clip's end (else hold the last pose).
    pub looped: bool,
    pub playing: bool,
}

impl Default for Animator {
    fn default() -> Self {
        Animator {
            clip: String::new(),
            time: 0.0,
            speed: 1.0,
            looped: true,
            playing: true,
        }
    }
}

/// Where on the screen a UI element hangs from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UiAnchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
    /// Above the entity in the world: its `Transform` plus `world_offset`, projected each frame
    /// (name tags, damage numbers).
    Entity,
}

/// Text on the screen, or over the entity. Game state like everything else: agents read what the
/// HUD says from the world, and a fork or replay shows the same text.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct UiText {
    pub text: String,
    /// Height of a line, logical pixels.
    pub size: f64,
    /// sRGB colour with alpha.
    pub color: [f64; 4],
    pub anchor: UiAnchor,
    /// Logical pixels from the anchor (+x right, +y down).
    pub offset: [f64; 2],
    /// `anchor: entity`: metres from the entity's origin.
    pub world_offset: [f64; 3],
    pub visible: bool,
}

impl Default for UiText {
    fn default() -> Self {
        UiText {
            text: String::new(),
            size: 20.0,
            color: [1.0, 1.0, 1.0, 1.0],
            anchor: UiAnchor::TopLeft,
            offset: [16.0, 16.0],
            world_offset: [0.0, 2.0, 0.0],
            visible: true,
        }
    }
}

/// A bar (health, mana, progress) on the screen or over the entity.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct UiBar {
    pub value: f64,
    pub max: f64,
    /// Logical pixels.
    pub size: [f64; 2],
    /// sRGB with alpha: the filled part, then the background.
    pub color: [f64; 4],
    pub back: [f64; 4],
    pub anchor: UiAnchor,
    pub offset: [f64; 2],
    pub world_offset: [f64; 3],
    pub visible: bool,
}

impl Default for UiBar {
    fn default() -> Self {
        UiBar {
            value: 1.0,
            max: 1.0,
            size: [60.0, 7.0],
            color: [0.35, 0.85, 0.35, 1.0],
            back: [0.0, 0.0, 0.0, 0.6],
            anchor: UiAnchor::Entity,
            offset: [0.0, 0.0],
            world_offset: [0.0, 2.2, 0.0],
            visible: true,
        }
    }
}

/// A sound playing from the entity: a looping ambience, an engine, a fire. `clip` is a file of the
/// project (wav, ogg, mp3, flac) or a synthesized `sfx:<preset>` (coin, jump, hit, explosion,
/// laser, powerup, blip, click, wind, surf). One-shot sounds are events instead (`sound`, or the
/// project's `[sounds]` table mapping event names to clips).
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AudioSource {
    pub clip: String,
    /// Linear gain, 0 to 4.
    pub volume: f64,
    /// Playback rate (1 = as recorded).
    pub pitch: f64,
    pub looped: bool,
    pub playing: bool,
    /// Heard from the entity's position (falls off with distance) rather than everywhere.
    pub spatial: bool,
}

impl Default for AudioSource {
    fn default() -> Self {
        AudioSource {
            clip: String::new(),
            volume: 1.0,
            pitch: 1.0,
            looped: true,
            playing: true,
            spatial: true,
        }
    }
}

/// A particle emitter at the entity (sparks, smoke, spray, fire). Particles are presentation: the
/// renderer simulates them on the GPU; the emitter's settings are world state like the rest.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ParticleEmitter {
    /// Particles per second while emitting.
    pub rate: f64,
    /// Particles released at once when `burst_id` changes (an explosion).
    pub burst: u32,
    /// Change it (any value) to fire `burst` again.
    pub burst_id: u32,
    pub emitting: bool,
    /// Seconds a particle lives.
    pub lifetime: f64,
    /// Initial speed, m/s, along the entity's +y within `spread_deg` of it.
    pub speed: f64,
    pub spread_deg: f64,
    /// Acceleration, m/s^2 (gravity is [0, -9.8, 0]; smoke rises).
    pub acceleration: [f64; 3],
    /// Fraction of velocity lost per second.
    pub drag: f64,
    /// Size at birth and death, metres.
    pub size: [f64; 2],
    /// Linear colour with alpha at birth and death; alpha 0 with bright colour adds light (sparks).
    pub color_start: [f64; 4],
    pub color_end: [f64; 4],
    /// Emit from a sphere of this radius around the entity.
    pub radius: f64,
}

impl Default for ParticleEmitter {
    fn default() -> Self {
        ParticleEmitter {
            rate: 30.0,
            burst: 0,
            burst_id: 0,
            emitting: true,
            lifetime: 1.5,
            speed: 3.0,
            spread_deg: 25.0,
            acceleration: [0.0, -2.0, 0.0],
            drag: 0.3,
            size: [0.15, 0.4],
            color_start: [1.0, 0.8, 0.4, 0.9],
            color_end: [0.4, 0.4, 0.4, 0.0],
            radius: 0.1,
        }
    }
}
