//! Ray-traced sun shadows (rt_shadows.rs, docs/spec/rt-shadows.md) fall where the cascaded ones
//! do, on each native backend whose adapter has ray queries. Three renderers follow the same ticks:
//! one without sun shadows, one with cascades, one with ray-traced shadows on a device that carries
//! ray queries. In every state the pixels each shadow path darkens must largely coincide, and from
//! one state to the next so must the pixels whose shadowing changed:
//!
//! 1. the mixed demo scene, static (every pipeline variant, alpha-masked checker meshes). Three
//!    more renderers draw it with the masked meshes made opaque: the ground that turns dark there
//!    is the light the checkers' cut-outs let through, which both paths must let through alike;
//! 2. every cell's instance moved 1 m, drawn halfway through the tick's interpolation and at its
//!    end: the top level is rebuilt at the drawn poses;
//! 3. three skinned characters added;
//! 4. their arms raised by the skinning alone: their bottom levels follow it.
//!
//! Each check fails when its defect is put back (docs/bench/rt-shadows.md). Skips (with a note)
//! without a GPU or without ray queries.

use glam::Quat;
use pocket_assets::ModelAsset;
use pocket_assets::frame::{AnimView, InstanceUpdate, Look, Pose, RenderFrame};
use pocket_assets::mesh::{AlphaMode, AnimationClip, Channel, ChannelPath, Interpolation};
use pocket_render::rt_shadows::RtShadowStats;
use pocket_render::{BackendChoice, Gpu, Renderer, demo};

const N: u32 = 4;
const W: u32 = 480;
const H: u32 = 270;
const HERO: &str = "models/hero.glb";
/// The mixed model with its masked materials made opaque (same meshes, same texture).
const OPAQUE_MODEL: &str = "demo/mixed-opaque.glb";
const DT: f64 = 1.0 / 60.0;

/// hero.glb with a clip that raises both arms from the idle clip's first pose (time 0) to nearly
/// horizontal (time 1), so the characters' shadows change with nothing but the skinning.
fn hero() -> ModelAsset {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../samples/anim/models/hero.glb"
    ))
    .expect("samples/anim/models/hero.glb");
    let mut hero = pocket_assets::import::import_glb_bytes(&bytes).expect("hero.glb imports");
    let channels = [("LeftArm", 1.0f32), ("RightArm", -1.0)]
        .map(|(name, side)| {
            let node = hero
                .skeleton
                .iter()
                .position(|n| n.name == name)
                .unwrap_or_else(|| panic!("hero.glb has a {name}"));
            let arm = |degrees: f32| Quat::from_rotation_z(side * degrees.to_radians()).to_array();
            Channel {
                node,
                path: ChannelPath::Rotation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![arm(6.0), arm(80.0)],
            }
        })
        .to_vec();
    hero.animations.push(AnimationClip {
        name: "raise".into(),
        duration: 1.0,
        channels,
    });
    hero
}

fn opaque_model() -> ModelAsset {
    let mut model = demo::mixed_model();
    for m in &mut model.materials {
        m.alpha_mode = AlphaMode::Opaque;
    }
    model
}

/// Tick 1: the mixed scene, its masked meshes drawn opaque if `opaque`.
fn first(opaque: bool) -> RenderFrame {
    let mut frame = demo::mixed(N);
    if opaque {
        for look in frame.instances.iter_mut().filter_map(|u| u.look.as_mut()) {
            if look.mesh.contains("#masked") {
                look.mesh = look.mesh.replace(demo::MIXED_MODEL, OPAQUE_MODEL);
            }
        }
    }
    frame
}

fn tick(tick: u64, instances: Vec<InstanceUpdate>) -> RenderFrame {
    RenderFrame {
        tick,
        t_s: tick as f64 * DT,
        dt_s: DT,
        instances,
        ..RenderFrame::default()
    }
}

/// Tick 2: every cell's instance (ids 2 and up; 1 is the ground) 1 m further along x.
fn moved(first: &RenderFrame) -> RenderFrame {
    let instances = first
        .instances
        .iter()
        .filter(|u| u.id >= 2)
        .map(|u| {
            let pose = u.pose.expect("posed");
            InstanceUpdate {
                id: u.id,
                pose: Some(Pose {
                    position: [pose.position[0] + 1.0, pose.position[1], pose.position[2]],
                    ..pose
                }),
                look: None,
                anim: None,
            }
        })
        .collect();
    tick(2, instances)
}

fn raise(time: f32) -> AnimView {
    AnimView {
        clip: "raise".into(),
        time,
        rate: 0.0,
        looped: false,
    }
}

