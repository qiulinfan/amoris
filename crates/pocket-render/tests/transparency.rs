//! The transparent stage (docs/spec/transparency.md; charter 4.4, Pioneer 2026-10-16). Under an
//! orthographic camera, with no light but what the materials emit (no sun, no ambient light), a
//! transparent surface of coverage `a` and emission `e` over a background `b` must show exactly
//! `a e + (1 - a) b`:
//!
//! - two overlapping half-transparent quads (glTF BLEND, alpha 0.5, a red one behind a green one)
//!   give the right colours where each is alone and where they overlap, the same to the bit
//!   whichever the feed lists first: the stage sorts by depth, not by submission;
//! - a double-sided BLEND quad seen from behind is drawn (its back faces), a single-sided one is not;
//! - a look whose alpha is below 1 on the inline material blends too;
//! - transparent surfaces never occlude: a field of opaque cubes behind a transparent pane draws
//!   the same image with the depth prepass and occlusion culling forced on as with both off, every
//!   cube shows through the pane, and the late culling pass finds none of them occluded (the pane is
//!   in neither the prepass's depth nor the depth pyramid);
//! - transparent instances are picked: the entity-id pass answers the pane where it covers a cube;
//! - the stage's pipeline follows the opaque pass's format on one renderer (anti-aliasing and GTAO
//!   switched back and forth give the images of renderers made so), and with GTAO on the stage
//!   draws in its own pass after GTAO, changing no pixel where GTAO darkens nothing;
//! - GTAO darkens what shows through a pane by the pane's transmittance and never the pane's own
//!   light: under a sky's light, with a box on a floor behind a pane of coverage 0.5, the pane's
//!   pixels lose exactly half of what GTAO takes from the same pixels without the pane;
//! - at a render scale below 1 (TAA upscaling) the panes converge to the same colours;
//! - transparent casters shadow by their opacity under the cascades and, on a device with ray
//!   queries, the ray-traced shadows: clear glass and a blend of alpha 0.35 cast nothing (and are
//!   not casters at all), a blend of alpha 0.6 casts as an opaque cube does.
//!
//! Checked with 4x MSAA and with one sample unless said. Skips when there is no GPU.

use glam::{Quat, Vec3};
use pocket_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};
use pocket_assets::mesh::{AlphaMode, MaterialData, ModelAsset, NodeData};
use pocket_assets::primitives::primitive;
use pocket_render::{
    Antialiasing, AoNormals, BackendChoice, CameraState, Gpu, Gtao, OcclusionMode, PrepassMode,
    Renderer,
};

const W: u32 = 320;
const H: u32 = 160;
/// The view's height in metres (20 pixels a metre).
const ORTHO: f32 = 8.0;
const MODEL: &str = "test/panes.glb";

/// Quads (the plane primitive) with BLEND materials of alpha 0.5 and no albedo, emitting red,
/// green and blue; the blue one double-sided.
fn model() -> ModelAsset {
    let mut asset = ModelAsset::default();
    for (i, (name, emissive, double_sided)) in [
        ("red", [1.0, 0.0, 0.0], false),
        ("green", [0.0, 1.0, 0.0], false),
        ("blue", [0.0, 0.0, 1.0], true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut mesh = primitive("plane").expect("the plane primitive");
        mesh.name = name.into();
        mesh.material = Some(i);
        asset.meshes.push(mesh);
        asset.materials.push(MaterialData {
            name: name.into(),
            base_color: [0.0, 0.0, 0.0, 0.5],
            emissive,
            alpha_mode: AlphaMode::Blend,
            double_sided,
            ..MaterialData::default()
        });
        asset.nodes.push(NodeData {
            name: name.into(),
            mesh: i,
            transform: glam::Mat4::IDENTITY.to_cols_array(),
            skin: None,
        });
    }
    asset
}

fn look(mesh: &str, color: [f32; 4], emissive: [f32; 3]) -> Look {
    Look {
        mesh: mesh.into(),
        material: String::new(),
        color,
        metallic: 0.0,
        roughness: 1.0,
        transmission: None,
        ior: None,
        emissive,
        cast_shadows: false,
        visible: true,
    }
}

/// A quad (the plane, facing +y) standing at (`x`, `y`, `z`) facing the camera (+z), or away.
fn quad(id: u64, mesh: &str, at: [f32; 3], toward: bool, size: f32) -> InstanceUpdate {
    let turn = (if toward { -1.0 } else { 1.0 }) * std::f32::consts::FRAC_PI_2;
    InstanceUpdate {
        id,
        pose: Some(Pose {
            position: at,
            rotation: Quat::from_rotation_x(-turn).to_array(),
            scale: [size, 1.0, size],
        }),
        look: Some(look(mesh, [1.0; 4], [0.0; 3])),
        anim: None,
    }
}

/// No light: no sun, no ambient light, a flat dark blue sky.
fn frame(instances: Vec<InstanceUpdate>) -> RenderFrame {
    RenderFrame {
        tick: 1,
        dt_s: 1.0 / 60.0,
        reset: true,
        instances,
        lights: Some(vec![]),
        cameras: Some(vec![]),
        environment: Some(EnvironmentView {
            sky: 1,
            sky_color: [0.1, 0.2, 0.4],
            ambient: 0.0,
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.0,
            fog_color: [0.6, 0.7, 0.8],
            exposure_ev: 0.0,
            bloom: 0.0,
        }),
        ..RenderFrame::default()
    }
}

fn renderer(gpu: &Gpu, aa: Antialiasing) -> Renderer {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(aa);
    r.set_gtao(Gtao::Off);
    r.add_model(MODEL, &model());
    let mut camera = CameraState::look_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO);
    camera.ortho_height = Some(ORTHO);
    r.set_camera_override(Some(camera));
    r
}

