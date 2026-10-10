//! The depth prepass leaves the image alone (docs/spec/prepass.md 6). Each scene is drawn by two
//! renderers on the same device through the same frames, one with the prepass off and one with it
//! forced on, and every compared frame must be identical to the bit, with the same entity-id
//! coverage:
//!
//! - the mixed scene (every pipeline variant: opaque, alpha-masked, double-sided and both, in the
//!   camera and four shadow cascades) with three skinned characters, with 4x MSAA, on the device
//!   `POCKET_BACKEND` and `POCKET_GPU_MINIMAL` describe and then on the two other draw paths;
//! - the occluder scene with occlusion culling forced on (a prepass before each of its two
//!   phases): the cold first frame, a warm frame, a camera cut;
//! - the anti-aliasing scene with TAA, one sample and four, its instances moving: both passes are
//!   jittered alike, and the history must accumulate the same images;
//! - the levels-of-detail field (level rows drawn as meshes);
//! - a neural material (a constant neural texture on a tiled ground, under the mixed scene's
//!   primitives): its variant's depth goes through its base variant's pipeline.
//!
//! Where the prepass's depth and the forward pass's disagree, the equal-depth test drops the
//! forward pass's samples and the cleared target shows through: the images differ there. So does
//! anything the prepass draws that the opaque pass does not, or the reverse. Skips without a GPU.

use pocket_assets::frame::{AnimView, InstanceUpdate, Look, Pose, RenderFrame};
use pocket_assets::neural::{
    Channel, GridSpec, LatentGrid, LatentLevel, NeuralLayout, NeuralTexture, Sampling, f32_to_f16,
    full_mip_count, level_dims,
};
use pocket_render::gpu::Minimal;
use pocket_render::{
    Antialiasing, BackendChoice, Gpu, Gtao, OcclusionMode, PrepassMode, Renderer, demo,
};

const W: u32 = 480;
const H: u32 = 270;
const HERO: &str = "models/hero.glb";

/// One compared frame.
struct Shot {
    step: String,
    rgba: Vec<u8>,
    visible: Vec<(u64, f32)>,
    prepass: &'static str,
}

/// Draws a frame at `t` with the id pass, keeps its image, then draws on (at `t`) until the id
/// pass answers.
fn shot(r: &mut Renderer, step: String, t: f64) -> Shot {
    r.request_visible();
    let (_, _, rgba) = r.capture_rgba(t);
    let prepass = r.last.prepass;
    let mut visible = None;
    for _ in 0..30 {
        if let Some(v) = r.take_visible() {
            visible = Some(v);
            break;
        }
        let _ = r.capture_rgba(t);
    }
    Shot {
        step,
        rgba,
        visible: visible.expect("the id pass answers"),
        prepass,
    }
}

fn renderer(gpu: &Gpu, prepass: PrepassMode, aa: Antialiasing) -> Renderer {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_prepass(prepass);
    r.set_antialiasing(aa);
    r.set_gtao(Gtao::Off);
    r.set_occlusion(OcclusionMode::Off);
    r
}

fn hero() -> pocket_assets::mesh::ModelAsset {
    let glb = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../samples/anim/models/hero.glb"
    ))
    .expect("samples/anim/models/hero.glb");
    pocket_assets::import::import_glb_bytes(&glb).expect("hero.glb imports")
}

fn look(mesh: &str, material: &str, color: [f32; 4]) -> Look {
    Look {
        mesh: mesh.into(),
        material: material.into(),
        color,
        metallic: 0.0,
        roughness: 0.6,
        transmission: None,
        ior: None,
        emissive: [0.0; 3],
        cast_shadows: true,
        visible: true,
    }
}

