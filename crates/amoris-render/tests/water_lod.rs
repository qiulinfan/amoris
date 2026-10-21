//! Independent inspection of actual GPU-selected triangles, against the original fine field.
use amoris_render::{
    Antialiasing, BackendChoice, Gpu, Gtao, LodMode, LodSettings, Renderer, demo::water,
};
use glam::{Vec2, Vec3};
use std::collections::BTreeMap;
type Triangle = [[f64; 3]; 3];
fn height(tris: &[Triangle], p: [f64; 2]) -> Option<f64> {
    for &[a, b, c] in tris {
        let det = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        let u = ((p[0] - a[0]) * (c[1] - a[1]) - (p[1] - a[1]) * (c[0] - a[0])) / det;
        let v = ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])) / det;
        if u >= -1e-9 && v >= -1e-9 && u + v <= 1.0 + 1e-9 {
            return Some(a[2] * (1.0 - u - v) + b[2] * u + c[2] * v);
        }
    }
    None
}
#[test]
fn actual_gpu_lod_has_bounded_projected_error_and_identical_patch_edges() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("water LOD requires a GPU");
    gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
        panic!("water LOD GPU validation: {e}")
    }));
    gpu.device.set_device_lost_callback(|r, m| {
        if r != wgpu::DeviceLostReason::Destroyed {
            panic!("water LOD device lost: {r:?}: {m}");
        }
    });
    for (side, width, height_px, scale, shallow, view, bounds) in [
        (
            128u32,
            320u32,
            180u32,
            1.0,
            false,
            "full",
            &[0.05, 0.25, 1.0][..],
        ),
        (128, 320, 180, 0.5, false, "full", &[0.25][..]),
        (128, 320, 180, 1.0, true, "full", &[1.0][..]),
        (512, 1280, 720, 1.0, false, "full", &[1.0][..]),
        (512, 1280, 720, 1.0, false, "near", &[1.0][..]),
    ] {
        let mut renderer =
            Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, width, height_px);
        renderer.set_antialiasing(Antialiasing::Msaa);
        renderer.set_gtao(Gtao::Off);
        renderer.set_render_scale(scale);
        renderer.set_prepass(amoris_render::PrepassMode::Off);
        renderer.set_occlusion(amoris_render::OcclusionMode::Off);
        let camera = water::mesh_camera(side, view);
        renderer.set_camera_override(Some(camera));
        let vp = camera.proj(width as f32 / height_px as f32) * camera.view();
        let mut frame = water::mesh_scene(side, "full");
        if shallow {
            for tile in &mut frame.water[0].tiles {
                for c in &mut tile.columns {
                    c[1] = 0.02;
                }
            }
        }
        let mut h = vec![0.0; (side * side) as usize];
        for tile in &frame.water[0].tiles {
            for (n, c) in tile.columns.iter().enumerate() {
                h[((tile.key[0] * 16 + n as u32 % 16) + side * (tile.key[1] * 16 + n as u32 / 16))
                    as usize] = f64::from(c[0]);
            }
        }
        let source = |i: usize, k: usize| h[i + side as usize * k];
        for &bound in bounds {
            renderer.set_lod_settings(LodSettings {
                mode: LodMode::On,
                pixels: bound,
                ..Default::default()
            });
            renderer.apply(frame.clone(), 0.0);
            renderer.capture_still(0.0);
            let stats = renderer.water_mesh_stats().unwrap()[0].1;
            assert_eq!(stats.levels.iter().sum::<u32>(), stats.visible);
            assert!(stats.max_error_pixels <= bound, "{stats:?}");
            if bound == 1.0 {
                assert!(
                    stats.levels[1..].iter().sum::<u32>() > 0,
                    "LOD comparison is insensitive: {stats:?}"
                );
                assert!(stats.active_triangles < stats.input_triangles);
            }
            let geometry = renderer.water_mesh_indices().unwrap().remove(0).1;
            let mut bins: BTreeMap<[usize; 2], Vec<Triangle>> = BTreeMap::new();
            for t in geometry.indices.as_chunks::<3>().0 {
                assert!(t.iter().all(|&v| v < side * side));
                let points = t
                    .iter()
                    .map(|&v| [(v % side) as f64, (v / side) as f64, h[v as usize]])
                    .collect::<Vec<_>>();
                let [a, b, c] = [points[0], points[1], points[2]];
                let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                if area.abs() < 1e-12 {
                    continue;
                }
                let key = [
                    ((a[0] + b[0] + c[0]) / 48.0).floor() as usize,
                    ((a[1] + b[1] + c[1]) / 48.0).floor() as usize,
                ];
                bins.entry(key).or_default().push([a, b, c]);
            }
            let mut maximum = 0.0f32;
            for k in (0..side as usize - 1).step_by(if side == 512 { 2 } else { 1 }) {
                for i in (0..side as usize - 1).step_by(if side == 512 { 2 } else { 1 }) {
                    for [x, z] in [[0.25, 0.5], [0.5, 0.25], [0.75, 0.75]] {
                        let q = [i as f64 + x, k as f64 + z];
                        let Some(tris) = bins.get(&[i / 16, k / 16]) else {
                            assert_ne!(view, "full", "overview mesh leaves a hole");
                            continue;
                        };
                        let coarse = height(tris, q).expect("retained GPU patch leaves a hole");
                        let a = source(i, k);
                        let b = source(i, k + 1);
                        let c = source(i + 1, k + 1);
                        let d = source(i + 1, k);
                        let fine = if z >= x {
                            a * (1.0 - z) + b * (z - x) + c * x
                        } else {
                            a * (1.0 - x) + c * z + d * (x - z)
                        };
                        if shallow {
                            assert!(
                                (coarse - fine).abs() <= 0.010005,
                                "coarse surface can penetrate a thin bed: {coarse}/{fine}"
                            );
                        }
                        let world = |y| {
                            Vec3::new((q[0] as f32 + 0.5) * 0.25, y, (q[1] as f32 + 0.5) * 0.25)
                        };
                        if view == "near" && (vp * world(fine as f32).extend(1.0)).w <= 0.0001 {
                            continue;
                        }
                        let error = (vp.project_point3(world(coarse as f32)).truncate()
                            - vp.project_point3(world(fine as f32)).truncate())
                            * Vec2::new(width as f32 * 0.5, height_px as f32 * 0.5);
                        maximum = maximum.max(error.length());
                    }
                }
            }
            assert!(
                maximum <= bound + 0.005,
                "GPU-selected surface exceeds {bound} pixels: {maximum}, {stats:?}"
            );
            assert!(
                maximum <= stats.max_error_pixels + 0.005,
                "reported bound underestimates actual geometry: {maximum}, {stats:?}"
            );
            for edge in (16..side as usize - 1).step_by(16) {
                for k in (0..side as usize - 1).step_by(if side == 512 { 2 } else { 1 }) {
                    let q = [edge as f64, k as f64 + 0.37];
                    let (Some(left_patch), Some(right_patch)) = (
                        bins.get(&[edge / 16 - 1, k / 16]),
                        bins.get(&[edge / 16, k / 16]),
                    ) else {
                        assert_ne!(view, "full");
                        continue;
                    };
                    let left = height(left_patch, q).unwrap();
                    let right = height(right_patch, q).unwrap();
                    let expected = source(edge, k) * 0.63 + source(edge, k + 1) * 0.37;
                    assert!(
                        (left - right).abs() < 1e-9 && (left - expected).abs() < 1e-9,
                        "cracked edge {edge}/{k}: {left}/{right}/{expected}"
                    );
                }
            }
            eprintln!("bound={bound} actual_max={maximum} {stats:?}");
            for edge in (16..side as usize - 1).step_by(16) {
                for i in (0..side as usize - 1).step_by(if side == 512 { 2 } else { 1 }) {
                    let q = [i as f64 + 0.37, edge as f64];
                    let (Some(lower), Some(upper)) = (
                        bins.get(&[i / 16, edge / 16 - 1]),
                        bins.get(&[i / 16, edge / 16]),
                    ) else {
                        assert_ne!(view, "full");
                        continue;
                    };
                    let a = height(lower, q).unwrap();
                    let b = height(upper, q).unwrap();
                    let expected = source(i, edge) * 0.63 + source(i + 1, edge) * 0.37;
                    assert!(
                        (a - b).abs() < 1e-9 && (a - expected).abs() < 1e-9,
                        "cracked horizontal edge {i}/{edge}: {a}/{b}/{expected}"
                    );
                }
            }
        }
        let mut changed = 0;
        let centre = Vec3::new(side as f32 * 0.125, 0.0, side as f32 * 0.125);
        for factor in [1.0, 0.75, 0.5, 0.3, 0.15, 0.3, 0.5, 0.75, 1.0] {
            let mut moving = camera;
            moving.position = centre + (camera.position - centre) * factor;
            renderer.set_camera_override(Some(moving));
            // Read the first moved frame; a settled TAA capture can end with zero rewrites.
            renderer.capture_rgba(0.0);
            let stats = renderer.water_mesh_stats().unwrap()[0].1;
            assert!(
                stats.max_error_pixels <= *bounds.last().unwrap(),
                "moving view exceeds budget: {stats:?}"
            );
            changed += stats.rewritten_patches;
        }
        assert!(changed > 0, "moving-view check did not change a patch");
        renderer.set_camera_override(Some(water::mesh_camera(side, "under")));
        renderer.capture_still(0.0);
        assert_eq!(
            renderer.water_mesh_stats().unwrap()[0].1.levels[1..]
                .iter()
                .sum::<u32>(),
            0,
            "underwater must retain the exact medium surface"
        );
        let mut ortho = camera;
        ortho.ortho_height = Some(40.0);
        renderer.set_camera_override(Some(ortho));
        renderer.capture_still(0.0);
        assert_eq!(
            renderer.water_mesh_stats().unwrap()[0].1.levels[1..]
                .iter()
                .sum::<u32>(),
            0
        );
    }
}
