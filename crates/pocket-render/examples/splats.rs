//! A hybrid mesh + Gaussian splat scene (docs/spec/splats.md, docs/bench/splats.md): a procedural
//! garden of splats (rolling grass with a dirt path, grass blades, flower beds, trees, a beach
//! ball, a rainbow ring) generated here without any downloaded data, an iridescent orb cloud with
//! degree-3 spherical harmonics drawn twice, and lit primitive meshes standing in and around them
//! (a pedestal, crates, a pillar through the ring), under the procedural sky.
//!
//! `cargo run --release -p pocket-render --example splats -- [--count N] [--ply PATH [--flip]
//!  [--scale S]] [--save PATH.ply] [--capture OUT.png [--angle DEG] [--distance F] [--look X,Y,Z]]
//!  [--bench FRAMES] [--headless-bench FRAMES] [--size WxH] [--key-bits 16|24|32] [--radiance R]
//!  [--raster quad|tile] [--antialias] [--compare PREFIX] [--visible] [--no-meshes] [--orbit]
//!  [--vsync]`
//!
//! `--raster` picks the quad draw or the compute tile rasterizer (default: `POCKET_SPLAT_RASTER`,
//! else quads). `--compare PREFIX` captures the same view with both and writes `PREFIX_quad.png`,
//! `PREFIX_tile.png`, `PREFIX_diff.png` (the absolute difference, x8) and prints the difference.
//! `--visible` (with `--capture`) also prints the entity-id pass's coverage: the garden is entity
//! 100, the orbs 101 and 102, the meshes 1 to 21.
//!
//! Without `--capture` or a benchmark it opens a window with a fly camera (right mouse to look,
//! WASD/QE to move); `--orbit` turns the camera around the scene instead.
//!
//! For tools/neural/splatfit: `--dataset DIR [--views N] [--size WxH]` renders a small mesh scene
//! from N cameras into `DIR/view_NNN.png` with `DIR/cameras.txt`; `--replay DIR --ply PATH` draws a
//! fitted cloud from the same cameras into `DIR/render_NNN.png`.

use std::f32::consts::{PI, TAU};

use glam::{Quat, Vec3};
use pocket_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame, SplatView,
};
use pocket_render::app::{Host, RunOptions, run};
use pocket_render::splat::{RawSplat, SplatCloud, SplatRaster, loader};
use pocket_render::{BackendChoice, CameraState, Renderer};

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

// ---------------------------------------------------------------------------------------------
// Randomness and noise (deterministic: the same count always builds the same garden).

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }
    fn normal(&mut self) -> f32 {
        let u = self.f().max(1e-7);
        let v = self.f();
        (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
    }
    fn unit(&mut self) -> Vec3 {
        let z = self.range(-1.0, 1.0);
        let a = self.range(0.0, TAU);
        let r = (1.0 - z * z).max(0.0).sqrt();
        Vec3::new(r * a.cos(), z, r * a.sin())
    }
    fn quat(&mut self) -> Quat {
        Quat::from_xyzw(self.normal(), self.normal(), self.normal(), self.normal()).normalize()
    }
}

fn hash2(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841);
    h = (h ^ (h >> 13)).wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

fn value_noise(x: f32, y: f32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (u, v) = (s(fx), s(fy));
    let a = hash2(xi, yi) + (hash2(xi + 1, yi) - hash2(xi, yi)) * u;
    let b = hash2(xi, yi + 1) + (hash2(xi + 1, yi + 1) - hash2(xi, yi + 1)) * u;
    a + (b - a) * v
}

fn fbm(x: f32, y: f32) -> f32 {
    value_noise(x, y) * 0.5
        + value_noise(x * 2.1, y * 2.1) * 0.3
        + value_noise(x * 4.3, y * 4.3) * 0.2
}

// ---------------------------------------------------------------------------------------------
// The garden.

const HALF: f32 = 12.0;

fn terrain(x: f32, z: f32) -> f32 {
    0.35 * (0.31 * x).sin() * (0.27 * z).cos() + 0.25 * fbm(x * 0.15 + 3.0, z * 0.15 - 2.0) - 0.15
}

fn terrain_normal(x: f32, z: f32) -> Vec3 {
    let e = 0.05;
    let dx = terrain(x + e, z) - terrain(x - e, z);
    let dz = terrain(x, z + e) - terrain(x, z - e);
    Vec3::new(-dx, 2.0 * e, -dz).normalize()
}