/// `demo::mixed(6)` with three skinned characters among the primitives.
fn mixed() -> RenderFrame {
    let mut frame = demo::mixed(6);
    for i in 0..3u64 {
        frame.instances.push(InstanceUpdate {
            id: 1000 + i,
            pose: Some(Pose {
                position: [-3.0 + 3.0 * i as f32, 0.0, 9.5],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.6; 3],
            }),
            look: Some(look(HERO, "", [1.0; 4])),
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

fn mixed_shots(gpu: &Gpu, prepass: PrepassMode) -> Vec<Shot> {
    let mut r = renderer(gpu, prepass, Antialiasing::Msaa);
    r.add_model(demo::MIXED_MODEL, &demo::mixed_model());
    r.add_model(HERO, &hero());
    r.apply(mixed(), 0.0);
    r.set_camera_override(Some(demo::mixed_camera(6)));
    // One fixed moment: both renderers interpolate and animate identically.
    let t = 1.0 / 120.0;
    vec![
        shot(&mut r, "mixed".into(), t),
        shot(&mut r, "mixed, again".into(), t),
    ]
}

fn occluder_shots(gpu: &Gpu, prepass: PrepassMode) -> Vec<Shot> {
    let [front, side] = demo::occluders_cameras();
    let mut r = renderer(gpu, prepass, Antialiasing::Msaa);
    r.set_occlusion(OcclusionMode::On);
    r.add_model(demo::MIXED_MODEL, &demo::mixed_model());
    r.apply(demo::occluders(3.0), 0.0);
    r.set_camera_override(Some(front));
    let mut t = 0.0;
    let mut shots = vec![shot(&mut r, "occluders cold".into(), t)];
    for _ in 0..4 {
        t += 1.0 / 60.0;
        let _ = r.capture_rgba(t);
    }
    shots.push(shot(&mut r, "occluders warm".into(), t));
    r.set_camera_override(Some(side));
    t += 1.0 / 60.0;
    shots.push(shot(&mut r, "occluders cut".into(), t));
    assert_eq!(r.last.occlusion, "on");
    shots
}

/// The anti-aliasing scene, its instances moving and its camera panning, with TAA (`aa`): the
/// shots of frames 1, 5 and 12 (the history accumulated between them).
fn taa_shots(gpu: &Gpu, prepass: PrepassMode, aa: Antialiasing) -> Vec<Shot> {
    let mut r = renderer(gpu, prepass, aa);
    r.add_model(demo::AA_MODEL, &demo::aa_model());
    r.apply(demo::aa_scene(0, true, true), 0.0);
    let mut shots = Vec::new();
    for tick in 1..=12u32 {
        let t = f64::from(tick) / 60.0;
        r.apply(demo::aa_scene(u64::from(tick), true, false), t);
        r.set_camera_override(Some(demo::aa_camera(tick, true)));
        if [1, 5, 12].contains(&tick) {
            shots.push(shot(&mut r, format!("{} tick {tick}", aa.name()), t));
        } else {
            let _ = r.capture_rgba(t);
        }
    }
    shots
}

fn lod_shots(gpu: &Gpu, prepass: PrepassMode) -> Vec<Shot> {
    let mut r = renderer(gpu, prepass, Antialiasing::Msaa);
    r.add_model(demo::LOD_MODEL, &demo::lod_model(4));
    r.apply(demo::lod_field(8, 5.0), 0.0);
    r.set_camera_override(Some(demo::lod_field_camera(8, 5.0, 0.2)));
    vec![shot(&mut r, "lod field".into(), 1.0 / 120.0)]
}

/// A texture whose every texel is `values` (tests/neural_material.rs `constant`): zero weights,
/// the values as output biases, latents anything; the default profile's shape.
fn constant_texture(values: &[(Channel, f32)]) -> NeuralTexture {
    let grid = GridSpec {
        features: 8,
        bits: 8,
    };
    let layout = NeuralLayout {
        channels: values.iter().map(|(c, _)| *c).collect(),
        fine: grid,
        coarse: grid,
        fine_shift: 2,
        sampling: Sampling::Bilinear,
        pe_octaves: 2,
        hidden: [32, 32],
    };
    let size = 64;
    let mips = full_mip_count(size, size);
    let levels = level_dims(&layout, size, size, mips)
        .iter()
        .map(|d| {
            let grid = |[w, h]: [u32; 2]| LatentGrid {
                width: w,
                height: h,
                words: (0..2 * w * h)
                    .map(|i| i.wrapping_mul(2_654_435_761))
                    .collect(),
            };
            LatentLevel {
                fine: grid(d.fine),
                coarse: grid(d.coarse),
            }
        })
        .collect();
    let mut weights = vec![0u16; layout.weight_count()];
    let [_, _, (_, b3)] = layout.offsets();
    for (i, (_, v)) in values.iter().enumerate() {
        weights[b3 + i] = f32_to_f16(*v);
    }
    NeuralTexture {
        name: "constant".into(),
        width: size,
        height: size,
        mip_count: mips,
        layout,
        levels,
        weights,
    }
}

/// The neural ground with a few primitives standing on it (overdraw in front of the neural
/// material, and the neural material in front of nothing).
fn neural_shots(gpu: &Gpu, prepass: PrepassMode) -> Vec<Shot> {
    const MATERIAL: &str = "materials/constant.ntex";
    let mut r = renderer(gpu, prepass, Antialiasing::Msaa);
    r.add_model(demo::NEURAL_GROUND, &demo::neural_ground_model(40.0, 8.0));
    let texture = constant_texture(&[
        (Channel::BaseR, 0.55),
        (Channel::BaseG, 0.4),
        (Channel::BaseB, 0.3),
        (Channel::NormalX, 0.5),
        (Channel::NormalY, 0.5),
        (Channel::Occlusion, 1.0),
        (Channel::Roughness, 0.7),
        (Channel::Metallic, 0.0),
    ]);
    r.add_neural_texture(MATERIAL, &texture)
        .expect("the texture loads");
    let mut frame = demo::neural_scene(MATERIAL, [1.0; 4], 0.7);
    for (i, mesh) in ["cube", "sphere", "cylinder", "cone"]
        .into_iter()
        .enumerate()
    {
        frame.instances.push(InstanceUpdate {
            id: 10 + i as u64,
            pose: Some(Pose {
                position: [-3.0 + 2.0 * i as f32, 0.6, -2.0 - i as f32],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.2; 3],
            }),
            look: Some(look(mesh, "", [0.3, 0.5, 0.8, 1.0])),
            anim: None,
        });
    }
    r.apply(frame, 0.0);
    r.set_camera_override(Some(demo::neural_camera(1)));
    vec![shot(&mut r, "neural".into(), 1.0 / 120.0)]
}

fn share(v: &[(u64, f32)], e: u64) -> f32 {
    v.iter().find(|x| x.0 == e).map_or(0.0, |x| x.1)
}

/// The shots with the prepass off and on agree to the bit.
fn agree(off: &[Shot], on: &[Shot], path: &str) {
    assert_eq!(off.len(), on.len());
    for (a, b) in off.iter().zip(on) {
        let step = &a.step;
        assert_eq!(a.prepass, "off", "{path}, {step}");
        assert_eq!(b.prepass, "on", "{path}, {step}");
        let differing: Vec<usize> = a
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b.rgba.as_chunks::<4>().0)
            .enumerate()
            .filter(|(_, (p, q))| p != q)
            .map(|(i, _)| i)
            .collect();
        let worst = a
            .rgba
            .iter()
            .zip(&b.rgba)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        eprintln!(
            "{path}, {step}: {} entities visible, {} pixels differ (worst {worst})",
            a.visible.len(),
            differing.len()
        );
        assert!(
            differing.is_empty(),
            "{path}, {step}: {} pixels differ with the prepass, worst by {worst}; first at {:?}",
            differing.len(),
            differing
                .iter()
                .take(8)
                .map(|i| (i % W as usize, i / W as usize))
                .collect::<Vec<_>>()
        );
        let mut ids: Vec<u64> = a.visible.iter().chain(&b.visible).map(|x| x.0).collect();
        ids.sort_unstable();
        ids.dedup();
        for e in ids {
            let (x, y) = (share(&a.visible, e), share(&b.visible, e));
            assert!(
                (x - y).abs() < 1e-6,
                "{path}, {step}: entity {e} covers {x} without the prepass, {y} with it"
            );
        }
        assert!(a.visible.len() >= 3, "{path}, {step}: {:?}", a.visible);
    }
}

fn check(gpu: &Gpu, label: &str, everything: bool) {
    let path = format!("{label} ({}, {})", gpu.backend_name(), gpu.info.name);
    let both = |f: &dyn Fn(&Gpu, PrepassMode) -> Vec<Shot>| {
        agree(&f(gpu, PrepassMode::Off), &f(gpu, PrepassMode::On), &path);
    };
    both(&mixed_shots);
    both(&occluder_shots);
    if everything {
        both(&|g, p| taa_shots(g, p, Antialiasing::Taa));
        both(&|g, p| taa_shots(g, p, Antialiasing::MsaaTaa));
        both(&lod_shots);
        both(&neural_shots);
    }
}

#[test]
fn the_depth_prepass_leaves_the_image_alone() {
    let choice = BackendChoice::from_env();
    let Ok(gpu) = Gpu::headless(choice) else {
        eprintln!("no GPU: skipped");
        return;
    };
    check(&gpu, "default device", true);
    let env = Minimal::from_env();
    // The two other draw paths on the same adapter.
    if !env.drops_first_instance() && gpu.caps.indirect_first_instance {
        let baseline = Gpu::headless_with(
            choice,
            Minimal {
                first_instance: true,
                ..env
            },
        )
        .expect("a baseline device");
        check(&baseline, "baseline path", false);
        if gpu.caps.multi_draw_indirect {
            let per_batch = Gpu::headless_with(
                choice,
                Minimal {
                    multi_draw: true,
                    ..env
                },
            )
            .expect("a per-batch device");
            check(&per_batch, "first-instance path", false);
        }
    }
}
