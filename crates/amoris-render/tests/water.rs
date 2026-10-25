//! Water regions in the renderer (docs/spec/water.md 9): a region's columns draw Smooth and Blocky,
//! a renderer fed only the tiles that changed draws what a fresh renderer given every tile draws,
//! and a camera under a region's surface sees the region's fog. Skips when there is no GPU.

use amoris_assets::frame::{
    BlockLook, ChunkUpdate, EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose,
    RenderFrame, VoxelUpdate, WaterLookView, WaterTileView, WaterUpdate,
};
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use glam::Vec3;

const W: u32 = 320;
const H: u32 = 180;
const REGION: u64 = 7;

// MoltenVK crashed with SIGSEGV when these multi-renderer cases opened separate devices in
// parallel. Serial complete cases pass with the same image assertions and strict GPU errors.
fn gpu() -> Option<(std::sync::MutexGuard<'static, ()>, Gpu)> {
    static CASE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let case = CASE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let gpu = Gpu::headless(BackendChoice::from_env()).ok();
    if let Some(gpu) = &gpu {
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|error| {
            panic!("water GPU validation failed: {error}");
        }));
        gpu.device.set_device_lost_callback(|reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                panic!("water device lost: {reason:?}: {message}");
            }
        });
    }
    if gpu.is_none() {
        eprintln!("no GPU: skipped");
    }
    gpu.map(|gpu| (case, gpu))
}

#[test]
fn gpu_patch_culling_keeps_exact_surface_pixels_and_counts_current_dry_and_visible_tiles() {
    let (_case, gpu) = gpu().expect("water mesh verification requires a GPU");
    let mut r = renderer(&gpu, EYE, AT);
    r.set_prepass(amoris_render::PrepassMode::Off);
    r.set_occlusion(amoris_render::OcclusionMode::Off);
    r.set_lod(amoris_render::LodMode::Off);
    r.set_water_foam(false);
    let mut rejected = 0;
    for field in ["full", "sparse", "cliff", "blocky", "dry"] {
        for view in ["full", "near", "local", "under", "away"] {
            let frame = amoris_render::demo::water::mesh_scene(128, field);
            r.set_camera_override(Some(amoris_render::demo::water::mesh_camera(128, view)));
            r.set_water_culling(false);
            r.apply(frame.clone(), 0.0);
            let a = r.capture_still(0.0).2;
            r.set_water_culling(true);
            r.apply(frame, 0.0);
            let b = r.capture_still(0.0).2;
            assert!(
                a == b,
                "{field}/{view} culling changed surface/mask/medium pixels"
            );
            let stats = r.water_mesh_stats().unwrap()[0].1;
            assert_eq!(
                stats.rewritten_patches, stats.patches,
                "new field must initialize every patch"
            );
            let steady = r.capture_still(0.0).2;
            assert!(steady == b, "steady cached field changed pixels");
            assert_eq!(
                r.water_mesh_stats().unwrap()[0].1.rewritten_patches,
                0,
                "unchanged visibility must reuse indices"
            );
            assert_eq!(stats.patches, stats.visible + stats.dry + stats.outside);
            assert!(
                stats.active_triangles <= stats.submitted_triangles
                    && stats.submitted_triangles <= stats.input_triangles
            );
            rejected += stats.dry + stats.outside;
            if field == "dry" || view == "away" {
                assert_eq!(stats.active_triangles, 0, "{field}/{view}: {stats:?}");
            }
            if field == "full" && view == "full" {
                assert_eq!(stats.active_triangles, stats.input_triangles);
            }
        }
    }
    assert!(rejected > 0, "culling comparison is insensitive");
    // The shared collapsed vertex can belong to a tile that has not arrived yet.
    let mut partial = amoris_render::demo::water::mesh_scene(128, "full");
    partial.water[0].tiles.retain(|t| t.key != [0, 0]);
    r.set_camera_override(Some(amoris_render::demo::water::mesh_camera(128, "full")));
    r.set_water_culling(false);
    r.apply(partial.clone(), 0.0);
    let a = r.capture_still(0.0).2;
    r.set_water_culling(true);
    r.apply(partial, 0.0);
    let b = r.capture_still(0.0).2;
    assert!(a == b, "missing origin tile changed masked surface pixels");
    // Reuse one row-major spike across odd dimensions, complete dry/wet transitions and movement.
    for (nx, nz) in [(37, 51), (17, 19), (72, 39)] {
        for wet in [true, false, true] {
            let mut f = scene(0, vec![], false);
            f.water.clear();
            r.apply(f, 0.0);
            let columns = (0..nx * nz)
                .map(|n| {
                    [
                        1.0 + (n % nx) as f32 * 0.01,
                        if wet { 1.0 } else { 0.0 },
                        1.0,
                        0.0,
                    ]
                })
                .collect();
            r.set_shallow_water(Some(amoris_render::shallow_water::ShallowWaterView {
                origin: [-4.0, -3.0],
                cell_m: 0.5,
                nx,
                nz,
                columns,
            }));
            r.set_camera_override(Some(CameraState::look_at(
                Vec3::new(3.0, 1.000005, 3.0),
                Vec3::new(8.0, 1.0, 8.0),
            )));
            r.set_water_culling(false);
            let a = r.capture_still(0.0).2;
            r.set_water_culling(true);
            let b = r.capture_still(0.0).2;
            assert!(a == b, "odd {nx}x{nz}, wet={wet}");
        }
    }
    r.set_shallow_water(None);
    for (aa, scale) in [
        (Antialiasing::Off, 1.0),
        (Antialiasing::Taa, 1.0),
        (Antialiasing::MsaaTaa, 0.5),
    ] {
        r.set_antialiasing(aa);
        r.set_render_scale(scale);
        r.set_camera_override(Some(amoris_render::demo::water::mesh_camera(128, "local")));
        let frame = amoris_render::demo::water::mesh_scene(128, "sparse");
        r.set_water_culling(false);
        r.apply(frame.clone(), 0.0);
        let a = r.capture_still(0.0).2;
        r.apply(frame.clone(), 0.0);
        let again = r.capture_still(0.0).2;
        assert!(
            a == again,
            "unculled repeat differs for {aa:?}/{scale}: {} channels",
            a.iter().zip(&again).filter(|(a, b)| a != b).count()
        );
        r.set_water_culling(true);
        r.apply(frame, 0.0);
        let b = r.capture_still(0.0).2;
        if a != b {
            dump("mesh-off", &a);
            dump("mesh-on", &b);
        }
        assert!(a == b, "jitter/render-scale {aa:?}/{scale}");
    }
    // Move the view and edit a retained tiled region without resetting its mask cache.
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_render_scale(1.0);
    let field = amoris_render::demo::water::mesh_scene(128, "sparse");
    r.apply(field, 0.0);
    let mut transitions = 0;
    for view in ["full", "local", "away", "under", "full"] {
        r.set_camera_override(Some(amoris_render::demo::water::mesh_camera(128, view)));
        r.set_water_culling(false);
        let a = r.capture_still(0.0).2;
        r.set_water_culling(true);
        let b = r.capture_still(0.0).2;
        assert!(a == b, "retained view change {view}");
        transitions += r.water_mesh_stats().unwrap()[0].1.rewritten_patches;
    }
    assert!(
        transitions > 64,
        "view-change comparison must rewrite initialized patches"
    );
    for case in ["dry", "full"] {
        let mut edit = amoris_render::demo::water::mesh_scene(128, case);
        edit.reset = false;
        r.apply(edit, 0.0);
        r.set_water_culling(false);
        let a = r.capture_still(0.0).2;
        r.set_water_culling(true);
        let b = r.capture_still(0.0).2;
        assert!(a == b, "retained tiled edit {case}");
        let stats = r.water_mesh_stats().unwrap()[0].1;
        assert!(
            stats.rewritten_patches > 0,
            "retained wet/dry change must refresh indices"
        );
        if case == "dry" {
            assert_eq!(stats.submitted_triangles, 0);
        }
    }
}