/// Distance from the winding dirt path's center line.
fn path_distance(x: f32, z: f32) -> f32 {
    (z - 2.2 * (0.28 * x).sin() - 1.0).abs()
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn jitter(c: [f32; 3], rng: &mut Rng, amount: f32) -> [f32; 3] {
    let k = 1.0 + rng.range(-amount, amount);
    c.map(|v| (v * k).clamp(0.0, 1.0))
}

/// A flat disc lying on a surface with normal `n`, `radius` its standard deviation.
fn disc(p: Vec3, n: Vec3, radius: f32, color: [f32; 3], opacity: f32, rng: &mut Rng) -> RawSplat {
    let q = Quat::from_rotation_arc(Vec3::Z, n.normalize())
        * Quat::from_rotation_z(rng.range(0.0, TAU));
    let stretch = rng.range(0.8, 1.25);
    RawSplat {
        position: p.to_array(),
        scale: [radius * stretch, radius / stretch, radius * 0.08],
        rotation: q.to_array(),
        color,
        opacity,
    }
}

/// A thin splat along `dir` (a grass blade, a stem segment), `length` its standard deviation.
fn needle(
    p: Vec3,
    dir: Vec3,
    length: f32,
    width: f32,
    color: [f32; 3],
    opacity: f32,
    rng: &mut Rng,
) -> RawSplat {
    let q = Quat::from_rotation_arc(Vec3::Y, dir.normalize())
        * Quat::from_rotation_y(rng.range(0.0, TAU));
    RawSplat {
        position: p.to_array(),
        scale: [width, length, width * 0.4],
        rotation: q.to_array(),
        color,
        opacity,
    }
}

struct Budget {
    ground: usize,
    blades: usize,
    flowers: usize,
    trees: usize,
    ball: usize,
    ring: usize,
}

impl Budget {
    fn new(count: usize) -> Budget {
        let f = |p: f32| (count as f32 * p) as usize;
        Budget {
            ground: f(0.36),
            blades: f(0.20),
            flowers: f(0.20),
            trees: f(0.10),
            ball: f(0.07),
            ring: f(0.07),
        }
    }
}

const FLOWER_COLORS: [[f32; 3]; 6] = [
    [0.92, 0.16, 0.18],
    [0.98, 0.84, 0.18],
    [0.62, 0.32, 0.86],
    [0.96, 0.95, 0.92],
    [0.96, 0.52, 0.72],
    [0.98, 0.56, 0.14],
];

/// Flower bed centers: away from the path and the mesh props.
const BEDS: [(f32, f32, f32); 7] = [
    (-6.5, 5.5, 2.4),
    (-1.5, 6.5, 2.0),
    (4.5, 6.0, 2.6),
    (7.5, -1.0, 1.8),
    (-7.0, -4.5, 2.2),
    (0.5, -6.5, 2.4),
    (3.0, 2.8, 1.4),
];

const TREES: [(f32, f32, f32); 3] = [(-8.5, -8.0, 1.0), (8.5, -7.5, 1.2), (-9.5, 8.5, 0.9)];
const BALL: (f32, f32, f32) = (-4.0, -2.5, 1.1);
const RING: (f32, f32) = (5.0, -3.5);

fn garden(count: usize) -> Vec<RawSplat> {
    let mut rng = Rng(0x5eed_0001 ^ count as u64);
    let b = Budget::new(count);
    let mut out = Vec::with_capacity(count + 1024);

    // Ground: discs on the terrain, grass greens mottled by noise, the dirt path, stones.
    let spacing = (4.0 * HALF * HALF / b.ground as f32).sqrt();
    for _ in 0..b.ground {
        let x = rng.range(-HALF, HALF);
        let z = rng.range(-HALF, HALF);
        let n = terrain_normal(x, z);
        let p = Vec3::new(x, terrain(x, z), z);
        let g = fbm(x * 0.7, z * 0.7);
        let grass = mix3([0.16, 0.36, 0.08], [0.42, 0.62, 0.16], g);
        let d = path_distance(x, z) + 0.25 * value_noise(x * 3.0, z * 3.0);
        let dirt = mix3(
            [0.47, 0.36, 0.24],
            [0.62, 0.52, 0.38],
            value_noise(x * 5.0, z * 5.0),
        );
        let t = ((1.05 - d) / 0.3).clamp(0.0, 1.0);
        let mut c = jitter(mix3(grass, dirt, t), &mut rng, 0.12);
        if t > 0.5 && rng.f() < 0.01 {
            c = jitter([0.62, 0.6, 0.57], &mut rng, 0.1);
        }
        out.push(disc(p, n, spacing * 0.8, c, 0.95, &mut rng));
    }

    // Grass blades off the path, leaning with a breeze.
    let mut placed = 0;
    while placed < b.blades {
        let x = rng.range(-HALF, HALF);
        let z = rng.range(-HALF, HALF);
        if path_distance(x, z) < 1.2 {
            continue;
        }
        placed += 1;
        let h = rng.range(0.05, 0.14) * (0.6 + fbm(x * 0.4, z * 0.4));
        let lean = Vec3::new(0.35 + rng.normal() * 0.15, 1.0, 0.1 + rng.normal() * 0.15);
        let base = Vec3::new(x, terrain(x, z), z);
        let c = mix3([0.18, 0.42, 0.08], [0.5, 0.72, 0.2], rng.f());
        out.push(needle(
            base + lean.normalize() * h,
            lean,
            h,
            0.006,
            c,
            0.9,
            &mut rng,
        ));
    }

    // Flowers in beds: a curved stem, five to eight petals, a center.
    let per_flower = 12 + 7 * 18 + 16;
    let flowers = (b.flowers / per_flower).max(1);
    for i in 0..flowers {
        let (bx, bz, br) = BEDS[i % BEDS.len()];
        let a = rng.range(0.0, TAU);
        let r = br * rng.f().sqrt();
        let (x, z) = (bx + r * a.cos(), bz + r * a.sin());
        let ground = Vec3::new(x, terrain(x, z), z);
        let height = rng.range(0.25, 0.6);
        let bend = Vec3::new(rng.normal(), 0.0, rng.normal()) * 0.08;
        let stem_color = [0.22, 0.48, 0.12];
        for s in 0..12 {
            let t = (s as f32 + 0.5) / 12.0;
            let p = ground + Vec3::Y * (height * t) + bend * (t * t);
            let dir = Vec3::Y * height + bend * (2.0 * t);
            out.push(needle(
                p,
                dir,
                height / 20.0,
                0.007,
                stem_color,
                0.95,
                &mut rng,
            ));
        }
        let head = ground + Vec3::Y * height + bend;
        let facing = (Vec3::Y + Vec3::new(rng.normal(), 0.0, rng.normal()) * 0.25).normalize();
        let color = FLOWER_COLORS[(hash2(i as i32, 7) * 6.0) as usize % 6];
        let petal_len = rng.range(0.05, 0.09);
        let petals = 5 + (rng.next() % 4) as usize;
        let tangent = facing.cross(Vec3::X).normalize();
        let bitangent = facing.cross(tangent);
        for k in 0..petals {
            let ang = k as f32 / petals as f32 * TAU + rng.range(-0.1, 0.1);
            let out_dir = tangent * ang.cos() + bitangent * ang.sin();
            for _ in 0..18 {
                let along = rng.f();
                let across = rng.normal() * 0.25 * (1.0 - along * 0.6);
                let side = facing.cross(out_dir);
                let p = head
                    + out_dir * (petal_len * (0.2 + along))
                    + side * (across * petal_len * 0.45)
                    + facing * (along * along * 0.02);
                let shade = 0.75 + 0.25 * along;
                out.push(disc(
                    p,
                    facing + out_dir * 0.3,
                    petal_len * 0.11,
                    jitter(color.map(|c| c * shade), &mut rng, 0.06),
                    0.95,
                    &mut rng,
                ));
            }
        }
        for _ in 0..16 {
            let a = rng.range(0.0, TAU);
            let r = petal_len * 0.22 * rng.f().sqrt();
            let p = head + (tangent * a.cos() + bitangent * a.sin()) * r + facing * 0.012;
            out.push(disc(
                p,
                facing,
                petal_len * 0.09,
                jitter([0.55, 0.36, 0.08], &mut rng, 0.15),
                1.0,
                &mut rng,
            ));
        }
    }

    // Trees: a bark-textured trunk and a leafy crown.
    let per_tree = b.trees / TREES.len();
    for &(tx, tz, s) in &TREES {
        let base = Vec3::new(tx, terrain(tx, tz) - 0.05, tz);
        let trunk_h = 2.6 * s;
        let trunk_r = 0.22 * s;
        let trunk_n = per_tree / 4;
        let bark_size = (TAU * trunk_r * trunk_h / trunk_n as f32).sqrt() * 0.8;
        for _ in 0..trunk_n {
            let a = rng.range(0.0, TAU);
            let y = rng.f() * trunk_h;
            let r = trunk_r * (1.0 - 0.3 * y / trunk_h);
            let n = Vec3::new(a.cos(), 0.0, a.sin());
            let stripe = value_noise(a * 6.0, y * 4.0);
            let c = jitter(
                mix3([0.25, 0.16, 0.09], [0.45, 0.33, 0.2], stripe),
                &mut rng,
                0.1,
            );
            out.push(disc(
                base + n * r + Vec3::Y * y,
                n,
                bark_size,
                c,
                0.98,
                &mut rng,
            ));
        }
        let crown = base + Vec3::Y * (trunk_h + 0.6 * s);
        for _ in trunk_n..per_tree {
            // Three overlapping blobs, denser at the surface (leaves catch the light there).
            let lobe = crown + Vec3::new(rng.normal(), rng.normal() * 0.6, rng.normal()) * 0.5 * s;
            let d = rng.unit();
            let r = 1.3 * s * rng.f().powf(0.35);
            let p = lobe + d * r;
            let lit = (0.5 + 0.5 * d.y).clamp(0.0, 1.0) * (0.4 + 0.6 * r / (1.3 * s));
            let c = jitter(
                mix3([0.08, 0.24, 0.06], [0.36, 0.6, 0.16], lit),
                &mut rng,
                0.15,
            );
            let q = rng.quat();
            out.push(RawSplat {
                position: p.to_array(),
                scale: [0.045 * s, 0.025 * s, 0.004],
                rotation: q.to_array(),
                color: c,
                opacity: 0.9,
            });
        }
    }

    // A beach ball: six colored gores by longitude, white caps.
    let (bx, bz, br) = BALL;
    let center = Vec3::new(bx, terrain(bx, bz) + br * 0.97, bz);
    let ball_size = (4.0 * PI * br * br / b.ball as f32).sqrt() * 0.8;
    let gores = [
        [0.9, 0.12, 0.12],
        [0.96, 0.96, 0.94],
        [0.1, 0.32, 0.85],
        [0.98, 0.82, 0.1],
        [0.1, 0.6, 0.22],
        [0.96, 0.96, 0.94],
    ];
    let tilt = Quat::from_rotation_z(0.35) * Quat::from_rotation_x(-0.2);
    for _ in 0..b.ball {
        let n = rng.unit();
        let local = tilt.inverse() * n;
        let lon = local.z.atan2(local.x);
        let g = (((lon + PI) / TAU * 6.0) as usize).min(5);
        let c = if local.y.abs() > 0.93 {
            [0.96, 0.96, 0.94]
        } else {
            gores[g]
        };
        out.push(disc(
            center + n * br,
            n,
            ball_size,
            jitter(c, &mut rng, 0.04),
            0.98,
            &mut rng,
        ));
    }

    // A standing ring (torus), its hue running around it.
    let (rx, rz) = RING;
    let (big, small) = (1.3f32, 0.22f32);
    let ring_center = Vec3::new(rx, terrain(rx, rz) + big + small + 0.05, rz);
    let ring_rot = Quat::from_rotation_y(0.6) * Quat::from_rotation_x(PI / 2.0);
    let ring_size = (4.0 * PI * PI * big * small / b.ring as f32).sqrt() * 0.8;
    for _ in 0..b.ring {
        let u = rng.range(0.0, TAU);
        let v = rng.range(0.0, TAU);
        let ring_dir = Vec3::new(u.cos(), 0.0, u.sin());
        let n = ring_dir * v.cos() + Vec3::Y * v.sin();
        let p = ring_dir * big + n * small;
        let c = hsv(u / TAU, 0.8, 0.95);
        out.push(disc(
            ring_center + ring_rot * p,
            ring_rot * n,
            ring_size,
            jitter(c, &mut rng, 0.05),
            0.98,
            &mut rng,
        ));
    }
    out
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let f = |n: f32| {
        let k = (n + h * 6.0) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [f(5.0), f(3.0), f(1.0)]
}

/// An orb whose color turns with the view: a silver base and degree-3 spherical harmonics.
fn orb(count: usize) -> (Vec<RawSplat>, Vec<f32>) {
    let mut rng = Rng(0x0eb5);
    let r = 0.45f32;
    let size = (4.0 * PI * r * r / count as f32).sqrt() * 0.8;
    let mut raw = Vec::with_capacity(count);
    let mut sh = Vec::with_capacity(count * 45);
    for _ in 0..count {
        let n = rng.unit();
        raw.push(disc(n * r, n, size, [0.55, 0.55, 0.6], 0.98, &mut rng));
        // Band 1 (coefficients 1-3 multiply -y, z, -x): red toward +x viewers, blue toward -x,
        // green from above; band 2 and 3 add a shimmer that follows the surface normal.
        let band1 = [[0.0, -0.35, 0.0], [0.1, 0.0, -0.1], [-0.45, 0.0, 0.45]];
        for c in band1 {
            sh.extend_from_slice(&c);
        }
        for k in 0..12 {
            let w = 0.12 * (n.x * (k as f32 * 1.7).sin() + n.z * (k as f32 * 0.9).cos());
            sh.extend_from_slice(&[w, -w * 0.5, w * 0.8]);
        }
    }
    (raw, sh)
}

// ---------------------------------------------------------------------------------------------
// The scene.

fn look(mesh: &str, color: [f32; 4], roughness: f32) -> Look {
    Look {
        mesh: mesh.into(),
        material: String::new(),
        color,
        metallic: 0.0,
        roughness,
        transmission: None,
        ior: None,
        emissive: [0.0; 3],
        cast_shadows: true,
        visible: true,
    }
}

fn mesh(
    id: u64,
    mesh: &str,
    position: Vec3,
    rotation: Quat,
    scale: Vec3,
    color: [f32; 4],
    roughness: f32,
) -> InstanceUpdate {
    InstanceUpdate {
        id,
        pose: Some(Pose {
            position: position.to_array(),
            rotation: rotation.to_array(),
            scale: scale.to_array(),
        }),
        look: Some(look(mesh, color, roughness)),
        anim: None,
    }
}

fn scene_frame(meshes: bool, ply: Option<(Quat, f32)>) -> RenderFrame {
    let mut instances = Vec::new();
    let mut splats = Vec::new();
    match ply {
        Some((rot, scale)) => splats.push(SplatView {
            id: 100,
            asset: "cloud".into(),
            pose: Pose {
                position: [0.0; 3],
                rotation: rot.to_array(),
                scale: [scale; 3],
            },
            visible: true,
        }),
        None => {
            splats.push(SplatView {
                id: 100,
                asset: "garden".into(),
                pose: Pose::default(),
                visible: true,
            });
            let ped = Vec3::new(0.0, terrain(0.0, 3.0), 3.0);
            splats.push(SplatView {
                id: 101,
                asset: "orb".into(),
                pose: Pose {
                    position: (ped + Vec3::Y * 1.38).to_array(),
                    ..Pose::default()
                },
                visible: true,
            });
            splats.push(SplatView {
                id: 102,
                asset: "orb".into(),
                pose: Pose {
                    position: [-2.6, terrain(-2.6, -6.0) + 0.75, -6.0],
                    rotation: Quat::from_rotation_y(2.0).to_array(),
                    scale: [1.6; 3],
                },
                visible: true,
            });
        }
    }
    if meshes && ply.is_none() {
        let stone = [0.55, 0.53, 0.5, 1.0];
        let wood = [0.55, 0.36, 0.2, 1.0];
        let ped = Vec3::new(0.0, terrain(0.0, 3.0), 3.0);
        instances.push(mesh(
            1,
            "cylinder",
            ped + Vec3::Y * 0.4,
            Quat::IDENTITY,
            Vec3::new(0.7, 0.9, 0.7),
            stone,
            0.8,
        ));
        instances.push(mesh(
            2,
            "cube",
            ped + Vec3::Y * 0.88,
            Quat::IDENTITY,
            Vec3::new(0.9, 0.08, 0.9),
            stone,
            0.7,
        ));
        for (i, (x, z, s, yaw)) in [
            (2.4f32, 3.4f32, 0.6f32, 0.4f32),
            (3.2, 2.1, 0.45, -0.3),
            (-2.2, 5.8, 0.55, 0.9),
        ]
        .into_iter()
        .enumerate()
        {
            let y = terrain(x, z) + s * 0.45;
            instances.push(mesh(
                10 + i as u64,
                "cube",
                Vec3::new(x, y, z),
                Quat::from_rotation_y(yaw),
                Vec3::splat(s),
                wood,
                0.9,
            ));
        }
        let (rx, rz) = RING;
        instances.push(mesh(
            20,
            "cylinder",
            Vec3::new(rx, terrain(rx, rz) + 1.6, rz),
            Quat::IDENTITY,
            Vec3::new(0.22, 3.2, 0.22),
            [0.85, 0.85, 0.88, 1.0],
            0.3,
        ));
        instances.push(mesh(
            21,
            "sphere",
            Vec3::new(6.5, terrain(6.5, 2.0) + 0.4, 2.0),
            Quat::IDENTITY,
            Vec3::splat(0.8),
            [0.9, 0.2, 0.15, 1.0],
            0.35,
        ));
    }
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
            direction: Vec3::new(-0.4, -1.0, -0.6).normalize().to_array(),
            color: [1.0, 0.96, 0.9],
            intensity: 6.0,
            range: 0.0,
            inner_deg: 0.0,
            outer_deg: 0.0,
            shadows: false,
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
            bloom: 0.08,
        }),
        sea: None,
        splats: Some(splats),
        ..RenderFrame::default()
    }
}

