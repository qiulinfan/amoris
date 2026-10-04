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