/// A flat emissive floor, a constant-depth water layer, and optionally a half-covered emissive
/// pane above or below it. No lighting/fog/foam can disguise transmission or composition errors.
fn optical_scene(depth: f32, pane_y: Option<f32>) -> RenderFrame {
    let plane = |id, y, alpha, emissive| InstanceUpdate {
        id,
        pose: Some(Pose {
            position: [8.0, y, 8.0],
            scale: [16.0, 1.0, 16.0],
            ..Pose::default()
        }),
        look: Some(Look {
            mesh: "plane".into(),
            color: [0.0, 0.0, 0.0, alpha],
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
    };
    let mut f = scene(1, vec![], false);
    f.lights = Some(vec![]);
    let env = f.environment.as_mut().unwrap();
    env.ambient = 0.0;
    env.sky_color = [0.0; 3];
    f.instances = vec![plane(100, 0.0, 1.0, [1.0, 0.8, 0.6])];
    if let Some(y) = pane_y {
        f.instances.push(plane(101, y, 0.5, [0.1, 0.3, 0.9]));
    }
    if depth > 0.0 {
        let mut w = region(false, &ALL, column);
        for tile in &mut w.tiles {
            tile.columns.fill([depth, depth, 0.0, 0.0]);
        }
        let look = w.look.as_mut().unwrap();
        look.absorb = [0.7, 0.2, 0.1];
        look.scatter = [0.0; 3];
        look.fog = [0.0; 4]; // Isolate RGB absorption from the authored extra scattering medium.
        f.water = vec![w];
    }
    f
}

#[test]
fn transmission_obeys_rgb_optical_path_and_foreground_translucency_composes_after_water() {
    let Some((_case, gpu)) = gpu() else { return };
    let mut r = renderer(&gpu, EYE, AT);
    let mut camera =
        CameraState::look_at_up(Vec3::new(8.0, 10.0, 8.0), Vec3::new(8.0, 0.0, 8.0), Vec3::Z);
    camera.ortho_height = Some(8.0);
    r.set_camera_override(Some(camera));
    r.set_water_foam(false);
    r.set_water_optics(false, false, false);
    let mut capture = |aa, gtao, scale, depth, pane| {
        r.set_antialiasing(aa);
        r.set_gtao(gtao);
        r.set_render_scale(scale);
        r.apply(optical_scene(depth, pane), 0.0);
        for _ in 0..12 {
            let _ = r.capture_hdr(0.0);
        }
        let img = r.capture_hdr(0.0).2;
        let p = ((H / 2 * W + W / 2) * 4) as usize;
        [img[p], img[p + 1], img[p + 2]]
    };
    let assert_rgb = |got: [f32; 3], want: [f32; 3]| {
        let err = (0..3).map(|c| (got[c] - want[c]).abs()).fold(0.0, f32::max);
        assert!(err < 0.003, "got {got:?}, expected {want:?}; error {err}");
    };
    for (aa, gtao, scale) in [
        (Antialiasing::Off, Gtao::Off, 1.0),
        (Antialiasing::Msaa, Gtao::Off, 1.0),
        (
            Antialiasing::Taa,
            Gtao::On(amoris_render::AoNormals::Depth),
            1.0,
        ),
        (
            Antialiasing::MsaaTaa,
            Gtao::On(amoris_render::AoNormals::Target),
            0.5,
        ),
    ] {
        let floor = capture(aa, gtao, scale, 0.0, None);
        assert_rgb(floor, [1.0, 0.8, 0.6]);
        for depth in [0.5f32, 2.0, 4.0] {
            let water = capture(aa, gtao, scale, depth, None);
            let expected = [0.7f32, 0.2, 0.1].map(|s| (-s * depth).exp());
            let expected = std::array::from_fn(|c| floor[c] * expected[c] * (1.0 - 0.02040816));
            assert_rgb(water, expected);
            let front = capture(aa, gtao, scale, depth, Some(depth + 1.0));
            assert_rgb(
                front,
                std::array::from_fn(|c| 0.5 * [0.1, 0.3, 0.9][c] + 0.5 * water[c]),
            );
            // A submerged pane belongs to the background layer and is attenuated. The current
            // single-depth scene approximation uses the opaque floor's path for that layer.
            let under = capture(aa, gtao, scale, depth, Some(depth * 0.5));
            let expected = std::array::from_fn(|c| {
                (0.5 * [0.1, 0.3, 0.9][c] + 0.5 * floor[c])
                    * (-[0.7f32, 0.2, 0.1][c] * depth).exp()
                    * (1.0 - 0.02040816)
            });
            assert_rgb(under, expected);
        }
    }
}

#[test]
fn refraction_of_a_surface_piercing_box_stays_on_its_snell_ray() {
    let (_case, gpu) = gpu().expect("surface-piercing optics require a GPU");
    let eye = Vec3::new(12.0, 6.0, 16.0);
    let target = Vec3::new(9.0, 1.5, 8.5);
    let camera = CameraState::look_at(eye, target);
    let mut r = renderer(&gpu, eye, target);
    r.set_water_foam(false);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_gtao(Gtao::Off);
    let mut frame = optical_scene(2.0, None);
    frame.instances[0].look.as_mut().unwrap().emissive = [0.0; 3];
    let center = Vec3::new(8.0, 2.09, 8.0);
    let mut object = frame.instances[0].clone();
    object.id = 102;
    object.pose.as_mut().unwrap().position = center.to_array();
    object.pose.as_mut().unwrap().scale = [0.6; 3];
    object.look.as_mut().unwrap().mesh = "cube".into();
    object.look.as_mut().unwrap().emissive = [1.0, 0.0, 0.0];
    frame.instances.push(object);
    frame.water[0].look.as_mut().unwrap().absorb = [0.0; 3];
    r.apply(frame, 0.0);
    r.set_water_optics(false, false, false);
    let straight = r.capture_hdr(0.0).2;
    r.set_water_optics(true, false, false);
    let refracted = r.capture_hdr(0.0).2;
    let inverse = (camera.proj(W as f32 / H as f32) * camera.view()).inverse();
    let (mut transmitted, mut false_pulls) = (0, 0);
    for y in 0..H {
        for x in 0..W {
            let p = ((x + y * W) * 4) as usize;
            if refracted[p] < 0.03 {
                continue;
            }
            let ndc = Vec3::new(
                (x as f32 + 0.5) / W as f32 * 2.0 - 1.0,
                1.0 - (y as f32 + 0.5) / H as f32 * 2.0,
                1.0,
            );
            let incident = (inverse.project_point3(ndc) - eye).normalize();
            let surface = eye + incident * ((2.0 - eye.y) / incident.y);
            // The pixel centre can miss an above-water face while an MSAA sample still covers it.
            // Use the same conservative 8-cm raster expansion for this foreground classification.
            let direct_a = (center - Vec3::splat(0.38) - eye) / incident;
            let direct_b = (center + Vec3::splat(0.38) - eye) / incident;
            let direct_near = direct_a.min(direct_b).max_element().max(0.0);
            let direct_far = direct_a.max(direct_b).min_element();
            if direct_far >= direct_near && direct_near < surface.distance(eye) {
                if straight[p] > 0.99 {
                    assert!(
                        (refracted[p] - straight[p]).abs() < 0.003,
                        "above-water foreground changed"
                    );
                }
                continue;
            }
            transmitted += 1;
            let cosine = -incident.y;
            let eta = 0.75f32;
            let ray = incident * eta
                + Vec3::Y * (eta * cosine - (1.0 - eta * eta * (1.0 - cosine * cosine)).sqrt());
            let a = (center - Vec3::splat(0.38) - surface) / ray;
            let b = (center + Vec3::splat(0.38) - surface) / ray;
            let lo = a.min(b).max_element().max(0.0);
            let hi = a.max(b).min_element();
            if hi < lo {
                false_pulls += 1;
            }
        }
    }
    eprintln!("surface-piercing box: {transmitted} transmitted, {false_pulls} false pulls");
    assert!(transmitted > 4, "the submerged box portion should transmit");
    assert_eq!(false_pulls, 0, "displaced light must hit the actual box");
}

/// A plank floats 0.25 to 0.35 m over still water 1 m deep, the sun 35 degrees up along +x. Under
/// water the sun comes in refracted (water.md 53), so the plank's shadow on the floor falls where
/// the light's path leaves the water, well short of where the sun in air would put it; without
/// the water it falls there. A level floor is as bright either way.
#[test]
fn a_shadow_under_water_falls_along_the_refracted_sun() {
    let (_case, gpu) = gpu().expect("the refracted sun requires a GPU");
    let elevation = 35f32.to_radians();
    let eye = Vec3::new(7.5, 9.0, 8.2);
    let target = Vec3::new(7.5, 0.0, 8.0);
    let camera = CameraState::look_at(eye, target);
    let view_proj = camera.proj(W as f32 / H as f32) * camera.view();
    // The floor's light at x along the plank's middle, z = 8.
    let shade = |depth: f32| {
        let mut r = renderer(&gpu, eye, target);
        r.set_water_foam(false);
        r.set_water_optics(false, false, false);
        let mut frame = optical_scene(depth, None);
        let floor = frame.instances[0].look.as_mut().unwrap();
        floor.color = [0.8, 0.8, 0.8, 1.0];
        floor.emissive = [0.0; 3];
        let mut plank = frame.instances[0].clone();
        plank.id = 102;
        plank.pose = Some(Pose {
            position: [6.0, 1.3, 8.0],
            scale: [0.4, 0.1, 6.0],
            ..Pose::default()
        });
        let look = plank.look.as_mut().unwrap();
        look.mesh = "cube".into();
        look.cast_shadows = true;
        frame.instances.push(plank);
        frame.lights = Some(vec![LightView {
            id: 0,
            kind: LightKindView::Directional,
            position: [0.0; 3],
            direction: [elevation.cos(), -elevation.sin(), 0.0],
            color: [1.0; 3],
            intensity: 6.0,
            range: 0.0,
            inner_deg: 0.0,
            outer_deg: 0.0,
            shadows: true,
        }]);
        r.apply(frame, 0.0);
        for _ in 0..4 {
            let _ = r.capture_hdr(0.0);
        }
        let hdr = r.capture_hdr(0.0).2;
        move |x: f32| {
            let p = view_proj.project_point3(Vec3::new(x, 0.0, 8.0));
            let i = ((p.x * 0.5 + 0.5) * W as f32) as u32;
            let j = ((0.5 - p.y * 0.5) * H as f32) as u32;
            let at = ((i + j * W) * 4) as usize;
            hdr[at] + hdr[at + 1] + hdr[at + 2]
        }
    };
    let (dry, wet) = (shade(0.0), shade(1.0));
    // The shadow's middle: the plank's middle, 1.3 m up, along the sun in air, or 0.3 m along it
    // to the surface and then 1 m down the refracted ray.
    let in_air = 6.0 + 1.3 / elevation.tan();
    let refracted = 6.0 + 0.3 / elevation.tan() + (0.75 * elevation.cos()).asin().tan();
    let open = 4.5;
    eprintln!(
        "dry: open {:.3}, in-air shadow {:.3}, refracted {:.3}; wet: {:.3}, {:.3}, {:.3}",
        dry(open),
        dry(in_air),
        dry(refracted),
        wet(open),
        wet(in_air),
        wet(refracted)
    );
    assert!(dry(in_air) < 0.2 * dry(open) && dry(refracted) > 0.8 * dry(open));
    assert!(wet(refracted) < 0.2 * wet(open) && wet(in_air) > 0.8 * wet(open));
}

#[test]
fn air_side_reflection_does_not_sample_the_submerged_bed() {
    let (_case, gpu) = gpu().expect("reflection medium-side case requires a GPU");
    let mut r = renderer(&gpu, Vec3::new(12.0, 6.0, 16.0), Vec3::new(9.0, 1.5, 8.5));
    r.set_water_foam(false);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_gtao(Gtao::Off);
    let mut frame = optical_scene(2.0, None);
    frame.instances[0].look.as_mut().unwrap().emissive = [1.0, 0.0, 0.0];
    frame.water[0].look.as_mut().unwrap().absorb = [0.0; 3];
    r.apply(frame, 0.0);
    r.set_water_optics(false, false, false);
    let baseline = r.capture_hdr(0.0).2;
    r.set_water_optics(false, true, false);
    let reflected = r.capture_hdr(0.0).2;
    let error = baseline
        .iter()
        .zip(&reflected)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(
        error < 0.003,
        "air reflection must not return a submerged emissive bed: {error}"
    );
}

#[test]
fn scene_refraction_moves_the_floor_and_local_reflection_sees_an_emissive_object() {
    let Some((_case, gpu)) = gpu() else { return };
    let mut r = renderer(&gpu, Vec3::new(8.0, 8.0, 16.0), Vec3::new(8.0, 0.0, 8.0));
    r.set_water_foam(false);
    let mut frame = optical_scene(2.0, None);
    frame.instances[0].look.as_mut().unwrap().emissive = [0.0; 3];
    let mut object = frame.instances[0].clone();
    object.id = 102;
    object.pose.as_mut().unwrap().position = [8.0, 4.0, 8.0];
    object.pose.as_mut().unwrap().scale = [2.0; 3];
    object.look.as_mut().unwrap().mesh = "cube".into();
    object.look.as_mut().unwrap().emissive = [1.0, 0.0, 0.0];
    frame.instances.push(object);
    r.apply(frame, 0.0);
    r.set_water_optics(false, false, false);
    let no_reflection = r.capture_hdr(0.0).2;
    r.set_water_optics(false, true, false);
    let reflection = r.capture_hdr(0.0).2;
    let center = ((H / 2 * W + W / 2) * 4) as usize;
    eprintln!(
        "reflection: {:?} -> {:?}",
        &no_reflection[center..center + 3],
        &reflection[center..center + 3]
    );
    assert!(no_reflection[center] < 0.001);
    assert!(reflection[center] > 0.015 && reflection[center] < 0.05);
    assert!(reflection[center + 1] < 0.001 && reflection[center + 2] < 0.001);
    // The reflected object itself stays unchanged: SSR must not replace foreground pixels.
    let unchanged = no_reflection
        .as_chunks::<4>()
        .0
        .iter()
        .zip(reflection.as_chunks::<4>().0)
        .filter(|(a, _)| a[0] > 0.9)
        .all(|(a, b)| a == b);
    assert!(unchanged, "the actual object must keep its emission");

    r.set_camera_override(Some(CameraState::look_at(
        Vec3::new(25.0, 5.0, 24.0),
        Vec3::new(14.0, 1.4, 16.0),
    )));
    r.apply(amoris_render::demo::water::scene("pool"), 0.0);
    r.set_water_optics(false, false, false);
    let straight = r.capture_rgba(0.0).2;
    r.set_water_optics(true, false, false);
    let refracted = r.capture_rgba(0.0).2;
    assert!(
        differing(&straight, &refracted) > 0.02,
        "Snell refraction must displace floor squares"
    );
    // The sky is entirely outside water and its color must remain unchanged.
    assert_eq!(
        &straight[..(W * 20 * 4) as usize],
        &refracted[..(W * 20 * 4) as usize]
    );
}

#[test]
fn refraction_does_not_pull_a_detached_copy_of_a_submerged_sphere() {
    let Some((_case, gpu)) = gpu() else { return };
    let camera = CameraState::look_at(Vec3::new(8.0, 8.0, 16.0), Vec3::new(8.0, 0.0, 8.0));
    let mut r = renderer(&gpu, camera.position, Vec3::new(8.0, 0.0, 8.0));
    r.set_water_foam(false);
    let mut f = optical_scene(2.0, None);
    f.instances[0].look.as_mut().unwrap().emissive = [0.0; 3];
    let mut sphere = f.instances[0].clone();
    sphere.id = 102;
    let center = Vec3::new(8.0, 0.75, 8.0);
    sphere.pose.as_mut().unwrap().position = center.to_array();
    sphere.pose.as_mut().unwrap().scale = [1.8; 3];
    sphere.look.as_mut().unwrap().mesh = "sphere".into();
    sphere.look.as_mut().unwrap().emissive = [1.0, 0.0, 0.0];
    f.instances.push(sphere);
    r.apply(f, 0.0);
    r.set_water_optics(false, false, false);
    let straight = r.capture_hdr(0.0).2;
    r.set_water_optics(true, false, false);
    let refracted = r.capture_hdr(0.0).2;
    let inverse = (camera.proj(W as f32 / H as f32) * camera.view()).inverse();
    let (mut coloured, mut displaced, mut false_pulls) = (0, 0, 0);
    for y in 0..H {
        for x in 0..W {
            let at = ((y * W + x) * 4) as usize;
            if refracted[at] < 0.03 {
                continue;
            }
            coloured += 1;
            if straight[at] >= 0.03 {
                continue;
            }
            displaced += 1;
            let ndc = Vec3::new(
                (x as f32 + 0.5) / W as f32 * 2.0 - 1.0,
                1.0 - (y as f32 + 0.5) / H as f32 * 2.0,
                1.0,
            );
            let incident = (inverse.project_point3(ndc) - camera.position).normalize();
            let surface = camera.position + incident * ((2.0 - camera.position.y) / incident.y);
            let cos = -incident.y;
            let eta = 0.75f32;
            let ray = incident * eta
                + Vec3::Y * (eta * cos - (1.0 - eta * eta * (1.0 - cos * cos)).sqrt());
            let offset = surface - center;
            let b = offset.dot(ray);
            // A conservative 8 cm expansion covers MSAA raster samples and the faceted sphere.
            let discriminant = b * b - (offset.length_squared() - 0.98f32.powi(2));
            if discriminant < 0.0 || -b < 0.0 {
                false_pulls += 1;
            }
        }
    }
    eprintln!(
        "submerged sphere: {coloured} coloured, {displaced} displaced, {false_pulls} false pulls"
    );
    assert!(
        coloured > 100 && displaced > 10,
        "the sphere must be visible through displaced water"
    );
    assert!(
        false_pulls <= 2,
        "depth-discontinuous refraction pulled a detached object: {false_pulls}"
    );
}

#[test]
fn foreground_refraction_disocclusion_does_not_cast_an_absorption_silhouette() {
    let (_case, gpu) = gpu().expect("disoccluded optics require hardware GPU");
    let eye = Vec3::new(12.0, 5.0, 13.0);
    let target = Vec3::new(8.0, 0.8, 8.0);
    let camera = CameraState::look_at(eye, target);
    for (aa, label, pad) in [
        (Antialiasing::Off, "single", 0.025),
        (Antialiasing::Msaa, "msaa", 0.08),
    ] {
        let mut r = renderer(&gpu, eye, target);
        r.set_water_foam(false);
        r.set_water_optics(true, false, false);
        r.set_antialiasing(aa);
        r.set_gtao(Gtao::Off);
        r.set_prepass(amoris_render::PrepassMode::Off);
        r.set_occlusion(amoris_render::OcclusionMode::Off);
        r.set_lod(amoris_render::LodMode::Off);
        let frame = optical_scene(1.0, None);
        r.apply(frame.clone(), 0.0);
        let reference = r.capture_hdr(0.0).2;
        let center = Vec3::new(8.0, 0.95, 8.0);
        let half = Vec3::new(1.0, 0.25, 1.0);
        let mut with_box = frame;
        let mut object = with_box.instances[0].clone();
        object.id = 102;
        object.pose.as_mut().unwrap().position = center.to_array();
        object.pose.as_mut().unwrap().scale = (half * 2.0).to_array();
        let look = object.look.as_mut().unwrap();
        look.mesh = "cube".into();
        look.emissive = [0.0; 3];
        look.color = [0.0, 0.0, 0.0, 1.0];
        look.cast_shadows = false;
        with_box.instances.push(object);
        r.apply(with_box, 0.0);
        let actual = r.capture_hdr(0.0).2;
        let inverse = (camera.proj(W as f32 / H as f32) * camera.view()).inverse();
        let mut false_dark = 0;
        let mut real_dark = 0;
        let mut max_error = 0.0f32;
        let intersects = |origin: Vec3, ray: Vec3, pad: f32| {
            let a = (center - half - Vec3::splat(pad) - origin) / ray;
            let b = (center + half + Vec3::splat(pad) - origin) / ray;
            let near = a.min(b).max_element().max(0.0);
            let far = a.max(b).min_element();
            (far >= near).then_some(near)
        };
        for y in 0..H {
            for x in 0..W {
                let i = ((x + y * W) * 4) as usize;
                let ndc = Vec3::new(
                    (x as f32 + 0.5) / W as f32 * 2.0 - 1.0,
                    1.0 - (y as f32 + 0.5) / H as f32 * 2.0,
                    1.0,
                );
                let incident = (inverse.project_point3(ndc) - eye).normalize();
                let t = (1.0 - eye.y) / incident.y;
                if t <= 0.0 {
                    continue;
                }
                let surface = eye + incident * t;
                if surface.x < 0.2 || surface.x > 15.8 || surface.z < 0.2 || surface.z > 15.8 {
                    continue;
                }
                if intersects(eye, incident, pad).is_some_and(|d| d < t) {
                    continue;
                }
                let eta = 0.75f32;
                let cosine = -incident.y;
                let ray = incident * eta
                    + Vec3::Y * (eta * cosine - (1.0 - eta * eta * (1.0 - cosine * cosine)).sqrt());
                if intersects(surface, ray, pad).is_some() {
                    if actual[i] < reference[i] - 0.02 {
                        real_dark += 1;
                    }
                    continue;
                }
                let error = (actual[i] - reference[i]).abs();
                max_error = max_error.max(error);
                if reference[i] > 0.05 && actual[i] < reference[i] - 0.01 {
                    if false_dark < 6 {
                        eprintln!(
                            "false dark pixel {x}/{y}: actual={} reference={} surface={surface:?} ray={ray:?}",
                            actual[i], reference[i]
                        );
                    }
                    false_dark += 1;
                }
            }
        }
        eprintln!(
            "disoccluded emissive floor {label}: {false_dark} false dark pixels, {real_dark} true body pixels, max_error={max_error}"
        );
        assert!(
            real_dark > 8,
            "the submerged body must still occlude refracted floor light"
        );
        assert_eq!(
            false_dark, 0,
            "an above-water depth occluder must not change absorption behind its silhouette"
        );
        assert!(
            max_error < 0.003,
            "body changed light on rays that do not hit it: {max_error}"
        );
    }
}

fn renderer(gpu: &Gpu, eye: Vec3, at: Vec3) -> Renderer {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_gtao(Gtao::Off);
    r.set_camera_override(Some(CameraState::look_at(eye, at)));
    r
}

/// Ground of 1 m blocks, its top at 2 m, with a pit 8 x 8 blocks whose floor's top is at 1 m.
fn ground() -> VoxelUpdate {
    let mut cells = vec![0u8; 4096];
    for x in 0..16 {
        for z in 0..16 {
            let top = if (4..12).contains(&x) && (4..12).contains(&z) {
                1
            } else {
                2
            };
            for y in 0..top {
                cells[x + 16 * (z + 16 * y)] = 1;
            }
        }
    }
    VoxelUpdate {
        id: 1,
        origin: [0.0; 3],
        cell_m: 1.0,
        blocks: Some(vec![
            BlockLook::default(),
            BlockLook {
                shape: 0,
                faces: [String::new(), String::new(), String::new()],
                fog: None,
                splash: None,
            },
        ]),
        chunks: vec![ChunkUpdate {
            key: [0, 0, 0],
            cells: Some(cells),
            states: None,
        }],
    }
}

/// A region of 0.5 m columns over the ground (32 x 32, two columns a block): the pit holds water
/// in two levels, 1.9 m over its first half and 1.3 m over its second, running toward +x.
fn column(i: u32, k: u32) -> [f32; 4] {
    let (bx, bz) = (i / 2, k / 2);
    let pit = (4..12).contains(&bx) && (4..12).contains(&bz);
    if !pit {
        return [2.0, 0.0, 0.0, 0.0];
    }
    let surface = if bx < 8 { 1.9 } else { 1.3 };
    [surface, surface - 1.0, 0.8, 0.0]
}

fn tile(key: [u32; 2], f: impl Fn(u32, u32) -> [f32; 4]) -> WaterTileView {
    WaterTileView {
        key,
        columns: (0..256)
            .map(|c| f(key[0] * 16 + c % 16, key[1] * 16 + c / 16))
            .collect(),
    }
}

fn region(blocky: bool, keys: &[[u32; 2]], f: impl Fn(u32, u32) -> [f32; 4]) -> WaterUpdate {
    WaterUpdate {
        id: REGION,
        origin: [0.0, 0.0],
        cell_m: 0.5,
        size: [32, 32],
        look: Some(WaterLookView {
            fog: [0.02, 0.1, 0.45, 0.3],
            absorb: [3.0, 1.2, 0.6],
            scatter: [0.02; 3],
            blocky,
            per: 2,
            surround_m: 0.0,
        }),
        tiles: keys.iter().map(|k| tile(*k, &f)).collect(),
    }
}

const ALL: [[u32; 2]; 4] = [[0, 0], [1, 0], [0, 1], [1, 1]];

fn scene(tick: u64, water: Vec<WaterUpdate>, with_ground: bool) -> RenderFrame {
    let mut f = RenderFrame {
        tick,
        t_s: tick as f64 / 60.0,
        dt_s: 1.0 / 60.0,
        reset: tick == 1,
        water,
        voxels: if with_ground { vec![ground()] } else { vec![] },
        ..RenderFrame::default()
    };
    if f.reset {
        f.lights = Some(vec![LightView {
            id: 0,
            kind: LightKindView::Directional,
            position: [0.0; 3],
            direction: Vec3::new(-0.4, -1.0, -0.5).normalize().to_array(),
            color: [1.0, 0.96, 0.9],
            intensity: 6.0,
            range: 0.0,
            inner_deg: 0.0,
            outer_deg: 0.0,
            shadows: true,
        }]);
        f.cameras = Some(vec![]);
        f.environment = Some(EnvironmentView {
            sky: 1,
            sky_color: [0.3, 0.5, 0.8],
            ambient: 1.0,
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.0,
            fog_color: [0.6, 0.7, 0.8],
            exposure_ev: 0.0,
            bloom: 0.0,
        });
    }
    f
}

fn shoot(r: &mut Renderer) -> Vec<u8> {
    for _ in 0..4 {
        let _ = r.capture_rgba(0.0);
    }
    r.capture_rgba(0.0).2
}

fn differing(a: &[u8], b: &[u8]) -> f64 {
    let n = a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > 8))
        .count();
    n as f64 / (a.len() / 4) as f64
}

