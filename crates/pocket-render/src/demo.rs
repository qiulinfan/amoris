//! Built-in demo scenes as render frames: the many_cubes stress test of Bevy, reproduced layout
//! for layout, for like-for-like benchmarks natively, in the browser and against three.js; and a
//! mixed scene whose draw batches cover every view and pipeline variant (the WebGPU baseline
//! path's check, batches.rs).

use std::collections::HashMap;

use glam::{Quat, Vec3};
use pocket_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};
use pocket_assets::mesh::{AlphaMode, ImageData, MaterialData, MeshData, ModelAsset, NodeData};
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

/// A camera circling the dense layout of `count` cubes from outside at frame `frame` (half a
/// degree per frame), looking at its centre: most cubes are hidden behind its faces, and every
/// frame uncovers some (the occlusion culling benchmark, docs/bench/occlusion.md).
pub fn orbit_camera(frame: u32, count: usize) -> crate::CameraState {
    let side = (count as f32).cbrt().round() * 1.25;
    let centre = Vec3::splat(side * 0.5);
    let a = frame as f32 * 0.5f32.to_radians();
    let eye = centre + Vec3::new(a.cos() * side * 1.5, side * 0.9, a.sin() * side * 1.5);
    crate::CameraState::look_at(eye, centre)
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
        // As the importer would (docs/spec/lod.md).
        pocket_assets::lod::build(&mut mesh, &pocket_assets::lod::LodOptions::default());
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

/// The occlusion culling check's scene (tests/occlusion.rs): on a ground, a wall of two boxes with
/// a narrow slit between them, an alpha-masked double-sided cylinder (the masked occluder of
/// [`mixed_model`], whose holes must not hide anything) and, behind them, a grid of every
/// primitive at several depths and heights, some peeking over the wall's top or past its ends,
/// some seen only through the slit or the holes; three boxes stand in front. Register
/// [`mixed_model`] under [`MIXED_MODEL`] first. `wall_y` is the left wall's height (the check
/// lowers it into the ground to uncover what it hid).
pub fn occluders(wall_y: f32) -> RenderFrame {
    let look = |mesh: &str, color: [f32; 3]| Look {
        mesh: mesh.into(),
        material: String::new(),
        color: [color[0], color[1], color[2], 1.0],
        metallic: 0.0,
        roughness: 0.6,
        transmission: None,
        ior: None,
        emissive: [0.0; 3],
        cast_shadows: true,
        visible: true,
    };
    let item =
        |id: u64, mesh: &str, color: [f32; 3], pos: [f32; 3], scale: [f32; 3]| InstanceUpdate {
            id,
            pose: Some(Pose {
                position: pos,
                rotation: Quat::from_rotation_y(id as f32 * 0.7).to_array(),
                scale,
            }),
            look: Some(look(mesh, color)),
            anim: None,
        };
    let wall = |id: u64, x: f32, y: f32| InstanceUpdate {
        id,
        pose: Some(Pose {
            position: [x, y, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [10.0, 6.0, 0.5],
        }),
        look: Some(look("cube", [0.75, 0.72, 0.68])),
        anim: None,
    };
    let mut instances = vec![
        InstanceUpdate {
            id: 1,
            pose: Some(Pose {
                position: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [80.0, 1.0, 80.0],
            }),
            look: Some(look("plane", [0.5, 0.5, 0.48])),
            anim: None,
        },
        wall(2, -5.25, wall_y),
        wall(3, 5.25, 3.0),
        InstanceUpdate {
            id: 4,
            pose: Some(Pose {
                position: [-8.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [3.0, 4.0, 3.0],
            }),
            look: Some(Look {
                color: [1.0; 4],
                ..look(&format!("{MIXED_MODEL}#masked_double"), [1.0; 3])
            }),
            anim: None,
        },
    ];
    for i in 0..3u64 {
        instances.push(item(
            10 + i,
            "cube",
            [0.9, 0.3, 0.2],
            [-4.0 + 4.0 * i as f32, 0.5, 5.0],
            [1.0; 3],
        ));
    }
    // A picket fence past the right wall's end and, behind the wall's top edge, small spheres
    // peeking over it by a few centimetres: partly hidden instances at every scale.
    for i in 0..9u64 {
        instances.push(InstanceUpdate {
            id: 20 + i,
            pose: Some(Pose {
                position: [10.9 + 0.8 * i as f32, 3.0, 1.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [0.3, 6.0, 0.3],
            }),
            look: Some(look("cube", [0.35, 0.3, 0.28])),
            anim: None,
        });
    }
    for i in 0..24u64 {
        let s = 0.2 + 0.05 * (i % 4) as f32;
        instances.push(item(
            40 + i,
            "sphere",
            [0.95, 0.85, 0.2],
            [
                -9.6 + 0.83 * i as f32,
                6.0 - s * 0.5 + 0.02 * (i % 5) as f32,
                -0.6,
            ],
            [s; 3],
        ));
    }
    let mut id = 100;
    for z in 0..4 {
        for x in 0..12 {
            for y in 0..3 {
                let mesh = PRIMITIVES[(id as usize) % PRIMITIVES.len()];
                let hue = id as f32 * 0.37;
                let size = 0.6 + 0.25 * ((x + y + z) % 3) as f32;
                instances.push(item(
                    id,
                    if mesh == "plane" { "cube" } else { mesh },
                    [
                        0.5 + 0.4 * hue.sin(),
                        0.5 + 0.4 * (hue + 2.1).sin(),
                        0.5 + 0.4 * (hue + 4.2).sin(),
                    ],
                    [
                        -17.6 + 3.2 * x as f32,
                        0.6 + 2.6 * y as f32 + 0.3 * z as f32,
                        -2.0 - 3.0 * z as f32,
                    ],
                    [size; 3],
                ));
                id += 1;
            }
        }
    }
    sunlit(instances, Vec3::new(-0.4, -1.0, -0.5), true)
}

/// The cameras of [`occluders`]: in front of the wall (most of the grid hidden), and from the
/// side, a cut away (most of it in view).
pub fn occluders_cameras() -> [crate::CameraState; 2] {
    [
        occluders_sweep(0.0),
        crate::CameraState::look_at(Vec3::new(22.0, 6.0, -4.0), Vec3::new(0.0, 2.0, -6.0)),
    ]
}

/// The front camera of [`occluders`] moved `x` metres sideways (and a little up and closer): a
/// sweep moves the wall's edges, the slit and the fence across the pyramid's texels.
pub fn occluders_sweep(x: f32) -> crate::CameraState {
    crate::CameraState::look_at(
        Vec3::new(0.5 + x, 6.4 + 0.11 * x, 15.0 - 0.3 * x.abs()),
        Vec3::new(0.3 * x, 2.5, 0.0),
    )
}

/// The path the LOD field's model is registered under (`Renderer::add_model`).
pub const LOD_MODEL: &str = "demo/lod.glb";

/// A bumpy rock: an icosphere subdivided `subdivisions` times (20 * 4^s triangles, every vertex
/// shared: no seams) of radius about 1, displaced along its normal by a sum of smooth waves seeded
/// by `seed`, with smooth normals.
pub fn rock_mesh(subdivisions: u32, seed: u32) -> MeshData {
    let t = (1.0 + 5f32.sqrt()) * 0.5;
    let mut positions: Vec<Vec3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| Vec3::from(*p).normalize())
    .collect();
    let mut faces: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdivisions {
        let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
        let mut midpoint = |a: u32, b: u32, positions: &mut Vec<Vec3>| -> u32 {
            *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                positions.push(((positions[a as usize] + positions[b as usize]) * 0.5).normalize());
                (positions.len() - 1) as u32
            })
        };
        let mut next = Vec::with_capacity(faces.len() * 4);
        for [a, b, c] in faces {
            let ab = midpoint(a, b, &mut positions);
            let bc = midpoint(b, c, &mut positions);
            let ca = midpoint(c, a, &mut positions);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        faces = next;
    }
    // Smooth waves in seeded directions: low frequencies shape the rock, high ones roughen it down
    // to a few edges of the finest subdivision per wavelength.
    let finest = 0.4 * (1u32 << subdivisions) as f32;
    let waves: Vec<(Vec3, f32, f32, f32)> = (0..12u32)
        .map(|k| {
            let h = |i: u32| unit_hash(seed.wrapping_mul(97).wrapping_add(k * 13 + i));
            let dir = Vec3::new(h(1) * 2.0 - 1.0, h(2) * 2.0 - 1.0, h(3) * 2.0 - 1.0)
                .normalize_or(Vec3::Y);
            let freq = 1.5 * 1.6f32.powi(k as i32);
            let amplitude = if freq > finest {
                0.0
            } else {
                0.16 / 1.45f32.powi(k as i32)
            };
            (dir, freq, amplitude, h(4) * std::f32::consts::TAU)
        })
        .collect();
    for p in &mut positions {
        let bump: f32 = waves
            .iter()
            .map(|(d, f, a, phase)| a * (p.dot(*d) * f + phase).sin())
            .sum();
        *p *= 1.0 + bump;
    }
    let indices: Vec<u32> = faces.iter().flatten().copied().collect();
    let uvs: Vec<[f32; 2]> = positions
        .iter()
        .map(|p| [p.x * 0.5 + 0.5, p.y * 0.5 + 0.5])
        .collect();
    smooth_mesh("rock", &positions, &uvs, indices)
}

/// A uniform value in [0, 1) from `i` (a PCG hash, as common.wgsl's).
fn unit_hash(i: u32) -> f32 {
    let s = i.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let w = ((s >> ((s >> 28) + 4)) ^ s).wrapping_mul(277_803_737);
    (((w >> 22) ^ w) >> 8) as f32 / 16_777_216.0
}

/// A (2, 3) torus knot's tube: `segments` rings along the curve of `sides` vertices each, closed
/// in both directions (no seams), about 1.7 across.
pub fn knot_mesh(segments: u32, sides: u32) -> MeshData {
    let (p, q) = (2.0f32, 3.0f32);
    let tube = 0.35;
    let scale = 0.5;
    // The curve: twice the torus's core circle plus `normal`, a unit vector perpendicular to the
    // curve (the core's tangent and `normal`'s own derivative are both perpendicular to it).
    let curve = |t: f32| {
        let core = Vec3::new((p * t).cos(), (p * t).sin(), 0.0);
        let normal = core * (q * t).cos() + Vec3::new(0.0, 0.0, -(q * t).sin());
        (core * 2.0 + normal, normal)
    };
    let mut positions = Vec::with_capacity((segments * sides) as usize);
    let mut uvs = Vec::with_capacity(positions.capacity());
    for i in 0..segments {
        let t = std::f32::consts::TAU * i as f32 / segments as f32;
        let (c, n) = curve(t);
        let tangent = (curve(t + 1e-3).0 - curve(t - 1e-3).0).normalize();
        let n = (n - tangent * tangent.dot(n)).normalize();
        let b = tangent.cross(n);
        for j in 0..sides {
            let a = std::f32::consts::TAU * j as f32 / sides as f32;
            positions.push((c + (n * a.cos() + b * a.sin()) * tube) * scale);
            uvs.push([8.0 * i as f32 / segments as f32, j as f32 / sides as f32]);
        }
    }
    let mut indices = Vec::with_capacity((segments * sides * 6) as usize);
    for i in 0..segments {
        let i1 = (i + 1) % segments;
        for j in 0..sides {
            let j1 = (j + 1) % sides;
            let (a, b) = (i * sides + j, i1 * sides + j);
            let (c, d) = (i1 * sides + j1, i * sides + j1);
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    smooth_mesh("knot", &positions, &uvs, indices)
}

/// A mesh with area-weighted smooth normals and tangents from positions, texture coordinates and
/// indices.
fn smooth_mesh(name: &str, positions: &[Vec3], uvs: &[[f32; 2]], indices: Vec<u32>) -> MeshData {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for t in indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| positions[i as usize]);
        let n = (b - a).cross(c - a);
        for &i in t {
            normals[i as usize] += n;
        }
    }
    let vertices = positions
        .iter()
        .zip(&normals)
        .zip(uvs)
        .map(|((p, n), uv)| pocket_assets::mesh::Vertex {
            position: p.to_array(),
            normal: n.normalize_or(Vec3::Y).to_array(),
            uv: *uv,
            tangent: [1.0, 0.0, 0.0, 1.0],
        })
        .collect();
    let mut m = MeshData::new(name, vertices, indices);
    m.compute_tangents();
    m
}

/// The LOD field's meshes without their levels: three seeded rocks and a torus knot. `detail` 6
/// gives rocks of 81,920 triangles and a knot of 49,152; each step down a quarter of that.
pub fn lod_meshes(detail: u32) -> Vec<MeshData> {
    let detail = detail.clamp(2, 7);
    vec![
        rock_mesh(detail, 1),
        rock_mesh(detail, 2),
        rock_mesh(detail, 3),
        knot_mesh(12 << detail, 4 << (detail / 2)),
    ]
}

/// The LOD field's model: [`lod_meshes`], each with its LOD chain (`pocket_assets::lod::build`, as
/// the importer gives a glTF's meshes).
pub fn lod_model(detail: u32) -> ModelAsset {
    let mut meshes = lod_meshes(detail);
    for m in &mut meshes {
        pocket_assets::lod::build(m, &pocket_assets::lod::LodOptions::default());
    }
    lod_asset(meshes)
}

/// The LOD field's model from its four meshes (levels already built or not): names and materials.
pub fn lod_asset(mut meshes: Vec<MeshData>) -> ModelAsset {
    let names = ["rock_a", "rock_b", "rock_c", "knot"];
    let colors = [
        [0.55, 0.5, 0.45],
        [0.5, 0.48, 0.46],
        [0.6, 0.55, 0.42],
        [0.85, 0.42, 0.18],
    ];
    let mut asset = ModelAsset::default();
    for (i, m) in meshes.iter_mut().enumerate().take(4) {
        m.name = names[i].into();
        m.material = Some(i);
        let c = colors[i];
        asset.materials.push(MaterialData {
            name: names[i].into(),
            base_color: [c[0], c[1], c[2], 1.0],
            roughness: if i == 3 { 0.35 } else { 0.85 },
            metallic: if i == 3 { 0.3 } else { 0.0 },
            ..MaterialData::default()
        });
    }
    asset.meshes = meshes;
    asset
}

/// A field of `n` x `n` cells `spacing` metres apart on a ground under a shadow-casting sun, each
/// holding a rock or a knot of [`lod_model`] (one knot in four), jittered, turned and scaled
/// (0.6 to 1.5): many dense meshes from a few metres to hundreds away. Register [`lod_model`]
/// under [`LOD_MODEL`] first.
pub fn lod_field(n: u32, spacing: f32) -> RenderFrame {
    let look = |mesh: &str, color: [f32; 4]| Look {
        mesh: mesh.into(),
        material: String::new(),
        color,
        metallic: 0.0,
        roughness: 0.8,
        transmission: None,
        ior: None,
        emissive: [0.0; 3],
        cast_shadows: true,
        visible: true,
    };
    let size = n as f32 * spacing;
    let mut instances = vec![InstanceUpdate {
        id: 1,
        pose: Some(Pose {
            position: [size * 0.5, 0.0, size * 0.5],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [size + 40.0, 1.0, size + 40.0],
        }),
        look: Some(look("plane", [0.42, 0.45, 0.36, 1.0])),
        anim: None,
    }];
    let hash = |i: u32, k: u32| unit_hash(i.wrapping_mul(8).wrapping_add(k));
    for i in 0..n * n {
        let (x, z) = (i % n, i / n);
        let kind = if i % 4 == 3 {
            "knot"
        } else {
            ["rock_a", "rock_b", "rock_c"][(i % 3) as usize]
        };
        let s = 0.6 + 0.9 * hash(i, 1);
        let rot = Quat::from_rotation_y(hash(i, 2) * std::f32::consts::TAU)
            * Quat::from_rotation_x((hash(i, 3) - 0.5) * 1.2);
        instances.push(InstanceUpdate {
            id: u64::from(i) + 2,
            pose: Some(Pose {
                position: [
                    (x as f32 + 0.5 + (hash(i, 4) - 0.5) * 0.6) * spacing,
                    s * 0.8,
                    (z as f32 + 0.5 + (hash(i, 5) - 0.5) * 0.6) * spacing,
                ],
                rotation: rot.to_array(),
                scale: [s; 3],
            }),
            look: Some(look(&format!("{LOD_MODEL}#{kind}"), [1.0; 4])),
            anim: None,
        });
    }
    sunlit(instances, Vec3::new(-0.5, -0.8, -0.3), true)
}

/// The LOD field's camera at `t` in [0, 1]: at head height in the field's first cell looking
/// along the diagonal (0), walking toward the middle (1).
pub fn lod_field_camera(n: u32, spacing: f32, t: f32) -> crate::CameraState {
    let size = n as f32 * spacing;
    let start = Vec3::new(0.35 * spacing, 2.2, 0.15 * spacing);
    let middle = Vec3::new(0.5 * size, 2.2, 0.5 * size - 0.2 * spacing);
    let eye = start.lerp(middle, t.clamp(0.0, 1.0));
    crate::CameraState::look_at(eye, eye + Vec3::new(1.0, -0.15, 1.0))
}

/// The path [`bent_model`] is registered under (`Renderer::add_model`).
pub const BENT_MODEL: &str = "demo/bent.glb";

/// A skinned column about 5 m tall standing on the origin, with its levels of detail: a rock of
/// 5,120 triangles ([`rock_mesh`]) stretched, on two joints, the second at 2 m (the weights blend
/// between 1.6 and 2.4 m). Its only clip bends the upper half 90 degrees toward -x, so a pose
/// drawn from the bind pose's vertices instead of the skinned ones stands out by metres. Its levels
/// are made from the straight bind pose.
pub fn bent_model() -> ModelAsset {
    use pocket_assets::mesh::{
        AnimationClip, Bounds, Channel, ChannelPath, Interpolation, SkeletonNode, SkinAsset,
        SkinWeights,
    };
    let mut m = rock_mesh(4, 5);
    m.name = "column".into();
    for v in &mut m.vertices {
        let [x, y, z] = v.position;
        v.position = [x * 0.5, y * 2.0 + 2.0, z * 0.5];
        let [nx, ny, nz] = v.normal;
        v.normal = Vec3::new(nx * 2.0, ny * 0.5, nz * 2.0)
            .normalize_or(Vec3::Y)
            .to_array();
    }
    m.bounds = Bounds::of(&m.vertices);
    m.compute_tangents();
    let weights = m
        .vertices
        .iter()
        .map(|v| {
            let t = ((v.position[1] - 1.6) / 0.8).clamp(0.0, 1.0);
            let t = t * t * (3.0 - 2.0 * t);
            [1.0 - t, t, 0.0, 0.0]
        })
        .collect();
    m.skin = Some(SkinWeights {
        joints: vec![[0, 1, 0, 0]; m.vertices.len()],
        weights,
    });
    pocket_assets::lod::build(&mut m, &pocket_assets::lod::LodOptions::default());
    let joint = |name: &str, parent: Option<usize>, y: f32| SkeletonNode {
        name: name.into(),
        parent,
        translation: [0.0, y, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    };
    ModelAsset {
        meshes: vec![m],
        nodes: vec![NodeData {
            name: "column".into(),
            mesh: 0,
            transform: glam::Mat4::IDENTITY.to_cols_array(),
            skin: Some(0),
        }],
        skeleton: vec![joint("root", None, 0.0), joint("bend", Some(0), 2.0)],
        skins: vec![SkinAsset {
            joints: vec![0, 1],
            inverse_bind: vec![
                glam::Mat4::IDENTITY.to_cols_array(),
                glam::Mat4::from_translation(Vec3::new(0.0, -2.0, 0.0)).to_cols_array(),
            ],
        }],
        animations: vec![AnimationClip {
            name: "bent".into(),
            duration: 1.0,
            channels: vec![Channel {
                node: 1,
                path: ChannelPath::Rotation,
                interpolation: Interpolation::Linear,
                times: vec![0.0],
                values: vec![Quat::from_rotation_z(std::f32::consts::FRAC_PI_2).to_array()],
            }],
        }],
        ..ModelAsset::default()
    }
}

/// `n` entities of [`bent_model`] (register it under [`BENT_MODEL`] first), `spacing` metres apart
/// along +x from `at`, ids from `first_id`: each gets its own skinned copy of the column and of
/// its levels (meshes.rs `add_dynamic`).
pub fn bent_columns(n: u32, at: Vec3, spacing: f32, first_id: u64) -> Vec<InstanceUpdate> {
    (0..n)
        .map(|i| InstanceUpdate {
            id: first_id + u64::from(i),
            pose: Some(Pose {
                position: (at + Vec3::X * spacing * i as f32).to_array(),
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            }),
            look: Some(Look {
                mesh: BENT_MODEL.into(),
                material: String::new(),
                color: [0.7, 0.62, 0.5, 1.0],
                metallic: 0.0,
                roughness: 0.8,
                transmission: None,
                ior: None,
                emissive: [0.0; 3],
                cast_shadows: true,
                visible: true,
            }),
            anim: None,
        })
        .collect()
}

/// The path the neural texture scene's ground is registered under (`Renderer::add_model`).
pub const NEURAL_GROUND: &str = "demo/neural-ground.glb";

/// A square ground of side `side` metres whose texture coordinates repeat `tiles` times (the
/// neural texture scene's mesh: a tiling material over a large floor).
pub fn neural_ground_model(side: f32, tiles: f32) -> ModelAsset {
    let n = [0.0, 1.0, 0.0];
    let h = side * 0.5;
    let v = |x: f32, z: f32, u: f32, w: f32| pocket_assets::Vertex {
        position: [x, 0.0, z],
        normal: n,
        uv: [u, w],
        tangent: [1.0, 0.0, 0.0, 1.0],
    };
    let vertices = vec![
        v(-h, h, 0.0, tiles),
        v(h, h, tiles, tiles),
        v(h, -h, tiles, 0.0),
        v(-h, -h, 0.0, 0.0),
    ];
    ModelAsset {
        meshes: vec![pocket_assets::MeshData::new(
            "ground",
            vertices,
            vec![0, 1, 2, 0, 2, 3],
        )],
        materials: vec![],
        images: vec![],
        nodes: vec![NodeData {
            name: "ground".into(),
            mesh: 0,
            transform: glam::Mat4::IDENTITY.to_cols_array(),
            skin: None,
        }],
        skeleton: vec![],
        skins: vec![],
        animations: vec![],
    }
}

/// The neural texture scene: [`neural_ground_model`] (registered under [`NEURAL_GROUND`]) drawn
/// with `material` (a `.ntex` path, or empty for the inline material `color`), under a sun.
pub fn neural_scene(material: &str, color: [f32; 4], roughness: f32) -> RenderFrame {
    let instances = vec![InstanceUpdate {
        id: 1,
        pose: Some(Pose::default()),
        look: Some(Look {
            mesh: NEURAL_GROUND.into(),
            material: material.into(),
            color,
            metallic: 0.0,
            roughness,
            transmission: None,
            ior: None,
            emissive: [0.0; 3],
            cast_shadows: false,
            visible: true,
        }),
        anim: None,
    }];
    sunlit(instances, Vec3::new(-0.3, -1.0, -0.4), false)
}

/// The neural texture scene's cameras: `0` looking straight down at the ground so it fills the
/// view (every pixel decodes, at about one level of detail), `1` a walker's view across it (the
/// level of detail grows with distance; the sky fills the top).
pub fn neural_camera(view: u32) -> crate::CameraState {
    match view {
        0 => crate::CameraState::look_at_up(Vec3::new(0.0, 4.0, 0.0), Vec3::ZERO, -Vec3::Z),
        _ => crate::CameraState::look_at(Vec3::new(0.0, 1.7, 6.0), Vec3::new(0.0, 0.0, -8.0)),
    }
}
