//! Built-in demo scenes as render frames: the many_cubes stress test of Bevy, reproduced layout
//! for layout, for like-for-like benchmarks natively, in the browser and against three.js; and a
//! mixed scene whose draw batches cover every view and pipeline variant (the WebGPU baseline
//! path's check, batches.rs).

use glam::{Quat, Vec3};
use pocket_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};
use pocket_assets::mesh::{AlphaMode, ImageData, MaterialData, ModelAsset, NodeData};
use pocket_assets::primitives::{PRIMITIVES, primitive};

/// The camera of the many_cubes benchmark at frame `frame` (Bevy's `move_camera` with
/// `--benchmark`: rotate about world z then x by 0.15/60 per frame; dense: fixed).
pub fn cubes_camera(frame: u32, dense: bool) -> crate::CameraState {
    if dense {
        return crate::CameraState::look_at(
            Vec3::new(100.0, 90.0, 100.0),
            Vec3::new(0.0, -10.0, 0.0),
        );
    }
    let d = 0.15 / 60.0;
    let step = Quat::from_rotation_x(d) * Quat::from_rotation_z(d);
    let mut rot = Quat::IDENTITY;
    // Exponentiate by squaring would be exact too; frames are few.
    for _ in 0..frame {
        rot = (step * rot).normalize();
    }
    let mut c = crate::CameraState::look_at(Vec3::ZERO, -Vec3::Z);
    c.rotation = rot;
    c
}

pub fn many_cubes(count: usize, dense: bool, shadows: bool) -> RenderFrame {
    let look = Look {
        mesh: "cube".into(),
        material: String::new(),
        color: [0.8, 0.7, 0.6, 1.0],
        metallic: 0.0,
        roughness: 0.6,
        transmission: None,
        ior: None,
        emissive: [0.0; 3],
        cast_shadows: shadows,
        visible: true,
    };
    let mut instances = Vec::with_capacity(count + 1);
    if dense {
        // Bevy's Layout::Dense, exactly: x, y wrap at cbrt(count), z grows continuously.
        let size = (count as f32).cbrt().round();
        let gap = 1.25;
        for i in 0..count {
            let x = i as f32 % size;
            let y = (i as f32 / size) % size;
            let z = i as f32 / (size * size);
            instances.push(InstanceUpdate {
                id: i as u64 + 1,
                pose: Some(Pose {
                    position: [x * gap, y * gap, z * gap],
                    ..Pose::default()
                }),
                look: Some(look.clone()),
                anim: None,
            });
        }
    } else {
        // Bevy's Layout::Sphere, exactly: a Fibonacci spiral on a sphere of radius 500, each cube
        // facing the centre, plus the inside-out box around them.
        let radius = 200.0f64 * 2.5;
        let golden = 0.5f64 * (1.0 + 5f64.sqrt());
        for i in 0..count {
            let theta = std::f64::consts::TAU * i as f64 / golden;
            let phi = (1.0 - 2.0 * (i as f64 + 0.5) / count as f64).acos();
            let p = glam::DVec3::new(phi.sin() * theta.cos(), phi.sin() * theta.sin(), phi.cos())
                * radius;
            let pos = p.as_vec3();
            let rot = glam::Quat::from_mat4(
                &glam::camera::rh::view::look_at_mat4(pos, Vec3::ZERO, Vec3::Y).inverse(),
            );
            instances.push(InstanceUpdate {
                id: i as u64 + 1,
                pose: Some(Pose {
                    position: pos.to_array(),
                    rotation: rot.to_array(),
                    scale: [1.0; 3],
                }),
                look: Some(look.clone()),
                anim: None,
            });
        }
        let s = radius as f32 * 2.2;
        instances.push(InstanceUpdate {
            id: count as u64 + 1,
            pose: Some(Pose {
                position: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [-s, -s, -s],
            }),
            look: Some(Look {
                color: [1.0; 4],
                cast_shadows: false,
                ..look.clone()
            }),
            anim: None,
        });
    }
    sunlit(instances, Vec3::new(0.0, -1.0, -1.0), shadows)
}

/// A first frame of `instances` under a sun shining along `dir`, with the atmosphere's sky.
fn sunlit(instances: Vec<InstanceUpdate>, dir: Vec3, shadows: bool) -> RenderFrame {
    RenderFrame {
        tick: 1,
        t_s: 0.0,
        dt_s: 1.0 / 60.0,
        reset: true,
        instances,
        removed: vec![],
        lights: Some(vec![LightView {
            id: 0,
            kind: LightKindView::Directional,
            position: [0.0; 3],
            direction: dir.normalize().to_array(),
            color: [1.0, 0.96, 0.9],
            intensity: 6.0,
            range: 0.0,
            inner_deg: 0.0,
            outer_deg: 0.0,
            shadows,
        }]),
        cameras: Some(vec![]),
        environment: Some(EnvironmentView {
            sky: 0,
            sky_color: [0.3, 0.5, 0.8],
            ambient: 1.0,
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.0,
            fog_color: [0.6, 0.7, 0.8],
            exposure_ev: 0.0,
            bloom: 0.1,
        }),
        sea: None,
        splats: None,
        ui: None,
        audio: None,
        emitters: None,
    }
}