fn dump(name: &str, img: &[u8]) {
    if let Some(dir) = std::env::var_os("WATER_DUMP") {
        let path = std::path::PathBuf::from(dir).join(format!("{name}.rgba"));
        std::fs::write(path, img).unwrap();
    }
}

const EYE: Vec3 = Vec3::new(11.0, 5.0, 13.0);
const AT: Vec3 = Vec3::new(8.0, 1.5, 8.0);

#[test]
fn a_region_draws_smooth_and_blocky() {
    let Some((_case, gpu)) = gpu() else { return };
    let mut dry = renderer(&gpu, EYE, AT);
    dry.apply(scene(1, vec![], true), 0.0);
    let ground = shoot(&mut dry);
    let mut smooth = renderer(&gpu, EYE, AT);
    smooth.apply(scene(1, vec![region(false, &ALL, column)], true), 0.0);
    let s = shoot(&mut smooth);
    let mut blocky = renderer(&gpu, EYE, AT);
    blocky.apply(scene(1, vec![region(true, &ALL, column)], true), 0.0);
    let b = shoot(&mut blocky);
    dump("ground", &ground);
    dump("smooth", &s);
    dump("blocky", &b);
    let (ds, db, sb) = (
        differing(&ground, &s),
        differing(&ground, &b),
        differing(&s, &b),
    );
    eprintln!(
        "water over the ground: smooth {ds:.4}, blocky {db:.4}; smooth against blocky {sb:.4}"
    );
    assert!(ds > 0.05, "the smooth water shows: {ds}");
    assert!(db > 0.05, "the blocky water shows: {db}");
    // The step between the two levels is a slope in one and a wall in the other.
    assert!(sb > 0.002, "the styles differ at the step: {sb}");
}

