//! Controlled analytic water fields shared by native and browser visual checks, not solver evidence.
use amoris_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
    WaterLookView, WaterTileView, WaterUpdate,
};
use glam::Vec3;

const N: u32 = 64;
fn box_at(id: u64, pos: [f32; 3], scale: [f32; 3], color: [f32; 4], mesh: &str) -> InstanceUpdate {
    InstanceUpdate {
        id,
        pose: Some(Pose {
            position: pos,
            scale,
            ..Pose::default()
        }),
        look: Some(Look {
            mesh: mesh.into(),
            color,
            metallic: 0.0,
            roughness: 0.75,
            material: String::new(),
            emissive: [0.0; 3],
            cast_shadows: true,
            visible: true,
            transmission: None,
            ior: None,
        }),
        anim: None,
    }
}
fn column(case: &str, i: u32, k: u32) -> [f32; 4] {
    let x = (i as f32 + 0.5) * 0.5;
    let z = (k as f32 + 0.5) * 0.5;
    match case {
        "pool" => [1.2, 1.2, 0.0, 0.0],
        "fall" => {
            let h = 1.2 + 1.4 * (13.0 - x).clamp(0.0, 1.0);
            [h, h, 4.0, 0.0]
        }
        _ => {
            let r2 = (x - 16.0).powi(2) + (z - 16.0).powi(2);
            let obstacle = (-r2 / 12.0).exp();
            let h = 1.5 + 0.02 * (32.0 - x) + 0.12 * (-r2 / 24.0).exp();
            [
                h,
                if r2 < 2.5 { 0.0 } else { h },
                5.0 * (1.0 - 0.9 * obstacle),
                3.0 * (z - 16.0).signum() * obstacle,
            ]
        }
    }
}
pub fn scene(case: &str) -> RenderFrame {
    let mut instances = vec![box_at(
        1,
        [16.0, -0.25, 16.0],
        [34.0, 0.5, 34.0],
        [0.22, 0.24, 0.25, 1.0],
        "cube",
    )];
    let mut id = 2;
    for k in 0..16 {
        for i in 0..16 {
            let color = if (i + k) % 2 == 0 {
                [0.55, 0.45, 0.3, 1.0]
            } else {
                [0.22, 0.29, 0.3, 1.0]
            };
            instances.push(box_at(
                id,
                [i as f32 * 2.0 + 1.0, 0.0, k as f32 * 2.0 + 1.0],
                [1.98, 0.08, 1.98],
                color,
                "cube",
            ));
            id += 1;
        }
    }
    for (pos, scale) in [
        ([16.0, 1.5, -0.5], [34.0, 3.0, 1.0]),
        ([16.0, 1.5, 32.5], [34.0, 3.0, 1.0]),
        ([-0.5, 1.5, 16.0], [1.0, 3.0, 32.0]),
        ([32.5, 1.5, 16.0], [1.0, 3.0, 32.0]),
    ] {
        instances.push(box_at(id, pos, scale, [0.42, 0.42, 0.38, 1.0], "cube"));
        id += 1;
    }
    if case == "rapids" {
        instances.push(box_at(
            id,
            [16.0, 1.0, 16.0],
            [3.8, 4.0, 3.8],
            [0.3, 0.25, 0.2, 1.0],
            "sphere",
        ));
    }
    let tiles = (0..N / 16)
        .flat_map(|z| {
            (0..N / 16).map(move |x| WaterTileView {
                key: [x, z],
                columns: (0..256)
                    .map(|n| column(case, x * 16 + n % 16, z * 16 + n / 16))
                    .collect(),
            })
        })
        .collect();
    RenderFrame {
        tick: 1,
        t_s: 0.0,
        dt_s: 1.0 / 60.0,
        reset: true,
        instances,
        water: vec![WaterUpdate {
            id: 7,
            origin: [0.0; 2],
            cell_m: 0.5,
            size: [N, N],
            tiles,
            look: Some(WaterLookView {
                fog: [0.03, 0.17, 0.22, 0.12],
                absorb: [0.7, 0.18, 0.08],
                scatter: [0.01, 0.025, 0.04],
                blocky: false,
                per: 1,
                surround_m: 0.0,
            }),
        }],
        lights: Some(vec![LightView {
            id: 0,
            kind: LightKindView::Directional,
            position: [0.0; 3],
            direction: Vec3::new(-0.4, -0.8, -0.3).normalize().to_array(),
            color: [1.0, 0.95, 0.86],
            intensity: 6.0,
            range: 0.0,
            inner_deg: 0.0,
            outer_deg: 0.0,
            shadows: true,
        }]),
        environment: Some(EnvironmentView {
            sky: 1,
            sky_color: [0.3, 0.5, 0.8],
            ambient: 1.0,
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.0,
            fog_color: [0.5, 0.6, 0.7],
            exposure_ev: 0.0,
            bloom: 0.0,
        }),
        ..RenderFrame::default()
    }
}

/// Geometry-scalability fixture, shared with actual browser checks. Its prescribed columns are
/// rendering inputs, not a shallow-water solver reference or a gameplay performance workload.
pub fn mesh_scene(side: u32, case: &str) -> RenderFrame {
    assert!((16..=512).contains(&side) && side.is_multiple_of(16));
    let mut frame = scene("pool");
    let extent = side as f32 * 0.25;
    frame.instances = vec![box_at(
        1,
        [extent * 0.5, -0.25, extent * 0.5],
        [extent + 2.0, 0.5, extent + 2.0],
        [0.35, 0.29, 0.2, 1.0],
        "cube",
    )];
    let water = &mut frame.water[0];
    water.size = [side, side];
    water.cell_m = 0.25;
    let look = water.look.as_mut().unwrap();
    look.blocky = case == "blocky";
    look.per = 4;
    water.tiles = (0..side / 16)
        .flat_map(|z| {
            (0..side / 16).map(move |x| WaterTileView {
                key: [x, z],
                columns: (0..256)
                    .map(|n| {
                        let i = x * 16 + n % 16;
                        let k = z * 16 + n / 16;
                        let h = 1.0 + 0.15 * (i as f32 * 0.09).sin() * (k as f32 * 0.07).cos();
                        let dry = (case == "sparse" && (i / 32 + k / 32) % 2 == 1) || case == "dry";
                        let h = if case == "cliff" && i >= side / 2 {
                            h + 1.7
                        } else {
                            h
                        };
                        if dry {
                            [0.0, 0.0, 0.0, 0.0]
                        } else {
                            [h, h, 2.0, 0.5]
                        }
                    })
                    .collect(),
            })
        })
        .collect();
    frame
}

pub fn mesh_camera(side: u32, case: &str) -> crate::CameraState {
    let extent = side as f32 * 0.25;
    let c = extent * 0.5;
    let (eye, at) = match case {
        "local" => (
            [extent * 0.15, 6.0, extent * 0.15],
            [extent * 0.15 + 3.0, 1.0, extent * 0.15 + 3.0],
        ),
        "near" => (
            [extent * 0.15, 3.0, extent * 0.15],
            [extent * 0.25, 1.0, extent * 0.25],
        ),
        "under" => ([c, 0.5, c], [c + extent * 0.1, 0.7, c + extent * 0.1]),
        "away" => ([-3.0, 3.0, -3.0], [-10.0, 3.0, -10.0]),
        _ => ([c, extent * 0.9, c + extent * 0.8], [c, 0.0, c]),
    };
    crate::CameraState::look_at(Vec3::from(eye), Vec3::from(at))
}
