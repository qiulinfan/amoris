//! Ray-traced sun shadows (rt_shadows.rs, docs/spec/rt-shadows.md) fall where the cascaded ones
//! do. The mixed demo scene (every pipeline variant, alpha-masked meshes) and three skinned
//! characters are drawn three times on each native backend whose adapter has ray queries: without
//! sun shadows, with cascades, and with ray-traced shadows on a device that carries ray queries.
//! The pixels each shadow path darkens must largely coincide. Skips (with a note) without a GPU or
//! without ray queries.

use pocket_assets::frame::{AnimView, InstanceUpdate, Look, Pose, RenderFrame};
use pocket_render::rt_shadows::RtShadowStats;
use pocket_render::{BackendChoice, Gpu, Renderer, demo};

const N: u32 = 4;
const W: u32 = 480;
const H: u32 = 270;
const HERO: &str = "models/hero.glb";

fn scene(shadows: bool) -> RenderFrame {
    let mut frame = demo::mixed(N);
    for light in frame.lights.iter_mut().flatten() {
        light.shadows = shadows;
    }
    // Skinned characters: their bottom levels are rebuilt from the skinned vertices every frame.
    for i in 0..3u64 {
        frame.instances.push(InstanceUpdate {
            id: 1000 + i,
            pose: Some(Pose {
                position: [-3.0 + 3.0 * i as f32, 0.0, 7.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.6; 3],
            }),
            look: Some(Look {
                mesh: HERO.into(),
                material: String::new(),
                color: [1.0; 4],
                metallic: 0.0,
                roughness: 0.6,
                transmission: None,
                ior: None,
                emissive: [0.0; 3],
                cast_shadows: true,
                visible: true,
            }),
            anim: Some(AnimView {
                clip: "idle".into(),
                time: 0.3 * i as f32,
                rate: 1.0,
                looped: true,
            }),
        });
    }
    frame
}

fn shoot(gpu: &Gpu, shadows: bool) -> (Vec<u8>, Option<RtShadowStats>) {
    let hero = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../samples/anim/models/hero.glb"
    ))
    .expect("samples/anim/models/hero.glb");
    let hero = pocket_assets::import::import_glb_bytes(&hero).expect("hero.glb imports");
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.add_model(demo::MIXED_MODEL, &demo::mixed_model());
    r.add_model(HERO, &hero);
    r.apply(scene(shadows), 0.0);
    r.set_camera_override(Some(demo::mixed_camera(N)));
    let t = 1.0 / 120.0;
    for _ in 0..4 {
        let _ = r.capture_rgba(t);
    }
    let (_, _, rgba) = r.capture_rgba(t);
    (rgba, r.rt_shadow_stats())
}

fn luminance(rgba: &[u8]) -> Vec<f32> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
        .collect()
}

#[test]
fn ray_traced_shadows_fall_where_cascades_do() {
    let backends: &[BackendChoice] = if cfg!(windows) {
        &[BackendChoice::Vulkan, BackendChoice::Dx12]
    } else if cfg!(any(target_os = "macos", target_os = "ios")) {
        &[BackendChoice::Metal]
    } else {
        &[BackendChoice::Vulkan]
    };
    for &choice in backends {
        let Ok(traced) = Gpu::headless_ray_query(choice, true) else {
            eprintln!("{choice:?}: no GPU, skipped");
            continue;
        };
        if !traced.caps.ray_query {
            eprintln!(
                "{choice:?} on {}: no ray queries, skipped",
                traced.info.name
            );
            continue;
        }
        let cascaded = Gpu::headless_ray_query(choice, false).expect("the same GPU without them");
        assert!(!cascaded.caps.ray_query);
        let name = format!("{} on {}", traced.backend_name(), traced.info.name);
        let (lit, _) = shoot(&cascaded, false);
        let (csm, csm_stats) = shoot(&cascaded, true);
        let (rt, rt_stats) = shoot(&traced, true);
        assert!(csm_stats.is_none(), "{name}: cascades without ray queries");
        let stats = rt_stats.expect("ray-traced shadows on a ray-query device");
        assert!(
            stats.instances as usize > N as usize * N as usize + 3 && stats.masked > 0,
            "{name}: every caster in the top level, masked ones too: {stats:?}"
        );
        assert!(
            stats.rebuilt,
            "{name}: skinned casters rebuild every frame: {stats:?}"
        );
        let (lit, csm, rt) = (luminance(&lit), luminance(&csm), luminance(&rt));
        let dark = |shot: &[f32]| -> Vec<bool> {
            shot.iter()
                .zip(&lit)
                .map(|(s, l)| *s < 0.8 * l && *l > 20.0)
                .collect()
        };
        let (a, b) = (dark(&csm), dark(&rt));
        let both = a.iter().zip(&b).filter(|(x, y)| **x && **y).count();
        let either = a.iter().zip(&b).filter(|(x, y)| **x || **y).count();
        let (in_csm, in_rt) = (
            a.iter().filter(|x| **x).count(),
            b.iter().filter(|x| **x).count(),
        );
        let iou = both as f64 / either.max(1) as f64;
        eprintln!(
            "{name}: shadowed pixels {in_csm} cascaded, {in_rt} ray traced, overlap {iou:.3}"
        );
        let pixels = (W * H) as usize;
        assert!(
            in_csm > pixels / 100 && in_rt > pixels / 100,
            "{name}: both paths cast visible shadows ({in_csm}, {in_rt} of {pixels})"
        );
        // Ray-traced shadows are hard where the cascades filter 3x3, and both test masked casters'
        // alpha at their own sample positions: the sets differ at edges (0.897 measured).
        assert!(
            iou > 0.85,
            "{name}: shadows fall in the same places (intersection over union {iou:.3})"
        );
    }
}
