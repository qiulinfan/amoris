//! Ray-traced water reflections (water.md 67), on each native backend whose adapter has ray queries
//! (`AMORIS_RT_REFLECTIONS=1`); each test skips, with a note, without a GPU or without them.
//!
//! - A red block high above the far end of a pond, out of the camera's view, shows in the pond's
//!   reflection when the reflected rays trace the top level, where the screen-space march, which
//!   can only find what the screen shows, leaves the sky.
//! - A grey block standing in the pond, in view and lit by the sun and the sky, is reflected by the
//!   traced rays in the colour the march finds for it on the screen: the hit is lit as the
//!   rasterizer lights it.

use amoris_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
    WaterLookView, WaterTileView, WaterUpdate,
};
use amoris_render::rt_shadows::RtEffects;
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use glam::Vec3;

const W: u32 = 320;
const H: u32 = 180;

/// A block 4 m on a side: its centre, base colour and emission.
struct Block {
    at: [f32; 3],
    color: [f32; 4],
    emissive: [f32; 3],
}

/// A pond 64 x 64 m, its surface 1 m up over a flat bed, `block` in or over it, the sun at `sun`
/// and a sky of `sky`.
fn scene(block: Option<Block>, sun: f32, sky: [f32; 3]) -> RenderFrame {
    let tiles = (0..4)
        .flat_map(|kz| (0..4).map(move |kx| [kx, kz]))
        .map(|key| WaterTileView {
            key,
            columns: vec![[1.0, 1.0, 0.0, 0.0]; 256],
        })
        .collect();
    RenderFrame {
        tick: 1,
        t_s: 1.0 / 60.0,
        dt_s: 1.0 / 60.0,
        reset: true,
        lights: Some(vec![LightView {
            id: 0,
            kind: LightKindView::Directional,
            position: [0.0; 3],
            direction: [0.3, -1.0, 0.2],
            color: [1.0, 1.0, 1.0],
            intensity: sun,
            range: 0.0,
            inner_deg: 0.0,
            outer_deg: 0.0,
            shadows: false,
        }]),
        cameras: Some(vec![]),
        environment: Some(EnvironmentView {
            sky: 1,
            sky_color: sky,
            ambient: if sky == [0.0; 3] { 0.0 } else { 1.0 },
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.0,
            fog_color: [0.0; 3],
            exposure_ev: 0.0,
            bloom: 0.0,
        }),
        instances: block
            .map(|b| InstanceUpdate {
                id: 100,
                pose: Some(Pose {
                    position: b.at,
                    scale: [4.0, 4.0, 4.0],
                    ..Pose::default()
                }),
                look: Some(Look {
                    mesh: "cube".into(),
                    color: b.color,
                    emissive: b.emissive,
                    cast_shadows: false,
                    visible: true,
                    metallic: 0.0,
                    roughness: 1.0,
                    material: String::new(),
                    transmission: None,
                    ior: None,
                }),
                anim: None,
            })
            .into_iter()
            .collect(),
        water: vec![WaterUpdate {
            id: 7,
            origin: [0.0, 0.0],
            cell_m: 1.0,
            size: [64, 64],
            look: Some(WaterLookView {
                fog: [0.0; 4],
                absorb: [0.3, 0.1, 0.05],
                scatter: [0.0; 3],
                ..WaterLookView::default()
            }),
            tiles,
        }],
        ..RenderFrame::default()
    }
}

/// `frame` from 2.5 m over the pond's near end, looking at `at`; the HDR image.
fn shoot(gpu: &Gpu, frame: RenderFrame, at: Vec3) -> Vec<f32> {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(Antialiasing::Off);
    r.set_gtao(Gtao::Off);
    r.set_water_foam(false);
    r.set_camera_override(Some(CameraState::look_at(Vec3::new(16.0, 2.5, 2.0), at)));
    r.apply(frame, 0.0);
    for _ in 0..4 {
        let _ = r.capture_hdr(0.0);
    }
    r.capture_hdr(0.0).2
}