/// Tick 3: three skinned characters in front of the grid, arms down.
fn heroes() -> RenderFrame {
    let instances = (0..3u64)
        .map(|i| InstanceUpdate {
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
            anim: Some(raise(0.0)),
        })
        .collect();
    tick(3, instances)
}

/// Tick 4: the characters' arms raised; nothing else changes.
fn raised() -> RenderFrame {
    let instances = (0..3u64)
        .map(|i| InstanceUpdate {
            id: 1000 + i,
            pose: None,
            look: None,
            anim: Some(raise(1.0)),
        })
        .collect();
    tick(4, instances)
}

fn renderer(gpu: &Gpu, hero: &ModelAsset) -> Renderer {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.add_model(demo::MIXED_MODEL, &demo::mixed_model());
    r.add_model(OPAQUE_MODEL, &opaque_model());
    r.add_model(HERO, hero);
    r.set_camera_override(Some(demo::mixed_camera(N)));
    r
}

/// The luminance of the frame drawn at `now`, after a few frames at that moment (they let the
/// temporal filter settle and masked casters' bottom levels join).
fn shoot(r: &mut Renderer, now: f64) -> Vec<f32> {
    for _ in 0..4 {
        let _ = r.capture_rgba(now);
    }
    let (_, _, rgba) = r.capture_rgba(now);
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]))
        .collect()
}

/// The pixels a shadow darkens by more than 20% against the same state drawn without shadows.
fn dark(shot: &[f32], lit: &[f32]) -> Vec<bool> {
    shot.iter()
        .zip(lit)
        .map(|(s, l)| *s < 0.8 * l && *l > 20.0)
        .collect()
}

fn count(a: &[bool]) -> usize {
    a.iter().filter(|x| **x).count()
}

/// Intersection over union of two pixel sets.
fn iou(a: &[bool], b: &[bool]) -> f64 {
    let both = a.iter().zip(b).filter(|(x, y)| **x && **y).count();
    let either = a.iter().zip(b).filter(|(x, y)| **x || **y).count();
    both as f64 / either.max(1) as f64
}

/// The pixels whose shadowing differs between two states.
fn changed(a: &[bool], b: &[bool]) -> Vec<bool> {
    a.iter().zip(b).map(|(x, y)| x != y).collect()
}

/// The renderers' devices: one without ray queries (cascades), one with them.
struct Devices {
    cascaded: Gpu,
    traced: Gpu,
}

/// One state as a set of renderers draws it: the unshadowed image and each path's shadow pixels.
struct State {
    lit: Vec<f32>,
    csm: Vec<bool>,
    rt: Vec<bool>,
    stats: RtShadowStats,
}

/// A renderer per path, fed the same ticks.
struct Paths {
    lit: Renderer,
    csm: Renderer,
    rt: Renderer,
}

impl Paths {
    fn new(devices: &Devices, hero: &ModelAsset) -> Paths {
        Paths {
            lit: renderer(&devices.cascaded, hero),
            csm: renderer(&devices.cascaded, hero),
            rt: renderer(&devices.traced, hero),
        }
    }

    fn apply(&mut self, frame: &RenderFrame, now: f64) {
        let mut unlit = frame.clone();
        for light in unlit.lights.iter_mut().flatten() {
            light.shadows = false;
        }
        self.lit.apply(unlit, now);
        self.csm.apply(frame.clone(), now);
        self.rt.apply(frame.clone(), now);
    }

    fn shoot(&mut self, now: f64) -> State {
        let lit = shoot(&mut self.lit, now);
        let csm = dark(&shoot(&mut self.csm, now), &lit);
        let rt = dark(&shoot(&mut self.rt, now), &lit);
        assert!(
            self.csm.rt_shadow_stats().is_none(),
            "cascades without ray queries"
        );
        let stats = self
            .rt
            .rt_shadow_stats()
            .expect("ray-traced shadows on a ray-query device");
        State {
            lit,
            csm,
            rt,
            stats,
        }
    }
}

/// Both paths cast visible shadows in `state`, and largely in the same places: ray-traced shadows
/// are hard where the cascades filter 3x3, and both test masked casters' alpha at their own sample
/// positions, so the sets differ at edges.
fn agree(name: &str, what: &str, state: &State, min_iou: f64) {
    let (in_csm, in_rt) = (count(&state.csm), count(&state.rt));
    let overlap = iou(&state.csm, &state.rt);
    eprintln!(
        "{name}, {what}: shadowed pixels {in_csm} cascaded, {in_rt} ray traced, overlap {overlap:.3}"
    );
    let pixels = (W * H) as usize;
    assert!(
        in_csm > pixels / 100 && in_rt > pixels / 100,
        "{name}, {what}: both paths cast visible shadows ({in_csm}, {in_rt} of {pixels})"
    );
    assert!(
        overlap > min_iou,
        "{name}, {what}: shadows fall in the same places (intersection over union {overlap:.3})"
    );
}

