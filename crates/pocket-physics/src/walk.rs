//! Static first-person exploration with Rapier shape sweeps. This state belongs to a presenter,
//! not the authoritative ECS player simulation, snapshot, or replay.

use pocket_sim::math::{max_f32, min_f32};
use rapier3d::control::{CharacterAutostep, CharacterLength, KinematicCharacterController};
use rapier3d::math::{Pose, Vector};
use rapier3d::prelude::{ColliderBuilder, PhysicsWorld, SharedShape, TriMeshFlags};

pub const WALK_EYE_HEIGHT: f32 = 1.65;
const HEIGHT: f32 = 1.8;
const RADIUS: f32 = 0.3;
const SKIN: f32 = 0.02;
const GRAVITY: f32 = 9.81;
const JUMP_SPEED: f32 = 5.0;
const SUBSTEP: f32 = 1.0 / 120.0;

/// An immutable world-space triangle collider with its broad-phase and triangle BVH built once.
/// Construct this off the presentation thread for large imported environments.
pub struct StaticWalkWorld {
    world: PhysicsWorld,
    triangles: usize,
}

impl StaticWalkWorld {
    pub fn new(vertices: Vec<[f32; 3]>, indices: Vec<[u32; 3]>) -> Result<Self, String> {
        if vertices.len() < 3 || indices.is_empty() {
            return Err("walk collision scene needs vertices and triangles".into());
        }
        if vertices
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > 1e6)
        {
            return Err("walk collision vertices must be finite within +/-1e6".into());
        }
        for triangle in &indices {
            for index in triangle {
                if usize::try_from(*index).map_err(|_| "walk vertex index overflow")?
                    >= vertices.len()
                {
                    return Err("walk triangle vertex index out of range".into());
                }
            }
        }
        let triangles = indices.len();
        let collider = ColliderBuilder::trimesh_with_flags(
            vertices.into_iter().map(Vector::from_array).collect(),
            indices,
            // GlTF may contain open/two-sided geometry. Avoid artificial seams in pavement while
            // keeping collision on either side of those triangles.
            TriMeshFlags::FIX_INTERNAL_EDGES_TWO_SIDED | TriMeshFlags::DELETE_DEGENERATE_TRIANGLES,
        )
        .map_err(|e| format!("walk triangle collider: {e:?}"))?
        .build();
        let mut world = PhysicsWorld::new();
        // A standalone collider is fixed. No character body or dynamics are inserted here.
        world.colliders.insert(collider);
        world.detect_collisions(&(), &());
        Ok(Self { world, triangles })
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WalkState {
    pub feet: [f32; 3],
    pub eye: [f32; 3],
    pub grounded: bool,
    pub vertical_speed: f32,
}

/// Capsule camera movement: Y-up gravity, wall sliding, 25 cm stairs and 45-degree slopes.
pub struct WalkController {
    feet: Vector,
    vertical_speed: f32,
    grounded: bool,
    recover_spawn: bool,
    capsule: SharedShape,
    controller: KinematicCharacterController,
}

impl WalkController {
    /// Start at an eye position; collision recovery and grounding happen at the first step.
    pub fn new(eye: [f32; 3]) -> Result<Self, String> {
        if eye.iter().any(|v| !v.is_finite() || v.abs() > 1e6) {
            return Err("walk eye position must be finite within +/-1e6".into());
        }
        Ok(Self {
            feet: Vector::from_array(eye) - Vector::Y * WALK_EYE_HEIGHT,
            vertical_speed: 0.0,
            grounded: false,
            recover_spawn: true,
            capsule: SharedShape::capsule_y(HEIGHT * 0.5 - RADIUS, RADIUS),
            controller: KinematicCharacterController {
                offset: CharacterLength::Absolute(SKIN),
                slide: true,
                autostep: Some(CharacterAutostep {
                    max_height: CharacterLength::Absolute(0.25),
                    min_width: CharacterLength::Absolute(0.15),
                    include_dynamic_bodies: false,
                }),
                max_slope_climb_angle: std::f32::consts::FRAC_PI_4,
                min_slope_slide_angle: std::f32::consts::FRAC_PI_4,
                snap_to_ground: Some(CharacterLength::Absolute(0.2)),
                ..KinematicCharacterController::default()
            },
        })
    }

    pub fn state(&self) -> WalkState {
        WalkState {
            feet: self.feet.to_array(),
            eye: (self.feet + Vector::Y * WALK_EYE_HEIGHT).to_array(),
            grounded: self.grounded,
            vertical_speed: self.vertical_speed,
        }
    }