#[test]
fn tiles_fed_one_by_one_draw_as_a_fresh_renderer_draws_them() {
    let Some((_case, gpu)) = gpu() else { return };
    // Every tile dry at first, then the tiles filled one frame at a time.
    fn empty(i: u32, k: u32) -> [f32; 4] {
        let c = column(i, k);
        [c[0] - c[1], 0.0, 0.0, 0.0]
    }
    for blocky in [false, true] {
        let mut kept = renderer(&gpu, EYE, AT);
        // This checks column streaming, independently of temporal foam and spray histories. Both captures
        // must also sample detail at the same simulation time now that it refracts the scene.
        kept.set_water_foam(false);
        kept.set_splashes(false);
        kept.apply(scene(1, vec![region(blocky, &ALL, empty)], true), 0.0);
        let _ = shoot(&mut kept);
        for (n, key) in ALL.iter().enumerate() {
            let mut u = region(blocky, &[*key], column);
            u.look = None;
            kept.apply(scene(2 + n as u64, vec![u], false), 0.0);
            let _ = kept.capture_rgba(0.0);
        }
        let after = shoot(&mut kept);
        let mut fresh = renderer(&gpu, EYE, AT);
        fresh.set_water_foam(false);
        fresh.set_splashes(false);
        let mut full = scene(1, vec![region(blocky, &ALL, column)], true);
        full.tick = 1 + ALL.len() as u64;
        full.t_s = full.tick as f64 / 60.0;
        fresh.apply(full, 0.0);
        let want = shoot(&mut fresh);
        let d = differing(&after, &want);
        eprintln!("blocky {blocky}: kept against fresh, {d:.5} of the pixels differ");
        assert!(d < 0.0005, "tile updates draw the final water: {d}");
    }
}

#[test]
fn under_a_regions_surface_the_view_is_its_fog() {
    let Some((_case, gpu)) = gpu() else { return };
    let mut under = renderer(&gpu, Vec3::new(6.0, 1.3, 6.0), Vec3::new(10.0, 1.3, 9.0));
    under.apply(scene(1, vec![region(false, &ALL, column)], true), 0.0);
    let u = shoot(&mut under);
    let mut above = renderer(&gpu, Vec3::new(6.0, 2.6, 6.0), Vec3::new(10.0, 2.6, 9.0));
    above.apply(scene(1, vec![region(false, &ALL, column)], true), 0.0);
    let a = shoot(&mut above);
    dump("under", &u);
    let mean = |img: &[u8]| {
        let px = img.as_chunks::<4>().0;
        let n = px.len() as f64;
        [0, 1, 2].map(|c| px.iter().map(|p| f64::from(p[c])).sum::<f64>() / n)
    };
    let (mu, ma) = (mean(&u), mean(&a));
    eprintln!("mean colour under {mu:?}, above {ma:?}");
    assert!(
        mu[2] > 1.5 * mu[0] && mu[2] > mu[1],
        "blue under the water: {mu:?}"
    );
    assert!(differing(&u, &a) > 0.5, "the fog changes the view");
}

#[test]
fn surrounding_water_meets_the_regions_edge() {
    let Some((_case, gpu)) = gpu() else { return };
    // A region full to the brim at 1.9 m but for its -x edge columns, 3 cm lower: the surround
    // stands at the edges' mean level, and the stitch joins it to each edge column's surface.
    fn full(i: u32, _k: u32) -> [f32; 4] {
        let s = if i == 0 { 1.87 } else { 1.9 };
        [s, s - 1.0, 0.0, 0.0]
    }
    // From just over the water past the -x edge, looking along the edge and inward.
    let (eye, at) = (Vec3::new(-3.0, 1.95, -2.0), Vec3::new(6.0, 1.85, 6.0));
    for blocky in [false, true] {
        let mut with = region(blocky, &ALL, full);
        with.look.as_mut().unwrap().surround_m = 200.0;
        let mut r = renderer(&gpu, eye, at);
        r.apply(scene(1, vec![with], false), 0.0);
        let surrounded = shoot(&mut r);
        let mut bare = renderer(&gpu, eye, at);
        bare.apply(scene(1, vec![region(blocky, &ALL, full)], false), 0.0);
        let alone = shoot(&mut bare);
        dump(
            if blocky {
                "surround-blocky"
            } else {
                "surround-smooth"
            },
            &surrounded,
        );
        // Below the horizon nothing but water: no pixel of the lower half is the bare sky's, as a
        // frame with no water at all draws it (water reflecting the sky is not the same pixel).
        let mut none = renderer(&gpu, eye, at);
        none.apply(scene(1, vec![], false), 0.0);
        let empty = shoot(&mut none);
        let lower = |img: &[u8]| {
            (H as usize / 2 + 8..H as usize)
                .flat_map(|y| (0..W as usize).map(move |x| 4 * (x + W as usize * y)))
                .filter(|&at| (0..3).all(|c| img[at + c].abs_diff(empty[at + c]) <= 2))
                .count()
        };
        let (gaps, open) = (lower(&surrounded), lower(&alone));
        eprintln!(
            "blocky {blocky}: bare sky below the horizon {gaps} pixels with the surround, {open} \
             without"
        );
        assert!(
            open > 1000,
            "without the surround the sky shows past the region: {open}"
        );
        assert_eq!(gaps, 0, "the surround and its stitch leave no gap");
    }
}

