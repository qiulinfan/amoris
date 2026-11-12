//! Shadow maps (docs/spec/shadows.md): the sun's cascades and spot lights' local views.
//!
//! Cascaded shadow maps for the sun: the camera's frustum split into four depth ranges (the
//! practical split scheme, blending logarithmic and uniform), each covered by a stable
//! orthographic box (a bounding sphere, so the box does not change size as the camera turns, and
//! texel-snapped, so shadows do not shimmer as it moves).
//!
//! A caster is drawn into a cascade when its sphere is in the cascade's box and, with
//! [`ShadowCull::On`] (the default), in the volume where it can change what the cascade's receivers
//! read: the camera slice the cascade shades, extruded toward the sun ([`slice_planes`],
//! docs/spec/occlusion.md 11). A receiver under a water surface reads the cascades up the sun's
//! refracted path, at most [`underwater_reach`] from itself, so with water both the box and that
//! volume hold the slice swept along the reach.
//!
//! A spot light whose `shadows` is set casts through a local view: one perspective shadow map
//! along its cone ([`spot_view`]), culled and drawn as a cascade is, in a layer of the cascades'
//! array after theirs. A frame draws at most [`LOCAL_VIEWS`] of them, chosen by [`local_views`]
//! from the lights and the camera alone; the rest are counted, never dropped silently.

use amoris_assets::frame::{LightKindView, LightView};
use glam::{Mat4, Vec2, Vec3, Vec4};

use crate::camera::CameraState;