/// The pixels whose shadowing changed from `a` to `b` are largely the same for both paths.
fn follow(name: &str, what: &str, a: &State, b: &State, min_pixels: usize) {
    let (csm, rt) = (changed(&a.csm, &b.csm), changed(&a.rt, &b.rt));
    let overlap = iou(&csm, &rt);
    eprintln!(
        "{name}, {what}: changed pixels {} cascaded, {} ray traced, overlap {overlap:.3}",
        count(&csm),
        count(&rt)
    );
    assert!(
        count(&csm) > min_pixels,
        "{name}, {what}: the cascaded shadows change ({} pixels)",
        count(&csm)
    );
    assert!(
        overlap > 0.8,
        "{name}, {what}: the ray-traced shadows change where the cascaded ones do \
         (intersection over union {overlap:.3})"
    );
}

/// The light masked casters' cut-outs let through in `still`: ground an opaque copy of them
/// shadows and the masked meshes leave lit, outside the masked meshes' own pixels. Ray-traced
/// shadows test each candidate hit's alpha as the masked shadow pass tests each fragment's, so they
/// must let about as much through as the cascades; masked casters treated as opaque let none.
fn cut_outs(name: &str, devices: &Devices, hero: &ModelAsset, still: &State) {
    let mut opaque = Paths::new(devices, hero);
    opaque.apply(&first(true), 0.0);
    let solid = opaque.shoot(1.0);
    let light = |masked: &[bool], solid_dark: &[bool]| -> Vec<bool> {
        (0..masked.len())
            .map(|i| (still.lit[i] - solid.lit[i]).abs() < 2.0 && solid_dark[i] && !masked[i])
            .collect()
    };
    let (csm, rt) = (light(&still.csm, &solid.csm), light(&still.rt, &solid.rt));
    let (in_csm, in_rt) = (count(&csm), count(&rt));
    eprintln!(
        "{name}: light through masked casters' cut-outs {in_csm} pixels cascaded, {in_rt} ray \
         traced, overlap {:.3}",
        iou(&csm, &rt)
    );
    assert!(
        in_csm > 50 && in_rt * 2 > in_csm && in_rt < in_csm * 2,
        "{name}: masked casters' cut-outs let the sun through as under the cascades ({in_rt} \
         pixels against {in_csm})"
    );
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
    let hero = hero();
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
        let devices = Devices { cascaded, traced };
        let mut paths = Paths::new(&devices, &hero);

        // 1. Static.
        let scene = first(false);
        paths.apply(&scene, 0.0);
        let still = paths.shoot(1.0);
        agree(&name, "static", &still, 0.9);
        assert!(
            still.stats.instances == N * N + 1 && still.stats.masked > 0,
            "{name}: every caster in the top level, masked ones too: {:?}",
            still.stats
        );
        assert!(
            !still.stats.rebuilt,
            "{name}: a static scene keeps its top level: {:?}",
            still.stats
        );
        cut_outs(&name, &devices, &hero, &still);

        // 2. Every cell moves 1 m during tick 2: halfway, then there.
        paths.apply(&moved(&scene), 2.0);
        let halfway = paths.shoot(2.0 + DT * 0.5);
        assert!(
            halfway.stats.rebuilt,
            "{name}: moving casters rebuild the top level every frame"
        );
        agree(&name, "halfway", &halfway, 0.9);
        follow(&name, "static to halfway", &still, &halfway, 1000);
        let there = paths.shoot(3.0);
        agree(&name, "moved", &there, 0.9);
        follow(&name, "halfway to moved", &halfway, &there, 1000);

        // 3. Skinned characters join; their bottom levels are rebuilt every frame.
        paths.apply(&heroes(), 4.0);
        let down = paths.shoot(5.0);
        assert!(
            down.stats.rebuilt && down.stats.instances > N * N + 1,
            "{name}: skinned casters rebuild every frame: {:?}",
            down.stats
        );
        agree(&name, "characters", &down, 0.85);
        follow(&name, "characters added", &there, &down, 1000);

        // 4. The skinning raises their arms.
        paths.apply(&raised(), 6.0);
        let up = paths.shoot(7.0);
        agree(&name, "arms raised", &up, 0.85);
        follow(&name, "arms raised", &down, &up, 500);
    }
}
