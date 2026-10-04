//! Cameras: a pose and a lens, turned into reversed-Z infinite-far matrices (depth 1 at the near
//! plane, 0 at infinity: float precision where it matters, no far plane to clip the ocean), and the
//! frustum planes the culling pass tests.

use glam::{Mat4, Quat, Vec3, Vec4};

/// What a view is drawn from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraState {
    pub position: Vec3,
    pub rotation: Quat,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    pub near: f32,
    /// Exposure compensation, stops.
    pub exposure_ev: f32,
}

impl Default for CameraState {
    fn default() -> Self {
        CameraState::look_at(Vec3::new(8.0, 6.0, 12.0), Vec3::ZERO)
    }
}

impl CameraState {
    pub fn look_at(eye: Vec3, target: Vec3) -> CameraState {
        let view = glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y);
        let rotation = Quat::from_mat4(&view.inverse());
        CameraState {
            position: eye,
            rotation: rotation.normalize(),
            fov_y: 60f32.to_radians(),
            near: 0.1,
            exposure_ev: 0.0,
        }
    }

    pub fn forward(&self) -> Vec3 {
        self.rotation * -Vec3::Z
    }

    pub fn view(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.rotation, self.position).inverse()
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        glam::camera::rh::proj::directx::perspective_infinite_reverse(self.fov_y, aspect, self.near)
    }
}

/// The six planes of a view-projection (inward normals, `n.p + d >= 0` inside), normalized. With
/// an infinite reversed-Z projection the far plane is degenerate and is replaced by one that
/// accepts everything.
pub fn frustum_planes(view_proj: Mat4, infinite: bool) -> [Vec4; 6] {
    let r0 = view_proj.row(0);
    let r1 = view_proj.row(1);
    let r2 = view_proj.row(2);
    let r3 = view_proj.row(3);
    let mut planes = [
        r3 + r0, // left
        r3 - r0, // right
        r3 + r1, // bottom
        r3 - r1, // top
        r3 - r2, // near (reversed Z: z <= w)
        r2,      // far (reversed Z: z >= 0)
    ];
    if infinite {
        planes[5] = Vec4::new(0.0, 0.0, 0.0, 1.0);
    }
    for p in &mut planes {
        let l = p.truncate().length();
        if l > 0.0 {
            *p /= l;
        }
    }
    planes
}

/// The planes of an orthographic shadow cascade (standard depth 0..1).
pub fn ortho_planes(view_proj: Mat4) -> [Vec4; 6] {
    let r0 = view_proj.row(0);
    let r1 = view_proj.row(1);
    let r2 = view_proj.row(2);
    let r3 = view_proj.row(3);
    let mut planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2];
    for p in &mut planes {
        let l = p.truncate().length();
        if l > 0.0 {
            *p /= l;
        }
    }
    planes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_point_ahead_is_inside_and_behind_is_outside() {
        let c = CameraState::look_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO);
        let vp = c.proj(1.5) * c.view();
        let planes = frustum_planes(vp, true);
        let inside = |p: Vec3| planes.iter().all(|pl| pl.truncate().dot(p) + pl.w >= 0.0);
        assert!(inside(Vec3::ZERO));
        assert!(inside(Vec3::new(0.0, 0.0, -10000.0)));
        assert!(!inside(Vec3::new(0.0, 0.0, 20.0)));
        assert!(!inside(Vec3::new(100.0, 0.0, 0.0)));
    }
}