struct Garden {
    frame: Option<RenderFrame>,
    clouds: Vec<(String, SplatCloud)>,
    key_bits: u32,
    radiance: f32,
    raster: SplatRaster,
    antialias: bool,
    orbit: bool,
    /// Orbit center, radius, height.
    focus: (Vec3, f32, f32),
    angle: f32,
    frames: u64,
}

impl Garden {
    fn camera(&self) -> CameraState {
        let (c, r, h) = self.focus;
        let a = self.angle + self.frames as f32 * 0.004;
        let eye = c + Vec3::new(r * a.cos(), h, r * a.sin());
        CameraState::look_at(eye, c)
    }
}

impl Host for Garden {
    fn update(&mut self, r: &mut Renderer, now: f64) {
        for (name, cloud) in self.clouds.drain(..) {
            r.splats.insert(&name, &cloud);
        }
        r.splats.key_bits = self.key_bits;
        r.splats.radiance = self.radiance;
        r.splats.raster = self.raster;
        r.splats.antialias = self.antialias;
        if let Some(f) = self.frame.take() {
            r.apply(f, now);
        }
        if self.orbit {
            r.set_camera_override(Some(self.camera()));
            self.frames += 1;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The fitting dataset: a mesh scene the splat fitter learns, and the replay of its result.

const FIT_SKY: [f32; 3] = [0.55, 0.65, 0.8];
const FIT_FOV_DEG: f32 = 50.0;
/// The renderer's exposure multiplier at 0 EV (renderer.rs).
const FIT_EXPOSURE: f32 = 0.9;

fn fit_frame(meshes: bool) -> RenderFrame {
    let mut f = scene_frame(false, None);
    f.splats = Some(Vec::new());
    if let Some(e) = f.environment.as_mut() {
        e.sky = 1;
        e.sky_color = FIT_SKY;
        e.bloom = 0.0;
    }
    if let Some(l) = f.lights.as_mut() {
        l[0].shadows = true;
    }
    if meshes {
        f.instances = vec![
            mesh(
                1,
                "plane",
                Vec3::ZERO,
                Quat::IDENTITY,
                Vec3::new(5.0, 1.0, 5.0),
                [0.45, 0.5, 0.35, 1.0],
                0.9,
            ),
            mesh(
                2,
                "sphere",
                Vec3::new(-0.9, 0.5, 0.3),
                Quat::IDENTITY,
                Vec3::ONE,
                [0.85, 0.15, 0.12, 1.0],
                0.4,
            ),
            mesh(
                3,
                "cube",
                Vec3::new(0.8, 0.35, -0.5),
                Quat::from_rotation_y(0.5),
                Vec3::splat(0.7),
                [0.55, 0.36, 0.2, 1.0],
                0.8,
            ),
            mesh(
                4,
                "cylinder",
                Vec3::new(0.3, 0.75, 1.0),
                Quat::IDENTITY,
                Vec3::new(0.3, 1.5, 0.3),
                [0.9, 0.9, 0.88, 1.0],
                0.5,
            ),
            mesh(
                5,
                "torus",
                Vec3::new(-0.5, 0.12, -1.1),
                Quat::IDENTITY,
                Vec3::splat(0.9),
                [0.95, 0.75, 0.1, 1.0],
                0.5,
            ),
            mesh(
                6,
                "cone",
                Vec3::new(1.4, 0.4, 0.8),
                Quat::IDENTITY,
                Vec3::new(0.6, 0.8, 0.6),
                [0.15, 0.3, 0.85, 1.0],
                0.6,
            ),
        ];
    } else {
        f.splats = Some(vec![SplatView {
            id: 100,
            asset: "cloud".into(),
            pose: Pose::default(),
            visible: true,
        }]);
    }
    f
}

/// Cameras on a ring around the scene at varying heights: (eye, target).
fn fit_cameras(n: usize) -> Vec<(Vec3, Vec3)> {
    let target = Vec3::new(0.0, 0.4, 0.0);
    (0..n)
        .map(|i| {
            let a = i as f32 * TAU / n as f32;
            let h = 0.8 + 2.6 * ((i as f32 * 0.618_034) % 1.0);
            let r = 4.6 - 0.3 * h;
            (Vec3::new(r * a.cos(), h, r * a.sin()), target)
        })
        .collect()
}

fn fit_render(r: &mut Renderer, eye: Vec3, target: Vec3, path: &std::path::Path) {
    let mut cam = CameraState::look_at(eye, target);
    cam.fov_y = FIT_FOV_DEG.to_radians();
    r.set_camera_override(Some(cam));
    for i in 0..3 {
        let _ = r.capture_rgba(f64::from(i) / 60.0);
    }
    let (w, h, px) = r.capture_rgba(0.1);
    image::save_buffer(path, &px, w, h, image::ColorType::Rgba8).expect("png");
}

fn dataset(dir: &str, views: usize, size: (u32, u32)) {
    let dir = std::path::Path::new(dir);
    std::fs::create_dir_all(dir).expect("dataset directory");
    let gpu = pocket_render::Gpu::headless(BackendChoice::from_env()).expect("gpu");
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, size.0, size.1);
    r.apply(fit_frame(true), 0.0);
    let mut txt = format!(
        "# width height fov_y_deg exposure sky_r sky_g sky_b (linear), then: file eye_xyz target_xyz\n{} {} {} {} {} {} {}\n",
        size.0, size.1, FIT_FOV_DEG, FIT_EXPOSURE, FIT_SKY[0], FIT_SKY[1], FIT_SKY[2]
    );
    for (i, (eye, target)) in fit_cameras(views).into_iter().enumerate() {
        let file = format!("view_{i:03}.png");
        fit_render(&mut r, eye, target, &dir.join(&file));
        txt += &format!(
            "{file} {} {} {} {} {} {}\n",
            eye.x, eye.y, eye.z, target.x, target.y, target.z
        );
    }
    std::fs::write(dir.join("cameras.txt"), txt).expect("cameras.txt");
    println!(
        "wrote {views} views of {}x{} to {}",
        size.0,
        size.1,
        dir.display()
    );
}

fn replay(dir: &str, cloud: &SplatCloud) {
    let dir = std::path::Path::new(dir);
    let txt = std::fs::read_to_string(dir.join("cameras.txt")).expect("cameras.txt");
    let mut lines = txt.lines().filter(|l| !l.starts_with('#'));
    let head: Vec<f32> = lines
        .next()
        .expect("header")
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    let size = (head[0] as u32, head[1] as u32);
    let gpu = pocket_render::Gpu::headless(BackendChoice::from_env()).expect("gpu");
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, size.0, size.1);
    r.splats.insert("cloud", cloud);
    r.splats.radiance = 1.0;
    r.apply(fit_frame(false), 0.0);
    let mut n = 0;
    for line in lines {
        let w: Vec<&str> = line.split_whitespace().collect();
        if w.len() < 7 {
            continue;
        }
        let v: Vec<f32> = w[1..7].iter().filter_map(|x| x.parse().ok()).collect();
        let file = w[0].replace("view_", "render_");
        fit_render(
            &mut r,
            Vec3::new(v[0], v[1], v[2]),
            Vec3::new(v[3], v[4], v[5]),
            &dir.join(file),
        );
        n += 1;
    }
    println!("rendered {n} views to {}", dir.display());
}

fn main() {
    env_logger_init();
    if let Some(dir) = arg("--dataset") {
        let views = arg("--views").and_then(|s| s.parse().ok()).unwrap_or(48);
        let size = arg("--size")
            .and_then(|s| {
                s.split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            })
            .unwrap_or((160u32, 160u32));
        dataset(&dir, views, size);
        return;
    }
    if let (Some(dir), Some(path)) = (arg("--replay"), arg("--ply")) {
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let cloud = loader::parse(&path, &bytes).unwrap_or_else(|e| panic!("{e}"));
        println!("loaded {path}: {} splats", cloud.len());
        replay(&dir, &cloud);
        return;
    }
    let count: usize = arg("--count")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000);
    let meshes = !flag("--no-meshes");
    let key_bits: u32 = arg("--key-bits").and_then(|s| s.parse().ok()).unwrap_or(24);
    let size = arg("--size")
        .and_then(|s| {
            s.split_once('x')
                .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        })
        .unwrap_or((1600u32, 900u32));
    let t = std::time::Instant::now();
    let mut clouds = Vec::new();
    let (ply, focus) = if let Some(path) = arg("--ply") {
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let cloud = loader::parse(&path, &bytes).unwrap_or_else(|e| panic!("{e}"));
        let scale: f32 = arg("--scale").and_then(|s| s.parse().ok()).unwrap_or(1.0);
        let rot = if flag("--flip") {
            Quat::from_rotation_x(PI)
        } else {
            Quat::IDENTITY
        };
        let (lo, hi) = cloud.bounds;
        let c = rot * ((lo + hi) * 0.5) * scale;
        let r = ((hi - lo).length() * 0.5 * scale).max(0.1);
        println!(
            "loaded {path}: {} splats, SH degree {} in {:.0} ms",
            cloud.len(),
            cloud.sh_degree,
            t.elapsed().as_secs_f64() * 1000.0
        );
        clouds.push(("cloud".to_owned(), cloud));
        (Some((rot, scale)), (c, r * 1.1, r * 0.35))
    } else {
        let raw = garden(count);
        let cloud = SplatCloud::from_raw(&raw, 0, &[]);
        let (oraw, osh) = orb(40_000);
        println!(
            "generated {} garden splats and a {}-splat orb in {:.0} ms",
            raw.len(),
            oraw.len(),
            t.elapsed().as_secs_f64() * 1000.0
        );
        if let Some(path) = arg("--save") {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&path).expect("create"));
            loader::write_ply(&raw, 0, &[], &mut f).expect("write");
            println!("saved {path}");
        }
        clouds.push(("garden".to_owned(), cloud));
        clouds.push(("orb".to_owned(), SplatCloud::from_raw(&oraw, 3, &osh)));
        (None, (Vec3::new(0.0, 0.8, 0.5), 11.0, 4.5))
    };
    let angle = arg("--angle")
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(65.0)
        .to_radians();
    // Brighter than an unlit material (1): the garden stands in sunlight next to lit meshes.
    let radiance = arg("--radiance")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.6);
    let raster = match arg("--raster").as_deref() {
        Some("tile") | Some("tiles") => SplatRaster::Tiles,
        Some("quad") | Some("quads") => SplatRaster::Quads,
        _ => SplatRaster::from_env(SplatRaster::Quads),
    };
    let mut host = Garden {
        frame: Some(scene_frame(meshes, ply)),
        clouds,
        key_bits,
        radiance,
        raster,
        antialias: flag("--antialias") || std::env::var("POCKET_SPLAT_AA").is_ok_and(|v| v == "1"),
        orbit: true,
        focus,
        angle,
        frames: 0,
    };
    if let Some(d) = arg("--distance").and_then(|s| s.parse::<f32>().ok()) {
        host.focus.1 *= d;
        host.focus.2 *= d;
    }
    if let Some(l) = arg("--look").and_then(|s| {
        let v: Vec<f32> = s.split(',').filter_map(|x| x.parse().ok()).collect();
        (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
    }) {
        host.focus.0 = l;
    }

    let compare = arg("--compare");
    if let Some(path) = arg("--capture").or(compare.clone()) {
        let gpu = pocket_render::Gpu::headless(BackendChoice::from_env()).expect("gpu");
        let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, size.0, size.1);
        let mut host = host;
        host.orbit = false;
        r.set_camera_override(Some(host.camera()));
        let shoot = |host: &mut Garden, r: &mut Renderer| {
            // Several frames: assets upload, and the readbacks that size buffers arrive late.
            for i in 0..8 {
                host.update(r, i as f64 / 60.0);
                let _ = r.capture_rgba(i as f64 / 60.0);
            }
            r.capture_rgba(0.1)
        };
        if compare.is_none() {
            let (w, h, px) = shoot(&mut host, &mut r);
            image::save_buffer(&path, &px, w, h, image::ColorType::Rgba8).expect("png");
            println!("saved {path} ({w}x{h}); {:?}; {:?}", r.last, r.splats.stats);
            if flag("--visible") {
                // What an agent's `render.visible` sees: the entity-id pass's coverage.
                r.request_visible();
                for i in 0..10 {
                    let _ = r.capture_rgba(f64::from(i) / 60.0);
                    if let Some(v) = r.take_visible() {
                        println!("visible (entity, share of the view): {v:?}");
                        break;
                    }
                }
                // Its GPU cost: the id pass timed over repeated requests.
                let mut times = Vec::new();
                for i in 0..40 {
                    r.request_visible();
                    let _ = r.capture_rgba(f64::from(i) / 60.0);
                    let _ = r.take_visible();
                    let t = r.last.passes.iter().find(|p| p.0 == "entity ids");
                    times.extend(t.map(|p| p.1));
                }
                times.sort_by(f32::total_cmp);
                if let Some(t) = times.get(times.len() / 2) {
                    println!(
                        "entity ids pass: median {t:.3} ms over {} timed frames",
                        times.len()
                    );
                }
            }
            return;
        }
        host.raster = SplatRaster::Quads;
        let (w, h, quad) = shoot(&mut host, &mut r);
        host.raster = SplatRaster::Tiles;
        let (_, _, tile) = shoot(&mut host, &mut r);
        println!("{:?}", r.splats.stats);
        let save = |suffix: &str, px: &[u8]| {
            let p = format!("{path}_{suffix}.png");
            image::save_buffer(&p, px, w, h, image::ColorType::Rgba8).expect("png");
            p
        };
        save("quad", &quad);
        save("tile", &tile);
        let mut diff = vec![255u8; quad.len()];
        let (mut sum, mut sq, mut max, mut over2, mut over8) = (0u64, 0f64, 0u8, 0u64, 0u64);
        let pixels = quad.as_chunks::<4>().0.iter().zip(tile.as_chunks::<4>().0);
        for (i, (a, b)) in pixels.enumerate() {
            let mut px_max = 0u8;
            for c in 0..3 {
                let d = a[c].abs_diff(b[c]);
                sum += u64::from(d);
                sq += f64::from(d) * f64::from(d);
                px_max = px_max.max(d);
                diff[i * 4 + c] = d.saturating_mul(8);
            }
            max = max.max(px_max);
            over2 += u64::from(px_max > 2);
            over8 += u64::from(px_max > 8);
        }
        let n = (w * h) as f64;
        let mse = sq / (n * 3.0);
        save("diff", &diff);
        println!(
            "compare {w}x{h}: mean abs diff {:.4} of 255, max {max}, PSNR {:.2} dB, pixels over 2: \
             {:.3}%, over 8: {:.3}%",
            sum as f64 / (n * 3.0),
            10.0 * (255.0f64 * 255.0 / mse.max(1e-12)).log10(),
            over2 as f64 / n * 100.0,
            over8 as f64 / n * 100.0
        );
        return;
    }
    if let Some(frames) = arg("--headless-bench").and_then(|s| s.parse::<u32>().ok()) {
        headless_bench(host, size, frames);
        return;
    }
    let bench: Option<u32> = arg("--bench").and_then(|s| s.parse().ok());
    host.orbit = bench.is_some() || flag("--orbit");
    let fly = (!host.orbit).then(|| host.camera());
    let options = RunOptions {
        title: format!("splats ({count})"),
        width: 1280,
        height: 720,
        backend: BackendChoice::from_env(),
        vsync: flag("--vsync"),
        fly_camera: fly,
        walk: false,
        bench: bench.map(|n| (60, n)),
    };
    match run(host, options) {
        Ok(Some(r)) => println!("{r:#?}"),
        Ok(None) => {}
        Err(e) => eprintln!("error: {e}"),
    }
}

