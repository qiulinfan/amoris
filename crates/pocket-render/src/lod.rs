//! Levels of detail on the GPU (charter 4.4's Pioneer note of 2026-10-09, docs/spec/lod.md): the
//! switch, the error bounds the culling pass (cull.wgsl) picks levels with, and what a frame drew
//! level by level, read back for checks and benchmarks.
//!
//! A mesh's coarser levels come from `pocket_assets::lod` (made when a glTF is imported or a
//! primitive generated) and are rows of the mesh table (meshes.rs). Per instance and view the
//! culling pass draws the coarsest level whose geometric error, scaled by the instance, stays
//! within a bound: on screen, `pixels` pixels at the distance of the instance's bounding sphere's
//! nearest point, with hysteresis; in a shadow cascade, `shadow_texels` of its texels.

use glam::Vec3;

use crate::camera::CameraState;

/// Whether instances draw coarser levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LodMode {
    /// Every instance draws its full mesh (level 0).
    Off,
    /// Levels are picked per instance and view (the default).
    On,
}

impl LodMode {
    /// `off` or `on` (any case; also `0`/`1`, `false`/`true`).
    pub fn parse(s: &str) -> Option<LodMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" | "no" => Some(LodMode::Off),
            "on" | "1" | "true" | "yes" => Some(LodMode::On),
            _ => None,
        }
    }

    /// From `POCKET_LOD` (natively); [`LodMode::On`] when it is unset, unknown, or in the browser.
    pub fn from_env() -> LodMode {
        match std::env::var("POCKET_LOD") {
            Ok(v) => LodMode::parse(&v).unwrap_or_else(|| {
                log::warn!("POCKET_LOD: {v:?} is not off or on; using on");
                LodMode::On
            }),
            Err(_) => LodMode::On,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LodMode::Off => "off",
            LodMode::On => "on",
        }
    }
}

/// How levels are picked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodSettings {
    pub mode: LodMode,
    /// The largest on-screen error of a level, in pixels of the output (`POCKET_LOD_PIXELS`).
    pub pixels: f32,
    /// The largest error of a level drawn into a shadow cascade, in that cascade's texels.
    pub shadow_texels: f32,
    /// A coarser level is taken on screen only when it would still be within the bound shrunk by
    /// this share (a finer one as soon as the current level exceeds the bound), so an instance at
    /// the edge between two levels does not switch every frame.
    pub hysteresis: f32,
}

impl Default for LodSettings {
    fn default() -> Self {
        LodSettings {
            mode: LodMode::On,
            pixels: 1.0,
            shadow_texels: 1.0,
            hysteresis: 0.2,
        }
    }
}

impl LodSettings {
    /// The defaults with `POCKET_LOD` and `POCKET_LOD_PIXELS` (natively).
    pub fn from_env() -> LodSettings {
        let mut s = LodSettings {
            mode: LodMode::from_env(),
            ..LodSettings::default()
        };
        let bound = |name: &str, default: f32| match std::env::var(name) {
            Ok(v) => match v.trim().parse::<f32>() {
                Ok(p) if p >= 0.0 && p.is_finite() => p,
                _ => {
                    log::warn!("{name}: {v:?} is not a number of at least 0; using {default}");
                    default
                }
            },
            Err(_) => default,
        };
        s.pixels = bound("POCKET_LOD_PIXELS", s.pixels);
        s.shadow_texels = bound("POCKET_LOD_SHADOW_TEXELS", s.shadow_texels);
        s
    }

    pub fn on(&self) -> bool {
        self.mode == LodMode::On
    }