/// The pixel at (`x`, `y`) metres from the view's centre.
fn pixel(x: f32, y: f32) -> (u32, u32) {
    let px = H as f32 / ORTHO;
    (
        (W as f32 * 0.5 + x * px) as u32,
        (H as f32 * 0.5 - y * px) as u32,
    )
}

fn rgb(img: &[f32], x: f32, y: f32) -> [f32; 3] {
    let (px, py) = pixel(x, y);
    let i = ((py * W + px) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
}

fn assert_near(what: &str, got: [f32; 3], want: [f32; 3]) {
    assert_within(what, got, want, 2e-3);
}

fn assert_within(what: &str, got: [f32; 3], want: [f32; 3], tolerance: f32) {
    let off = (0..3).map(|k| (got[k] - want[k]).abs()).fold(0.0, f32::max);
    assert!(
        off < tolerance,
        "{what}: got {got:?}, expected {want:?} (off by {off})"
    );
}

/// `a e + (1 - a) b` for coverage 0.5.
fn over(e: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        0.5 * e[0] + 0.5 * b[0],
        0.5 * e[1] + 0.5 * b[1],
        0.5 * e[2] + 0.5 * b[2],
    ]
}

/// The quads: red at z = -1 spanning x in [-3, 1], green at z = 1 spanning x in [-1, 3] (both 4 m
/// square, centred at y = 1.5); below them, at y = -2.5, the double-sided blue quad and a red one
/// facing away; an inline quad with a look alpha of 0.5 emitting white at x = 6.5. `reversed` lists
/// them in the opposite order.
fn panes(reversed: bool) -> RenderFrame {
    let mut v = vec![
        quad(1, &format!("{MODEL}#red"), [-1.0, 1.5, -1.0], true, 4.0),
        quad(2, &format!("{MODEL}#green"), [1.0, 1.5, 1.0], true, 4.0),
        quad(3, &format!("{MODEL}#blue"), [-3.0, -2.5, 0.0], false, 2.0),
        quad(4, &format!("{MODEL}#red"), [1.0, -2.5, 0.0], false, 2.0),
        InstanceUpdate {
            look: Some(look("plane", [0.0, 0.0, 0.0, 0.5], [1.0, 1.0, 1.0])),
            ..quad(5, "plane", [6.5, 1.5, 0.0], true, 2.0)
        },
    ];
    if reversed {
        v.reverse();
        for (k, u) in v.iter_mut().enumerate() {
            u.id = 1 + k as u64;
        }
    }
    frame(v)
}

