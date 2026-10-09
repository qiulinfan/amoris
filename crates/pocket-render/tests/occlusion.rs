//! Occlusion culling never drops a visible instance (docs/spec/occlusion.md). The occluder scene
//! (`demo::occluders`: a wall with a slit, an alpha-masked occluder with holes, a picket fence,
//! small spheres peeking over the wall, a grid of every primitive behind them) is drawn by two
//! renderers on the same device, one with occlusion culling off and one with it forced on,
//! through the same sequence of frames: the cold first frame, a warm frame, a camera cut, a
//! sideways sweep (the wall's edges, the slit and the fence move across the pyramid's texels), an
//! occluder moving away (mid-interpolation and arrived), a resize, a camera against the slit and
//! an orthographic camera. At each step the entity-id pass must show the same entities with the
//! same pixel coverage and the images must match; the forced-on renderer must also have culled
//! something. Each step's id pass is drawn in the first frame after the change, when last frame's
//! visible set is the most wrong. Runs on the device `POCKET_BACKEND` and `POCKET_GPU_MINIMAL`
//! describe, then on the two other draw paths (batches.rs); skips without a GPU.

use pocket_render::gpu::Minimal;
use pocket_render::{BackendChoice, Gpu, OcclusionMode, OcclusionStats, Renderer, demo};

struct Shot {
    step: &'static str,
    rgba: Vec<u8>,
    visible: Vec<(u64, f32)>,
    stats: Option<OcclusionStats>,
    occlusion: &'static str,
}

/// Draws one frame with the id pass at `t`, keeps its image, then draws on until the id pass's
/// answer arrives.
fn shot(r: &mut Renderer, step: &'static str, t: &mut f64) -> Shot {
    r.request_visible();
    *t += 1.0 / 60.0;
    let (_, _, rgba) = r.capture_rgba(*t);
    let occlusion = r.last.occlusion;
    let mut visible = None;
    for _ in 0..30 {
        if let Some(v) = r.take_visible() {
            visible = Some(v);
            break;
        }
        *t += 1.0 / 60.0;
        let _ = r.capture_rgba(*t);
    }
    Shot {
        step,
        rgba,
        visible: visible.expect("the id pass answers"),
        stats: r.last.occlusion_stats,
        occlusion,
    }
}

fn frames(r: &mut Renderer, n: u32, t: &mut f64) {
    for _ in 0..n {
        *t += 1.0 / 60.0;
        let _ = r.capture_rgba(*t);
    }
}

fn run(gpu: &Gpu, mode: OcclusionMode) -> Vec<Shot> {
    let [front, side] = demo::occluders_cameras();
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 480, 270);
    r.set_occlusion(mode);
    r.add_model(demo::MIXED_MODEL, &demo::mixed_model());
    let mut t = 0.0;
    r.apply(demo::occluders(3.0), t);
    r.set_camera_override(Some(front));
    let mut shots = vec![shot(&mut r, "cold", &mut t)];
    frames(&mut r, 4, &mut t);
    shots.push(shot(&mut r, "warm", &mut t));
    r.set_camera_override(Some(side));
    shots.push(shot(&mut r, "cut", &mut t));
    // A sideways sweep: each shot is the first frame from a new position.
    for (i, step) in [
        "sweep 1", "sweep 2", "sweep 3", "sweep 4", "sweep 5", "sweep 6",
    ]
    .into_iter()
    .enumerate()
    {
        r.set_camera_override(Some(demo::occluders_sweep(i as f32 * 1.37 - 3.1)));
        shots.push(shot(&mut r, step, &mut t));
    }
    r.set_camera_override(Some(front));
    frames(&mut r, 4, &mut t);
    // The left wall sinks into the ground over one tick: drawn halfway, then arrived.
    let mut moved = demo::occluders(-3.5);
    moved.tick = 2;
    moved.reset = false;
    moved.lights = None;
    moved.environment = None;
    moved.cameras = None;
    moved.instances.retain(|i| i.id == 2);
    moved.instances[0].look = None;
    r.apply(moved, t + 1.0 / 60.0 - 1.0 / 120.0);
    shots.push(shot(&mut r, "moving", &mut t));
    shots.push(shot(&mut r, "moved", &mut t));
    r.resize(640, 360);
    shots.push(shot(&mut r, "resized", &mut t));
    // Against the right wall's slit end: the grid behind is seen through 0.5 m.
    r.set_camera_override(Some(pocket_render::CameraState::look_at(
        glam::Vec3::new(0.0, 3.0, 0.6),
        glam::Vec3::new(0.0, 2.8, -10.0),
    )));
    shots.push(shot(&mut r, "close", &mut t));
    r.set_camera_override(Some(pocket_render::CameraState {
        ortho_height: Some(18.0),
        ..front
    }));
    shots.push(shot(&mut r, "ortho", &mut t));
    shots
}

