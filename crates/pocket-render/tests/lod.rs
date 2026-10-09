//! Levels of detail (docs/spec/lod.md) on the GPU. Skips without a GPU.
//!
//! 1. The LOD field (`demo::lod_field`: dense rocks and knots from a few metres to over a hundred
//!    away) drawn with levels off and on from two camera positions: with levels on the camera and
//!    the shadow cascades draw far fewer triangles over several levels, and every pixel whose
//!    entity differs lies within two pixels of where the full meshes put that entity (levels are
//!    picked so their error projects to at most a pixel). Forcing level 0 draws the full meshes
//!    again.
//! 2. The same field with levels on is drawn identically, entity for entity and pixel for pixel,
//!    on the three draw paths of batches.rs and with occlusion culling forced on: a level is a row
//!    of the mesh table like any other, and the late pass draws the level the early pass chose.
//! 3. Hysteresis: one rock seen from distances around the one where its first coarser level becomes
//!    acceptable keeps its level until the margin is passed, both ways.

use pocket_assets::frame::{InstanceUpdate, Look, Pose, RenderFrame};
use pocket_render::gpu::Minimal;
use pocket_render::lod::DrawCounts;
use pocket_render::{BackendChoice, CameraState, Gpu, LodMode, OcclusionMode, Renderer, demo};

const W: u32 = 640;
const H: u32 = 360;
const N: u32 = 10;
const SPACING: f32 = 5.0;
/// Rocks of 20,480 triangles, knots of 12,288.
const DETAIL: u32 = 5;

struct Shot {
    rgba: Vec<u8>,
    ids: Vec<u64>,
    counts: DrawCounts,
}

/// The next frame, drawn at `now`: its pixels, its id image and what it drew.
fn shoot(r: &mut Renderer, now: f64) -> Shot {
    r.request_id_image();
    let (_, _, rgba) = r.capture_rgba(now);
    // The arguments still hold that frame's counts; its id image arrives a frame or two later.
    let counts = r.draw_counts();
    let mut ids = None;
    for _ in 0..30 {
        let _ = r.capture_rgba(now);
        if r.take_visible().is_some() {
            ids = r.take_id_image();
            break;
        }
    }
    Shot {
        rgba,
        ids: ids.expect("the id pass answers"),
        counts,
    }
}

fn field(gpu: &Gpu, model: &pocket_assets::ModelAsset, mode: LodMode) -> Renderer {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_lod(mode);
    r.add_model(demo::LOD_MODEL, model);
    r.apply(demo::lod_field(N, SPACING), 0.0);
    r
}

/// The pixels whose entity differs between `a` and `b`, and of those the ones further than
/// `reach` pixels from every edge of `a` (a pixel of another entity): where a silhouette moved
/// further than that. (Not "`b`'s entity is near in `a`": where two silhouettes overlapping in `a`
/// each shrink by a pixel, `b` shows what was behind both, which `a` showed nowhere near.)
fn id_mismatch(a: &[u64], b: &[u64], reach: i32) -> (usize, usize) {
    let (w, h) = (W as i32, H as i32);
    let mut differ = 0;
    let mut far = 0;
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            if a[i] == b[i] {
                continue;
            }
            differ += 1;
            let mut near = false;
            'search: for dy in -reach..=reach {
                for dx in -reach..=reach {
                    let (u, v) = (x + dx, y + dy);
                    if u >= 0 && v >= 0 && u < w && v < h && a[(v * w + u) as usize] != a[i] {
                        near = true;
                        break 'search;
                    }
                }
            }
            if !near {
                far += 1;
            }
        }
    }
    (differ, far)
}

fn levels_used(c: &pocket_render::lod::SetCounts) -> usize {
    c.instances[1..].iter().filter(|&&n| n > 0).count()
}

fn camera_levels_used(c: &DrawCounts) -> usize {
    camera_levels(c)[1..].iter().filter(|&&n| n > 0).count()
}

/// The camera's instances per level over both phases of occlusion culling.
fn camera_levels(c: &DrawCounts) -> [u64; 8] {
    std::array::from_fn(|l| c.camera.instances[l] + c.late.instances[l])
}