/// The panes' colours in `img` over the empty scene's `sky`, within `tolerance`.
fn assert_panes(what: &str, img: &[f32], sky: &[f32], tolerance: f32) {
    let b = rgb(sky, -2.0, 1.5);
    let red = over([1.0, 0.0, 0.0], b);
    let green = over([0.0, 1.0, 0.0], b);
    let near =
        |part: &str, got, want| assert_within(&format!("{what}: {part}"), got, want, tolerance);
    near("red alone", rgb(img, -2.0, 1.5), red);
    near("green alone", rgb(img, 2.0, 1.5), green);
    near(
        "green over red",
        rgb(img, 0.0, 1.5),
        over([0.0, 1.0, 0.0], red),
    );
    near(
        "the double-sided quad from behind",
        rgb(img, -3.0, -2.5),
        over([0.0, 0.0, 1.0], rgb(sky, -3.0, -2.5)),
    );
    near(
        "the single-sided quad from behind (not drawn)",
        rgb(img, 1.0, -2.5),
        rgb(sky, 1.0, -2.5),
    );
    near(
        "the inline look of alpha 0.5",
        rgb(img, 6.5, 1.5),
        over([1.0, 1.0, 1.0], rgb(sky, 6.5, 1.5)),
    );
}

fn check_panes(gpu: &Gpu, aa: Antialiasing) {
    let mut r = renderer(gpu, aa);
    r.apply(frame(vec![]), 0.0);
    let sky = r.capture_hdr(0.0).2;
    let mut images = Vec::new();
    for reversed in [false, true] {
        r.apply(panes(reversed), 0.0);
        let img = r.capture_hdr(0.0).2;
        assert_eq!(
            r.last.transparent, 5,
            "{aa:?}: five transparent instances drawn"
        );
        assert_panes(&format!("{aa:?}"), &img, &sky, 2e-3);
        images.push(img);
    }
    assert!(
        images[0] == images[1],
        "{aa:?}: the image depends on the order the feed lists the quads in"
    );
}

#[test]
fn overlapping_blended_quads_compose_back_to_front_in_any_order() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    for aa in [Antialiasing::Msaa, Antialiasing::Off] {
        check_panes(&gpu, aa);
    }
}

/// A 5 x 3 field of opaque cubes (white, emitting grey) at z = -2 behind a transparent pane (alpha
/// 0.5, emitting nothing: it halves what is behind) at z = 2 that covers them all.
fn field() -> RenderFrame {
    let mut v = Vec::new();
    for row in 0..3 {
        for col in 0..5 {
            v.push(InstanceUpdate {
                id: 1 + (row * 5 + col) as u64,
                pose: Some(Pose {
                    position: [-6.0 + 3.0 * col as f32, -2.0 + 2.0 * row as f32, -2.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                }),
                look: Some(look("cube", [1.0; 4], [0.5, 0.5, 0.5])),
                anim: None,
            });
        }
    }
    v.push(InstanceUpdate {
        look: Some(look("plane", [0.0, 0.0, 0.0, 0.5], [0.0; 3])),
        ..quad(100, "plane", [0.0, 0.0, 2.0], true, 15.0)
    });
    frame(v)
}

/// Draws `frames` frames of the field, then reads the image and the pane's pick at a cube's centre.
fn field_shot(
    gpu: &Gpu,
    aa: Antialiasing,
    prepass: PrepassMode,
    occlusion: OcclusionMode,
) -> (Vec<f32>, Renderer) {
    let mut r = renderer(gpu, aa);
    r.set_prepass(prepass);
    r.set_occlusion(occlusion);
    r.apply(field(), 0.0);
    for _ in 0..12 {
        let _ = r.capture_rgba(0.0);
    }
    let img = r.capture_hdr(0.0).2;
    (img, r)
}

#[test]
fn transparent_instances_never_occlude_and_are_picked() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    for aa in [Antialiasing::Msaa, Antialiasing::Off] {
        let (plain, _) = field_shot(&gpu, aa, PrepassMode::Off, OcclusionMode::Off);
        let (culled, mut r) = field_shot(&gpu, aa, PrepassMode::On, OcclusionMode::On);
        assert!(
            plain == culled,
            "{aa:?}: the prepass or occlusion culling changed the image under a transparent pane"
        );
        // Every cube shows through the pane: half its grey.
        for row in 0..3 {
            for col in 0..5 {
                let (x, y) = (-6.0 + 3.0 * col as f32, -2.0 + 2.0 * row as f32);
                let c = rgb(&culled, x, y);
                assert_near(
                    &format!("{aa:?}: the cube at ({x}, {y}) through the pane"),
                    c,
                    [0.25, 0.25, 0.25],
                );
            }
        }
        let stats = r
            .last
            .occlusion_stats
            .expect("the late culling pass's counters arrived");
        assert_eq!(
            (stats.frustum, stats.occluded),
            (15, 0),
            "{aa:?}: the cubes in the frustum and those the depth pyramid hid ({stats:?})"
        );
        // The pane covers the cubes in the entity-id pass.
        let (px, py) = pixel(0.0, 0.0);
        r.request_pick(px, py);
        let mut picked = None;
        for _ in 0..30 {
            let _ = r.capture_rgba(0.0);
            if let Some(p) = r.take_pick() {
                picked = Some(p);
                break;
            }
        }
        assert_eq!(picked, Some(Some(100)), "{aa:?}: the pick through the pane");
    }
}

