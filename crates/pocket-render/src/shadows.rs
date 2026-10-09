//! Cascaded shadow maps for the sun: the camera's frustum split into four depth ranges (the
//! practical split scheme, blending logarithmic and uniform), each covered by a stable
//! orthographic box (a bounding sphere, so the box does not change size as the camera turns, and
//! texel-snapped, so shadows do not shimmer as it moves).

use glam::{Mat4, Vec3, Vec4};

use crate::camera::CameraState;

pub const CASCADES: usize = 4;
pub const SHADOW_SIZE: u32 = 2048;

pub struct Cascades {
    pub view_proj: [Mat4; CASCADES],
    pub splits: [f32; CASCADES],
    /// A texel's width in metres, per cascade (levels of detail drawn into it; lod.rs).
    pub texel: [f32; CASCADES],
}

/// Cascades over `[near, distance]` of the camera, for light travelling along `-to_sun`.
pub fn cascades(cam: &CameraState, aspect: f32, to_sun: Vec3, distance: f32) -> Cascades {
    let near = cam.near.max(0.05);
    let lambda = 0.8;
    let mut splits = [0.0f32; CASCADES];
    for (i, s) in splits.iter_mut().enumerate() {
        let f = (i + 1) as f32 / CASCADES as f32;
        let log = near * (distance / near).powf(f);
        let uni = near + (distance - near) * f;
        *s = lambda * log + (1.0 - lambda) * uni;
    }
    let tan_y = (cam.fov_y * 0.5).tan();
    let tan_x = tan_y * aspect;
    let fwd = cam.forward();
    let right = cam.rotation * Vec3::X;
    let up = cam.rotation * Vec3::Y;
    let light_dir = -to_sun.normalize();
    let light_up = if light_dir.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let mut view_proj = [Mat4::IDENTITY; CASCADES];
    let mut texels = [0.0f32; CASCADES];
    let mut prev = near;
    for (i, &far) in splits.iter().enumerate() {
        // The slice's eight corners, then the sphere around them.
        let mut corners = [Vec3::ZERO; 8];
        for (k, d) in [prev, far].into_iter().enumerate() {
            let c = cam.position + fwd * d;
            let (x, y) = (right * tan_x * d, up * tan_y * d);
            corners[k * 4] = c - x - y;
            corners[k * 4 + 1] = c + x - y;
            corners[k * 4 + 2] = c + x + y;
            corners[k * 4 + 3] = c - x + y;
        }
        let center = corners.iter().copied().sum::<Vec3>() / 8.0;
        let radius = corners
            .iter()
            .map(|c| c.distance(center))
            .fold(0.0f32, f32::max)
            .max(0.5);
        let radius = (radius * 16.0).ceil() / 16.0;
        // Snap the center to the shadow map's texels in light space.
        let light_view = glam::camera::rh::view::look_at_mat4(Vec3::ZERO, light_dir, light_up);
        let texel = 2.0 * radius / SHADOW_SIZE as f32;
        texels[i] = texel;
        let mut lc = light_view.transform_point3(center);
        lc.x = (lc.x / texel).floor() * texel;
        lc.y = (lc.y / texel).floor() * texel;
        let center = light_view.inverse().transform_point3(lc);
        // Casters behind the slice (toward the sun) still cast into it: pull the near plane back.
        let back = radius + 200.0;
        let eye = center - light_dir * back;
        let view = glam::camera::rh::view::look_at_mat4(eye, center, light_up);
        let proj = glam::camera::rh::proj::directx::orthographic(
            -radius,
            radius,
            -radius,
            radius,
            0.0,
            back + radius,
        );
        view_proj[i] = proj * view;
        prev = far;
    }
    Cascades {
        view_proj,
        splits,
        texel: texels,
    }
}

/// The cascade's planes for culling (standard depth 0..1).
pub fn planes(vp: Mat4) -> [Vec4; 6] {
    crate::camera::ortho_planes(vp)
}