#[test]
fn temporal_foam_pauses_with_simulated_time_and_reset_discards_its_history() {
    let Some((_case, gpu)) = gpu() else { return };
    let mut bare = renderer(&gpu, EYE, AT);
    let mut foamy = renderer(&gpu, EYE, AT);
    bare.set_water_foam(false);
    for r in [&mut bare, &mut foamy] {
        r.apply(scene(1, vec![region(false, &ALL, column)], true), 0.0);
    }
    for tick in 2..=60 {
        let t = tick as f64 / 60.0;
        for r in [&mut bare, &mut foamy] {
            r.apply(scene(tick, vec![], false), t);
            let _ = r.capture_rgba(t);
        }
    }
    let a = bare.capture_rgba(10.0).2;
    let b = foamy.capture_rgba(10.0).2;
    assert!(
        differing(&a, &b) > 0.001,
        "the time-integrated foam must be visible"
    );
    assert_eq!(
        b,
        foamy.capture_rgba(20.0).2,
        "wall-clock time cannot advance paused foam"
    );
    // At tick 61, (61/60 - 1/60) is one f64 ulp below 1.0. Interpolation roundoff must not
    // be mistaken for a timeline rewind and erase a visible trail.
    foamy.apply(scene(61, vec![], false), 30.0);
    let rounded = foamy.capture_rgba(30.0).2;
    assert!(
        differing(&b, &rounded) < 0.0001,
        "one-ulp interpolation roundoff erased foam"
    );
    for r in [&mut bare, &mut foamy] {
        r.apply(scene(50, vec![], false), 40.0);
    }
    assert_eq!(
        bare.capture_rgba(40.0).2,
        foamy.capture_rgba(40.0).2,
        "a real time rewind must clear the trail"
    );
    // A reset at the same boundary clears presentation history independently of numerical state.
    let mut reset = scene(1, vec![region(false, &ALL, column)], true);
    reset.tick = 61;
    reset.t_s = 61.0 / 60.0;
    foamy.apply(reset.clone(), 50.0);
    bare.apply(reset, 50.0);
    let a = bare.capture_rgba(60.0).2;
    let b = foamy.capture_rgba(60.0).2;
    assert_eq!(
        a, b,
        "fresh foam history is empty until simulated time advances"
    );
}

#[test]
fn resident_programs_draw_the_current_field_after_skipped_frames_compaction_and_rewind() {
    use amoris_assets::{
        frame::ResidentWaterUpdate,
        water_field::{ColumnDelta, FieldCheckpoint, FieldRecipe, FieldStep},
    };
    use amoris_render::{water_compute::GpuWater, water_resident::ResidentWater};
    use std::sync::Arc;
    let (_case, gpu) = gpu().expect("resident rendering requires a GPU");
    let mut a = renderer(&gpu, EYE, AT);
    let mut b = renderer(&gpu, EYE, AT);
    for renderer in [&mut a, &mut b] {
        renderer.set_water_foam(false);
        renderer.set_water_optics(false, false, false);
    }
    let mut field: Vec<[f32; 4]> = (0..1024)
        .map(|n| [0.0, if n % 32 < 16 { 2.0 } else { 0.2 }, 0.0, 0.0])
        .collect();
    let mut checkpoint = Arc::new(
        FieldCheckpoint::new(
            [32, 32],
            0.0,
            field.iter().map(|c| c.map(f32::to_bits)).collect(),
        )
        .checked()
        .unwrap(),
    );
    let mut recipe = FieldRecipe::new(checkpoint.key(), vec![], checkpoint.digest(), String::new());
    let mut resident = ResidentWater::new(&gpu).unwrap();
    let mut reference = GpuWater::new(&gpu.device, &gpu.queue, [32, 32]).unwrap();
    let mut reference_reactions =
        amoris_render::water_reactions::WaterReactions::new(&gpu.device, &gpu.queue);
    let mut far_faces = vec![0u32; 64];
    let mut pending = scene(1, vec![], false);
    let look = region(false, &ALL, column).look.unwrap();
    let mut saved = Vec::new();
    let export = std::env::var_os("WATER_RADIATION_FRAMES").map(std::path::PathBuf::from);
    if let Some(path) = &export {
        std::fs::create_dir_all(path).unwrap();
    }
    for tick in 1..=70 {
        let previous_steps = recipe.steps().len();
        let step = FieldStep {
            sea: (tick >= 50).then(|| {
                let k = std::f64::consts::TAU / 128.0;
                let omega = (9.81 * k).sqrt();
                amoris_assets::water_field::FieldSea {
                    level_relative_m: 1.0,
                    waves: vec![amoris_assets::water_field::FieldSeaWave {
                        amplitude_m: 0.01,
                        wave_vector_rad_m: [k, 0.0],
                        angular_frequency_rad_s: omega,
                        phase_at_start_rad: 0.7 - omega * f64::from(tick - 1) / 60.0,
                    }],
                }
            }),
            boundaries: if tick >= 50 {
                [
                    amoris_assets::water_field::FieldBoundary::Sea,
                    amoris_assets::water_field::FieldBoundary::Sea,
                    amoris_assets::water_field::FieldBoundary::Wall,
                    amoris_assets::water_field::FieldBoundary::Wall,
                ]
            } else if tick >= 40 {
                [
                    amoris_assets::water_field::FieldBoundary::Radiation {
                        surface_relative_m: 1.4,
                        outward_velocity_mps: -0.1,
                    },
                    amoris_assets::water_field::FieldBoundary::Radiation {
                        surface_relative_m: 1.0,
                        outward_velocity_mps: 0.1,
                    },
                    amoris_assets::water_field::FieldBoundary::Wall,
                    amoris_assets::water_field::FieldBoundary::Wall,
                ]
            } else if tick >= 20 {
                [
                    amoris_assets::water_field::FieldBoundary::Inflow { m3s_per_m: 0.2 },
                    amoris_assets::water_field::FieldBoundary::Outflow,
                    amoris_assets::water_field::FieldBoundary::Wall,
                    amoris_assets::water_field::FieldBoundary::Wall,
                ]
            } else {
                [amoris_assets::water_field::FieldBoundary::Wall; 4]
            },
            cell_m: 1.0,
            substep_dt: 1.0 / 120.0,
            substeps: 1 + tick % 3,
            friction: 0.02,
            calm_per_s: amoris_assets::water_field::DEFAULT_CALM_PER_S,
            dry_below: 0.0,
            impulses: if tick % 13 == 0 {
                vec![amoris_assets::water_field::PointImpulse {
                    at: [15.5, 8.5],
                    impulse: [10.0, -4.0],
                }]
            } else {
                vec![]
            },
            displacements: vec![],
            physics: Default::default(),
            edits: if tick % 11 == 0 {
                vec![ColumnDelta {
                    column: 17,
                    depth_m: 0.25,
                }]
            } else {
                vec![]
            },
        };
        let mut input = FieldCheckpoint::new(
            [32, 32],
            0.0,
            field.iter().map(|c| c.map(f32::to_bits)).collect(),
        );
        input.far_faces.clone_from(&far_faces);
        reference.upload_checkpoint(&input).unwrap();
        if !step.impulses.is_empty() {
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            reference_reactions
                .encode(&mut encoder, &reference, step.cell_m, &step.impulses)
                .unwrap();
            gpu.queue.submit([encoder.finish()]);
            let read = reference.checkpoint_complete(0.0).unwrap();
            field = read.columns.iter().map(|c| c.map(f32::from_bits)).collect();
            far_faces = read.far_faces;
        }
        for e in &step.edits {
            field[e.column as usize][1] += e.depth_m as f32;
        }
        let mut input = FieldCheckpoint::new(
            [32, 32],
            0.0,
            field.iter().map(|c| c.map(f32::to_bits)).collect(),
        );
        input.far_faces.clone_from(&far_faces);
        reference.upload_checkpoint(&input).unwrap();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        reference
            .encode_transport(&mut encoder, &step, None)
            .unwrap();
        gpu.queue.submit([encoder.finish()]);
        let read = reference.checkpoint_complete(0.0).unwrap();
        field = read.columns.iter().map(|c| c.map(f32::from_bits)).collect();
        far_faces = read.far_faces;
        let out = resident
            .advance(&checkpoint, &recipe, &step, &[], false)
            .unwrap();
        let new_seed = if tick == 1 {
            Some(checkpoint.clone())
        } else {
            out.checkpoint.clone()
        };
        recipe = out.recipe;
        if let Some(c) = out.checkpoint {
            checkpoint = c;
        }
        let update = RenderFrame {
            tick: tick.into(),
            t_s: f64::from(tick) / 60.0,
            dt_s: 1.0 / 60.0,
            resident_water: vec![ResidentWaterUpdate {
                id: REGION,
                origin: [0.0; 2],
                cell_m: 1.0,
                look: look.clone(),
                recipe: recipe.patch(if new_seed.is_some() {
                    0
                } else {
                    previous_steps
                }),
                checkpoint: new_seed,
                edge_level: None,
            }],
            ..Default::default()
        };
        pending.merge(update).unwrap();
        if ![1, 27, 63, 64, 70].contains(&tick) {
            continue;
        }
        let published = std::mem::take(&mut pending);
        if let Some(path) = &export {
            std::fs::write(path.join(format!("tick-{tick}.bin")), published.encode()).unwrap();
        }
        a.apply(published, 0.0);
        let mut view = region(false, &ALL, |i, k| {
            let n = (i + 32 * k) as usize;
            let c = field[n];
            let u = if i + 1 < 32 {
                field[n + 1][2]
            } else {
                f32::from_bits(far_faces[k as usize])
            };
            let w = if k + 1 < 32 {
                field[n + 32][3]
            } else {
                f32::from_bits(far_faces[32 + i as usize])
            };
            [c[0] + c[1], c[1], (c[2] + u) * 0.5, (c[3] + w) * 0.5]
        });
        view.cell_m = 1.0;
        let frame = scene(1, vec![view], false);
        b.apply(frame.clone(), 0.0);
        let image = a.capture_rgba(0.0).2;
        assert!(
            image == b.capture_rgba(0.0).2,
            "resident presentation differs at {tick}"
        );
        let mut camera =
            CameraState::look_at_up(Vec3::new(8.0, 0.1, 8.0), Vec3::new(8.0, -1.0, 8.0), Vec3::Z);
        camera.ortho_height = Some(4.0);
        a.set_camera_override(Some(camera));
        b.set_camera_override(Some(camera));
        assert!(
            a.capture_rgba(0.0).2 == b.capture_rgba(0.0).2,
            "resident GPU medium differs at {tick}"
        );
        a.set_camera_override(Some(CameraState::look_at(EYE, AT)));
        b.set_camera_override(Some(CameraState::look_at(EYE, AT)));
        if tick == 27 || tick == 63 {
            saved.push((tick, checkpoint.clone(), recipe.clone(), frame, image));
        }
    }
    for (tick, checkpoint, recipe, frame, image) in saved {
        // A renderer may rewind without being recreated; a new full feed supplies the seed again.
        let mut reset = frame;
        reset.water.clear();
        reset.resident_water = vec![ResidentWaterUpdate {
            id: REGION,
            origin: [0.0; 2],
            cell_m: 1.0,
            look: look.clone(),
            checkpoint: Some(checkpoint.clone()),
            recipe: recipe.patch(0),
            edge_level: None,
        }];
        if let Some(path) = &export {
            std::fs::write(path.join(format!("reset-{tick}.bin")), reset.encode()).unwrap();
        }
        a.apply(reset, 0.0);
        assert!(
            image == a.capture_rgba(0.0).2,
            "rewound resident picture differs"
        );
    }
}

