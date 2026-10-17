//! Cascaded shadow maps for the sun: the camera's frustum split into four depth ranges (the
//! practical split scheme, blending logarithmic and uniform), each covered by a stable
//! orthographic box (a bounding sphere, so the box does not change size as the camera turns, and
//! texel-snapped, so shadows do not shimmer as it moves).
//!
//! A caster is drawn into a cascade when its sphere is in the cascade's box and, with
//! [`ShadowCull::On`] (the default), in the volume where it can change what the cascade's receivers
//! read: the camera slice the cascade shades, extruded toward the sun ([`slice_planes`],
//! docs/spec/occlusion.md 11).

use glam::{Mat4, Vec2, Vec3, Vec4};

use crate::camera::CameraState;

pub const CASCADES: usize = 4;
pub const SHADOW_SIZE: u32 = 2048;
/// Planes per cascade in the culling pass's slice test: up to eight sides (the hull of eight
/// corners) and the far plane.
pub const SLICE_PLANES: usize = 9;
/// A plane every sphere passes (`n.c + w >= -r` with `n = 0`, `w = 1`): unused slots, and every
/// slot with the slice test off.
pub const PASS: Vec4 = Vec4::new(0.0, 0.0, 0.0, 1.0);

/// Whether a cascade also culls its casters by the extruded volume of the slice it shades
/// ([`slice_planes`]). The image is the same either way (docs/spec/occlusion.md 11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowCull {
    /// The cascade's box only.
    Off,
    /// The box and the extruded slice (the default).
    On,
}

impl ShadowCull {
    /// `off` or `on` (any case; also `0`/`1`, `false`/`true`).
    pub fn parse(s: &str) -> Option<ShadowCull> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" | "no" => Some(ShadowCull::Off),
            "on" | "1" | "true" | "yes" => Some(ShadowCull::On),
            _ => None,
        }
    }

    /// From `POCKET_SHADOW_CULL` (natively); [`ShadowCull::On`] when it is unset, unknown, or in
    /// the browser.
    pub fn from_env() -> ShadowCull {
        match std::env::var("POCKET_SHADOW_CULL") {
            Ok(v) => ShadowCull::parse(&v).unwrap_or_else(|| {
                log::warn!("POCKET_SHADOW_CULL: {v:?} is not off or on; using on");
                ShadowCull::On
            }),
            Err(_) => ShadowCull::On,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ShadowCull::Off => "off",
            ShadowCull::On => "on",
        }
    }
}

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

