//! The benchmark scenes, built to match Jolt's PerformanceTest scenes (JoltPhysics/PerformanceTest,
//! read at commit 5830c342) as closely as Rapier allows. Differences are listed in
//! docs/bench/physics.md.

use rapier3d::prelude::*;

/// Jolt's defaults that Rapier does not share (PhysicsSettings.h, BodyCreationSettings.h): every
/// body is damped 0.05, shapes weigh 1,000 kg per cubic metre, friction 0.2.
pub const JOLT_DAMPING: f32 = 0.05;
pub const JOLT_DENSITY: f32 = 1000.0;
pub const JOLT_FRICTION: f32 = 0.2;

fn dynamic_at(p: Vector) -> RigidBodyBuilder {
    RigidBodyBuilder::dynamic()
        .translation(p)
        .linear_damping(JOLT_DAMPING)
        .angular_damping(JOLT_DAMPING)
}

/// Jolt's PyramidScene with `height` layers: a 100 x 2 x 100 floor and boxes of 2 m with no convex
/// radius, each layer 0.5 m above the one below and shifted by half a box on odd layers, never
/// sleeping. 15 layers make 1,240 boxes (Jolt's scene), 30 make 9,455.
pub fn pyramid(world: &mut PhysicsWorld, height: i32) {
    world.insert(
        RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0, 0.0)),
        ColliderBuilder::cuboid(50.0, 1.0, 50.0).friction(JOLT_FRICTION),
    );
    let size = 2.0f32;
    let separation = 0.5f32;
    let half = 0.5 * size;
    let h = height;
    for i in 0..h {
        for j in i / 2..h - (i + 1) / 2 {
            for k in i / 2..h - (i + 1) / 2 {
                let shift = if i & 1 == 1 { half } else { 0.0 };
                let p = Vector::new(
                    -h as f32 + size * j as f32 + shift,
                    1.0 + (size + separation) * i as f32,
                    -h as f32 + size * k as f32 + shift,
                );
                world.insert(
                    dynamic_at(p).can_sleep(false),
                    ColliderBuilder::cuboid(half, half, half)
                        .density(JOLT_DENSITY)
                        .friction(JOLT_FRICTION),
                );
            }
        }
    }
}

/// Jolt's ConvexVsMeshScene: a 100 x 100 cell sine terrain (20,000 triangles, 3 m cells, 5 m high)
/// and 21 x 4 x 21 = 1,764 bodies dropped on it, a box, a sphere, a capsule and a square pyramid
/// hull in turn; friction 0.5 and restitution 0.6 everywhere, sleeping allowed.
pub fn convex_vs_mesh(world: &mut PhysicsWorld) {
    let n = 100usize;
    let cell = 3.0f32;
    let max_height = 5.0f32;
    let center = n as f32 * cell / 2.0;
    let mut vertices = vec![Vector::ZERO; (n + 1) * (n + 1)];
    for x in 0..=n {
        for z in 0..=n {
            let height = libm::sinf(x as f32 * 50.0 / n as f32) * libm::cosf(z as f32 * 50.0 / n as f32);
            vertices[z * (n + 1) + x] = Vector::new(cell * x as f32, max_height * height, cell * z as f32);
        }
    }
    let mut indices = Vec::with_capacity(n * n * 2);
    for x in 0..n {
        for z in 0..n {
            let start = ((n + 1) * z + x) as u32;
            let n = n as u32;
            indices.push([start, start + n + 1, start + 1]);
            indices.push([start + 1, start + n + 1, start + n + 2]);
        }
    }
    let terrain = ColliderBuilder::trimesh_with_flags(vertices, indices, TriMeshFlags::FIX_INTERNAL_EDGES)
        .expect("terrain mesh")
        .friction(0.5)
        .restitution(0.6);
    world.insert(
        RigidBodyBuilder::fixed().translation(Vector::new(-center, max_height, -center)),
        terrain,
    );

    let hull = [
        Vector::new(0.0, 1.0, 0.0),
        Vector::new(1.0, 0.0, 0.0),
        Vector::new(-1.0, 0.0, 0.0),
        Vector::new(0.0, 0.0, 1.0),
        Vector::new(0.0, 0.0, -1.0),
    ];
    let shapes: [SharedShape; 4] = [
        SharedShape::cuboid(0.5, 0.75, 1.0),
        SharedShape::ball(0.5),
        SharedShape::capsule_y(0.75, 0.5),
        SharedShape::convex_hull(&hull).expect("hull"),
    ];
    for x in -10..=10 {
        for (y, shape) in shapes.iter().enumerate() {
            for z in -10..=10 {
                let p = Vector::new(7.5 * x as f32, 15.0 + 2.0 * y as f32, 7.5 * z as f32);
                world.insert(
                    dynamic_at(p),
                    ColliderBuilder::new(shape.clone())
                        .density(JOLT_DENSITY)
                        .friction(0.5)
                        .restitution(0.6),
                );
            }
        }
    }
}

/// The Raycast scene's 10,000 rays (the Jolt side: bench/physics/jolt/jolt_bench.cpp, MakeRays): a
/// 100 x 100 grid 40 m above the ConvexVsMesh terrain, each ray 100 m long, tilted by a small
/// pattern. The direction is not normalized, so a time of impact is the fraction of the ray, as in
/// Jolt.
pub fn ray_grid() -> Vec<Ray> {
    let mut rays = Vec::with_capacity(10_000);
    for i in 0..100 {
        for j in 0..100 {
            let origin = Vector::new(-148.5 + 3.0 * i as f32, 40.0, -148.5 + 3.0 * j as f32);
            let dir = Vector::new(0.1 * (i % 7 - 3) as f32, -1.0, 0.1 * (j % 5 - 2) as f32);
            rays.push(Ray::new(origin, dir * 100.0));
        }
    }
    rays
}
