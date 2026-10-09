//! The renderer's WGSL, kept in `.wgsl` files (charter 4.4) and composed at load time: WGSL has no
//! includes, so `common.wgsl` (and `atmosphere.wgsl` for the sky bake, `splat_common.wgsl` for the
//! splat passes) is prepended.

const COMMON: &str = include_str!("../shaders/common.wgsl");
const ATMOSPHERE: &str = include_str!("../shaders/atmosphere.wgsl");
const SPLAT_COMMON: &str = include_str!("../shaders/splat_common.wgsl");

/// The shaders by name.
pub fn source(name: &str) -> String {
    let body = match name {
        "cull" => include_str!("../shaders/cull.wgsl"),
        "hiz" => include_str!("../shaders/hiz.wgsl"),
        "forward" => {
            return format!(
                "{COMMON}\n{}\n{}",
                include_str!("../shaders/probe_gi.wgsl"),
                include_str!("../shaders/forward.wgsl")
            );
        }
        "sky" => include_str!("../shaders/sky.wgsl"),
        "sky_bake" => {
            return format!(
                "{COMMON}\n{ATMOSPHERE}\n{}",
                include_str!("../shaders/sky_bake.wgsl")
            );
        }
        "ibl" => include_str!("../shaders/ibl.wgsl"),
        "cluster" => include_str!("../shaders/cluster.wgsl"),
        "post" => include_str!("../shaders/post.wgsl"),
        "overlay" => include_str!("../shaders/overlay.wgsl"),
        "skin" => include_str!("../shaders/skin.wgsl"),
        "ui" => include_str!("../shaders/ui.wgsl"),
        "particles" => include_str!("../shaders/particles.wgsl"),
        "splat_sort" => include_str!("../shaders/splat_sort.wgsl"),
        "splat_depth" => include_str!("../shaders/splat_depth.wgsl"),
        "splat_preprocess" => {
            return format!(
                "{COMMON}\n{SPLAT_COMMON}\n{}",
                include_str!("../shaders/splat_preprocess.wgsl")
            );
        }
        "splat_draw" => {
            return format!(
                "{COMMON}\n{SPLAT_COMMON}\n{}",
                include_str!("../shaders/splat_draw.wgsl")
            );
        }
        "splat_tile" => {
            return format!(
                "{COMMON}\n{SPLAT_COMMON}\n{}",
                include_str!("../shaders/splat_tile.wgsl")
            );
        }
        "splat_composite" => include_str!("../shaders/splat_composite.wgsl"),
        other => panic!("no shader named {other}"),
    };
    format!("{COMMON}\n{body}")
}

pub fn module(device: &wgpu::Device, name: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(name),
        source: wgpu::ShaderSource::Wgsl(source(name).into()),
    })
}

/// Compute pipelines' options. The renderer's compute shaders initialize the workgroup memory they
/// read, so naga's automatic zeroing is off: it is redundant, and on MoltenVK its SPIR-V for
/// workgroup arrays compiles to MSL that names an undeclared `gl_WorkGroupSize`, which loses the
/// device. Browsers zero workgroup memory regardless (WebGPU requires it).
pub fn compute_options() -> wgpu::PipelineCompilationOptions<'static> {
    wgpu::PipelineCompilationOptions {
        zero_initialize_workgroup_memory: false,
        ..Default::default()
    }
}