/// The panes drawn by a renderer made with `aa` and `gtao`.
fn fresh_panes(gpu: &Gpu, aa: Antialiasing, gtao: Gtao) -> Vec<f32> {
    let mut r = renderer(gpu, aa);
    r.set_gtao(gtao);
    r.apply(panes(false), 0.0);
    r.capture_hdr(0.0).2
}

#[test]
fn the_stage_follows_the_opaque_pass_format_on_one_renderer() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let on = Gtao::On(AoNormals::Depth);
    let msaa = fresh_panes(&gpu, Antialiasing::Msaa, Gtao::Off);
    let one = fresh_panes(&gpu, Antialiasing::Off, Gtao::Off);
    // No light reaches the panes' scene indirectly (no ambient light): GTAO darkens nothing, so
    // with it on, the stage drawn in its own pass after GTAO gives the same image to the bit.
    assert!(
        fresh_panes(&gpu, Antialiasing::Msaa, on) == msaa,
        "4x MSAA: the stage after GTAO changed the image"
    );
    assert!(
        fresh_panes(&gpu, Antialiasing::Off, on) == one,
        "one sample: the stage after GTAO changed the image"
    );
    // One renderer through the formats: each switch rebuilds the stage's pipeline.
    let mut r = renderer(&gpu, Antialiasing::Msaa);
    r.apply(panes(false), 0.0);
    let steps = [
        (Antialiasing::Msaa, Gtao::Off, &msaa),
        (Antialiasing::Off, Gtao::Off, &one),
        (Antialiasing::Msaa, Gtao::Off, &msaa),
        (Antialiasing::Msaa, on, &msaa),
        (Antialiasing::Off, on, &one),
        (Antialiasing::Off, Gtao::Off, &one),
    ];
    for (k, (aa, gtao, want)) in steps.into_iter().enumerate() {
        r.set_antialiasing(aa);
        r.set_gtao(gtao);
        let img = r.capture_hdr(0.0).2;
        assert_eq!(
            r.last.transparent, 5,
            "step {k}: five transparent instances"
        );
        assert!(
            img == *want,
            "step {k} ({aa:?}, GTAO {}): not the image of a renderer made so",
            gtao.name()
        );
    }
}

/// A box (2 m, grey) on a floor under a flat sky's light only (no sun), seen from above and in
/// front; with `pane`, a pane of coverage 0.5 (the inline material, black, emitting 0.2) between
/// the camera and the box's foot, where GTAO darkens the floor and the box.
fn gtao_scene(pane: bool) -> RenderFrame {
    let at = |id, position, scale, look| InstanceUpdate {
        id,
        pose: Some(Pose {
            position,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale,
        }),
        look: Some(look),
        anim: None,
    };
    let grey = [0.8, 0.8, 0.8, 1.0];
    let mut v = vec![
        at(
            1,
            [0.0; 3],
            [20.0, 1.0, 20.0],
            look("plane", grey, [0.0; 3]),
        ),
        at(2, [0.0, 1.0, 0.0], [2.0; 3], look("cube", grey, [0.0; 3])),
    ];
    if pane {
        v.push(InstanceUpdate {
            look: Some(look("plane", [0.0, 0.0, 0.0, 0.5], [0.2; 3])),
            ..quad(3, "plane", [0.0, 1.0, 2.5], true, 6.0)
        });
    }
    let mut f = frame(v);
    if let Some(env) = &mut f.environment {
        env.sky_color = [0.35, 0.5, 0.75];
        env.ambient = 1.0;
    }
    f
}

