//! The renderer (charter 4.4): wgpu with WGSL, Metal and Vulkan natively and WebGPU in the browser,
//! one code path for all three. GPU-driven (every instance culled on the GPU, indirect draws: one
//! multi-draw per pass and variant natively, one draw per batch on WebGPU's baseline), physically based clustered forward shading with cascaded shadows, a baked
//! atmosphere with image-based light, HDR bloom and AgX. It reads the world only through the render
//! feed (`amoris_assets::frame`), so it can never change the simulation.

#[cfg(all(feature = "window", not(target_arch = "wasm32")))]
pub mod app;
pub mod batches;
mod blit;
pub mod camera;
pub mod demo;
pub mod fluid;
mod fluid_surface;
pub mod gi;
pub mod gpu;
mod gtao;
pub mod loader;
pub mod lod;
pub mod materials;
pub mod meshes;
pub mod neural;
pub mod occlusion;
mod ocean;
mod ocean_waves;
pub mod overlay;
mod par;
mod particles;
pub mod picking;
mod post;
pub mod prepass;
pub mod profiler;
mod rain;
mod readback;
#[cfg(not(target_arch = "wasm32"))]
mod renderdoc;
pub mod renderer;
pub mod rt_shadows;
pub mod scene;
mod shaders;
mod shadows;
/// The spike's host-made field (docs/spikes/shallow-water.md), drawn as a water region.
pub mod shallow_water {
    pub use crate::water::ShallowWaterView;
}
mod skinning;
mod sky;
pub mod splash;
pub mod splat;
#[cfg(not(target_arch = "wasm32"))]
pub mod stderr_log;
pub mod taa;
pub mod terrain;
pub mod trace;
mod transparent;
mod ui;
mod upload;
pub mod voxels;
pub mod water;
pub mod water_acoustics;
mod water_caustics;
mod water_compact;
pub mod water_compute;
mod water_edits;
pub mod water_foam;
pub mod water_lip;
mod water_medium;
pub mod water_mesh;
pub mod water_nh;
mod water_occupancy;
mod water_optics;
pub mod water_reactions;
mod water_replica;
pub mod water_resident;
pub mod water_ripples;
pub mod water_samples;
pub mod water_summary;
mod water_surface;
mod water_topology;
mod water_waves;

pub use camera::CameraState;
pub use gpu::{BackendChoice, Gpu, GpuError};
pub use loader::AssetSource;
pub use lod::{DrawCounts, LodMode, LodSettings};
pub use occlusion::{OcclusionMode, OcclusionStats};
pub use post::{Antialiasing, AoNormals, Gtao, SceneFormat, defaults_for};
pub use prepass::PrepassMode;
pub use renderer::{FrameStats, Renderer, web_time};
pub use shadows::ShadowCull;

/// A renderer shader's composed WGSL (tools and backend probes).
pub fn shader_source(name: &str) -> String {
    shaders::source(name)
}

pub mod water_spray;

pub mod water_ledge;