pub const CASCADES: usize = 4;
pub const SHADOW_SIZE: u32 = 2048;
/// The spot lights a frame can shadow: the local views after the camera's and the cascades' in the
/// culling pass (cull.wgsl), the layers after the cascades' in the shadow array.
pub const LOCAL_VIEWS: usize = 4;
/// A local view's near plane, metres from the light (half the range for a shorter one).
const LOCAL_NEAR_M: f32 = 0.05;
/// The widest half angle a local view's map covers, degrees: a wider cone is shadowed inside it
/// only.
const LOCAL_MAX_HALF_DEG: f32 = 80.0;
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

    /// From `AMORIS_SHADOW_CULL` (natively); [`ShadowCull::On`] when it is unset, unknown, or in
    /// the browser.
    pub fn from_env() -> ShadowCull {
        match std::env::var("AMORIS_SHADOW_CULL") {
            Ok(v) => ShadowCull::parse(&v).unwrap_or_else(|| {
                log::warn!("AMORIS_SHADOW_CULL: {v:?} is not off or on; using on");
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

/// Whether spot lights with `shadows` set cast through local views. Off draws the frame, the
/// lists and the shadow array of a scene whose spots all have `shadows` off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalShadows {
    /// No local view: spot lights are unshadowed.
    Off,
    /// A local view per shadowed spot in view, at most [`LOCAL_VIEWS`] (the default).
    On,
}

impl LocalShadows {
    /// `off` or `on` (any case; also `0`/`1`, `false`/`true`).
    pub fn parse(s: &str) -> Option<LocalShadows> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" | "no" => Some(LocalShadows::Off),
            "on" | "1" | "true" | "yes" => Some(LocalShadows::On),
            _ => None,
        }
    }

    /// From `AMORIS_LOCAL_SHADOWS` (natively); [`LocalShadows::On`] when it is unset, unknown, or
    /// in the browser.
    pub fn from_env() -> LocalShadows {
        match std::env::var("AMORIS_LOCAL_SHADOWS") {
            Ok(v) => LocalShadows::parse(&v).unwrap_or_else(|| {
                log::warn!("AMORIS_LOCAL_SHADOWS: {v:?} is not off or on; using on");
                LocalShadows::On
            }),
            Err(_) => LocalShadows::On,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LocalShadows::Off => "off",
            LocalShadows::On => "on",
        }
    }
}

/// Whether a light asks for a local view: a spot with `shadows` set. The lists and the shadow
/// array are sized by the lights this counts, so nothing a light animates (its intensity, pose or
/// cone) lays them out again.
pub fn casts_locally(l: &LightView) -> bool {
    l.kind == LightKindView::Spot && l.shadows
}

/// A sphere holding a spot light's lit sector: the points within `range` of `pos` and within
/// `half_rad` of the unit axis `dir`.
pub fn cone_sphere(pos: Vec3, dir: Vec3, range: f32, half_rad: f32) -> (Vec3, f32) {
    let (sin, cos) = half_rad.sin_cos();
    if half_rad <= std::f32::consts::FRAC_PI_4 {
        // Through the apex and the rim of the sector's cap.
        let r = range / (2.0 * cos);
        (pos + dir * r, r)
    } else if half_rad <= std::f32::consts::FRAC_PI_2 {
        // The rim's own sphere, which holds the apex from 45 degrees on.
        (pos + dir * (range * cos), range * sin)
    } else {
        (pos, range)
    }
}

/// A spot light's shadow view-projection (perspective, standard depth 0..1) and its map's texel:
/// metres wide per metre of depth from the light. Along the cone, a degree wider than the outer
/// angle (the receivers' sample offsets and filter taps stay inside the map), at most
/// [`LOCAL_MAX_HALF_DEG`] of half angle; from [`LOCAL_NEAR_M`] to the range. `None` for a light
/// that lights nothing or cannot be placed: a field that is not finite, no direction, or a range,
/// outer angle or intensity of 0 or less.
pub fn spot_view(l: &LightView) -> Option<(Mat4, f32)> {
    let (pos, dir) = (Vec3::from(l.position), Vec3::from(l.direction));
    let finite = pos.is_finite()
        && dir.is_finite()
        && l.color.iter().all(|c| c.is_finite())
        && l.intensity.is_finite()
        && l.range.is_finite()
        && l.inner_deg.is_finite()
        && l.outer_deg.is_finite();
    if !finite || l.range <= 0.0 || l.outer_deg <= 0.0 || l.intensity <= 0.0 {
        return None;
    }
    let dir = dir.try_normalize()?;
    // The range the shader lights to (renderer.rs `LightGpu`).
    let range = l.range.max(0.01);
    let half = (l.outer_deg + 1.0)
        .clamp(1.0, LOCAL_MAX_HALF_DEG)
        .to_radians();
    let near = LOCAL_NEAR_M.min(0.5 * range);
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let proj = glam::camera::rh::proj::directx::perspective(2.0 * half, 1.0, near, range);
    let view = glam::camera::rh::view::look_to_mat4(pos, dir, up);
    Some((proj * view, 2.0 * half.tan() / SHADOW_SIZE as f32))
}

/// A spot light's local view this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalView {
    /// The light's entity.
    pub id: u64,
    /// Its row among the lights that are not directional: the lights buffer's order.
    pub light: u32,
    pub view_proj: Mat4,
}

/// A frame's local views, in slot order, and the shadowed spots in view left without one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalViews {
    pub views: Vec<LocalView>,
    pub dropped: u32,
}