/// Renders offscreen at `size` and reports mean per-pass GPU times over `frames` frames with the
/// splat draw on, then over as many with it off (the opaque pass's difference is the draw).
fn headless_bench(mut host: Garden, size: (u32, u32), frames: u32) {
    let gpu = pocket_render::Gpu::headless(BackendChoice::from_env()).expect("gpu");
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, size.0, size.1);
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench target"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    println!(
        "{} on {}, {}x{}, key bits {}, {:?}",
        gpu.backend_name(),
        gpu.info.name,
        size.0,
        size.1,
        host.key_bits,
        host.raster
    );
    for draw in [true, false] {
        r.splats.draw_enabled = draw;
        let mut sums: Vec<(&'static str, f64)> = Vec::new();
        let mut wall = Vec::new();
        let mut visible = 0u64;
        let mut quad_pixels = 0u64;
        let warm = 20;
        for i in 0..warm + frames {
            let t = std::time::Instant::now();
            host.update(&mut r, f64::from(i) / 60.0);
            let stats = r.render(&view, f64::from(i) / 60.0);
            let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
            if i >= warm {
                wall.push(t.elapsed().as_secs_f64() * 1000.0);
                for (l, ms) in &stats.passes {
                    match sums.iter_mut().find(|(k, _)| k == l) {
                        Some(e) => e.1 += f64::from(*ms),
                        None => sums.push((l, f64::from(*ms))),
                    }
                }
                visible += u64::from(r.splats.stats.visible.unwrap_or(0));
                quad_pixels += r.splats.stats.quad_pixels.unwrap_or(0);
            }
        }
        let n = f64::from(frames);
        wall.sort_by(f64::total_cmp);
        let gpu_total: f64 = sums.iter().map(|(_, s)| s / n).sum();
        println!(
            "draw {}: frame (submit to idle) mean {:.2} ms, p50 {:.2} ms; GPU {:.2} ms; {} submitted, {:.0} visible, \
             {:.1} M quad pixels ({:.1}x the screen) on average",
            if draw { "on" } else { "off" },
            wall.iter().sum::<f64>() / n,
            wall[wall.len() / 2],
            gpu_total,
            r.splats.stats.submitted,
            visible as f64 / n,
            quad_pixels as f64 / n / 1e6,
            quad_pixels as f64 / n / f64::from(size.0 * size.1)
        );
        for (l, s) in &sums {
            println!("  {l:<18} {:.3} ms", s / n);
        }
        let st = &r.splats.stats;
        if let Some(p) = st.tile_pairs {
            println!(
                "  tile pairs {p} ({:.2} per visible splat), capacity {}, dropped {}; splats \
                 left out {}; frames that dropped {}; splat GPU buffers {:.0} MB",
                p as f64 / f64::from(st.visible.unwrap_or(1).max(1)),
                st.pair_capacity,
                st.tile_dropped,
                st.tile_splats_dropped,
                st.tile_drop_frames,
                st.gpu_bytes as f64 / 1e6
            );
            if draw {
                // The raster's work, counted after the timed frames (counting costs time).
                r.splats.count_tests = true;
                let t0 = f64::from(warm + frames) / 60.0;
                for i in 0..6 {
                    host.update(&mut r, t0 + f64::from(i) / 60.0);
                    let _ = r.render(&view, t0 + f64::from(i) / 60.0);
                    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
                }
                r.splats.count_tests = false;
                let st = &r.splats.stats;
                if let (Some(v), Some(p)) = (st.tile_visits, st.tile_pairs) {
                    println!(
                        "  raster tests {v} (pixel, splat) pairs: {:.1} per pixel, {:.1} per tile \
                         pair",
                        v as f64 / f64::from(size.0 * size.1),
                        v as f64 / p.max(1) as f64
                    );
                }
            }
        }
    }
}

fn env_logger_init() {
    struct L;
    impl log::Log for L {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            m.level() <= log::Level::Info
                && !m.target().starts_with("wgpu")
                && !m.target().starts_with("naga")
        }
        fn log(&self, r: &log::Record<'_>) {
            if self.enabled(r.metadata()) {
                eprintln!("[{}] {}", r.level(), r.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: L = L;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}
