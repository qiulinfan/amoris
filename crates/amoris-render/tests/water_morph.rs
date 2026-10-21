//! Actual GPU bridge vertices, current-field bounds and continuous endpoint stencils.
use amoris_render::{Antialiasing, BackendChoice, Gpu, Gtao, LodMode, Renderer, demo::water};
use glam::{Vec2, Vec3};
use std::collections::{BTreeMap, BTreeSet};

fn fine(h: &[f64], p: [f64; 2]) -> f64 {
    let [i, k] = p.map(|x| (x.floor() as usize).min(126));
    let [x, z] = [p[0] - i as f64, p[1] - k as f64];
    let [a, b, c, d] = [
        h[i + 128 * k],
        h[i + 128 * (k + 1)],
        h[i + 1 + 128 * (k + 1)],
        h[i + 1 + 128 * k],
    ];
    if z >= x {
        a * (1.0 - z) + b * (z - x) + c * x
    } else {
        a * (1.0 - x) + c * z + d * (x - z)
    }
}
#[test]
fn gpu_transitions_use_bridge_geometry_and_the_same_vertex_evaluation() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("morph validation requires a GPU");
    gpu.device
        .on_uncaptured_error(std::sync::Arc::new(|e| panic!("morph GPU validation: {e}")));
    gpu.device.set_device_lost_callback(|r, m| {
        if r != wgpu::DeviceLostReason::Destroyed {
            panic!("morph GPU lost: {r:?}: {m}");
        }
    });
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 640, 360);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_gtao(Gtao::Off);
    r.set_prepass(amoris_render::PrepassMode::Off);
    r.set_occlusion(amoris_render::OcclusionMode::Off);
    r.set_lod(LodMode::On);
    let frame = water::mesh_scene(128, "full");
    let mut h = vec![0.0f64; 128 * 128];
    for tile in &frame.water[0].tiles {
        for (n, c) in tile.columns.iter().enumerate() {
            h[(tile.key[0] as usize * 16 + n % 16) + 128 * (tile.key[1] as usize * 16 + n / 16)] =
                f64::from(c[0]);
        }
    }
    let camera = water::mesh_camera(128, "full");
    r.set_camera_override(Some(camera));
    r.apply(frame.clone(), 0.0);
    r.capture_rgba(0.0);
    let mut observed = 0;
    let mut now = 0.0;
    let mut held_camera = camera;
    for n in 1..=48 {
        now = n as f64 / 60.0;
        let factor = 1.0 - n as f32 * 0.014;
        let centre = Vec3::new(16.0, 0.0, 16.0);
        let mut c = camera;
        c.position = centre + (camera.position - centre) * factor;
        r.set_camera_override(Some(c));
        r.capture_rgba(now);
        let stats = r.water_mesh_stats().unwrap()[0].1;
        assert!(
            stats.max_error_pixels <= 1.0,
            "unbounded transition: {stats:?}"
        );
        assert!(stats.active_triangles <= stats.capacity_triangles);
        if stats.transition_patches == 0 {
            continue;
        }
        let geometry = r.water_mesh_indices().unwrap().remove(0).1;
        let vertices = geometry
            .indices
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|id| {
                let (p, indices, weights) = geometry.stencil(id);
                let y = (0..6)
                    .map(|n| h[indices[n] as usize] * weights[n])
                    .sum::<f64>();
                (id, [p[0], p[1], y])
            })
            .collect::<BTreeMap<_, _>>();
        let mut areas = BTreeMap::<[u32; 2], f64>::new();
        let mut edges = BTreeMap::<([u32; 2], [u32; 2], [u32; 2]), u32>::new();
        let vp = c.proj(640.0 / 360.0) * c.view();
        let mut maximum = 0.0f32;
        let mut active = 0;
        for t in geometry.indices.as_chunks::<3>().0 {
            let [a, b, c] = t.map(|id| vertices[&id]);
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if area.abs() < 1e-10 {
                continue;
            }
            assert!(area < 0.0, "inverted bridge triangle");
            active += 1;
            let p = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
            let patch = p.map(|x| (x / 16.0).floor() as u32);
            *areas.entry(patch).or_default() -= area * 0.5;
            let world = |y| Vec3::new((p[0] as f32 + 0.5) * 0.25, y, (p[1] as f32 + 0.5) * 0.25);
            let delta = (vp
                .project_point3(world(((a[2] + b[2] + c[2]) / 3.0) as f32))
                .truncate()
                - vp.project_point3(world(fine(&h, p) as f32)).truncate())
                * Vec2::new(320.0, 180.0);
            maximum = maximum.max(delta.length());
            for (a, b) in [(a, b), (b, c), (c, a)] {
                if (a[0] == b[0] && a[0] % 16.0 == 0.0) || (a[1] == b[1] && a[1] % 16.0 == 0.0) {
                    let pa = [a[0] as u32, a[1] as u32];
                    let pb = [b[0] as u32, b[1] as u32];
                    assert_eq!(
                        pa[0].abs_diff(pb[0]) + pa[1].abs_diff(pb[1]),
                        1,
                        "non-unit boundary"
                    );
                    assert_eq!(a[2], h[pa[0] as usize + 128 * pa[1] as usize]);
                    assert_eq!(b[2], h[pb[0] as usize + 128 * pb[1] as usize]);
                    *edges.entry((patch, pa.min(pb), pa.max(pb))).or_default() += 1;
                }
            }
        }
        assert_eq!(active, stats.active_triangles);
        assert_eq!(areas.len(), stats.visible as usize);
        for (patch, area) in areas {
            let size = patch.map(|v| (127 - v * 16).min(16));
            assert!(
                (area - f64::from(size[0] * size[1])).abs() < 1e-3,
                "bridge coverage: {patch:?} {area}"
            );
        }
        assert!(edges.values().all(|&n| n == 1), "overlapping boundary");
        assert!(
            maximum <= stats.max_error_pixels + 0.005,
            "reported bound misses actual bridge: {maximum}/{stats:?}"
        );
        let ids = geometry
            .indices
            .iter()
            .copied()
            .filter(|v| v & 0x80000000 != 0)
            .collect::<BTreeSet<_>>();
        assert!(
            !ids.is_empty(),
            "transition counter without bridge vertices"
        );
        let boundary = |id| {
            let (p, _, _) = geometry.stencil(id);
            p.iter().any(|x| x % 16.0 == 0.0)
        };
        let ids = ids
            .iter()
            .copied()
            .filter(|&id| boundary(id))
            .chain(ids.iter().copied().step_by(23).take(48))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let positions = r.water_probe_vertices(7, &ids).unwrap();
        for (n, &id) in ids.iter().enumerate() {
            let (p, indices, weights) = geometry.stencil(id);
            let height = (0..6)
                .map(|n| h[indices[n] as usize] * weights[n])
                .sum::<f64>();
            let expected = [(p[0] + 0.5) * 0.25, height, (p[1] + 0.5) * 0.25, 1.0];
            if boundary(id) {
                assert_eq!(
                    positions[n * 2][1].to_bits(),
                    (height as f32).to_bits(),
                    "GPU seam moved"
                );
            }
            for (a, b) in positions[n * 2].iter().zip(expected) {
                assert!(
                    (f64::from(*a) - b).abs() < 8e-6 * b.abs().max(1.0),
                    "GPU shared vertex function differs: {a}/{b}"
                );
            }
        }
        observed += 1;
        eprintln!("bridge sample {observed}: actual_max={maximum}, {stats:?}");
        held_camera = c;
        if observed == 3 {
            break;
        }
    }
    assert!(observed > 0, "camera sweep did not exercise a morph");
    // Finish a live transition at a fixed view, without recreating any region or source field.
    r.set_camera_override(Some(held_camera));
    let mut states = Vec::new();
    for _ in 0..36 {
        now += 1.0 / 60.0;
        r.capture_rgba(now);
        let s = r.water_mesh_stats().unwrap()[0].1;
        assert!(s.max_error_pixels <= 1.0);
        states.push(s.transition_patches);
    }
    assert_eq!(
        *states.last().unwrap(),
        0,
        "transition did not reach an endpoint: {states:?}"
    );
    // A newly corrugated current field invalidates every coarse endpoint. Accuracy takes
    // precedence over retaining a smooth transition based on yesterday's height field.
    let mut edited = frame;
    edited.reset = false;
    edited.tick += 1;
    for tile in &mut edited.water[0].tiles {
        for (n, column) in tile.columns.iter_mut().enumerate() {
            column[0] += if (n % 16 + n / 16) % 2 == 0 {
                0.4
            } else {
                -0.4
            };
        }
    }
    now += 1.0 / 60.0;
    r.apply(edited, now);
    r.capture_rgba(now);
    let s = r.water_mesh_stats().unwrap()[0].1;
    assert!(s.max_error_pixels <= 1.0);
    assert_eq!(
        s.levels[1..].iter().sum::<u32>(),
        0,
        "current field did not invalidate coarse geometry"
    );
    assert!(
        s.accuracy_resets > 0,
        "field edit did not exercise accuracy recovery"
    );
    // Full-detail exclusions and an abrupt accuracy-sensitive camera move stay bounded.
    r.set_camera_override(Some(water::mesh_camera(128, "under")));
    now += 1.0 / 60.0;
    r.capture_rgba(now);
    let s = r.water_mesh_stats().unwrap()[0].1;
    assert_eq!(s.transition_patches, 0);
    assert_eq!(s.levels[1..].iter().sum::<u32>(), 0);
}
