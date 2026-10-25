//! Rain through the air (water.md 67, rain.rs): where the weather says it rains, streaks fall
//! across the view, slanted by the wind and hidden behind what stands in front of them; without
//! rain nothing is drawn; and the rain stands still while drawn time does.

use amoris_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RainView, RenderFrame,
    WeatherView, WindSeaView,
};
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use glam::Vec3;

const W: u32 = 320;
const H: u32 = 180;

/// A dark wall 6 m ahead filling the right half of the view, under a grey sky, with or without
/// rain of 20 mm an hour blown toward +x at 8 m/s, at `tick`.
fn scene(rain: bool, tick: u64) -> RenderFrame {
    let first = tick == 1;
    RenderFrame {
        tick,
        t_s: tick as f64 / 60.0,
        dt_s: 1.0 / 60.0,
        reset: first,
        lights: first.then(|| {
            vec![LightView {
                id: 0,
                kind: LightKindView::Directional,
                position: [0.0; 3],
                direction: [0.3, -1.0, 0.2],
                color: [1.0, 1.0, 1.0],
                intensity: 2.0,
                range: 0.0,
                inner_deg: 0.0,
                outer_deg: 0.0,
                shadows: false,
            }]
        }),
        cameras: first.then(Vec::new),
        environment: first.then_some(EnvironmentView {
            sky: 1,
            sky_color: [0.3, 0.32, 0.36],
            ambient: 1.0,
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.002,
            fog_color: [0.5, 0.53, 0.58],
            exposure_ev: 0.0,
            bloom: 0.0,
        }),
        instances: if first {
            vec![InstanceUpdate {
                id: 100,
                pose: Some(Pose {
                    position: [5.0, 0.0, -6.0],
                    scale: [10.0, 10.0, 0.5],
                    ..Pose::default()
                }),
                look: Some(Look {
                    mesh: "cube".into(),
                    color: [0.05, 0.05, 0.05, 1.0],
                    emissive: [0.0; 3],
                    cast_shadows: false,
                    visible: true,
                    metallic: 0.0,
                    roughness: 1.0,
                    material: String::new(),
                    transmission: None,
                    ior: None,
                }),
                anim: None,
            }]
        } else {
            vec![]
        },
        weather: first.then_some(WeatherView {
            wind: Some(WindSeaView {
                speed: 8.0,
                toward_deg: 90.0,
                fetch_km: 0.1,
            }),
            rain: rain.then_some(RainView {
                intensity_mm_h: 20.0,
                drop_mm: 2.5,
            }),
        }),
        ..RenderFrame::default()
    }
}

fn renderer(gpu: &Gpu) -> Renderer {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(Antialiasing::Off);
    r.set_gtao(Gtao::Off);
    r.set_camera_override(Some(CameraState::look_at(
        Vec3::new(0.0, 1.6, 0.0),
        Vec3::new(0.0, 1.6, -10.0),
    )));
    r
}

fn luminance(img: &[f32], x: u32, y: u32) -> f32 {
    let p = ((y * W + x) * 4) as usize;
    0.2126 * img[p] + 0.7152 * img[p + 1] + 0.0722 * img[p + 2]
}

/// The share of the pixels in columns `x0..x1` brighter in `wet` than in `dry` by over 0.01.
fn streaked(wet: &[f32], dry: &[f32], x0: u32, x1: u32) -> f64 {
    let mut n = 0;
    for y in 0..H {
        for x in x0..x1 {
            if luminance(wet, x, y) > luminance(dry, x, y) + 0.01 {
                n += 1;
            }
        }
    }
    f64::from(n) / f64::from(H * (x1 - x0))
}

#[test]
fn rain_falls_where_it_rains_and_stands_still_while_time_does() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let (mut dry, mut wet) = (renderer(&gpu), renderer(&gpu));
    dry.apply(scene(false, 1), 0.0);
    wet.apply(scene(true, 1), 0.0);
    let a = dry.capture_hdr(0.0).2;
    let b = wet.capture_hdr(0.0).2;
    // Over the sky (the left half) and over the dark wall (the right half), the streaks show.
    let (sky, wall) = (streaked(&b, &a, 0, W / 2), streaked(&b, &a, W / 2 + 16, W));
    eprintln!("streaked: over the sky {sky:.4}, over the wall {wall:.4}");
    assert!(sky > 0.005 && wall > 0.005, "{sky} {wall}");
    // Drawn a frame later (the poses' interpolation run to the tick), the rain has fallen; drawn
    // again at that moment, it is where it was.
    let later = wet.capture_hdr(5.0).2;
    assert!(b != later, "the rain stood still while time passed");
    assert_eq!(
        later,
        wet.capture_hdr(9.0).2,
        "the rain moved while drawn time stood still"
    );
    // Without rain, the weather draws nothing.
    let mut calm = renderer(&gpu);
    calm.apply(scene(false, 1), 0.0);
    assert_eq!(a, calm.capture_hdr(0.0).2);
}