    /// The culling pass's camera terms: its position with, in `w`, the world-space error a level
    /// may have per metre from the camera (perspective) or in all (orthographic), and whether the
    /// camera is orthographic. `height` is the output's height in pixels.
    pub fn camera_terms(&self, cam: &CameraState, height: u32) -> ([f32; 4], bool) {
        let h = height.max(1) as f32;
        let p = cam.position;
        match cam.ortho_height {
            // A pixel covers `view height / h` metres at any distance.
            Some(view) => ([p.x, p.y, p.z, self.pixels * view / h], true),
            // At distance d a pixel covers 2 d tan(fov / 2) / h metres.
            None => (
                [
                    p.x,
                    p.y,
                    p.z,
                    self.pixels * 2.0 * (cam.fov_y * 0.5).tan() / h,
                ],
                false,
            ),
        }
    }

    /// Each cascade's bound: `shadow_texels` of its texels, in metres.
    pub fn cascade_terms(&self, texels: &[f32; 4]) -> [f32; 4] {
        texels.map(|t| t * self.shadow_texels)
    }
}

/// What one argument set of a frame drew: instances and triangles, per level of detail (index 0 is
/// the full meshes; meshes without levels count there).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetCounts {
    pub instances: [u64; 8],
    pub triangles: [u64; 8],
}

impl SetCounts {
    pub fn instances(&self) -> u64 {
        self.instances.iter().sum()
    }

    pub fn triangles(&self) -> u64 {
        self.triangles.iter().sum()
    }
}

/// A frame's draws read back from the GPU: the camera's set, each cascade's and the late set
/// (occlusion culling's second phase; empty without it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawCounts {
    pub camera: SetCounts,
    pub cascades: [SetCounts; 4],
    pub late: SetCounts,
}

impl DrawCounts {
    /// The camera view's instances and triangles over both phases.
    pub fn camera_triangles(&self) -> u64 {
        self.camera.triangles() + self.late.triangles()
    }

    pub fn shadow_triangles(&self) -> u64 {
        self.cascades.iter().map(SetCounts::triangles).sum()
    }
}

/// The distance at which an error of `error` world units projects to the bound (for the spec's
/// examples and the tests): the camera draws a level of that error from this distance on.
pub fn switch_distance(settings: &LodSettings, cam: &CameraState, height: u32, error: f32) -> f32 {
    let (eye, ortho) = settings.camera_terms(cam, height);
    if ortho {
        return if error <= eye[3] { 0.0 } else { f32::INFINITY };
    }
    error / eye[3]
}

/// The nearest point of a sphere's distance from `eye` (the distance cull.wgsl measures).
pub fn sphere_distance(eye: Vec3, center: Vec3, radius: f32) -> f32 {
    (eye.distance(center) - radius).max(1e-4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pixel_at_a_distance() {
        let s = LodSettings::default();
        let cam = CameraState::look_at(Vec3::ZERO, -Vec3::Z);
        // 60 degree field of view over 1000 pixels: a pixel at 1 m is 2 tan(30) / 1000 m.
        let (eye, ortho) = s.camera_terms(&cam, 1000);
        assert!(!ortho);
        assert!((eye[3] - 2.0 * 30f32.to_radians().tan() / 1000.0).abs() < 1e-9);
        // An error of 1 cm is a pixel at about 8.66 m.
        let d = switch_distance(&s, &cam, 1000, 0.01);
        assert!((d - 8.660254).abs() < 1e-3, "{d}");
        let half = LodSettings {
            pixels: 0.5,
            ..LodSettings::default()
        };
        assert!((switch_distance(&half, &cam, 1000, 0.01) - 2.0 * d).abs() < 1e-3);
        let ortho_cam = CameraState {
            ortho_height: Some(10.0),
            ..cam
        };
        let (eye, ortho) = s.camera_terms(&ortho_cam, 1000);
        assert!(ortho);
        assert!((eye[3] - 0.01).abs() < 1e-9);
    }

    #[test]
    fn modes_parse() {
        assert_eq!(LodMode::parse("OFF"), Some(LodMode::Off));
        assert_eq!(LodMode::parse(" on "), Some(LodMode::On));
        assert_eq!(LodMode::parse("auto"), None);
        assert_eq!(LodMode::On.name(), "on");
    }
}