/// The path the mixed scene's model is registered under (`Renderer::add_model`).
pub const MIXED_MODEL: &str = "demo/mixed.glb";

/// The mixed scene's model: four primitive meshes, one per pipeline variant (opaque, alpha-masked,
/// double-sided, both), the masked ones cut by a checker texture's alpha.
pub fn mixed_model() -> ModelAsset {
    let size = 64u32;
    let mut rgba8 = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let on = ((x / 8) + (y / 8)) % 2 == 0;
            rgba8.extend_from_slice(&[240, 236, 228, if on { 255 } else { 0 }]);
        }
    }
    let image = ImageData {
        name: "checker".into(),
        width: size,
        height: size,
        rgba8,
        srgb: true,
    };
    let parts = [
        (
            "opaque",
            "torus",
            [0.9, 0.45, 0.2],
            AlphaMode::Opaque,
            false,
        ),
        ("masked", "sphere", [0.3, 0.8, 0.4], AlphaMode::Mask, false),
        ("double", "plane", [0.3, 0.5, 0.95], AlphaMode::Opaque, true),
        (
            "masked_double",
            "cylinder",
            [0.9, 0.8, 0.3],
            AlphaMode::Mask,
            true,
        ),
    ];
    let mut asset = ModelAsset {
        images: vec![image],
        ..ModelAsset::default()
    };
    for (i, (name, shape, c, alpha_mode, double_sided)) in parts.into_iter().enumerate() {
        let mut mesh = primitive(shape).unwrap_or_default();
        mesh.name = name.into();
        mesh.material = Some(i);
        asset.meshes.push(mesh);
        asset.materials.push(MaterialData {
            name: name.into(),
            base_color: [c[0], c[1], c[2], 1.0],
            roughness: 0.55,
            base_color_texture: (alpha_mode == AlphaMode::Mask).then_some(0),
            alpha_mode,
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

/// A grid of `n` x `n` cells on a ground under a shadow-casting sun, cycling through the seven
/// primitives and the four meshes of [`mixed_model`]: many meshes in every pipeline variant, so
/// nearly every draw batch starts at a nonzero offset in each of the five views (the camera and
/// four shadow cascades). Register [`mixed_model`] under [`MIXED_MODEL`] first.
pub fn mixed(n: u32) -> RenderFrame {
    let kinds: Vec<String> = PRIMITIVES
        .iter()
        .map(|p| (*p).to_owned())
        .chain(
            ["opaque", "masked", "double", "masked_double"]
                .iter()
                .map(|m| format!("{MIXED_MODEL}#{m}")),
        )
        .collect();
    let look = |mesh: &str, color: [f32; 4]| Look {
        mesh: mesh.into(),
        material: String::new(),
        color,
        metallic: 0.0,
        roughness: 0.6,
        transmission: None,
        ior: None,
        emissive: [0.0; 3],
        cast_shadows: true,
        visible: true,
    };
    let gap = 3.0;
    let half = (n as f32 - 1.0) * gap * 0.5;
    let mut instances = vec![InstanceUpdate {
        id: 1,
        pose: Some(Pose {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [n as f32 * gap + 8.0, 1.0, n as f32 * gap + 8.0],
        }),
        look: Some(look("plane", [0.55, 0.55, 0.52, 1.0])),
        anim: None,
    }];
    for i in 0..n * n {
        let (x, z) = (i % n, i / n);
        let kind = &kinds[i as usize % kinds.len()];
        let hue = i as f32 * 0.61;
        let color = [
            0.55 + 0.4 * hue.sin(),
            0.55 + 0.4 * (hue + 2.1).sin(),
            0.55 + 0.4 * (hue + 4.2).sin(),
            1.0,
        ];
        // Model meshes keep their material's color (white tint); planes stand up to show both
        // faces.
        let tint = if kind.contains('#') { [1.0; 4] } else { color };
        let upright = kind.ends_with("#double") || kind == "plane";
        let yaw = Quat::from_rotation_y(i as f32 * 0.9);
        let rot = if upright {
            yaw * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)
        } else {
            yaw
        };
        let scale = if upright { 1.6 } else { 1.0 };
        instances.push(InstanceUpdate {
            id: u64::from(i) + 2,
            pose: Some(Pose {
                position: [x as f32 * gap - half, 1.0, z as f32 * gap - half],
                rotation: rot.to_array(),
                scale: [scale; 3],
            }),
            look: Some(look(kind, tint)),
            anim: None,
        });
    }
    sunlit(instances, Vec3::new(-0.45, -1.0, -0.35), true)
}

/// The mixed scene's camera: above and in front of the grid of `n` x `n` cells.
pub fn mixed_camera(n: u32) -> crate::CameraState {
    let d = n as f32 * 3.0;
    crate::CameraState::look_at(Vec3::new(0.0, d * 0.55, d * 0.95), Vec3::new(0.0, 0.5, 0.0))
}