/// The local views a frame draws, at most `limit`: of the spots that cast ([`casts_locally`], with
/// a [`spot_view`]) and whose lit sector's sphere ([`cone_sphere`]) meets `planes` (the camera's
/// frustum, inward normals), those largest on screen: by the sphere's radius over its centre's
/// distance from the eye (the radius alone under an orthographic camera), then by entity id. The
/// views come in id order, a view's position its slot. A function of the lights, the camera and
/// the planes only: no frame before this one enters, and the order of `lights` does not either.
pub fn local_views(
    lights: &[LightView],
    cam: &CameraState,
    planes: &[Vec4; 6],
    limit: usize,
) -> LocalViews {
    // (score, the view).
    let mut found: Vec<(f32, LocalView)> = Vec::new();
    let mut row = 0u32;
    for l in lights {
        if l.kind == LightKindView::Directional {
            continue;
        }
        let light = row;
        row += 1;
        if !casts_locally(l) {
            continue;
        }
        let Some((view_proj, _)) = spot_view(l) else {
            continue;
        };
        let (pos, dir) = (Vec3::from(l.position), Vec3::from(l.direction).normalize());
        let (centre, radius) = cone_sphere(pos, dir, l.range.max(0.01), l.outer_deg.to_radians());
        if planes
            .iter()
            .any(|p| p.truncate().dot(centre) + p.w < -radius)
        {
            continue;
        }
        let score = match cam.ortho_height {
            Some(_) => radius,
            None => radius / cam.position.distance(centre).max(1e-3),
        };
        found.push((
            score,
            LocalView {
                id: l.id,
                light,
                view_proj,
            },
        ));
    }
    found.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then(a.1.id.cmp(&b.1.id))
            .then(a.1.light.cmp(&b.1.light))
    });
    let dropped = found.len().saturating_sub(limit) as u32;
    found.truncate(limit);
    let mut views: Vec<LocalView> = found.into_iter().map(|f| f.1).collect();
    views.sort_by(|a, b| a.id.cmp(&b.id).then(a.light.cmp(&b.light)));
    LocalViews { views, dropped }
}

/// How far up the refracted sun a point under water reads the cascades, metres of height
/// (water_caustics_light.wgsl `SUN_REACH_M`).
pub const UNDERWATER_REACH_M: f32 = 4.0;

/// The farthest a receiver under a water surface reads the cascades from itself, for light
/// travelling along `-to_sun`: up the sun refracted by flat water (Snell's law, air over water
/// 1/1.333) for [`UNDERWATER_REACH_M`] of height, or toward the surface if that is nearer
/// (water_caustics_light.wgsl `sun_path`). Zero with the sun overhead or down.
pub fn underwater_reach(to_sun: Vec3) -> Vec3 {
    let l = to_sun.normalize();
    if l.y <= 0.0 {
        return Vec3::ZERO;
    }
    let dir = Vec3::new(
        0.75 * l.x,
        (1.0 - 0.5625 * (1.0 - l.y * l.y)).sqrt(),
        0.75 * l.z,
    );
    dir * (UNDERWATER_REACH_M / dir.y)
}

/// The points a cascade's receivers read around: the slice's corners and, with water, the corners
/// moved by `reach` ([`underwater_reach`]), whose hull holds every point of the slice so moved.
fn swept(corners: &[Vec3; 8], reach: Vec3) -> Vec<Vec3> {
    if reach == Vec3::ZERO {
        return corners.to_vec();
    }
    corners.iter().flat_map(|&p| [p, p + reach]).collect()
}

pub struct Cascades {
    pub view_proj: [Mat4; CASCADES],
    pub splits: [f32; CASCADES],
    /// A texel's width in metres, per cascade (levels of detail drawn into it; lod.rs).
    pub texel: [f32; CASCADES],
}