    /// Desired x/z velocity is in metres/second; input Y is ignored. Jump is a key-edge request.
    /// Elapsed time is capped at 100 ms and split into at most 1/120 s shape sweeps, so a stalled
    /// frame cannot teleport the camera. Sweeps also block thin walls at fast horizontal speeds.
    pub fn step(
        &mut self,
        world: &StaticWalkWorld,
        dt: f32,
        desired_horizontal_velocity: [f32; 3],
        jump_request: bool,
    ) -> Result<WalkState, String> {
        if !dt.is_finite() || dt < 0.0 || desired_horizontal_velocity.iter().any(|v| !v.is_finite())
        {
            return Err("walk dt and velocity must be finite; dt must be nonnegative".into());
        }
        let queries = world.world.query_pipeline();
        if self.recover_spawn {
            let recovery = self.controller.move_shape(
                0.0,
                &queries,
                self.capsule.as_ref(),
                &Pose::from_translation(self.feet + Vector::Y * (HEIGHT * 0.5)),
                Vector::ZERO,
                |_| {},
            );
            self.feet += recovery.translation;
            self.grounded = recovery.grounded;
            self.recover_spawn = false;
        }
        if dt == 0.0 {
            return Ok(self.state());
        }
        if jump_request && self.grounded {
            self.vertical_speed = JUMP_SPEED;
            self.grounded = false;
        }
        let mut remaining = min_f32(dt, 0.1);
        while remaining > 1e-7 {
            let step_dt = min_f32(remaining, SUBSTEP);
            remaining -= step_dt;
            self.vertical_speed = max_f32(self.vertical_speed - GRAVITY * step_dt, -50.0);
            let desired = Vector::new(
                desired_horizontal_velocity[0],
                self.vertical_speed,
                desired_horizontal_velocity[2],
            ) * step_dt;
            let movement = self.controller.move_shape(
                step_dt,
                &queries,
                self.capsule.as_ref(),
                &Pose::from_translation(self.feet + Vector::Y * (HEIGHT * 0.5)),
                desired,
                |_| {},
            );
            if !movement.translation.is_finite() {
                return Err("walk shape sweep returned nonfinite movement".into());
            }
            self.feet += movement.translation;
            self.grounded = movement.grounded && self.vertical_speed <= 0.0;
            if self.grounded
                || (self.vertical_speed > 0.0 && movement.translation.y + 1e-5 < desired.y)
            {
                self.vertical_speed = 0.0;
            }
        }
        Ok(self.state())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mesh {
        vertices: Vec<[f32; 3]>,
        indices: Vec<[u32; 3]>,
    }
    impl Mesh {
        fn floor() -> Self {
            let mut mesh = Self {
                vertices: Vec::new(),
                indices: Vec::new(),
            };
            mesh.quad([
                [-20.0, 0.0, -20.0],
                [-20.0, 0.0, 20.0],
                [20.0, 0.0, 20.0],
                [20.0, 0.0, -20.0],
            ]);
            mesh
        }
        fn quad(&mut self, vertices: [[f32; 3]; 4]) {
            let index = u32::try_from(self.vertices.len()).unwrap();
            self.vertices.extend(vertices);
            self.indices
                .extend([[index, index + 1, index + 2], [index, index + 2, index + 3]]);
        }
        fn world(self) -> StaticWalkWorld {
            StaticWalkWorld::new(self.vertices, self.indices).unwrap()
        }
        fn wall(&mut self, x: f32) {
            self.quad([
                [x, 0.0, -20.0],
                [x, 3.0, -20.0],
                [x, 3.0, 20.0],
                [x, 0.0, 20.0],
            ]);
        }
        fn step(&mut self, height: f32) {
            self.quad([
                [1.0, 0.0, -3.0],
                [1.0, height, -3.0],
                [1.0, height, 3.0],
                [1.0, 0.0, 3.0],
            ]);
            self.quad([
                [1.0, height, -3.0],
                [1.0, height, 3.0],
                [4.0, height, 3.0],
                [4.0, height, -3.0],
            ]);
        }
    }

    fn settle(world: &StaticWalkWorld, controller: &mut WalkController) -> WalkState {
        for _ in 0..120 {
            controller.step(world, 1.0 / 60.0, [0.0; 3], false).unwrap();
        }
        controller.state()
    }

    #[test]
    fn falls_to_floor_and_recovers_a_slightly_embedded_spawn() {
        let world = Mesh::floor().world();
        let mut controller = WalkController::new([0.0, 4.0, 0.0]).unwrap();
        let state = settle(&world, &mut controller);
        assert!(state.grounded, "{state:?}");
        assert!((state.feet[1] - SKIN).abs() < 0.025, "{state:?}");
        assert!((state.eye[1] - state.feet[1] - WALK_EYE_HEIGHT).abs() < 1e-5);
        let mut embedded = WalkController::new([0.0, WALK_EYE_HEIGHT - 0.1, 0.0]).unwrap();
        let state = settle(&world, &mut embedded);
        assert!(state.grounded && state.feet[1] >= -0.005, "{state:?}");
    }

    #[test]
    fn fast_movement_cannot_cross_thin_wall_and_diagonal_input_slides() {
        let mut mesh = Mesh::floor();
        mesh.wall(2.0);
        let world = mesh.world();
        let mut controller = WalkController::new([0.0, WALK_EYE_HEIGHT + SKIN, 0.0]).unwrap();
        settle(&world, &mut controller);
        let state = controller
            .step(&world, 0.1, [100.0, 0.0, 0.0], false)
            .unwrap();
        assert!(state.feet[0] > 1.5 && state.feet[0] < 1.72, "{state:?}");
        for _ in 0..60 {
            controller
                .step(&world, 1.0 / 60.0, [3.0, 0.0, 3.0], false)
                .unwrap();
        }
        let state = controller.state();
        assert!(state.feet[0] < 1.72 && state.feet[2] > 2.5, "{state:?}");
    }

    #[test]
    fn climbs_short_stair_but_rejects_tall_obstacle() {
        for (height, should_climb) in [(0.2, true), (0.6, false)] {
            let mut mesh = Mesh::floor();
            mesh.step(height);
            let world = mesh.world();
            let mut controller = WalkController::new([0.0, WALK_EYE_HEIGHT + SKIN, 0.0]).unwrap();
            settle(&world, &mut controller);
            for _ in 0..75 {
                controller
                    .step(&world, 1.0 / 60.0, [2.0, 0.0, 0.0], false)
                    .unwrap();
            }
            let state = controller.state();
            if should_climb {
                assert!(state.feet[0] > 2.0 && state.feet[1] > 0.18, "{state:?}");
            } else {
                assert!(state.feet[0] < 1.0 && state.feet[1] < 0.1, "{state:?}");
            }
        }
    }

    #[test]
    fn jump_has_gravity_and_lands_without_midair_jump() {
        let world = Mesh::floor().world();
        let mut controller = WalkController::new([0.0, WALK_EYE_HEIGHT + SKIN, 0.0]).unwrap();
        let start = settle(&world, &mut controller);
        let first = controller.step(&world, 1.0 / 60.0, [0.0; 3], true).unwrap();
        assert!(
            !first.grounded && first.eye[1] > start.eye[1] && first.vertical_speed > 4.7,
            "{first:?}"
        );
        let midair = controller.step(&world, 1.0 / 60.0, [0.0; 3], true).unwrap();
        assert!(midair.vertical_speed < first.vertical_speed);
        let mut maximum = midair.eye[1];
        for _ in 0..120 {
            let state = controller
                .step(&world, 1.0 / 60.0, [0.0; 3], false)
                .unwrap();
            maximum = max_f32(maximum, state.eye[1]);
        }
        assert!(maximum - start.eye[1] > 1.1 && maximum - start.eye[1] < 1.4);
        assert!(controller.state().grounded);
        assert!((controller.state().eye[1] - start.eye[1]).abs() < 0.03);
    }

    #[test]
    fn validates_inputs_and_bounds_paused_frame_time() {
        assert!(StaticWalkWorld::new(vec![[0.0; 3]; 3], vec![[0, 1, 3]]).is_err());
        assert!(WalkController::new([f32::NAN; 3]).is_err());
        let world = Mesh::floor().world();
        let mut controller = WalkController::new([0.0, WALK_EYE_HEIGHT + SKIN, 0.0]).unwrap();
        settle(&world, &mut controller);
        assert!(controller.step(&world, f32::NAN, [0.0; 3], false).is_err());
        assert!(
            controller
                .step(&world, 0.01, [f32::INFINITY, 0.0, 0.0], false)
                .is_err()
        );
        let state = controller
            .step(&world, 10.0, [3.0, 50.0, 0.0], false)
            .unwrap();
        // Contact resolution may shorten travel at the floor seam; elapsed time still bounds it.
        assert!(state.feet[0] > 0.25 && state.feet[0] <= 0.301, "{state:?}");
        assert!(state.grounded, "input Y must not fly: {state:?}");
    }
}
