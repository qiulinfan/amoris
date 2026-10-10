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
//! 4. Shadow cascades: one rock seen from distances that put it in every cascade, at scales 1 and
//!    2 and with a bound of one and two texels: each cascade draws the coarsest level whose scaled
//!    error is within its bound, computed on the CPU from the cascades' texels.
//! 5. Skinned meshes: a column bent by its clip, far enough away to draw level 2 or coarser, puts
//!    its silhouette where its full mesh does (its levels are skinned, not drawn from the bind
//!    pose); on the baseline path each skinned copy's levels are draw calls of their own.
//! 6. The binding limit: on WebGPU's default limits, a scene whose lists would pass 128 MiB with
//!    levels draws exactly as it does with levels off, and gets its levels back when it shrinks.

use pocket_assets::frame::{InstanceUpdate, Look, Pose, RenderFrame};
use pocket_render::gpu::Minimal;
use pocket_render::lod::DrawCounts;
use pocket_render::{
    BackendChoice, CameraState, Gpu, LodMode, LodSettings, OcclusionMode, Renderer, demo,
};

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

/// Nearest axial depth giving `projected` effective metres when the largest transverse
/// distance is `side`: invert d^2 / sqrt(d^2 + side^2) = projected.
fn axial_distance(projected: f32, side: f32) -> f32 {
    let square = projected * projected;
    ((square + (square * square + 4.0 * square * side * side).sqrt()) * 0.5).sqrt()
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

/// One rock at the origin scaled by `scale`, casting shadows or not, nothing else.
fn rock_alone(scale: f32, cast_shadows: bool) -> RenderFrame {
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
            cast_shadows,
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
        r.apply(rock_alone(scale, false), 0.0);
        let settings = r.lod();
        // The culling pass bounds the sphere's perspective projection; level 1 is
        // acceptable from distance e1 / w there, and taken (coarsening) from (1 + hysteresis) e1 /
        // w.
        let probe = CameraState::look_at(glam::Vec3::Z, glam::Vec3::ZERO);
        let w = settings.camera_terms(&probe, H).0[3];
        let centre = glam::Vec3::from(rock.bounds.center) * scale;
        let at = |projected_distance: f32| {
            // Invert d / sqrt(1 + (r / d)^2) for an on-axis sphere. These probes
            // straddle the same projected error thresholds and hysteresis margins.
            let radius = rock.bounds.radius * scale;
            let d = axial_distance(projected_distance, radius);
            let eye = centre + glam::Vec3::new(0.0, 0.0, d + radius);
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

/// The coarsest level (0: the full mesh; `k`: `errors[k - 1]`) whose error, scaled by `scale`, is
/// at most `bound`: cull.wgsl's `coarsest` on the CPU.
fn coarsest(errors: &[f32], scale: f32, bound: f32) -> usize {
    errors
        .iter()
        .rposition(|&e| e * scale <= bound)
        .map_or(0, |k| k + 1)
}

#[test]
fn cascades_pick_levels_by_their_texels() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let model = demo::lod_model(DETAIL);
    let errors: Vec<f32> = model.meshes[0].lods.iter().map(|l| l.error).collect();
    let mut seen = [0usize; 4];
    let (mut unlike_camera, mut unlike_four) = (0, 0);
    // A scaled instance's errors scale with it; the texel bound's factor is `shadow_texels`.
    for (scale, texels) in [(1.0f32, 1.0f32), (2.0, 1.0), (1.0, 2.0)] {
        let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
        r.set_occlusion(OcclusionMode::Off);
        r.set_lod_settings(LodSettings {
            shadow_texels: texels,
            ..LodSettings::default()
        });
        r.add_model(demo::LOD_MODEL, &model);
        r.apply(rock_alone(scale, true), 0.0);
        let mut now = 1.0;
        // The rock from inside the nearest cascade's slice to inside the farthest's (the splits
        // fall near 8, 18, 42 and 150 m); a cascade's box reaches past its slice, so the rock is
        // often in two.
        for d in [5.0f32, 10.0, 16.0, 30.0, 70.0, 130.0] {
            let cam = CameraState::look_at(glam::Vec3::new(0.0, 1.0, d), glam::Vec3::ZERO);
            r.set_camera_override(Some(cam));
            now += 1.0 / 60.0;
            let _ = r.capture_rgba(now);
            let counts = r.draw_counts();
            let camera = camera_levels(&counts).iter().position(|&n| n == 1);
            let texel = r.cascade_texels(&cam);
            for (i, c) in counts.cascades.iter().enumerate() {
                match c.instances() {
                    0 => continue,
                    1 => {}
                    n => panic!("cascade {i} drew the rock {n} times"),
                }
                let got = c.instances.iter().position(|&n| n == 1).unwrap_or(99);
                let bound = texel[i] * texels;
                // Within rounding of a level's error, either side is right.
                if errors
                    .iter()
                    .any(|&e| (e * scale - bound).abs() <= bound * 1e-3)
                {
                    eprintln!("at {d} m, cascade {i}: bound {bound} on a level's error, skipped");
                    continue;
                }
                let want = coarsest(&errors, scale, bound);
                eprintln!(
                    "scale {scale}, {texels} texels, at {d} m: cascade {i} (texel {:.4} m) draws \
                     level {got}, the camera {camera:?}",
                    texel[i]
                );
                assert_eq!(
                    got, want,
                    "scale {scale}, {texels} texels, at {d} m: cascade {i}'s level (bound {bound})"
                );
                seen[i] += 1;
                unlike_camera += usize::from(camera != Some(want));
                unlike_four += usize::from(coarsest(&errors, scale, 4.0 * bound) != want);
            }
        }
    }
    // The placements must tell a cascade's own level from the camera's and from a bound four
    // times too loose, in most cascades.
    eprintln!(
        "checks per cascade {seen:?}; {unlike_camera} unlike the camera's level, {unlike_four} \
         unlike a bound four times as loose"
    );
    assert!(seen.iter().filter(|&&n| n > 0).count() >= 3, "{seen:?}");
    assert!(unlike_camera > 0 && unlike_four > 0);
}

/// One frame of `n` skinned columns (`demo::bent_columns`) under the LOD field's sun.
fn columns(n: u32) -> RenderFrame {
    let mut f = demo::lod_field(1, SPACING);
    f.instances = demo::bent_columns(n, glam::Vec3::ZERO, 3.0, 100);
    f
}

#[test]
fn skinned_levels_follow_the_pose() {
    let choice = BackendChoice::from_env();
    let Ok(gpu) = Gpu::headless(choice) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let model = demo::bent_model();
    let column = &model.meshes[0];
    assert!(column.lods.len() >= 3, "{} levels", column.lods.len());
    // Far enough that the copy's level 2 is taken on the first frame: the culling pass measures
    // the perspective projection of the copy's grown sphere and coarsens past the hysteresis margin.
    let settings = LodSettings::default();
    let probe = CameraState::look_at(glam::Vec3::Z, glam::Vec3::ZERO);
    let w = settings.camera_terms(&probe, H).0[3];
    let centre = glam::Vec3::from(column.bounds.center);
    let radius = column.bounds.radius * pocket_render::meshes::SKINNED_GROW;
    let projected_distance = 1.25 * column.lods[1].error * (1.0 + settings.hysteresis) / w;
    // Include the camera's sideways offset in the projection bound, as cull.wgsl does.
    let d = axial_distance(projected_distance, radius + 1.0) + radius;
    let cam = CameraState::look_at(
        centre + glam::Vec3::new(-0.8, 0.0, d),
        glam::Vec3::new(-0.8, 1.6, 0.0),
    );
    let make = |gpu: &Gpu, mode: LodMode, n: u32| {
        let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
        r.set_lod(mode);
        r.set_occlusion(OcclusionMode::Off);
        r.add_model(demo::BENT_MODEL, &model);
        r.apply(columns(n), 0.0);
        r.set_camera_override(Some(cam));
        r
    };
    let mut full = make(&gpu, LodMode::Off, 1);
    let mut lod = make(&gpu, LodMode::On, 1);
    let a = shoot(&mut full, 1.0);
    let b = shoot(&mut lod, 1.0);
    let (la, lb) = (camera_levels(&a.counts), camera_levels(&b.counts));
    let covered = a.ids.iter().filter(|&&id| id != 0).count();
    let (differ, far) = id_mismatch(&a.ids, &b.ids, 2);
    eprintln!(
        "column at {d:.1} m ({covered} pixels): levels {la:?} off, {lb:?} on; {differ} pixels \
         show another entity, {far} more than 2 pixels from an edge"
    );
    assert_eq!(la[0], 1, "levels off draws the full mesh");
    assert_eq!(
        lb[2..].iter().sum::<u64>(),
        1,
        "the copy draws level 2 or coarser"
    );
    assert!(covered > 2000, "the column fills {covered} pixels");
    // A level skinned like the full mesh puts the bent column where the full mesh does; one drawn
    // from the bind pose's vertices stands straight up, metres away.
    assert_eq!(
        far, 0,
        "the skinned level's silhouette stays within two pixels"
    );

    // What a skinned entity costs on the per-batch paths: its copy's levels are batches of their
    // own, each live with its one instance, in every view (docs/bench/lod.md 6).
    let Ok(baseline) = Gpu::headless_with(
        choice,
        Minimal {
            first_instance: true,
            ..Minimal::default()
        },
    ) else {
        return;
    };
    let levels = 1 + column.lods.len() as u32;
    for n in [1u32, 16] {
        let mut calls = [0u32; 2];
        let mut path = "";
        for (k, mode) in [LodMode::Off, LodMode::On].into_iter().enumerate() {
            let mut r = make(&baseline, mode, n);
            let _ = r.capture_rgba(1.0);
            calls[k] = r.last.draw_calls;
            path = r.draw_path();
        }
        eprintln!(
            "{path} path, {n} skinned columns of {levels} levels: {} draw calls with levels off, \
             {} on",
            calls[0], calls[1]
        );
        // The camera and the four cascades each draw every level of every copy.
        assert_eq!(calls[1] - calls[0], n * (levels - 1) * 5);
    }
}

/// `shown` spheres in front of a camera at the origin and `hidden` far behind it, out of every
/// view, under a sun straight above: the hidden ones fill the views' lists and draw nothing.
fn spheres(shown: u32, hidden: u32) -> RenderFrame {
    let mut f = demo::lod_field(1, SPACING);
    if let Some(lights) = &mut f.lights {
        for l in lights {
            l.direction = [0.0, -1.0, 0.0];
        }
    }
    let look = Look {
        mesh: "sphere".into(),
        material: String::new(),
        color: [0.8, 0.7, 0.6, 1.0],
        metallic: 0.0,
        roughness: 0.6,
        transmission: None,
        ior: None,
        emissive: [0.0; 3],
        cast_shadows: true,
        visible: true,
    };
    let side = (f64::from(hidden).sqrt().ceil() as u32).max(1);
    f.instances = (0..shown + hidden)
        .map(|i| {
            let position = if i < shown {
                // Along the view from 3 to 60 m, spread across it.
                let z = 3.0 + 57.0 * i as f32 / shown as f32;
                [(i as f32 * 0.7).sin() * z * 0.4, 0.0, -z]
            } else {
                let k = i - shown;
                [
                    (k % side) as f32 * 2.0 - side as f32,
                    0.0,
                    1000.0 + (k / side) as f32 * 2.0,
                ]
            };
            InstanceUpdate {
                id: u64::from(i) + 1,
                pose: Some(Pose {
                    position,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                }),
                look: Some(look.clone()),
                anim: None,
            }
        })
        .collect();
    f
}

#[test]
fn levels_give_way_at_the_binding_limit() {
    let choice = BackendChoice::from_env();
    // WebGPU's default limits: a storage buffer binds at most 128 MiB.
    let Ok(gpu) = Gpu::headless_with(
        choice,
        Minimal {
            limits: true,
            ..Minimal::default()
        },
    ) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let limit = gpu.device.limits().max_storage_buffer_binding_size;
    let mut sphere = pocket_assets::primitives::primitive("sphere").expect("the sphere");
    pocket_assets::lod::build(&mut sphere, &pocket_assets::lod::LodOptions::default());
    let levels = 1 + sphere.lods.len() as u64;
    assert!(levels >= 4, "the sphere has {levels} levels");
    // Just past the limit with levels (the camera's list takes 48 bytes per instance and level),
    // far within it without them.
    let shown = 64u32;
    let hidden = (limit / (48 * levels) * 21 / 20) as u32;
    let cam = CameraState::look_at(
        glam::Vec3::new(0.0, 1.5, 0.0),
        glam::Vec3::new(0.0, 0.0, -20.0),
    );
    let mut runs = Vec::new();
    for mode in [LodMode::Off, LodMode::On] {
        let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
        r.set_lod(mode);
        r.set_occlusion(OcclusionMode::Off);
        r.set_camera_override(Some(cam));
        r.apply(spheres(shown, hidden), 0.0);
        let _ = r.capture_rgba(1.0);
        runs.push((r.draw_counts(), r.last.lod, r.list_bytes(), r));
    }
    let (off, on) = (&runs[0], &runs[1]);
    eprintln!(
        "{hidden} hidden spheres of {levels} levels, limit {limit} bytes: levels {} ({} list \
         bytes), camera instances {} and {}, cascades' triangles {} and {}",
        on.1,
        on.2,
        off.0.camera.instances(),
        on.0.camera.instances(),
        off.0.shadow_triangles(),
        on.0.shadow_triangles(),
    );
    assert_eq!(on.1, "off-limit", "levels give way");
    assert!(off.0.camera.instances() > 0 && off.0.shadow_triangles() > 0);
    assert_eq!(on.0, off.0, "the scene draws as it does with levels off");
    let shown_drawn = off.0.camera.instances();
    // Once the scene shrinks, levels come back.
    let mut r = runs.pop().expect("the run with levels").3;
    let mut fewer = spheres(shown, 0);
    fewer.reset = false;
    fewer.tick = 2;
    fewer.instances.clear();
    fewer.removed = (shown + 1..=shown + hidden).map(u64::from).collect();
    r.apply(fewer, 1.5);
    let _ = r.capture_rgba(2.0);
    let counts = r.draw_counts();
    eprintln!(
        "without the hidden ones: levels {}, camera instances per level {:?}",
        r.last.lod,
        camera_levels(&counts)
    );
    assert_eq!(r.last.lod, "on");
    assert_eq!(counts.camera.instances(), shown_drawn);
    assert!(camera_levels_used(&counts) > 0);
}