/// Per cascade, planes (inward normals, `n.p + w >= 0` inside) bounding where a caster can change
/// what the cascade's receivers read, for light travelling along `-to_sun`, with the camera drawn
/// at `size` pixels and its projection moved by `jitter_ndc` (TAA's; zero without it).
///
/// `shadow_factor` (forward.wgsl) reads cascade `c` only for fragments whose view depth lies in
/// `(splits[c - 1], splits[c]]` (`[0, splits[0]]` for the first). A fragment's point lies on the
/// ray through its pixel centre: with MSAA a partly covered pixel shades at its centre, the point
/// extrapolated along the triangle's plane, at any depth on that ray, also behind the eye, where
/// `shadow_factor` reads no cascade (a negative view depth is unshadowed). So the receivers of
/// cascade `c` lie in that slice of the camera's frustum. A receiver at `p` with unit normal `n`
/// samples the map at
/// `q = p + n * o`, where `o = 1.5 * splits[c] / 1024` is the shader's normal offset, comparing
/// `q`'s light depth (less a bias) with nine bilinear comparison taps one texel apart: the texel
/// centres it reads lie within two texels of `q` on each axis of the map, so within `2 sqrt(2)`
/// texels of it. A caster changes what a receiver reads only if it covers one of those texel
/// centres (its triangles, inside its sphere, write the texels whose centres they cover) at a
/// depth nearer the sun than `q`; a caster that covers them only farther away leaves every
/// comparison as the cleared map would (a positive depth bias only pushes it farther). So a caster
/// can matter only if its sphere reaches within `o + 2 sqrt(2) texel` of the slice sideways (in
/// the plane across the light) and is not entirely farther from the sun than the slice's farthest
/// point plus `o`. Toward the sun the volume stays open (the cascade's box closes it): casters
/// off screen between the slice and the sun still shadow it.
///
/// The slice is the camera's frustum between the two depths, widened by the jitter and one more
/// pixel per side (fragments shade at pixel centres of the jittered projection). Its eight
/// corners, projected across the light, give a convex hull; each hull edge is a plane containing
/// the light direction, placed at the corners' extreme along its normal (so float error in the
/// hull can only widen it), then moved out by the margin `o + 4 texels` (4 > 2 sqrt(2): the rest
/// covers float error in the culling shader) and `1e-5` of the scene's scale; the far plane at
/// the corners' largest light depth gets the same margin. Unused slots are [`PASS`].
pub fn slice_planes(
    cam: &CameraState,
    size: (u32, u32),
    jitter_ndc: Vec2,
    to_sun: Vec3,
    casc: &Cascades,
) -> [[Vec4; SLICE_PLANES]; CASCADES] {
    let light = -to_sun.normalize();
    let (a, b) = light.any_orthonormal_pair();
    // How far the jittered frustum reaches past the unjittered one, in its own units, plus a pixel.
    let kx = 1.0 + jitter_ndc.x.abs() + 2.0 / size.0.max(1) as f32;
    let ky = 1.0 + jitter_ndc.y.abs() + 2.0 / size.1.max(1) as f32;
    let aspect = size.0 as f32 / size.1.max(1) as f32;
    let fwd = cam.forward();
    let right = cam.rotation * Vec3::X;
    let up = cam.rotation * Vec3::Y;
    // The half extents across the view at depth `d`.
    let half = |d: f32| match cam.ortho_height {
        Some(h) => Vec2::new(h * 0.5 * aspect * kx, h * 0.5 * ky),
        None => {
            let ty = (cam.fov_y * 0.5).tan();
            Vec2::new(ty * aspect * kx, ty * ky) * d
        }
    };
    let mut out = [[PASS; SLICE_PLANES]; CASCADES];
    let mut prev = 0.0f32;
    for (c, planes) in out.iter_mut().enumerate() {
        let far = casc.splits[c];
        let mut corners = [Vec3::ZERO; 8];
        for (k, d) in [prev, far].into_iter().enumerate() {
            let e = half(d);
            let centre = cam.position + fwd * d;
            for (j, (sx, sy)) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .into_iter()
                .enumerate()
            {
                corners[k * 4 + j] = centre + right * (sx * e.x) + up * (sy * e.y);
            }
        }
        prev = far;
        let scale = cam.position.abs().max_element() + far + 1.0;
        let margin = 1.5 * far / 1024.0 + 4.0 * casc.texel[c] + 1e-5 * scale;
        let flat = corners.map(|p| Vec2::new(p.dot(a), p.dot(b)));
        let hull = convex_hull(&flat);
        let mut n = 0;
        if hull.len() >= 3 {
            for (i, &from) in hull.iter().enumerate() {
                let e = hull[(i + 1) % hull.len()] - from;
                let len = e.length();
                if len <= 1e-6 * scale {
                    continue;
                }
                // Counter-clockwise: the inside is to the left of each edge.
                let inward = Vec2::new(-e.y, e.x) / len;
                let normal = a * inward.x + b * inward.y;
                let lowest = corners
                    .iter()
                    .map(|p| normal.dot(*p))
                    .fold(f32::INFINITY, f32::min);
                planes[n] = normal.extend(margin - lowest);
                n += 1;
            }
        }
        let deepest = corners
            .iter()
            .map(|p| light.dot(*p))
            .fold(f32::NEG_INFINITY, f32::max);
        planes[n] = (-light).extend(deepest + margin);
    }
    out
}