fn share(v: &[(u64, f32)], e: u64) -> f32 {
    v.iter().find(|x| x.0 == e).map_or(0.0, |x| x.1)
}

fn agree(off: &Shot, on: &Shot, path: &str) {
    let step = off.step;
    let mut ids: Vec<u64> = off.visible.iter().chain(&on.visible).map(|x| x.0).collect();
    ids.sort_unstable();
    ids.dedup();
    for e in ids {
        let (a, b) = (share(&off.visible, e), share(&on.visible, e));
        assert!(
            (a - b).abs() < 1e-6,
            "{path}, {step}: entity {e} covers {a} of the view without occlusion culling and \
             {b} with it ({:?})",
            on.stats
        );
    }
    assert_eq!(off.rgba.len(), on.rgba.len(), "{path}, {step}");
    let differing = off
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(on.rgba.as_chunks::<4>().0)
        .filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > 2))
        .count();
    assert!(
        differing * 2000 < off.rgba.len() / 4,
        "{path}, {step}: {differing} pixels differ"
    );
}

fn check(gpu: &Gpu, label: &str) {
    let path = format!("{label} ({}, {})", gpu.backend_name(), gpu.info.name);
    let off = run(gpu, OcclusionMode::Off);
    let on = run(gpu, OcclusionMode::On);
    for (a, b) in off.iter().zip(&on) {
        eprintln!(
            "{path}, {}: {} entities visible; {} {:?}",
            a.step,
            a.visible.len(),
            b.occlusion,
            b.stats
        );
        assert_eq!(a.occlusion, "off");
        assert_eq!(b.occlusion, "on");
        agree(a, b, &path);
    }
    // The scene is a real test: the grid is partly hidden from the front, and shown from the side.
    let front = &off[1];
    assert!(front.visible.len() >= 30, "{:?}", front.visible);
    assert!(
        (100..244).any(|e| share(&front.visible, e) == 0.0),
        "nothing is hidden from the front"
    );
    // Occlusion culling culled: by the warm step, many instances in the frustum were occluded.
    let warm = on[1].stats.expect("a reading by the warm step");
    assert!(warm.occluded >= 20, "{warm:?}");
    // The lowered wall uncovered instances it hid.
    let moved = off
        .iter()
        .find(|s| s.step == "moved")
        .expect("a moved step");
    let (before, after) = (&off[1], moved);
    assert!(
        after
            .visible
            .iter()
            .any(|(e, _)| share(&before.visible, *e) == 0.0),
        "lowering the wall uncovered nothing"
    );
}

#[test]
fn occlusion_culling_never_drops_a_visible_instance() {
    let choice = BackendChoice::from_env();
    let Ok(gpu) = Gpu::headless(choice) else {
        eprintln!("no GPU: skipped");
        return;
    };
    check(&gpu, "default device");
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
        check(&baseline, "baseline path");
        if gpu.caps.multi_draw_indirect {
            let per_batch = Gpu::headless_with(
                choice,
                Minimal {
                    multi_draw: true,
                    ..env
                },
            )
            .expect("a per-batch device");
            check(&per_batch, "first-instance path");
        }
    }
}