/// Cascades over `[near, distance]` of the camera, for light travelling along `-to_sun`, each box
/// holding its slice swept along `reach` ([`underwater_reach`] with water, else zero).
pub fn cascades(
    cam: &CameraState,
    aspect: f32,
    to_sun: Vec3,
    distance: f32,
    reach: Vec3,
) -> Cascades {
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
    // Only the reach across the light widens a box; along it, the box is deep enough.
    let across = reach.reject_from(light_dir);
    let light_up = if light_dir.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let mut view_proj = [Mat4::IDENTITY; CASCADES];
    let mut texels = [0.0f32; CASCADES];
    let mut prev = near;
    for (i, &far) in splits.iter().enumerate() {
        // The slice's eight corners (swept along the reach), then the sphere around them.
        let mut corners = [Vec3::ZERO; 8];
        for (k, d) in [prev, far].into_iter().enumerate() {
            let c = cam.position + fwd * d;
            let (x, y) = (right * tan_x * d, up * tan_y * d);
            corners[k * 4] = c - x - y;
            corners[k * 4 + 1] = c + x - y;
            corners[k * 4 + 2] = c + x + y;
            corners[k * 4 + 3] = c - x + y;
        }
        let corners = swept(&corners, across);
        let center = corners.iter().copied().sum::<Vec3>() / corners.len() as f32;
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

/// A shadow view's planes for culling (standard depth 0..1): a cascade's box, or a local view's
/// frustum.
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
///
/// A receiver under water samples around its refracted path's point instead, `p` moved by up to
/// `reach` ([`underwater_reach`]; zero without water) before the normal offset. The hull is then
/// the corners' and the moved corners', the slice swept along the reach: a hexahedron's outline
/// has at most six sides and the sweep adds two. Should float error split a side, the longest
/// eight are kept, which only widens the volume. The moved corners are nearer the sun, so the far
/// plane stays.
pub fn slice_planes(
    cam: &CameraState,
    size: (u32, u32),
    jitter_ndc: Vec2,
    to_sun: Vec3,
    casc: &Cascades,
    reach: Vec3,
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
        let corners = swept(&corners, reach);
        let flat: Vec<Vec2> = corners
            .iter()
            .map(|p| Vec2::new(p.dot(a), p.dot(b)))
            .collect();
        let hull = convex_hull(&flat);
        let mut n = 0;
        if hull.len() >= 3 {
            let mut sides: Vec<(Vec2, f32)> = (0..hull.len())
                .map(|i| {
                    let e = hull[(i + 1) % hull.len()] - hull[i];
                    (e, e.length())
                })
                .filter(|&(_, len)| len > 1e-6 * scale)
                .collect();
            if sides.len() > SLICE_PLANES - 1 {
                sides.sort_by(|x, y| y.1.total_cmp(&x.1));
                sides.truncate(SLICE_PLANES - 1);
            }
            for (e, len) in sides {
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
    use crate::camera::frustum_planes;
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
    /// out of that plane, the sphere beyond it touching the point read. In a third of the cases
    /// the receivers are under water: the point read first moves up the refracted sun, by up to
    /// [`underwater_reach`] (the whole reach in the worst case).
    #[test]
    fn every_caster_a_receiver_can_read_is_kept() {
        let mut rng = Rng(0x9e37_79b9);
        let mut checked = 0;
        for case in 0..400 {
            let (cam, size, jitter, to_sun) = setup(&mut rng, case);
            let aspect = size.0 as f32 / size.1 as f32;
            let reach = if case % 3 == 0 {
                underwater_reach(to_sun)
            } else {
                Vec3::ZERO
            };
            let casc = cascades(&cam, aspect, to_sun, 150.0, reach);
            let planes = slice_planes(&cam, size, jitter, to_sun, &casc, reach);
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
                    let q = p + reach * pick(&mut rng, 0.0, 1.0) + normal * (1.5 * far / 1024.0);
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

    /// With water, each cascade's box holds its slice swept up the refracted sun: every point of
    /// the slice moved by up to [`underwater_reach`], which a receiver under water reads around,
    /// lies in the box (to the texel its centre snaps by), the sweep's far end at the slice's
    /// corners included. The slice is the perspective frustum's, which [`cascades`] fits.
    #[test]
    fn a_cascade_box_holds_its_slice_swept_up_the_refracted_sun() {
        let mut rng = Rng(0x2545_f491);
        for case in 0..300 {
            let (cam, size, _, to_sun) = setup(&mut rng, case);
            let reach = underwater_reach(to_sun);
            if reach == Vec3::ZERO {
                continue;
            }
            let aspect = size.0 as f32 / size.1 as f32;
            let casc = cascades(&cam, aspect, to_sun, 150.0, reach);
            let (fwd, right, up) = (
                cam.forward(),
                cam.rotation * Vec3::X,
                cam.rotation * Vec3::Y,
            );
            let ty = (cam.fov_y * 0.5).tan();
            let scale = cam.position.abs().max_element() + 151.0;
            let mut prev = cam.near.max(0.05);
            for (c, &far) in casc.splits.iter().enumerate() {
                let boxp = planes(casc.view_proj[c]);
                for k in 0..100 {
                    let corner = k % 2 == 0;
                    let pick = |rng: &mut Rng, lo: f32, hi: f32| {
                        if !corner {
                            rng.range(lo, hi)
                        } else if rng.unit() < 0.5 {
                            lo
                        } else {
                            hi
                        }
                    };
                    let d = pick(&mut rng, prev, far);
                    let p = cam.position
                        + fwd * d
                        + right * (pick(&mut rng, -1.0, 1.0) * ty * aspect * d)
                        + up * (pick(&mut rng, -1.0, 1.0) * ty * d);
                    let read = p + reach * if corner { 1.0 } else { rng.unit() };
                    assert!(
                        keeps(&boxp, read, 2.0 * casc.texel[c] + 1e-5 * scale),
                        "case {case}, cascade {c}: {read} (receiver {p}, reach {reach}) is \
                         outside the box"
                    );
                }
                prev = far;
            }
        }
    }

    /// The slice culls casters the cascade's box keeps: over spheres spread through each box, the
    /// share culled by the slice planes, at a head-height camera looking along a field (the LOD
    /// field's view, docs/bench/lod.md).
    #[test]
    fn the_slice_culls_inside_the_box() {
        let eye = Vec3::new(2.0, 2.2, 1.0);
        let cam = CameraState::look_at(eye, eye + Vec3::new(1.0, -0.15, 1.0));
        let to_sun = -Vec3::new(-0.5, -0.8, -0.3).normalize();
        let casc = cascades(&cam, 16.0 / 9.0, to_sun, 150.0, Vec3::ZERO);
        let planes = slice_planes(&cam, (1280, 720), Vec2::ZERO, to_sun, &casc, Vec3::ZERO);
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
        assert_eq!(LocalShadows::parse(" OFF "), Some(LocalShadows::Off));
        assert_eq!(LocalShadows::parse("yes"), Some(LocalShadows::On));
        assert_eq!(LocalShadows::parse("auto"), None);
        assert_eq!(LocalShadows::Off.name(), "off");
    }

    fn spot(id: u64, pos: Vec3, dir: Vec3, range: f32, outer_deg: f32) -> LightView {
        LightView {
            id,
            kind: LightKindView::Spot,
            position: pos.to_array(),
            direction: dir.to_array(),
            color: [1.0; 3],
            intensity: 100.0,
            range,
            inner_deg: outer_deg * 0.5,
            outer_deg,
            shadows: true,
        }
    }

    /// A unit direction within `half` radians of the unit `axis`, at exactly `half` when `rim`.
    fn within(rng: &mut Rng, axis: Vec3, half: f32, rim: bool) -> Vec3 {
        let (a, b) = axis.any_orthonormal_pair();
        let angle = if rim { half } else { rng.range(0.0, half) };
        let turn = rng.range(0.0, std::f32::consts::TAU);
        axis * angle.cos() + (a * turn.cos() + b * turn.sin()) * angle.sin()
    }

    /// Every point a spot lights that its map can hold is inside the map: of the lit sector (the
    /// outer angle, up to 79 degrees, the range) from the near plane on, each point projects into
    /// the view's clip volume with at least two texels to spare on each side, and a sphere holding
    /// it passes the view's culling planes. Vertical and nearly vertical axes included (the view's
    /// up vector changes there). The near plane is flat: toward the rim of a wide cone it cuts the
    /// sector farther from the light than along the axis, and a short range's whole rim.
    #[test]
    fn a_spot_view_holds_its_cone() {
        let mut rng = Rng(0x1234_5678);
        let mut checked = 0;
        for case in 0..300u32 {
            let offset = if case.is_multiple_of(7) { 3000.0 } else { 40.0 };
            let pos = Vec3::new(
                rng.range(-offset, offset),
                rng.range(0.0, 30.0),
                rng.range(-offset, offset),
            );
            let dir = match case % 6 {
                0 => Vec3::NEG_Y,
                1 => Vec3::Y,
                2 => Vec3::new(0.1, -0.995, 0.0).normalize(),
                3 => Vec3::new(0.0, 0.9899, 0.1418).normalize(),
                _ => rng.direction(),
            };
            let range = match case % 5 {
                0 => rng.range(0.02, 0.2),
                _ => rng.range(0.5, 60.0),
            };
            let outer = match case % 4 {
                0 => 79.0,
                1 => rng.range(0.2, 5.0),
                _ => rng.range(5.0, 79.0),
            };
            // The direction's length is the author's: the view normalizes it.
            let l = spot(1, pos, dir * rng.range(0.2, 3.0), range, outer);
            let (vp, texel) = spot_view(&l).expect("a valid spot has a view");
            let half = (outer + 1.0).to_radians();
            assert!((texel - 2.0 * half.tan() / SHADOW_SIZE as f32).abs() <= 1e-6 * texel);
            let near = LOCAL_NEAR_M.min(0.5 * range);
            let planes = planes(vp);
            let scale = pos.abs().max_element() + range + 1.0;
            for k in 0..100 {
                let u = within(&mut rng, dir, outer.to_radians(), k % 2 == 0);
                // From the near plane (along the axis) to the range.
                let lo = near / u.dot(dir);
                if lo * 1.001 > range {
                    continue;
                }
                let d = match k % 5 {
                    0 => range,
                    1 => lo * 1.001,
                    _ => rng.range(lo * 1.001, range),
                };
                let p = pos + u * d;
                let c = vp * p.extend(1.0);
                let slack = 1e-5 * scale;
                assert!(c.w > 0.0, "case {case}: {p} is behind the view");
                let edge = (1.0 - 2.0 * 2.0 / SHADOW_SIZE as f32) * c.w + slack;
                assert!(
                    c.x.abs() <= edge && c.y.abs() <= edge,
                    "case {case} (outer {outer}): {p} is within two texels of the map's edge \
                     ({c})"
                );
                assert!(
                    c.z >= -slack && c.z <= c.w + slack,
                    "case {case}: {p} is outside the map's depth ({c})"
                );
                let r = rng.range(0.0, 2.0);
                let centre = p + rng.direction() * (r * rng.unit());
                assert!(
                    keeps(&planes, centre, r + slack),
                    "case {case}: a caster at {centre} (r {r}) holding the lit point {p} is culled"
                );
                checked += 1;
            }
        }
        assert!(checked > 300 * 80, "{checked} points checked");
    }

    /// The sphere candidates are tested with holds the lit sector, for any outer angle.
    #[test]
    fn the_cone_sphere_holds_the_sector() {
        let mut rng = Rng(0x0bad_cafe);
        for case in 0..240u32 {
            let half = (1.0 + case as f32 * 0.5).to_radians();
            let pos = Vec3::new(rng.range(-50.0, 50.0), rng.range(0.0, 20.0), 3.0);
            let dir = rng.direction();
            let range = rng.range(0.1, 80.0);
            let (centre, radius) = cone_sphere(pos, dir, range, half);
            assert!(
                radius <= range * 1.0001,
                "half {half}: {radius} over {range}"
            );
            for k in 0..200 {
                let u = within(&mut rng, dir, half.min(std::f32::consts::PI), k % 2 == 0);
                let d = match k % 3 {
                    0 => range,
                    1 => 0.0,
                    _ => rng.range(0.0, range),
                };
                let p = pos + u * d;
                assert!(
                    p.distance(centre) <= radius + 1e-4 * (range + pos.length()),
                    "half {} degrees: {p} is {} from {centre}, over {radius}",
                    half.to_degrees(),
                    p.distance(centre)
                );
            }
        }
    }

    /// The frame's choice: among the spots that cast and are in view, the largest on screen keep a
    /// view, equal ones by the lower id, whatever the order of the lights; the slots follow the
    /// ids; what is left over is counted, and nothing else is.
    #[test]
    fn local_views_pick_by_size_on_screen_then_id() {
        let cam = CameraState::look_at(Vec3::ZERO, Vec3::NEG_Z);
        let planes = frustum_planes(cam.proj(16.0 / 9.0) * cam.view(), true);
        // Six spots ahead, shining down, each farther than the last: ids out of order.
        let ids = [40u64, 12, 33, 7, 25, 18];
        let ahead: Vec<LightView> = ids
            .iter()
            .enumerate()
            .map(|(i, &id)| {
                spot(
                    id,
                    Vec3::new(0.0, 3.0, -6.0 - 5.0 * i as f32),
                    Vec3::NEG_Y,
                    6.0,
                    30.0,
                )
            })
            .collect();
        let chosen = local_views(&ahead, &cam, &planes, LOCAL_VIEWS);
        // The four nearest (the largest on screen), slots in id order.
        let got: Vec<u64> = chosen.views.iter().map(|v| v.id).collect();
        assert_eq!(got, [7, 12, 33, 40]);
        assert_eq!(chosen.dropped, 2);
        for v in &chosen.views {
            let row = ids.iter().position(|&id| id == v.id).unwrap();
            assert_eq!(v.light as usize, row);
            assert_eq!(v.view_proj, spot_view(&ahead[row]).unwrap().0);
        }
        // Any order of the lights gives the same views in the same slots.
        for turn in 0..ahead.len() {
            for reversed in [false, true] {
                let mut lights = ahead.clone();
                lights.rotate_left(turn);
                if reversed {
                    lights.reverse();
                }
                let again = local_views(&lights, &cam, &planes, LOCAL_VIEWS);
                assert_eq!(again.dropped, 2);
                let key = |v: &LocalViews| -> Vec<(u64, Mat4)> {
                    v.views.iter().map(|v| (v.id, v.view_proj)).collect()
                };
                assert_eq!(
                    key(&again),
                    key(&chosen),
                    "turned {turn}, reversed {reversed}"
                );
                for v in &again.views {
                    assert_eq!(lights[v.light as usize].id, v.id);
                }
            }
        }
        // A smaller limit keeps the largest; none keeps none and counts them all.
        let two = local_views(&ahead, &cam, &planes, 2);
        assert_eq!(two.views.iter().map(|v| v.id).collect::<Vec<_>>(), [12, 40]);
        assert_eq!(two.dropped, 4);
        let none = local_views(&ahead, &cam, &planes, 0);
        assert!(none.views.is_empty());
        assert_eq!(none.dropped, 6);

        // Equal on screen (mirrored about the view's axis): the lower id keeps its view.
        let twins = [
            spot(9, Vec3::new(4.0, 3.0, -12.0), Vec3::NEG_Y, 6.0, 30.0),
            spot(4, Vec3::new(-4.0, 3.0, -12.0), Vec3::NEG_Y, 6.0, 30.0),
        ];
        for lights in [twins.to_vec(), twins.iter().rev().cloned().collect()] {
            let one = local_views(&lights, &cam, &planes, 1);
            assert_eq!(one.views.len(), 1);
            assert_eq!((one.views[0].id, one.dropped), (4, 1));
        }

        // Not candidates: out of view (behind the camera), a point light, a spot with `shadows`
        // off, and spots that light nothing or cannot be placed. They take no view and are not
        // counted as dropped; the rows count every light that is not directional.
        let good = spot(5, Vec3::new(0.0, 3.0, -8.0), Vec3::NEG_Y, 6.0, 30.0);
        let invalid = [
            LightView {
                range: 0.0,
                ..good.clone()
            },
            LightView {
                direction: [0.0; 3],
                ..good.clone()
            },
            LightView {
                position: [f32::NAN, 0.0, 0.0],
                ..good.clone()
            },
            LightView {
                outer_deg: f32::INFINITY,
                ..good.clone()
            },
            LightView {
                intensity: 0.0,
                ..good.clone()
            },
            LightView {
                outer_deg: 0.0,
                ..good.clone()
            },
        ];
        let mut lights = vec![
            LightView {
                kind: LightKindView::Directional,
                ..good.clone()
            },
            LightView {
                kind: LightKindView::Point,
                ..good.clone()
            },
            LightView {
                shadows: false,
                ..good.clone()
            },
            spot(6, Vec3::new(0.0, 3.0, 30.0), Vec3::NEG_Y, 6.0, 30.0),
        ];
        for l in &invalid {
            assert!(
                casts_locally(l),
                "a spot with shadows asks for a view: {l:?}"
            );
            assert!(spot_view(l).is_none(), "{l:?}");
        }
        lights.extend(invalid.iter().cloned());
        lights.push(good.clone());
        assert!(!casts_locally(&lights[0]) && !casts_locally(&lights[1]));
        assert!(!casts_locally(&lights[2]) && casts_locally(&lights[3]));
        let only = local_views(&lights, &cam, &planes, LOCAL_VIEWS);
        assert_eq!(only.dropped, 0);
        assert_eq!(only.views.len(), 1);
        // The directional light has no row in the lights buffer.
        assert_eq!(
            (only.views[0].id, only.views[0].light as usize),
            (5, lights.len() - 2)
        );

        // Under an orthographic camera the size on screen is the sphere's radius: the longer
        // range wins, however far.
        let mut ortho = cam;
        ortho.ortho_height = Some(40.0);
        let planes = frustum_planes(ortho.proj(16.0 / 9.0) * ortho.view(), true);
        let pair = [
            spot(1, Vec3::new(0.0, 3.0, -6.0), Vec3::NEG_Y, 4.0, 30.0),
            spot(2, Vec3::new(0.0, 3.0, -60.0), Vec3::NEG_Y, 8.0, 30.0),
        ];
        let far = local_views(&pair, &ortho, &planes, 1);
        assert_eq!((far.views[0].id, far.dropped), (2, 1));
    }

    /// The shaders' sizes are literals: the culling pass's views and its workgroup counters, the
    /// view uniform's matrices and the constants the forward shader indexes with follow the
    /// constants here.
    #[test]
    fn the_shaders_hold_the_same_constants() {
        let views = crate::batches::VIEWS as usize + LOCAL_VIEWS;
        let counters = views * amoris_assets::lod::MAX_LEVELS;
        let cull = crate::shaders::source("cull");
        for text in [
            format!("planes: array<Planes, {views}>"),
            format!("var<workgroup> wg_count: array<atomic<u32>, {counters}>;"),
            format!("var<workgroup> wg_base: array<u32, {counters}>;"),
            format!("var inside: array<bool, {views}>;"),
            format!("var level: array<u32, {views}>;"),
            format!("var local_slot: array<u32, {views}>;"),
            format!("const LATE: u32 = {}u;", crate::batches::LATE),
        ] {
            assert_eq!(cull.matches(&text).count(), 1, "cull.wgsl: {text}");
        }
        // Both of `main`'s guards over the counters (the first zeroes them: compute pipelines run
        // without automatic workgroup zeroing).
        assert_eq!(cull.matches(&format!("lid < {counters}u")).count(), 2);
        let forward = crate::shaders::source("forward");
        for text in [
            format!("const CASCADES: u32 = {CASCADES}u;"),
            format!("const LOCAL_VIEWS: u32 = {LOCAL_VIEWS}u;"),
            format!("cascades: array<mat4x4f, {CASCADES}>,"),
            format!("local_views: array<mat4x4f, {LOCAL_VIEWS}>,"),
        ] {
            assert_eq!(forward.matches(&text).count(), 1, "forward.wgsl: {text}");
        }
    }
}
