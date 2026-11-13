use amoris_render::{BackendChoice, Gpu};
fn try_tex(gpu: &Gpu, layers: u32, fmt: wgpu::TextureFormat) -> wgpu::Texture {
    gpu.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let t = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("probe"),
        size: wgpu::Extent3d { width: 2048, height: 2048, depth_or_array_layers: layers },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: fmt,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let e = pollster::block_on(gpu.device.pop_error_scope());
    eprintln!("PROBE {fmt:?} layers {layers}: {}", if e.is_some() { "OOM" } else { "ok" });
    t
}
#[test]
fn probe() {
    let gpu = Gpu::headless(BackendChoice::from_env()).unwrap();
    for l in [4, 8, 12, 14, 15, 16] {
        let _t = try_tex(&gpu, l, wgpu::TextureFormat::Depth32Float);
    }
    // Two at once: cascades then locals.
    let _a = try_tex(&gpu, 4, wgpu::TextureFormat::Depth32Float);
    let _b = try_tex(&gpu, 12, wgpu::TextureFormat::Depth32Float);
    let _c = try_tex(&gpu, 12, wgpu::TextureFormat::Depth32Float);
    let _d = try_tex(&gpu, 12, wgpu::TextureFormat::Depth32Float);
}
