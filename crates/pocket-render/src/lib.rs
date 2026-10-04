//! The renderer (charter 4.4): wgpu with WGSL, Metal and Vulkan natively and WebGPU in the browser,
//! one code path for all three. GPU-driven (every instance culled on the GPU, one indirect
//! multi-draw per pass), physically based clustered forward shading with cascaded shadows, a baked
//! atmosphere with image-based light, HDR bloom and AgX. It reads the world only through the render
//! feed (`pocket_assets::frame`), so it can never change the simulation.

#[cfg(all(feature = "window", not(target_arch = "wasm32")))]
pub mod app;
mod blit;
pub mod camera;
pub mod gpu;
pub mod loader;
pub mod materials;
pub mod meshes;
mod ocean;
mod post;
pub mod profiler;
pub mod renderer;
pub mod scene;
mod shaders;
mod shadows;
mod sky;

pub use camera::CameraState;
pub use gpu::{BackendChoice, Gpu, GpuError};
pub use loader::AssetSource;
pub use renderer::{FrameStats, Renderer, web_time};

/// A renderer shader's composed WGSL (tools and backend probes).
pub fn shader_source(name: &str) -> String {
    shaders::source(name)
}
