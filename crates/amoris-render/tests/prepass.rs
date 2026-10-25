//! The depth prepass leaves the image alone (docs/spec/prepass.md 6). Each scene is drawn by two
//! renderers on the same device through the same frames, one with the prepass off and one with it
//! forced on, and every compared frame must be identical to the bit, with the same entity-id
//! coverage:
//!
//! - the mixed scene (every pipeline variant: opaque, alpha-masked, double-sided and both, in the
//!   camera and four shadow cascades) with three skinned characters, with 4x MSAA, on the device
//!   `AMORIS_BACKEND` and `AMORIS_GPU_MINIMAL` describe and then on the two other draw paths;
//! - the occluder scene with occlusion culling forced on (a prepass before each of its two
//!   phases): the cold first frame, a warm frame, a camera cut;
//! - the anti-aliasing scene with TAA, one sample and four, its instances moving: both passes are
//!   jittered alike, and the history must accumulate the same images;
//! - the levels-of-detail field (level rows drawn as meshes).
//!
//! Where the prepass's depth and the forward pass's disagree, the equal-depth test drops the
//! forward pass's samples and the cleared target shows through: the images differ there. So does
//! anything the prepass draws that the opaque pass does not, or the reverse. A separate coplanar
//! regression distinguishes fully transparent holes from valid surfaces tied in depth: holes
//! stay transparent, while valid ties follow the documented draw-order rule. Skips without a GPU.

use amoris_assets::frame::{AnimView, InstanceUpdate, Look, Pose, RenderFrame};
use amoris_assets::mesh::{
    AlphaMode, ImageData, MaterialData, MeshData, ModelAsset, NodeData, Vertex,
};
use amoris_render::gpu::Minimal;
use amoris_render::{
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

fn hero() -> amoris_assets::mesh::ModelAsset {
    let glb = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../samples/anim/models/hero.glb"
    ))
    .expect("samples/anim/models/hero.glb");
    amoris_assets::import::import_glb_bytes(&glb).expect("hero.glb imports")
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
    let mut frame = demo::occluders(3.0);
    // The demo cylinder's bottom cap and ground both lie at y=0. Valid equal-depth surfaces
    // have draw-order-dependent winners, tested separately below; this fixture checks invariant
    // depths and coverage where the visible surface is unambiguous. Keep the demo unchanged.
    frame
        .instances
        .iter_mut()
        .find(|instance| instance.id == 4)
        .expect("the masked cylinder")
        .pose
        .as_mut()
        .unwrap()
        .position[1] += 0.01;
    r.apply(frame, 0.0);
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
        } else {
            eprintln!(
                "no multi-draw: the default device draws batch by batch; multi-draw not checked"
            );
        }
    } else {
        eprintln!(concat!(
            "no INDIRECT_FIRST_INSTANCE: the default device takes the baseline path; ",
            "first-instance paths not checked"
        ));
    }
}

/// Two exactly coplanar quads, blue and red. A transparent red mask must never replace blue,
/// even when blue writes the same depth into the mask's holes. Making both quads the same
/// pipeline variant lets reversing the mesh rows also reverse their actual draw order.
fn coplanar_model(
    background_masked: bool,
    cutout_alpha: u8,
    double_sided: bool,
    reverse: bool,
) -> ModelAsset {
    let vertices = [
        ([-1.0, -1.0, 0.0], [0.0, 1.0]),
        ([1.0, -1.0, 0.0], [1.0, 1.0]),
        ([1.0, 1.0, 0.0], [1.0, 0.0]),
        ([-1.0, 1.0, 0.0], [0.0, 0.0]),
    ]
    .into_iter()
    .map(|(position, uv)| Vertex {
        position,
        uv,
        normal: [0.0, 0.0, 1.0],
        tangent: [1.0, 0.0, 0.0, 1.0],
    })
    .collect();
    let mut blue = MeshData::new("blue", vertices, vec![0, 1, 2, 0, 2, 3]);
    blue.material = Some(0);
    let mut red = blue.clone();
    red.name = "red".into();
    red.material = Some(1);
    let meshes = if reverse {
        vec![red, blue]
    } else {
        vec![blue, red]
    };
    let nodes = meshes
        .iter()
        .enumerate()
        .map(|(mesh, data)| NodeData {
            name: data.name.clone(),
            mesh,
            transform: glam::Mat4::IDENTITY.to_cols_array(),
            skin: None,
        })
        .collect();
    ModelAsset {
        meshes,
        nodes,
        materials: vec![
            MaterialData {
                base_color: [0.0, 0.0, 1.0, 1.0],
                emissive: [0.0, 0.0, 1.0],
                alpha_mode: if background_masked {
                    AlphaMode::Mask
                } else {
                    AlphaMode::Opaque
                },
                double_sided,
                ..MaterialData::default()
            },
            MaterialData {
                base_color: [1.0, 0.0, 0.0, 1.0],
                emissive: [1.0, 0.0, 0.0],
                base_color_texture: Some(0),
                alpha_mode: AlphaMode::Mask,
                double_sided,
                ..MaterialData::default()
            },
        ],
        images: vec![ImageData {
            name: "cutout".into(),
            width: 1,
            height: 1,
            rgba8: vec![255, 255, 255, cutout_alpha],
            srgb: true,
        }],
        ..ModelAsset::default()
    }
}