fn gtao_shot(
    gpu: &Gpu,
    aa: Antialiasing,
    occlusion: OcclusionMode,
    gtao: Gtao,
    pane: bool,
) -> Vec<f32> {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(aa);
    r.set_gtao(gtao);
    r.set_occlusion(occlusion);
    r.set_prepass(PrepassMode::Off);
    r.set_camera_override(Some(CameraState::look_at(
        Vec3::new(0.0, 4.0, 8.0),
        Vec3::new(0.0, 0.5, 0.0),
    )));
    r.apply(gtao_scene(pane), 0.0);
    // Occlusion culling's early pass draws what the last frame saw.
    for _ in 0..3 {
        let _ = r.capture_rgba(0.0);
    }
    r.capture_hdr(0.0).2
}

fn luminance(img: &[f32], i: usize) -> f32 {
    0.2126 * img[4 * i] + 0.7152 * img[4 * i + 1] + 0.0722 * img[4 * i + 2]
}

#[test]
fn gtao_darkens_what_shows_through_a_pane_by_its_transmittance_only() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let on = Gtao::On(AoNormals::Depth);
    // The pane passes 1 - 0.5 of what is behind it (black: no see-through tint).
    let t = 0.5;
    for aa in [Antialiasing::Msaa, Antialiasing::Off] {
        for occlusion in [OcclusionMode::Off, OcclusionMode::On] {
            let what = format!("{aa:?}, occlusion {occlusion:?}");
            let shot = |gtao, pane| gtao_shot(&gpu, aa, occlusion, gtao, pane);
            let (b_off, b_on) = (shot(Gtao::Off, false), shot(on, false));
            let (p_off, p_on) = (shot(Gtao::Off, true), shot(on, true));
            // The pane's pixels, less two at its silhouette: with 4x MSAA a pixel the pane covers
            // in part passes more of what is behind.
            let covered: Vec<bool> = (0..(W * H) as usize)
                .map(|i| (luminance(&p_off, i) - luminance(&b_off, i)).abs() > 1e-3)
                .collect();
            let inside = |i: usize| {
                let (x, y) = ((i as u32 % W) as i32, (i as u32 / W) as i32);
                (-2..=2).all(|dy| {
                    (-2..=2).all(|dx| {
                        let (nx, ny) = (x + dx, y + dy);
                        nx >= 0
                            && ny >= 0
                            && nx < W as i32
                            && ny < H as i32
                            && covered[(ny as u32 * W + nx as u32) as usize]
                    })
                })
            };
            let (mut pixels, mut taken, mut expected, mut worst) = (0, 0.0f64, 0.0f64, 0.0f32);
            for i in 0..(W * H) as usize {
                let ao = luminance(&b_off, i) - luminance(&b_on, i);
                if ao < 0.02 * luminance(&b_off, i) || !inside(i) {
                    continue;
                }
                pixels += 1;
                for c in 0..3 {
                    let k = 4 * i + c;
                    let through = t * (b_off[k] - b_on[k]);
                    let got = p_off[k] - p_on[k];
                    taken += f64::from(got);
                    expected += f64::from(through);
                    worst = worst.max((got - through).abs());
                }
            }
            assert!(
                pixels > 200,
                "{what}: {pixels} pane pixels with occlusion behind"
            );
            let ratio = taken / expected;
            eprintln!("{what}: {pixels} pixels, ratio {ratio:.4}, worst {worst}");
            assert!(
                (ratio - 1.0).abs() < 0.005 && worst < 1e-3,
                "{what}: over {pixels} pixels the pane passes {ratio:.4} of the occlusion behind \
                 it (1 expected), worst pixel off by {worst}"
            );
        }
    }
}

#[test]
fn panes_converge_under_taa_at_a_render_scale_below_one() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    for scale in [1.0, 0.67] {
        let mut r = renderer(&gpu, Antialiasing::Taa);
        r.set_render_scale(scale);
        r.apply(frame(vec![]), 0.0);
        let sky = r.capture_hdr(0.0).2;
        r.apply(panes(false), 0.0);
        for _ in 0..47 {
            let _ = r.capture_rgba(0.0);
        }
        let img = r.capture_hdr(0.0).2;
        assert_eq!(r.last.transparent, 5);
        assert_panes(&format!("TAA at scale {scale}"), &img, &sky, 4e-3);
    }
}