#[test]
fn levels_cut_triangles_and_stay_within_a_pixel() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let model = demo::lod_model(DETAIL);
    let mut full = field(&gpu, &model, LodMode::Off);
    let mut lod = field(&gpu, &model, LodMode::On);
    // Occlusion culling's auto mode decides from the triangles it saves, which levels change:
    // keep it out of this comparison (the next test draws levels with it).
    full.set_occlusion(OcclusionMode::Off);
    lod.set_occlusion(OcclusionMode::Off);
    let pixels = (W * H) as usize;
    for (k, t) in [0.0f32, 0.45].into_iter().enumerate() {
        let cam = demo::lod_field_camera(N, SPACING, t);
        full.set_camera_override(Some(cam));
        lod.set_camera_override(Some(cam));
        let now = 1.0 + k as f64;
        let a = shoot(&mut full, now);
        let b = shoot(&mut lod, now);
        let (ca, cb) = (&a.counts, &b.counts);
        eprintln!(
            "t {t}: camera triangles {} full, {} with levels (instances per level {:?}); shadows {} \
             and {}",
            ca.camera_triangles(),
            cb.camera_triangles(),
            camera_levels(cb),
            ca.shadow_triangles(),
            cb.shadow_triangles(),
        );
        assert_eq!(camera_levels_used(ca), 0, "levels off draws full meshes");
        assert_eq!(
            ca.camera.instances(),
            cb.camera.instances(),
            "the same instances"
        );
        assert!(
            cb.camera_triangles() * 8 < ca.camera_triangles(),
            "levels cut the camera's triangles"
        );
        assert!(
            cb.shadow_triangles() * 8 < ca.shadow_triangles(),
            "levels cut the cascades' triangles"
        );
        assert!(camera_levels_used(cb) >= 3, "{:?}", camera_levels(cb));
        assert!(cb.cascades.iter().any(|c| levels_used(c) > 0));
        let (differ, far) = id_mismatch(&a.ids, &b.ids, 2);
        let (_, beyond_one) = id_mismatch(&a.ids, &b.ids, 1);
        let differing = a
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b.rgba.as_chunks::<4>().0)
            .filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > 32))
            .count();
        eprintln!(
            "t {t}: {differ} pixels show another entity ({:.3}%), {beyond_one} of them more than \
             1 pixel and {far} more than 2 pixels from an edge of the full meshes' id image; \
             {differing} pixels differ by more than 32 ({:.3}%)",
            differ as f64 * 100.0 / pixels as f64,
            differing as f64 * 100.0 / pixels as f64,
        );
        assert!(differ > 0, "levels change silhouettes a little");
        assert_eq!(far, 0, "silhouettes stay within two pixels");
        assert!(
            differing * 50 < pixels,
            "{differing} pixels differ by more than 32"
        );
    }
    // Forcing level 0 draws the full meshes again.
    lod.set_lod(LodMode::Off);
    let c = shoot(&mut lod, 3.0);
    full.set_camera_override(Some(demo::lod_field_camera(N, SPACING, 0.45)));
    let d = shoot(&mut full, 3.0);
    assert_eq!(camera_levels_used(&c.counts), 0);
    assert_eq!(c.counts.camera_triangles(), d.counts.camera_triangles());
    assert_eq!(c.counts.shadow_triangles(), d.counts.shadow_triangles());
}