fn coplanar_shot(r: &mut Renderer, mesh: &str) -> Vec<u8> {
    r.apply(
        RenderFrame {
            reset: true,
            instances: vec![InstanceUpdate {
                id: 1,
                pose: Some(Pose::default()),
                look: Some(Look {
                    cast_shadows: false,
                    ..look(mesh, "", [1.0; 4])
                }),
                anim: None,
            }],
            ..RenderFrame::default()
        },
        0.0,
    );
    r.capture_rgba(0.0).2
}

#[test]
fn coplanar_masks_preserve_holes_and_valid_ties_follow_draw_order() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let mut off = renderer(&gpu, PrepassMode::Off, Antialiasing::Off);
    let mut on = renderer(&gpu, PrepassMode::On, Antialiasing::Off);
    let camera = amoris_render::CameraState::look_at([0.0, 0.0, 4.0].into(), [0.0; 3].into());
    off.set_camera_override(Some(camera));
    on.set_camera_override(Some(camera));
    for aa in [Antialiasing::Off, Antialiasing::Msaa] {
        off.set_antialiasing(aa);
        on.set_antialiasing(aa);
        for double_sided in [false, true] {
            for reverse in [false, true] {
                for background_masked in [false, true] {
                    let name =
                        format!("test/coplanar-{background_masked}-{double_sided}-{reverse}.glb");
                    let model = coplanar_model(background_masked, 0, double_sided, reverse);
                    off.add_model(&name, &model);
                    on.add_model(&name, &model);
                    let blue = coplanar_shot(&mut off, &format!("{name}#blue"));
                    let center = ((H / 2 * W + W / 2) * 4) as usize;
                    assert!(
                        blue[center + 2] > blue[center] + 100,
                        "the blue surface is visible"
                    );
                    for r in [&mut off, &mut on] {
                        let actual = coplanar_shot(r, &name);
                        assert!(
                            actual == blue,
                            "{aa:?}, {name}, {}: a fully transparent mask changed the blue surface",
                            r.last.prepass
                        );
                    }
                }
                // Both masks are now solid, so both legitimately pass Equal. Off keeps the
                // first drawn mesh; On shades the last. Reversing rows reverses the winners.
                let name = format!("test/solid-tie-{double_sided}-{reverse}.glb");
                let model = coplanar_model(true, 255, double_sided, reverse);
                off.add_model(&name, &model);
                on.add_model(&name, &model);
                let first = if reverse { "red" } else { "blue" };
                let last = if reverse { "blue" } else { "red" };
                for (r, winner) in [(&mut off, first), (&mut on, last)] {
                    let expected = coplanar_shot(r, &format!("{name}#{winner}"));
                    let actual = coplanar_shot(r, &name);
                    assert!(
                        actual == expected,
                        "{aa:?}, {name}, {}: {winner} should win the valid depth tie",
                        r.last.prepass
                    );
                }
            }
        }
    }
}
