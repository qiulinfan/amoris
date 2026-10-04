//! The renderer's WGSL, kept in `.wgsl` files (charter 4.4) and composed at load time: WGSL has no
//! includes, so `common.wgsl` (and `atmosphere.wgsl` for the sky bake) is prepended.

const COMMON: &str = include_str!("../shaders/common.wgsl");
const ATMOSPHERE: &str = include_str!("../shaders/atmosphere.wgsl");

/// The shaders by name.
pub fn source(name: &str) -> String {
    let body = match name {
        "cull" => include_str!("../shaders/cull.wgsl"),
        "forward" => include_str!("../shaders/forward.wgsl"),
        "sky" => include_str!("../shaders/sky.wgsl"),
        "sky_bake" => return format!("{COMMON}\n{ATMOSPHERE}\n{}", include_str!("../shaders/sky_bake.wgsl")),
        "ibl" => include_str!("../shaders/ibl.wgsl"),
        "cluster" => include_str!("../shaders/cluster.wgsl"),
        "post" => include_str!("../shaders/post.wgsl"),
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