/// The convex hull of `points`, counter-clockwise, without collinear points (Andrew's monotone
/// chain); fewer than three points come back as they are.
fn convex_hull(points: &[Vec2]) -> Vec<Vec2> {
    let mut p = points.to_vec();
    p.sort_by(|u, v| u.x.total_cmp(&v.x).then(u.y.total_cmp(&v.y)));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let turns_left = |o: Vec2, u: Vec2, v: Vec2| (u - o).perp_dot(v - o) > 0.0;
    let mut hull: Vec<Vec2> = Vec::with_capacity(p.len() + 1);
    // The lower chain left to right, then the upper chain right to left.
    for chain in [p.clone(), p.iter().rev().copied().collect()] {
        let start = hull.len();
        for q in chain {
            while hull.len() >= start + 2
                && !turns_left(hull[hull.len() - 2], hull[hull.len() - 1], q)
            {
                hull.pop();
            }
            hull.push(q);
        }
        // The chain's last point starts the next one.
        hull.pop();
    }
    hull
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;

    /// A small deterministic generator (xorshift).
    struct Rng(u32);

    impl Rng {
        fn unit(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            (self.0 >> 8) as f32 / (1u32 << 24) as f32
        }

        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.unit()
        }

        fn direction(&mut self) -> Vec3 {
            loop {
                let v = Vec3::new(
                    self.range(-1.0, 1.0),
                    self.range(-1.0, 1.0),
                    self.range(-1.0, 1.0),
                );
                if v.length_squared() > 1e-3 && v.length_squared() <= 1.0 {
                    return v.normalize();
                }
            }
        }
    }

    fn keeps(planes: &[Vec4], c: Vec3, r: f32) -> bool {
        planes.iter().all(|p| p.truncate().dot(c) + p.w >= -r)
    }

    /// A random camera, output size, jitter and sun (vertical and grazing suns included).
    fn setup(rng: &mut Rng, case: u32) -> (CameraState, (u32, u32), Vec2, Vec3) {
        let offset = if case.is_multiple_of(7) { 3000.0 } else { 60.0 };
        let eye = Vec3::new(
            rng.range(-offset, offset),
            rng.range(0.5, 40.0),
            rng.range(-offset, offset),
        );
        let mut cam = CameraState::look_at(eye, eye + rng.direction());
        cam.rotation =
            Quat::from_axis_angle(rng.direction(), rng.range(0.0, std::f32::consts::TAU))
                .normalize();
        cam.fov_y = rng.range(15.0, 110.0).to_radians();
        cam.near = rng.range(0.01, 1.0);
        if case.is_multiple_of(5) {
            cam.ortho_height = Some(rng.range(2.0, 80.0));
        }
        let sizes = [(1280, 720), (64, 48), (1920, 1080), (300, 900), (1, 1)];
        let size = sizes[(case as usize) % sizes.len()];
        // Up to two pixels either way (TAA's amplitude is half a pixel by default; tuning scales
        // it).
        let jitter = Vec2::new(
            rng.range(-4.0, 4.0) / size.0 as f32,
            rng.range(-4.0, 4.0) / size.1 as f32,
        );
        let to_sun = match case % 9 {
            0 => Vec3::Y,
            1 => Vec3::new(1e-4, 1.0, 0.0).normalize(),
            2 => Vec3::new(1.0, 0.02, 0.3).normalize(),
            _ => rng.direction(),
        };
        (cam, size, jitter, to_sun)
    }

    /// Every caster sphere that holds a point some receiver of a cascade reads passes that
    /// cascade's planes: receivers in the cascade's slice of the jittered view (from the eye on for
    /// the first: MSAA's extrapolated points can lie nearer than the near plane), their sample
    /// point moved by the normal offset along a unit normal, then within the PCF taps' reach
    /// across the light (`2 sqrt(2)` texels). Half the receivers are anywhere, with random normals,
    /// reaches and spheres; half are the worst case for one of the planes: a corner of the slice
    /// (its pixel centre at the jittered view's edge), the normal and the reach pointing straight
    /// out of that plane, the sphere beyond it touching the point read.
    #[test]
    fn every_caster_a_receiver_can_read_is_kept() {
        let mut rng = Rng(0x9e37_79b9);
        let mut checked = 0;
        for case in 0..400 {
            let (cam, size, jitter, to_sun) = setup(&mut rng, case);
            let aspect = size.0 as f32 / size.1 as f32;
            let casc = cascades(&cam, aspect, to_sun, 150.0);
            let planes = slice_planes(&cam, size, jitter, to_sun, &casc);
            let light = -to_sun.normalize();
            let (fwd, right, up) = (
                cam.forward(),
                cam.rotation * Vec3::X,
                cam.rotation * Vec3::Y,
            );
            let ty = (cam.fov_y * 0.5).tan();
            // The first slice's receivers start at the eye (`shadow_factor` reads no cascade at a
            // negative view depth).
            let mut prev = 0.0;
            for (c, (kept, &far)) in planes.iter().zip(&casc.splits).enumerate() {
                let live: Vec<Vec4> = kept.iter().copied().filter(|p| *p != PASS).collect();
                for k in 0..200 {
                    let worst = k % 2 == 1;
                    let pick = |rng: &mut Rng, lo: f32, hi: f32| {
                        if !worst {
                            rng.range(lo, hi)
                        } else if rng.unit() < 0.5 {
                            lo
                        } else {
                            hi
                        }
                    };
                    let d = pick(&mut rng, prev, far);
                    // A pixel centre of the jittered projection, in unjittered device coordinates.
                    let (ex, ey) = (1.0 - 1.0 / size.0 as f32, 1.0 - 1.0 / size.1 as f32);
                    let x = pick(&mut rng, -ex, ex) - jitter.x;
                    let y = pick(&mut rng, -ey, ey) - jitter.y;
                    let (hx, hy) = match cam.ortho_height {
                        Some(h) => (h * 0.5 * aspect, h * 0.5),
                        None => (ty * aspect * d, ty * d),
                    };
                    let p = cam.position + fwd * d + right * (x * hx) + up * (y * hy);
                    // Out of one plane (worst), or any way.
                    let out = if worst {
                        -live[(rng.unit() * live.len() as f32) as usize % live.len()].truncate()
                    } else {
                        rng.direction()
                    };
                    let normal = if worst { out } else { rng.direction() };
                    let q = p + normal * (1.5 * far / 1024.0);
                    let across = out.reject_from(light).normalize_or_zero();
                    let reach = 2.0f32.sqrt() * 2.0 * casc.texel[c];
                    let read = q + across * if worst { reach } else { rng.range(0.0, reach) };
                    let r = rng.range(0.0, 3.0);
                    let centre = read + out * r * if worst { 1.0 } else { rng.unit() };
                    assert!(
                        keeps(kept, centre, r),
                        "case {case}, cascade {c}: a caster at {centre} (r {r}) reaching {read} \
                         (receiver {p}) is culled; planes {:?}",
                        kept
                    );
                    checked += 1;
                }
                prev = far;
            }
        }
        assert_eq!(checked, 400 * 4 * 200);
    }

    /// The slice culls casters the cascade's box keeps: over spheres spread through each box, the
    /// share culled by the slice planes, at a head-height camera looking along a field (the LOD
    /// field's view, docs/bench/lod.md).
    #[test]
    fn the_slice_culls_inside_the_box() {
        let eye = Vec3::new(2.0, 2.2, 1.0);
        let cam = CameraState::look_at(eye, eye + Vec3::new(1.0, -0.15, 1.0));
        let to_sun = -Vec3::new(-0.5, -0.8, -0.3).normalize();
        let casc = cascades(&cam, 16.0 / 9.0, to_sun, 150.0);
        let planes = slice_planes(&cam, (1280, 720), Vec2::ZERO, to_sun, &casc);
        let mut shares = [0.0f32; CASCADES];
        for c in 0..CASCADES {
            let boxp = super::planes(casc.view_proj[c]);
            let inv = casc.view_proj[c].inverse();
            let (mut inside, mut kept) = (0, 0);
            for i in 0..40 {
                for j in 0..40 {
                    for k in 0..40 {
                        let ndc = Vec3::new(
                            (i as f32 + 0.5) / 20.0 - 1.0,
                            (j as f32 + 0.5) / 20.0 - 1.0,
                            (k as f32 + 0.5) / 40.0,
                        );
                        let p = inv.project_point3(ndc);
                        let r = 0.5;
                        if !keeps(&boxp, p, r) {
                            continue;
                        }
                        inside += 1;
                        kept += usize::from(keeps(&planes[c], p, r));
                    }
                }
            }
            shares[c] = 1.0 - kept as f32 / inside as f32;
        }
        eprintln!("share of the box culled by the slice, per cascade: {shares:?}");
        assert!(shares.iter().all(|&s| s > 0.3), "{shares:?}");
    }

    #[test]
    fn hull_drops_inner_and_collinear_points() {
        let pts = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(0.0, 2.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 0.0),
        ];
        let h = convex_hull(&pts);
        assert_eq!(
            h,
            vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(2.0, 0.0),
                Vec2::new(2.0, 2.0),
                Vec2::new(0.0, 2.0)
            ]
        );
    }

    #[test]
    fn modes_parse() {
        assert_eq!(ShadowCull::parse(" OFF "), Some(ShadowCull::Off));
        assert_eq!(ShadowCull::parse("1"), Some(ShadowCull::On));
        assert_eq!(ShadowCull::parse("auto"), None);
        assert_eq!(ShadowCull::On.name(), "on");
    }
}