/// `RT_REFLECTIONS_DUMP=dir` keeps each image as a PNG.
fn dump(name: &str, img: &[f32]) {
    if let Some(dir) = std::env::var_os("RT_REFLECTIONS_DUMP") {
        let bytes: Vec<u8> = img
            .iter()
            .enumerate()
            .map(|(i, v)| {
                if i % 4 == 3 {
                    255
                } else {
                    (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8
                }
            })
            .collect();
        image::RgbaImage::from_raw(W, H, bytes)
            .unwrap()
            .save(std::path::Path::new(&dir).join(format!("{name}.png")))
            .unwrap();
    }
}

/// The two devices on `backend`, without ray queries and with them for reflections: none when
/// there is no GPU or no ray queries.
fn devices(backend: BackendChoice) -> Option<(Gpu, Gpu)> {
    let Ok(traced) = Gpu::headless_ray_traced(
        backend,
        RtEffects {
            shadows: false,
            reflections: true,
        },
    ) else {
        eprintln!("{backend:?}: no GPU; skipped");
        return None;
    };
    if !traced.caps.ray_query {
        eprintln!(
            "{} ({:?}): no ray queries; skipped",
            traced.info.name, backend
        );
        return None;
    }
    let plain = Gpu::headless(backend).expect("the same adapter without ray queries");
    Some((plain, traced))
}

/// The image's red pixels and their red, summed: with the block off the screen, its reflection's.
fn red(img: &[f32]) -> (f32, usize) {
    let mut sum = 0.0;
    let mut count = 0;
    for p in img.as_chunks::<4>().0 {
        if p[0] > 0.05 && p[0] > 3.0 * p[1].max(p[2]) {
            sum += p[0];
            count += 1;
        }
    }
    (sum, count)
}

/// From the pond's near end looking down at it 23 degrees under the horizon: the view's top edge
/// is 7 degrees over it, the block's lowest corner 10, and the block's reflection lies 16 to 26
/// degrees under it, about the middle of the view. No sky light, so the reflection is all a red
/// pixel can be.
#[test]
fn the_water_reflects_what_the_screen_does_not_show() {
    for backend in BackendChoice::under_test() {
        let Some((plain, traced)) = devices(backend) else {
            continue;
        };
        let frame = || {
            scene(
                Some(Block {
                    at: [16.0, 10.0, 30.0],
                    color: [0.0, 0.0, 0.0, 1.0],
                    emissive: [4.0, 0.0, 0.0],
                }),
                0.0,
                [0.0; 3],
            )
        };
        let at = Vec3::new(16.0, 0.0, 8.0);
        let (a, b) = (shoot(&plain, frame(), at), shoot(&traced, frame(), at));
        dump("marched", &a);
        dump("traced", &b);
        let (marched, marched_pixels) = red(&a);
        let (rt, rt_pixels) = red(&b);
        eprintln!(
            "{} ({:?}): red pixels in the water, screen-space {marched_pixels}, traced {rt_pixels} \
             (summed red {marched:.2}, {rt:.2})",
            traced.info.name, traced.info.backend
        );
        assert_eq!(marched_pixels, 0, "the block is off the screen");
        assert!(
            rt_pixels > 200,
            "the traced reflection shows the block: {rt_pixels}"
        );
    }
}

/// A grey block standing in the pond's far end, seen nearly level under a blue sky with the sun
/// high: where the traced image's water differs from the pond without the block, it shows the
/// block's reflection; there the traced and the marched water agree within a tenth.
#[test]
fn the_traced_reflection_is_lit_as_the_screen_shows_it() {
    for backend in BackendChoice::under_test() {
        let Some((plain, traced)) = devices(backend) else {
            continue;
        };
        let sky = [0.25, 0.4, 0.7];
        let frame = |with: bool| {
            scene(
                with.then_some(Block {
                    at: [16.0, 3.0, 30.0],
                    color: [0.6, 0.6, 0.6, 1.0],
                    emissive: [0.0; 3],
                }),
                3.0,
                sky,
            )
        };
        let at = Vec3::new(16.0, 2.0, 30.0);
        let marched = shoot(&plain, frame(true), at);
        let rt = shoot(&traced, frame(true), at);
        let bare = shoot(&traced, frame(false), at);
        dump("lit-marched", &marched);
        dump("lit-traced", &rt);
        dump("lit-bare", &bare);
        // The water under the horizon whose traced colour the block changed.
        let (mut sums, mut count) = ([0.0f64; 2], 0);
        for y in H / 2 + 4..H {
            for x in 0..W {
                let p = ((y * W + x) * 4) as usize;
                let changed: f32 = (0..3).map(|c| (rt[p + c] - bare[p + c]).abs()).sum();
                if changed > 0.03 {
                    let luminance =
                        |img: &[f32]| 0.2126 * img[p] + 0.7152 * img[p + 1] + 0.0722 * img[p + 2];
                    sums[0] += f64::from(luminance(&marched));
                    sums[1] += f64::from(luminance(&rt));
                    count += 1;
                }
            }
        }
        let (m, t) = (sums[0] / count as f64, sums[1] / count as f64);
        eprintln!(
            "{} ({:?}): the block's reflection over {count} pixels, mean luminance screen-space \
             {m:.4}, traced {t:.4}",
            traced.info.name, traced.info.backend
        );
        assert!(count > 300, "the reflection's pixels: {count}");
        assert!((t - m).abs() < 0.1 * m, "screen-space {m}, traced {t}");
    }
}