fn center_hdr(r: &mut Renderer) -> [f32; 3] {
    for _ in 0..12 {
        r.capture_hdr(0.0);
    }
    let image = r.capture_hdr(0.0).2;
    let mut c = [0.0; 3];
    for y in [H / 2 - 1, H / 2] {
        for x in [W / 2 - 1, W / 2] {
            let n = ((x + y * W) * 4) as usize;
            for k in 0..3 {
                c[k] += image[n + k] * 0.25;
            }
        }
    }
    c
}
fn close_hdr(got: [f32; 3], want: [f32; 3]) {
    let error = (0..3).map(|c| (got[c] - want[c]).abs()).fold(0.0, f32::max);
    assert!(
        error < 0.003,
        "HDR {got:?}, expected {want:?}, error {error}"
    );
}
#[test]
fn perspective_water_inside_the_near_plane_still_occludes_the_floor() {
    let (_case, gpu) = gpu().expect("near water interface requires a GPU");
    let mut r = renderer(&gpu, EYE, AT);
    r.set_water_foam(false);
    r.set_water_optics(false, false, false);
    r.set_gtao(Gtao::Off);
    let camera = CameraState::look_at(Vec3::new(8.0, 2.04, 8.0), Vec3::new(8.0, 2.04, 15.0));
    r.set_camera_override(Some(camera));
    for aa in [Antialiasing::Off, Antialiasing::Msaa] {
        r.set_antialiasing(aa);
        r.apply(optical_scene(2.0, None), 0.0);
        let image = r.capture_hdr(0.0).2;
        // This entire bottom strip sees the floor through water less than 0.1 m from the eye.
        // A vertex-only depth clamp lets the opaque floor win the depth test in this strip.
        for y in (H - 20)..(H - 2) {
            for x in (W / 2 - 10)..(W / 2 + 10) {
                let p = ((x + y * W) * 4) as usize;
                assert!(
                    image[p] < 0.15,
                    "floor leaked through near water at {x},{y}: {:?}",
                    &image[p..p + 3]
                );
                assert!(image[p + 2] > 0.1, "water must transmit the floor");
            }
        }
    }
}
/// A camera at the surface (water.md 58): its near plane is its port, and each row sees the air or
/// the water by where its point on that plane lies against the surface. Looking level, the rows
/// whose port points lie under the surface start in the water, so the waterline moves with the
/// eye's height over the surface; before the port, every row split at the horizon.
#[test]
fn a_camera_at_the_surface_splits_along_the_waterline_on_its_port() {
    let (_case, gpu) = gpu().expect("the waterline requires a GPU");
    // The near plane's half height for the test camera: 0.1 m away, 60 degrees.
    let half = 0.1 * 30f32.to_radians().tan();
    for rise in [0.012f32, 0.0, -0.012] {
        let eye = Vec3::new(8.0, 2.0 + rise, 1.0);
        let mut r = renderer(&gpu, eye, eye + Vec3::Z);
        r.set_water_foam(false);
        r.set_water_optics(false, false, false);
        // The sky stays black and the water glows with its haze, so the rows are told apart.
        let mut frame = optical_scene(2.0, None);
        frame.environment.as_mut().unwrap().ambient = 1.0;
        frame.water[0].look.as_mut().unwrap().fog = [0.1, 0.4, 0.8, 0.2];
        r.apply(frame, 0.0);
        for _ in 0..4 {
            r.capture_hdr(0.0);
        }
        let image = r.capture_hdr(0.0).2;
        let blue = |y: u32| image[((W / 2 + y * W) * 4 + 2) as usize];
        // A row's port point lies under the surface when (y + 0.5) / H > (1 + rise / half) / 2.
        let line = (H as f32 * 0.5 * (1.0 + rise / half) - 0.5).ceil() as u32;
        let first = (0..H)
            .find(|&y| blue(y) > 0.3)
            .expect("rows under the water");
        eprintln!("eye {rise:+}: first row in the water {first}, the port's waterline {line}");
        assert!(
            first.abs_diff(line) <= 2,
            "waterline at row {first}, expected {line}"
        );
        // Above the horizon the air's rows see the black sky (between the horizon and a lower
        // waterline they see the water's surface from above).
        assert!(blue(line.min(H / 2) - 8) < 0.02, "air above the waterline");
        assert!(blue(line + 8) > 0.4, "water under it");
    }
}

/// The sun under water (water.md 58): looking 15 degrees off the refracted sun through flat water,
/// the light the water scatters toward the eye along its path matches single scattering with
/// Henyey and Greenstein's phase function (flat water focuses nothing), and an opaque plank over
/// the water takes it away along its shadow.
#[test]
fn the_sun_scatters_toward_an_eye_under_water_along_its_refracted_path() {
    let (_case, gpu) = gpu().expect("light under water requires a GPU");
    let (s, c) = 60f32.to_radians().sin_cos();
    let l = Vec3::new(0.0, s, -c);
    let up = Vec3::new(
        0.75 * l.x,
        (1.0 - 0.5625 * (1.0 - l.y * l.y)).sqrt(),
        0.75 * l.z,
    );
    let (s, c) = (-15f32).to_radians().sin_cos();
    let dir = Vec3::new(0.0, up.y * c - up.z * s, up.y * s + up.z * c).normalize();
    let eye = Vec3::new(8.0, 1.0, 9.0);
    let center = |sun: bool, plank: bool| {
        let mut r = renderer(&gpu, eye, eye + dir);
        r.set_water_foam(false);
        r.set_water_optics(false, false, false);
        let mut frame = optical_scene(4.0, None);
        let look = frame.water[0].look.as_mut().unwrap();
        look.absorb = [0.04; 3];
        look.scatter = [0.1; 3];
        if sun {
            frame.lights = Some(vec![LightView {
                id: 0,
                kind: LightKindView::Directional,
                position: [0.0; 3],
                direction: (-l).to_array(),
                color: [1.0; 3],
                intensity: 6.0,
                range: 0.0,
                inner_deg: 0.0,
                outer_deg: 0.0,
                shadows: true,
            }]);
        }
        if plank {
            // Over the water where the sun's paths to this view leave it, 0.6 m up.
            let mut plank = frame.instances[0].clone();
            plank.id = 102;
            plank.pose = Some(Pose {
                position: [8.0, 4.6, 6.9],
                scale: [2.0, 0.1, 2.0],
                ..Pose::default()
            });
            let look = plank.look.as_mut().unwrap();
            look.mesh = "cube".into();
            look.emissive = [0.0; 3];
            look.cast_shadows = true;
            frame.instances.push(plank);
        }
        r.apply(frame, 0.0);
        center_hdr(&mut r)
    };
    let difference = |a: [f32; 3], b: [f32; 3]| [0, 1, 2].map(|k| a[k] - b[k]);
    let open = difference(center(true, false), center(false, false));
    let shaded = difference(center(true, true), center(false, true));
    // Single scattering along the path to the surface, 3 m up: the sun's beam widened by its
    // refraction and less its reflection, absorbed down to each point and back to the eye.
    let sigma = 0.14f64;
    let (lift, length) = (3.0f64, 3.0 / f64::from(dir.y));
    let steps = 4096;
    let path: f64 = (0..steps)
        .map(|i| {
            let t = (i as f64 + 0.5) * length / steps as f64;
            let depth = lift - f64::from(dir.y) * t;
            (-sigma * (t + depth / f64::from(up.y))).exp() * length / steps as f64
        })
        .sum();
    let g = 0.8f64;
    let d = 1.0 + g * g - 2.0 * g * f64::from(up.dot(dir));
    let phase = (1.0 - g * g) / (d * d.sqrt());
    let cosine = f64::from(l.y);
    let ct = (1.0 - 0.5625 * (1.0 - cosine * cosine)).sqrt();
    let rs = (0.75 * cosine - ct) / (0.75 * cosine + ct);
    let rp = (cosine - 0.75 * ct) / (cosine + 0.75 * ct);
    let fresnel = 0.5 * (rs * rs + rp * rp);
    let expected = 0.1 * 6.0 / (4.0 * std::f64::consts::PI)
        * (cosine / f64::from(up.y))
        * (1.0 - fresnel)
        * phase
        * path;
    eprintln!("open {open:?}, shaded {shaded:?}, expected {expected:.4}");
    for k in 0..3 {
        let got = f64::from(open[k]);
        assert!(
            (got - expected).abs() < 0.08 * expected,
            "scattered sun {got}, expected {expected}"
        );
        assert!(
            shaded[k] < 0.25 * open[k],
            "the plank's shadow keeps the sun from the path: {shaded:?} against {open:?}"
        );
    }
}

