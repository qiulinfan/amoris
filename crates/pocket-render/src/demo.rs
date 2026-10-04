//! Built-in demo scenes as render frames: the many_cubes stress test of Bevy, reproduced layout
//! for layout, for like-for-like benchmarks natively, in the browser and against three.js.

use glam::{Quat, Vec3};
use pocket_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};

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
            direction: Vec3::new(0.0, -1.0, -1.0).normalize().to_array(),
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
            fog_density: 0.0,
            fog_color: [0.6, 0.7, 0.8],
            exposure_ev: 0.0,
            bloom: 0.1,
        }),
        sea: None,
        splats: None,
    }
}
