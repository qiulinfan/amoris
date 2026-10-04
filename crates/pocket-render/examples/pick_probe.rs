//! Picks a few pixels of the many_cubes dense scene headless and prints the entities found.

fn main() {
    let gpu = pocket_render::Gpu::headless(pocket_render::BackendChoice::from_env()).expect("gpu");
    let mut r = pocket_render::Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 800, 450);
    r.apply(pocket_render::demo::many_cubes(20_000, true, false), 0.0);
    r.set_camera_override(Some(pocket_render::demo::cubes_camera(0, true)));
    for (x, y) in [(400, 225), (100, 50), (700, 400)] {
        r.request_pick(x, y);
        let mut got = None;
        for i in 0..20 {
            let _ = r.capture_rgba(i as f64 / 60.0);
            if let Some(p) = r.take_pick() {
                got = Some(p);
                break;
            }
        }
        println!("pick ({x}, {y}) -> {got:?}   [{}]", r.pick_debug());
    }
    r.request_visible();
    for i in 0..20 {
        let _ = r.capture_rgba(i as f64 / 60.0);
        if let Some(v) = r.take_visible() {
            println!("visible: {} entities, top {:?}", v.len(), &v[..v.len().min(3)]);
            break;
        }
    }
}