#[test]
fn underwater_rgb_paths_stop_at_the_field_bed_and_preserve_near_clipped_interfaces() {
    let (_case, gpu) = gpu().expect("underwater cases require a GPU");
    let mut r = renderer(&gpu, EYE, AT);
    r.set_water_foam(false);
    r.set_water_optics(false, false, false);
    for (aa, gtao, scale) in [
        (Antialiasing::Off, Gtao::Off, 1.0),
        (Antialiasing::Msaa, Gtao::Off, 1.0),
        (
            Antialiasing::MsaaTaa,
            Gtao::On(amoris_render::AoNormals::Target),
            0.5,
        ),
    ] {
        r.set_antialiasing(aa);
        r.set_gtao(gtao);
        r.set_render_scale(scale);
        for eye_y in [0.25f32, 1.0, 1.75, 2.0, 2.0001, 2.25] {
            let mut camera = CameraState::look_at_up(
                Vec3::new(8.0, eye_y, 8.0),
                Vec3::new(8.0, -1.0, 8.0),
                Vec3::Z,
            );
            camera.ortho_height = Some(8.0);
            r.set_camera_override(Some(camera));
            r.apply(optical_scene(2.0, None), 0.0);
            eprintln!("underwater path {aa:?}/{gtao:?}/{scale}, eye {eye_y}");
            let path = eye_y.min(2.0);
            let boundary = if eye_y > 2.0 { 1.0 - 0.02040816 } else { 1.0 };
            close_hdr(
                center_hdr(&mut r),
                std::array::from_fn(|c| {
                    [1.0, 0.8, 0.6][c] * (-[0.7, 0.2, 0.1][c] * path).exp() * boundary
                }),
            );
        }
    }
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_render_scale(1.0);
    r.set_gtao(Gtao::Off);
    // No visible bed mesh: the field's own bed still terminates attenuation.
    let mut scene = optical_scene(2.0, None);
    scene.instances[0].pose.as_mut().unwrap().position[1] = -20.0;
    let mut camera = CameraState::look_at_up(
        Vec3::new(8.0, 1.0, 8.0),
        Vec3::new(8.0, -20.0, 8.0),
        Vec3::Z,
    );
    camera.ortho_height = Some(8.0);
    r.set_camera_override(Some(camera));
    r.apply(scene.clone(), 0.0);
    close_hdr(
        center_hdr(&mut r),
        std::array::from_fn(|c| [1.0, 0.8, 0.6][c] * (-[0.7f32, 0.2, 0.1][c]).exp()),
    );
    let mut camera = CameraState::look_at_up(
        Vec3::new(8.0, -1.0, 8.0),
        Vec3::new(8.0, -20.0, 8.0),
        Vec3::Z,
    );
    camera.ortho_height = Some(8.0);
    r.set_camera_override(Some(camera));
    r.apply(scene, 0.0);
    close_hdr(center_hdr(&mut r), [1.0, 0.8, 0.6]);
}

#[test]
fn underwater_paths_end_at_region_edges_and_snell_window_uses_dielectric_fresnel() {
    let (_case, gpu) = gpu().expect("underwater cases require a GPU");
    let mut r = renderer(&gpu, EYE, AT);
    r.set_water_foam(false);
    r.set_water_optics(false, false, false);
    let mut scene = optical_scene(2.0, None);
    let pose = scene.instances[0].pose.as_mut().unwrap();
    pose.position = [20.0, 1.0, 8.0];
    pose.scale = [64.0, 1.0, 64.0];
    pose.rotation = glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2).to_array();
    let look = scene.water[0].look.as_mut().unwrap();
    look.absorb = [0.1, 0.2, 0.3];
    let mut camera =
        CameraState::look_at_up(Vec3::new(8.0, 1.0, 8.0), Vec3::new(20.0, 1.0, 8.0), Vec3::Y);
    camera.ortho_height = Some(1.0);
    r.set_camera_override(Some(camera));
    r.apply(scene.clone(), 0.0);
    // The rendered smooth field ends at the last centre (15.75), before the wall at x=20.
    close_hdr(
        center_hdr(&mut r),
        std::array::from_fn(|c| [1.0, 0.8, 0.6][c] * (-[0.1f32, 0.2, 0.3][c] * 7.75).exp()),
    );
    // A horizontal ray just above the top is outside, even within classification's roundoff
    // tolerance. The exact-prism shortcut must retain the DDA's zero-length result.
    let mut camera = CameraState::look_at_up(
        Vec3::new(8.0, 2.000001, 8.0),
        Vec3::new(20.0, 2.000001, 8.0),
        Vec3::Y,
    );
    camera.ortho_height = Some(0.001);
    r.set_camera_override(Some(camera));
    r.apply(scene, 0.0);
    for _ in 0..12 {
        r.capture_hdr(0.0);
    }
    let image = r.capture_hdr(0.0).2;
    let p = ((W / 2 + (H / 2 - 1) * W) * 4) as usize;
    close_hdr([image[p], image[p + 1], image[p + 2]], [1.0, 0.8, 0.6]);
    for degrees in [0.0f32, 30.0, 45.0, 48.0, 50.0, 55.0] {
        let angle = degrees.to_radians();
        let dir = Vec3::new(angle.sin(), angle.cos(), 0.0);
        let mut scene = optical_scene(2.0, None);
        let pose = scene.instances[0].pose.as_mut().unwrap();
        pose.position = [8.0, 3.0, 8.0];
        pose.scale = [64.0, 1.0, 64.0];
        pose.rotation = glam::Quat::from_rotation_x(std::f32::consts::PI).to_array();
        let mut camera = CameraState::look_at_up(
            Vec3::new(8.0, 1.0, 8.0),
            Vec3::new(8.0, 1.0, 8.0) + dir,
            Vec3::Z,
        );
        camera.ortho_height = Some(1.0);
        r.set_camera_override(Some(camera));
        r.apply(scene, 0.0);
        let cosine = f64::from(angle).cos();
        let eta = 4.0 / 3.0;
        let transmitted = 1.0 - eta * eta * (1.0 - cosine * cosine);
        let fresnel = if transmitted <= 0.0 {
            1.0
        } else {
            let ct = transmitted.sqrt();
            let rs = (eta * cosine - ct) / (eta * cosine + ct);
            let rp = (cosine - eta * ct) / (cosine + eta * ct);
            (rs * rs + rp * rp) * 0.5
        };
        close_hdr(
            center_hdr(&mut r),
            std::array::from_fn(|c| {
                (f64::from([1.0f32, 0.8, 0.6][c])
                    * (1.0 - fresnel)
                    * (-f64::from([0.7f32, 0.2, 0.1][c]) / cosine).exp()) as f32
            }),
        );
    }
}

#[test]
fn medium_uses_piecewise_field_beds_and_the_same_authored_haze_on_both_sides() {
    let (_case, gpu) = gpu().expect("underwater cases require a GPU");
    let mut r = renderer(&gpu, EYE, AT);
    r.set_water_foam(false);
    r.set_water_optics(false, false, false);
    let mut scene = optical_scene(2.0, None);
    let mut water = region(false, &ALL, |i, k| {
        let x = (i as f32 + 0.5) * 0.5;
        let z = (k as f32 + 0.5) * 0.5;
        let bed = 0.02 * (x + z);
        [2.0, 2.0 - bed, 0.0, 0.0]
    });
    water.look = scene.water[0].look.clone();
    scene.water = vec![water];
    let pose = scene.instances[0].pose.as_mut().unwrap();
    pose.position = [20.0, 0.1, 20.0];
    pose.scale = [64.0, 1.0, 64.0];
    pose.rotation = glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2).to_array();
    let mut camera = CameraState::look_at_up(
        Vec3::new(2.0, 0.1, 2.0),
        Vec3::new(20.0, 0.1, 20.0),
        Vec3::Y,
    );
    camera.ortho_height = Some(0.1);
    r.set_camera_override(Some(camera));
    r.apply(scene, 0.0);
    // The bed rises from .08 to .1 at (2.5,2.5): the ray leaves water after sqrt(.5²+.5²).
    close_hdr(
        center_hdr(&mut r),
        std::array::from_fn(|c| {
            [1.0, 0.8, 0.6][c] * (-[0.7f32, 0.2, 0.1][c] * 0.5f32.sqrt()).exp()
        }),
    );
    for eye in [1.0f32, 2.25] {
        let mut scene = optical_scene(2.0, None);
        let look = scene.water[0].look.as_mut().unwrap();
        look.absorb = [0.0; 3];
        look.scatter = [0.0; 3];
        look.fog = [0.1, 0.4, 0.8, 0.2];
        scene.environment.as_mut().unwrap().ambient = 1.0; // The controlled extra source is fog.rgb * ambient.
        let mut camera =
            CameraState::look_at_up(Vec3::new(8.0, eye, 8.0), Vec3::new(8.0, 0.0, 8.0), Vec3::Z);
        camera.ortho_height = Some(8.0);
        r.set_camera_override(Some(camera));
        r.apply(scene, 0.0);
        let path = eye.min(2.0);
        let transmission = (-0.2 * path).exp();
        let boundary = if eye > 2.0 { 1.0 - 0.02040816 } else { 1.0 };
        close_hdr(
            center_hdr(&mut r),
            std::array::from_fn(|c| {
                ([1.0, 0.8, 0.6][c] * transmission + [0.1, 0.4, 0.8][c] * (1.0 - transmission))
                    * boundary
            }),
        );
    }
}

#[test]
fn block_medium_matches_block_geometry_and_overlapping_regions_claim_pixels_once() {
    let (_case, gpu) = gpu().expect("underwater cases require a GPU");
    let mut r = renderer(&gpu, EYE, AT);
    r.set_water_foam(false);
    r.set_water_optics(false, false, false);
    let mut scene = optical_scene(2.0, None);
    let mut block = region(true, &ALL, |i, _| {
        [2.0, 2.0 - (i % 4) as f32 * 0.125, 0.0, 0.0]
    });
    let mut look = scene.water[0].look.clone().unwrap();
    look.blocky = true;
    look.per = 2;
    block.look = Some(look);
    scene.water = vec![block];
    scene.instances[0].pose.as_mut().unwrap().position[1] = -20.0;
    let mut camera = CameraState::look_at_up(
        Vec3::new(0.25, 0.5, 0.25),
        Vec3::new(0.25, -20.0, 0.25),
        Vec3::Z,
    );
    camera.ortho_height = Some(0.1);
    r.set_camera_override(Some(camera));
    r.apply(scene, 0.0);
    close_hdr(
        center_hdr(&mut r),
        std::array::from_fn(|c| [1.0, 0.8, 0.6][c] * (-[0.7f32, 0.2, 0.1][c] * 0.4375).exp()),
    );
    let mut scene = optical_scene(2.0, None);
    let mut second = scene.water[0].clone();
    second.id += 1;
    second.look.as_mut().unwrap().absorb = [1.0; 3];
    scene.water.push(second);
    let mut camera =
        CameraState::look_at_up(Vec3::new(8.0, 1.0, 8.0), Vec3::new(8.0, 0.0, 8.0), Vec3::Z);
    camera.ortho_height = Some(4.0);
    r.set_camera_override(Some(camera));
    r.apply(scene, 0.0);
    close_hdr(
        center_hdr(&mut r),
        std::array::from_fn(|c| [1.0, 0.8, 0.6][c] * (-[0.7f32, 0.2, 0.1][c]).exp()),
    );
}

