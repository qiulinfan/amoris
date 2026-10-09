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
        "gtao" => include_str!("../shaders/gtao.wgsl"),
        "taa" => include_str!("../shaders/taa.wgsl"),
        other => panic!("no shader named {other}"),
    };
    format!("{COMMON}\n{body}")
}

/// The forward shader of the neural-texture variants: forward.wgsl with the decoder for `layout`
/// (half precision when `f16`) and forward_neural.wgsl's `fs_neural`, its sun shadows traced when
/// `traced` (rt_shadows.rs). The other variants keep `source("forward")`, which has no decoder.
pub fn forward_neural(
    layout: &pocket_assets::neural::NeuralLayout,
    f16: bool,
    traced: bool,
) -> String {
    let forward = format!(
        "{}\n{}",
        source("forward"),
        include_str!("../shaders/forward_neural.wgsl")
    );
    let body = crate::neural::decoder_library(layout, f16, &forward);
    let body = if traced {
        crate::rt_shadows::RtShadows::traced(&body)
    } else {
        body
    };
    format!(
        "{}{}{body}",
        if f16 { "enable f16;\n" } else { "" },
        if traced {
            "enable wgpu_ray_query;\n"
        } else {
            ""
        }
    )
}

pub fn module(device: &wgpu::Device, name: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(name),
        source: wgpu::ShaderSource::Wgsl(source(name).into()),
    })
}

/// `name`'s source for an opaque pass of `samples` samples per pixel. The shaders that read the
/// opaque pass's depth declare it `texture_depth_multisampled_2d`; with one sample it is a
/// `texture_depth_2d`, whose `textureLoad` takes a mip level (0) where the multisampled one takes a
/// sample index (0 too), and which has one sample.
pub fn depth_source(name: &str, samples: u32) -> String {
    let s = source(name);
    if samples > 1 {
        return s;
    }
    s.replace("texture_depth_multisampled_2d", "texture_depth_2d")
        .replace("textureNumSamples(depth)", "1u")
}

pub fn depth_module(device: &wgpu::Device, name: &str, samples: u32) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(name),
        source: wgpu::ShaderSource::Wgsl(depth_source(name, samples).into()),
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