/// Four cubes (1 m) 2 m above a floor, 5 m apart along x, under a sun slanting toward -x (each
/// cube's shadow falls 1.5 m to its left) and a little sky light: an opaque one, clear glass, and
/// blends of alpha 0.6 and 0.35. With `shadows` off, the same lit without them.
fn casters(shadows: bool) -> RenderFrame {
    let at = |id, x: f32, y: f32, scale, look| InstanceUpdate {
        id,
        pose: Some(Pose {
            position: [x, y, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale,
        }),
        look: Some(Look {
            cast_shadows: true,
            ..look
        }),
        anim: None,
    };
    let grey = |alpha| look("cube", [0.7, 0.7, 0.7, alpha], [0.0; 3]);
    let mut f = frame(vec![
        at(
            1,
            0.0,
            0.0,
            [40.0, 1.0, 40.0],
            look("plane", [0.8, 0.8, 0.8, 1.0], [0.0; 3]),
        ),
        at(2, -7.5, 2.5, [1.0; 3], grey(1.0)),
        at(
            3,
            -2.5,
            2.5,
            [1.0; 3],
            Look {
                transmission: Some(1.0),
                ..grey(1.0)
            },
        ),
        at(4, 2.5, 2.5, [1.0; 3], grey(0.6)),
        at(5, 7.5, 2.5, [1.0; 3], grey(0.35)),
    ]);
    f.lights = Some(vec![LightView {
        id: 1,
        kind: LightKindView::Directional,
        position: [0.0; 3],
        direction: Vec3::new(-0.6, -1.0, 0.0).normalize().to_array(),
        color: [1.0; 3],
        intensity: 3.0,
        range: 0.0,
        inner_deg: 0.0,
        outer_deg: 0.0,
        shadows,
    }]);
    if let Some(env) = &mut f.environment {
        env.ambient = 0.3;
    }
    f
}

/// The mean luminance of the floor where each cube's shadow falls, drawn top-down (orthographic,
/// 16 pixels a metre).
fn shadow_spots(gpu: &Gpu, shadows: bool) -> (Vec<f32>, Renderer) {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(Antialiasing::Off);
    r.set_gtao(Gtao::Off);
    let mut camera = CameraState::look_at_up(Vec3::new(0.0, 20.0, 0.0), Vec3::ZERO, Vec3::NEG_Z);
    camera.ortho_height = Some(10.0);
    r.set_camera_override(Some(camera));
    r.apply(casters(shadows), 0.0);
    // Masked casters' bottom levels join a frame after they are asked for.
    for _ in 0..4 {
        let _ = r.capture_rgba(0.0);
    }
    let img = r.capture_hdr(0.0).2;
    let px = H as f32 / 10.0;
    let spots = [-7.5f32, -2.5, 2.5, 7.5]
        .iter()
        .map(|x0| {
            let cx = (W as f32 * 0.5 + (x0 - 1.5) * px) as i32;
            let cy = (H as f32 * 0.5) as i32;
            let mut sum = 0.0;
            for y in cy - 3..=cy + 3 {
                for x in cx - 3..=cx + 3 {
                    sum += luminance(&img, (y as u32 * W + x as u32) as usize);
                }
            }
            sum / 49.0
        })
        .collect();
    (spots, r)
}

#[test]
fn transparent_casters_shadow_by_their_opacity() {
    let choice = BackendChoice::from_env();
    let Ok(gpu) = Gpu::headless(choice) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let (lit, _) = shadow_spots(&gpu, false);
    let mut paths = vec![("cascades", gpu)];
    match Gpu::headless_ray_query(choice, true) {
        Ok(traced) if traced.caps.ray_query => paths.push(("ray traced", traced)),
        _ => eprintln!("no ray queries: ray-traced shadows skipped"),
    }
    for (path, gpu) in &paths {
        let (shot, r) = shadow_spots(gpu, true);
        eprintln!("{path}: lit {lit:?}, shadowed {shot:?}");
        let dark: Vec<bool> = shot.iter().zip(&lit).map(|(s, l)| *s < 0.6 * l).collect();
        assert_eq!(
            dark,
            [true, false, true, false],
            "{path}: shadows under the opaque cube, the glass, alpha 0.6 and alpha 0.35"
        );
        if let Some(stats) = r.rt_shadow_stats() {
            // The floor, the opaque cube and the blend of alpha 0.6 (alpha-masked).
            assert_eq!((stats.instances, stats.masked), (3, 1), "{path}: {stats:?}");
        }
    }
}