#[test]
fn complete_outside_faces_reach_the_rendered_velocity_field() {
    use amoris_assets::{
        frame::ResidentWaterUpdate,
        water_field::{FieldCheckpoint, FieldRecipe},
    };
    let (_case, gpu) = gpu().expect("complete face rendering requires a GPU");
    let eye = Vec3::new(34.0, 4.0, 34.0);
    let target = Vec3::new(30.5, 1.0, 30.5);
    let mut actual = renderer(&gpu, eye, target);
    let mut expected = renderer(&gpu, eye, target);
    for r in [&mut actual, &mut expected] {
        r.set_water_foam(false);
    }
    let mut cp = FieldCheckpoint::new([32, 32], 0.0, vec![[0, 1.0f32.to_bits(), 0, 0]; 1024]);
    cp.far_faces[..32].fill(4.0f32.to_bits());
    cp.far_faces[32..].fill(5.0f32.to_bits());
    let mut full = scene(1, vec![], false);
    full.t_s = 1.5;
    full.resident_water.push(ResidentWaterUpdate {
        id: REGION,
        origin: [0.0; 2],
        cell_m: 1.0,
        look: region(false, &ALL, column).look.unwrap(),
        recipe: FieldRecipe::new(cp.key(), vec![], cp.digest(), String::new()).patch(0),
        checkpoint: Some(cp.checked().unwrap().into()),
        edge_level: None,
    });
    let fixture = std::env::var_os("WATER_FACE_FIXTURE").map(std::path::PathBuf::from);
    if let Some(dir) = &fixture {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("complete.bin"), full.encode()).unwrap();
    }
    actual.apply(full, 0.0);
    let mut view = region(false, &ALL, |i, k| {
        [
            1.0,
            1.0,
            if i == 31 { 2.0 } else { 0.0 },
            if k == 31 { 2.5 } else { 0.0 },
        ]
    });
    view.cell_m = 1.0;
    let mut frame = scene(1, vec![view], false);
    frame.t_s = 1.5;
    if let Some(dir) = &fixture {
        std::fs::write(dir.join("centred.bin"), frame.encode()).unwrap();
    }
    expected.apply(frame, 0.0);
    let a = actual.capture_rgba(0.0).2;
    assert!(
        a == expected.capture_rgba(0.0).2,
        "outside face conversion differs from centred CPU reference"
    );
    let mut wrong = region(false, &ALL, |_, _| [1.0, 1.0, 0.0, 0.0]);
    wrong.cell_m = 1.0;
    // The exported browser calibration is a standalone reset too: its lights/environment
    // must match, so a difference can only come from the deliberately missing velocities.
    let mut frame = scene(1, vec![wrong], false);
    frame.t_s = 1.5;
    if let Some(dir) = &fixture {
        std::fs::write(dir.join("missing.bin"), frame.encode()).unwrap();
    }
    expected.apply(frame, 0.0);
    assert!(
        a != expected.capture_rgba(0.0).2,
        "reference must detect lost boundary velocities"
    );
}

#[test]
fn analytic_sea_excludes_all_owned_regions_including_dry_and_ninth_and_releases_removed_bounds() {
    let (_case, gpu) = gpu().expect("ocean ownership requires GPU");
    let mut fields = Vec::new();
    for z in 0..3 {
        for x in 0..3 {
            let mut u = region(false, &ALL, |_, _| [0.0; 4]);
            u.id = 100 + x + z * 3;
            u.origin = [x as f32 * 32.0, z as f32 * 32.0];
            u.cell_m = 1.0;
            fields.push(u);
        }
    }
    let mut frame = scene(1, fields, false);
    let mut floor = amoris_render::demo::water::mesh_scene(64, "full")
        .instances
        .remove(0);
    floor.id = 1;
    let p = floor.pose.as_mut().unwrap();
    p.position = [48.0, 0.0, 48.0];
    p.scale = [100.0, 1.0, 100.0];
    let look = floor.look.as_mut().unwrap();
    look.mesh = "plane".into();
    look.color = [0.0, 0.0, 0.0, 1.0];
    look.emissive = [0.2, 0.3, 0.4];
    look.cast_shadows = false;
    frame.instances.push(floor);
    let eye = Vec3::new(80.0, 10.0, 80.0);
    let at = Vec3::new(80.0, 0.0, 80.0);
    let mut camera = CameraState::look_at_up(eye, at, Vec3::Z);
    camera.ortho_height = Some(16.0);
    let mut reference = renderer(&gpu, eye, at);
    let mut masked = renderer(&gpu, eye, at);
    let mut ocean = renderer(&gpu, eye, at);
    for r in [&mut reference, &mut masked, &mut ocean] {
        r.set_camera_override(Some(camera));
        r.set_water_foam(false);
        r.set_water_optics(false, false, false);
        r.set_lod(amoris_render::LodMode::Off);
    }
    let export = std::env::var_os("WATER_OCEAN_MASK_FIXTURE").map(std::path::PathBuf::from);
    if let Some(path) = &export {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("reference.bin"), frame.encode()).unwrap();
    }
    reference.apply(frame.clone(), 0.0);
    let expected = shoot(&mut reference);
    frame.sea = Some(Some(amoris_assets::frame::SeaView {
        level: 1.0,
        waves: vec![],
    }));
    if let Some(path) = &export {
        std::fs::write(path.join("masked.bin"), frame.encode()).unwrap();
    }
    masked.apply(frame.clone(), 0.0);
    assert_eq!(
        shoot(&mut masked),
        expected,
        "analytic sea overwrote an owned dry region"
    );
    let mut calibration = frame.clone();
    calibration.water.pop();
    if let Some(path) = &export {
        std::fs::write(path.join("calibration.bin"), calibration.encode()).unwrap();
    }
    ocean.apply(calibration, 0.0);
    let flooded = shoot(&mut ocean);
    assert!(
        differing(&expected, &flooded) > 0.1,
        "ocean calibration did not cover the viewport"
    );
    masked.apply(
        RenderFrame {
            tick: 2,
            t_s: frame.t_s,
            dt_s: frame.dt_s,
            water_removed: vec![108],
            ..Default::default()
        },
        0.0,
    );
    assert_eq!(
        shoot(&mut masked),
        flooded,
        "removed ownership rectangle remained masked"
    );
}

#[test]
fn bed_ledge_film_covers_the_solid_wall_and_keeps_foreground_objects_in_front() {
    let (_case, gpu) = gpu().expect("solid ledge image needs GPU");
    let eye = Vec3::new(12.0, 3.5, 8.0);
    let at = Vec3::new(8.0, 3.5, 8.0);
    let camera = CameraState::look_at(eye, at);
    let mut r = renderer(&gpu, eye, at);
    r.set_water_foam(false);
    r.set_splashes(false);
    r.set_water_optics(true, false, false);
    r.set_lod(amoris_render::LodMode::Off);
    let cube = |id, position, scale, emissive, alpha| InstanceUpdate {
        id,
        pose: Some(Pose {
            position,
            scale,
            ..Pose::default()
        }),
        look: Some(Look {
            mesh: "cube".into(),
            color: [0.0, 0.0, 0.0, alpha],
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
    };
    let mut frame = scene(1, vec![], false);
    frame.lights = Some(vec![]);
    let env = frame.environment.as_mut().unwrap();
    env.ambient = 0.0;
    env.sky_color = [0.0; 3];
    env.sky = 0;
    frame.instances = vec![cube(81, [4.0, 3.0, 8.0], [8.0, 6.0, 16.0], [2.0; 3], 1.0)];
    r.apply(frame.clone(), 0.0);
    let dry = r.capture_still(0.0).2;
    let mut wet = region(false, &ALL, |i, _| {
        if i < 16 {
            [6.4, 0.4, 8.0, 0.0]
        } else {
            [0.6, 0.6, 8.0, 0.0]
        }
    });
    wet.look.as_mut().unwrap().absorb = [8.0, 0.3, 0.1];
    wet.look.as_mut().unwrap().scatter = [0.0; 3];
    wet.look.as_mut().unwrap().fog = [0.0; 4];
    frame.water = vec![wet];
    r.apply(frame.clone(), 0.0);
    let film = r.capture_still(0.0).2;
    let counts = r.water_ledge_stats().unwrap();
    assert_eq!(
        counts[0].1.active_quads, 31,
        "one straight cut across the field"
    );
    let inverse = (camera.proj(W as f32 / H as f32) * camera.view()).inverse();
    let (mut upper, mut covered) = (0, 0);
    for y in 0..H {
        for x in 0..W {
            let ndc = Vec3::new(
                (x as f32 + 0.5) / W as f32 * 2.0 - 1.0,
                1.0 - (y as f32 + 0.5) / H as f32 * 2.0,
                1.0,
            );
            let direction = (inverse.project_point3(ndc) - eye).normalize();
            let hit = eye + direction * ((8.0 - eye.x) / direction.x);
            if hit.y > 3.5 && hit.y < 5.8 && hit.z > 1.0 && hit.z < 15.0 {
                let p = ((x + y * W) * 4) as usize;
                upper += 1;
                if i32::from(dry[p]) - i32::from(film[p]) > 10 {
                    covered += 1;
                }
            }
        }
    }
    assert!(
        upper > 500 && covered as f64 / upper as f64 > 0.98,
        "film missed solid upper wall: {covered}/{upper}"
    );
    r.set_water_culling(false);
    r.apply(frame.clone(), 0.0);
    assert_eq!(
        film,
        r.capture_still(0.0).2,
        "unculled ledge changed mask/color"
    );
    r.set_water_culling(true);
    r.apply(frame.clone(), 0.0);
    assert_eq!(
        film,
        r.capture_still(0.0).2,
        "recreated ledge changed mask/color"
    );
    let mut flat = frame.clone();
    flat.reset = false;
    for tile in &mut flat.water[0].tiles {
        tile.columns.fill([0.6, 0.6, 8.0, 0.0]);
    }
    r.apply(flat, 0.0);
    r.capture_still(0.0);
    assert_eq!(
        r.water_ledge_stats().unwrap()[0].1.active_quads,
        0,
        "stale active list after field edit"
    );
    frame.reset = false;
    r.apply(frame.clone(), 0.0);
    assert_eq!(
        film,
        r.capture_still(0.0).2,
        "retained field edit did not restore the ledge"
    );
    // A foreground emissive box must replace the film, rather than being attenuated as its input.
    frame.instances.push(cube(
        82,
        [9.0, 3.5, 8.0],
        [0.2, 1.0, 1.0],
        [0.0, 2.0, 0.0],
        1.0,
    ));
    r.apply(frame.clone(), 0.0);
    let foreground = r.capture_still(0.0).2;
    frame.water.clear();
    r.apply(frame, 0.0);
    let foreground_dry = r.capture_still(0.0).2;
    for y in H / 2 - 8..H / 2 + 8 {
        for x in W / 2 - 8..W / 2 + 8 {
            let p = ((x + y * W) * 4) as usize;
            assert_eq!(
                &foreground[p..p + 4],
                &foreground_dry[p..p + 4],
                "foreground is inside the film at {x}/{y}"
            );
        }
    }
}