#[test]
fn every_draw_path_and_occlusion_draw_the_same_levels() {
    let choice = BackendChoice::from_env();
    let Ok(gpu) = Gpu::headless_with(choice, Minimal::default()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let model = demo::lod_model(DETAIL);
    let cam = demo::lod_field_camera(N, SPACING, 0.2);
    // The cold first frame (with occlusion culling every instance is drawn by the late pass, at
    // the level the early pass chose) and a warm one (most by the early pass).
    let run = |gpu: &Gpu, occlusion: OcclusionMode| {
        let mut r = field(gpu, &model, LodMode::On);
        r.set_occlusion(occlusion);
        r.set_camera_override(Some(cam));
        let cold = shoot(&mut r, 1.0);
        for i in 0..3 {
            let _ = r.capture_rgba(1.0 + f64::from(i) / 60.0);
        }
        let warm = shoot(&mut r, 1.1);
        ([cold, warm], r.draw_path())
    };
    let (reference, path) = run(&gpu, OcclusionMode::Off);
    assert!(camera_levels_used(&reference[0].counts) >= 3);
    let per_batch = Gpu::headless_with(
        choice,
        Minimal {
            multi_draw: true,
            ..Minimal::default()
        },
    )
    .expect("another device");
    let baseline = Gpu::headless_with(
        choice,
        Minimal {
            first_instance: true,
            ..Minimal::default()
        },
    )
    .expect("another device");
    let others = [
        run(&gpu, OcclusionMode::On),
        run(&per_batch, OcclusionMode::Off),
        run(&baseline, OcclusionMode::Off),
        run(&baseline, OcclusionMode::On),
    ];
    for (i, (shots, other)) in others.iter().enumerate() {
        for (when, (a, b)) in ["cold", "warm"]
            .into_iter()
            .zip(reference.iter().zip(shots))
        {
            let (differ, _) = id_mismatch(&a.ids, &b.ids, 0);
            let differing = a
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .zip(b.rgba.as_chunks::<4>().0)
                .filter(|(p, q)| (0..3).any(|c| p[c].abs_diff(q[c]) > 8))
                .count();
            let (la, lb) = (camera_levels(&a.counts), camera_levels(&b.counts));
            eprintln!(
                "{when}, {path} vs {other} ({i}): {differ} pixels of another entity, {differing} \
             pixels differ; levels {la:?} and {lb:?}, late {:?}",
                b.counts.late.instances
            );
            // Occlusion culling leaves out what is hidden, never a level for another.
            if i == 0 || i == 3 {
                assert!((0..8).all(|l| lb[l] <= la[l]) && lb.iter().sum::<u64>() > 0);
            } else {
                assert_eq!(
                    la, lb,
                    "{when}, {other} ({i}): the camera draws the same levels"
                );
            }
            assert_eq!(differ, 0, "{when}, {other} ({i})");
            assert!(differing * 2000 < (W * H) as usize, "{when}, {other} ({i})");
        }
    }
}

/// One rock at the origin scaled by `scale`, nothing else.
fn rock_alone(scale: f32) -> RenderFrame {
    let mut f = demo::lod_field(1, SPACING);
    f.instances = vec![InstanceUpdate {
        id: 7,
        pose: Some(Pose {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [scale; 3],
        }),
        look: Some(Look {
            mesh: format!("{}#rock_a", demo::LOD_MODEL),
            material: String::new(),
            color: [1.0; 4],
            metallic: 0.0,
            roughness: 0.8,
            transmission: None,
            ior: None,
            emissive: [0.0; 3],
            cast_shadows: false,
            visible: true,
        }),
        anim: None,
    }];
    f
}

#[test]
fn hysteresis_holds_a_level_until_the_margin() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let model = demo::lod_model(DETAIL);
    let rock = &model.meshes[0];
    let (e1, e2) = (rock.lods[0].error, rock.lods[1].error);
    assert!(e2 > e1 * 1.6, "the chain's errors grow ({e1}, {e2})");
    // With occlusion culling on, the late pass rewrites the state word that keeps the level; a
    // scaled instance's errors scale with it.
    for (occlusion, scale) in [
        (OcclusionMode::Off, 1.0f32),
        (OcclusionMode::On, 1.0),
        (OcclusionMode::Off, 2.0),
    ] {
        let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
        r.set_occlusion(occlusion);
        r.add_model(demo::LOD_MODEL, &model);
        r.apply(rock_alone(scale), 0.0);
        let settings = r.lod();
        // The culling pass measures from the bounding sphere's nearest point; level 1 is
        // acceptable from distance e1 / w there, and taken (coarsening) from (1 + hysteresis) e1 /
        // w.
        let probe = CameraState::look_at(glam::Vec3::Z, glam::Vec3::ZERO);
        let w = settings.camera_terms(&probe, H).0[3];
        let centre = glam::Vec3::from(rock.bounds.center) * scale;
        let at = |d: f32| {
            let eye = centre + glam::Vec3::new(0.0, 0.0, d + rock.bounds.radius * scale);
            CameraState::look_at(eye, centre)
        };
        let one = e1 * scale / w;
        let h = 1.0 + settings.hysteresis;
        let mut now = 1.0;
        let mut level_at = |d: f32| {
            r.set_camera_override(Some(at(d)));
            now += 1.0 / 60.0;
            // Two frames: the second reads the level the first stored.
            let _ = r.capture_rgba(now);
            let _ = r.capture_rgba(now);
            let c = camera_levels(&r.draw_counts());
            assert_eq!(c.iter().sum::<u64>(), 1, "{c:?}");
            c.iter().position(|&n| n == 1).unwrap_or(99)
        };
        // Past the margin and short of where level 2 could be taken.
        let (lo, hi) = (h * one, e2 * scale / w / h);
        assert!(lo < hi, "level 2 starts past level 1's margin");
        let steps = [
            (0.9 * one, 0, "short of level 1's distance"),
            (0.5 * (1.0 + h) * one, 0, "past it, within the margin: kept"),
            (0.5 * (lo + hi), 1, "past the margin"),
            (0.5 * (1.0 + h) * one, 1, "back within the margin: kept"),
            (0.9 * one, 0, "short of it again: finer at once"),
        ];
        for (d, want, what) in steps {
            let got = level_at(d);
            eprintln!(
                "occlusion {}, scale {scale}: at {d:.2} m ({:.3} of level 1's distance): level \
                 {got}",
                occlusion.name(),
                d / one
            );
            assert_eq!(
                got,
                want,
                "occlusion {}, scale {scale}: {what}",
                occlusion.name()
            );
        }
    }
}
