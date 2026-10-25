//! The liquid surface (fluid_surface.rs, docs/spec/splashes.md 5): a block of liquid in front of a
//! white wall shows the wall through it, absorbed along its thickness as Beer has it (red far more
//! than blue for water's absorption) and bent by refraction; what stands in front of the liquid
//! hides it, pixel for pixel. Skips when there is no GPU.

use amoris_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};
use amoris_render::fluid::{FluidSpec, FluidTuning};
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use glam::Vec3;

const W: u32 = 320;
const H: u32 = 180;

fn cube(
    id: u64,
    position: [f32; 3],
    scale: [f32; 3],
    color: [f32; 4],
    emissive: [f32; 3],
) -> InstanceUpdate {
    InstanceUpdate {
        id,
        pose: Some(Pose {
            position,
            scale,
            ..Pose::default()
        }),
        look: Some(Look {
            mesh: "cube".into(),
            color,
            emissive,
            cast_shadows: false,
            visible: true,
            metallic: 0.0,
            roughness: 1.0,
            material: String::new(),
            transmission: None,
            ior: None,
        }),
        anim: None,
    }
}

/// A white emissive wall 2 m behind the fluid box's middle, and, with `occluder`, a black plate
/// between the eye and the liquid's left half.
fn scene(occluder: bool) -> RenderFrame {
    let mut instances = vec![cube(
        1,
        [0.0, 0.5, -2.0],
        [8.0, 6.0, 0.1],
        [0.0, 0.0, 0.0, 1.0],
        [1.0; 3],
    )];
    if occluder {
        instances.push(cube(
            2,
            [-1.47, 0.45, 1.5],
            [2.0, 3.0, 0.05],
            [0.0, 0.0, 0.0, 1.0],
            [0.0; 3],
        ));
    }
    RenderFrame {
        tick: 1,
        t_s: 1.0 / 60.0,
        dt_s: 1.0 / 60.0,
        reset: true,
        lights: Some(vec![LightView {
            id: 0,
            kind: LightKindView::Directional,
            position: [0.0; 3],
            direction: [0.3, -1.0, -0.2],
            color: [1.0; 3],
            intensity: 2.0,
            range: 0.0,
            inner_deg: 0.0,
            outer_deg: 0.0,
            shadows: false,
        }]),
        cameras: Some(Vec::new()),
        environment: Some(EnvironmentView {
            sky: 1,
            sky_color: [0.05, 0.05, 0.06],
            ambient: 0.2,
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.0,
            fog_color: [0.5, 0.5, 0.5],
            exposure_ev: 0.0,
            bloom: 0.0,
        }),
        instances,
        ..RenderFrame::default()
    }
}

/// The spike's fluid box, 1.6 m across in 64 cells, its block of liquid 0.5 m wide, 0.77 m high and
/// 1.05 m deep (water's absorption: 6, 1.8 and 0.9 per metre).
fn liquid() -> FluidSpec {
    let n = 64;
    let grid = [n, n * 3 / 4, n * 3 / 4];
    let cell_m = 1.6 / n as f32;
    FluidSpec {
        origin: [
            -0.5 * grid[0] as f32 * cell_m,
            0.0,
            -0.5 * grid[2] as f32 * cell_m,
        ],
        cell_m,
        grid,
        block_lo: [3.0, 3.0, 3.0],
        block_hi: [
            grid[0] as f32 * 0.36,
            grid[1] as f32 * 0.7,
            grid[2] as f32 - 3.0,
        ],
        spacing: 0.5,
        substeps: 10,
        tuning: FluidTuning::default(),
    }
}

/// The frame with or without the liquid and the occluder, read as HDR.
fn shoot(gpu: &Gpu, fluid: bool, occluder: bool) -> Vec<f32> {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(Antialiasing::Off);
    r.set_gtao(Gtao::Off);
    r.set_camera_override(Some(CameraState::look_at(
        Vec3::new(-0.47, 0.45, 2.5),
        Vec3::new(-0.47, 0.45, 0.0),
    )));
    r.apply(scene(occluder), 0.0);
    if fluid {
        r.set_fluid(Some(liquid()));
    }
    // The same moment twice: the liquid does not move between the frames.
    let _ = r.capture_hdr(0.0);
    r.capture_hdr(0.0).2
}

fn rgb(img: &[f32], x: u32, y: u32) -> [f32; 3] {
    let p = ((y * W + x) * 4) as usize;
    [img[p], img[p + 1], img[p + 2]]
}

#[test]
fn liquid_absorbs_by_its_thickness_and_what_stands_in_front_hides_it() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let wall = shoot(&gpu, false, false);
    let wet = shoot(&gpu, true, false);
    // Through the liquid's middle (about a metre of water along the view), the wall's white comes
    // through with its red absorbed far more than its blue: e^-6 against e^-0.9 for a metre, and
    // more than the sky the surface reflects can make up.
    let (mut red, mut blue, mut n) = (0.0f64, 0.0f64, 0);
    for y in H / 2 - 6..H / 2 + 6 {
        for x in W / 2 - 6..W / 2 + 6 {
            let (a, b) = (rgb(&wall, x, y), rgb(&wet, x, y));
            red += f64::from(b[0] / a[0].max(1e-4));
            blue += f64::from(b[2] / a[2].max(1e-4));
            n += 1;
        }
    }
    let (red, blue) = (red / f64::from(n), blue / f64::from(n));
    eprintln!("through the liquid: red keeps {red:.4}, blue {blue:.4}");
    assert!(blue > 0.05 && red < 0.25 * blue, "red {red} blue {blue}");
    // A plate in front of the liquid's left half: that half is the plate's, liquid or not.
    let plate = shoot(&gpu, false, true);
    let both = shoot(&gpu, true, true);
    let mut differ = 0;
    for y in 0..H {
        for x in 0..W / 2 - 2 {
            if rgb(&plate, x, y) != rgb(&both, x, y) {
                differ += 1;
            }
        }
    }
    assert_eq!(
        differ, 0,
        "the liquid showed through the plate in front of it"
    );
    // And the right half shows the liquid.
    let shown = (W / 2 + 4..W / 2 + 12).any(|x| rgb(&plate, x, H / 2) != rgb(&both, x, H / 2));
    assert!(shown, "the liquid's right half is not drawn");
}
