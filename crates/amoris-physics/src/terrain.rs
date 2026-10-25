//! Terrain (water.md 65): ground declared by a few features rather than a mesh made elsewhere. A
//! heightfield of `size` cells of `cell_m` metres from its entity's `Transform` (the corner, x and
//! z; y lifts it), its heights worked out from `base_m` and its features in order: slopes, hills
//! and noise add, then channels carve. The renderer draws it, the solver stands bodies on it as a
//! fixed triangle mesh, and water regions take their beds from it.

use amoris_contract::{Problem, detail};
use amoris_sim::{EntityId, math};
use bevy_ecs::prelude::{Commands, Component, Entity, Query};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{Collider, RigidBody, Shape};

#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Terrain {
    /// Cells along x and z, 1 to 1024 each; the heights stand at their corners.
    pub size: [u32; 2],
    /// A cell's edge, metres: 0.1 to 16.
    pub cell_m: f64,
    /// The ground's height before the features, metres above the entity.
    #[serde(default)]
    pub base_m: f64,
    /// Applied in order; channels carve after everything else.
    #[serde(default)]
    pub features: Vec<TerrainFeature>,
    #[serde(default)]
    pub look: TerrainLook,
}

/// What shapes the ground. Points are metres from the terrain's corner (x, z), heights metres above
/// its entity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub enum TerrainFeature {
    /// The ground rises `grade` metres a metre toward the bearing `toward_deg` (0: +x, 90: +z),
    /// level at the corner.
    Slope { toward_deg: f64, grade: f64 },
    /// A smooth hill `height_m` high (negative: a basin) within `radius_m` of `at`.
    Hill {
        at: [f64; 2],
        radius_m: f64,
        height_m: f64,
    },
    /// Rolling ground about `amplitude_m` high over about `wavelength_m`.
    Noise {
        amplitude_m: f64,
        wavelength_m: f64,
        #[serde(default)]
        seed: u32,
    },
    /// A river's channel along the points of `path` (x, the bed's height, z): its bed `width_m`
    /// wide, its banks rising to the ground over `bank_m`. A step in the bed's height between two
    /// points makes a fall.
    Channel {
        path: Vec<[f64; 3]>,
        width_m: f64,
        bank_m: f64,
    },
}

/// How the ground is drawn: each triangle takes the first of `layers` it is steeper than (1 - its
/// normal's y) and lower than (metres above the entity) where the layer says, else the last.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainLook {
    pub layers: Vec<TerrainLayer>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainLayer {
    /// An asset material (`file#name`), or empty for a plain one of `color`.
    #[serde(default)]
    pub material: String,
    /// Linear RGBA.
    pub color: [f64; 4],
    /// 0 mirror to 1 rough.
    pub roughness: f64,
    #[serde(default)]
    pub steeper_than: Option<f64>,
    #[serde(default)]
    pub below_m: Option<f64>,
}

impl Default for TerrainLook {
    /// Rock where steep, sand under half a metre, grass elsewhere.
    fn default() -> Self {
        let layer = |color: [f64; 3], roughness, steeper_than, below_m| TerrainLayer {
            material: String::new(),
            color: [color[0], color[1], color[2], 1.0],
            roughness,
            steeper_than,
            below_m,
        };
        TerrainLook {
            layers: vec![
                layer([0.30, 0.28, 0.26], 0.9, Some(0.35), None),
                layer([0.62, 0.53, 0.36], 0.95, None, Some(0.5)),
                layer([0.20, 0.30, 0.12], 0.9, None, None),
            ],
        }
    }
}

impl amoris_sim::Persisted for Terrain {
    const NAME: &'static str = "Terrain";
    const VERSION: u32 = 1;
    fn trace(
        t: &mut serde_reflection::Tracer,
        s: &serde_reflection::Samples,
    ) -> serde_reflection::Result<()> {
        t.trace_type::<TerrainFeature>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

/// The terrain a terrain entity's `Collider` was made from, by its hash: the collider is made
/// again when they differ (`physics.terrain`).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerrainGround {
    pub terrain: u64,
}

impl amoris_sim::Persisted for TerrainGround {
    const NAME: &'static str = "TerrainGround";
    const VERSION: u32 = 1;
}

/// The most cells along a side.
pub const MAX_CELLS: u32 = 1024;
/// The most points a channel's path holds.
pub const MAX_PATH: usize = 256;

fn invalid(field: &str, message: impl Into<String>) -> Problem {
    Problem::new(
        "terrain.invalid",
        message.into(),
        detail([("field", json!(field))]),
    )
}

fn finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}

impl Terrain {
    pub fn validate(&self) -> Result<(), Problem> {
        if self.size.iter().any(|n| !(1..=MAX_CELLS).contains(n)) {
            return Err(invalid(
                "size",
                format!("a terrain has 1 to {MAX_CELLS} cells a side"),
            ));
        }
        if !(0.1..=16.0).contains(&self.cell_m) {
            return Err(invalid("cell_m", "a terrain's cells are 0.1 to 16 m"));
        }
        if !self.base_m.is_finite() {
            return Err(invalid("base_m", "the base height must be finite"));
        }
        for (n, f) in self.features.iter().enumerate() {
            let field = format!("features.{n}");
            let ok = match f {
                TerrainFeature::Slope { toward_deg, grade } => {
                    finite(&[*toward_deg, *grade]) && grade.abs() <= 10.0
                }
                TerrainFeature::Hill {
                    at,
                    radius_m,
                    height_m,
                } => finite(&[at[0], at[1], *radius_m, *height_m]) && *radius_m > 0.0,
                TerrainFeature::Noise {
                    amplitude_m,
                    wavelength_m,
                    ..
                } => finite(&[*amplitude_m, *wavelength_m]) && *wavelength_m > 0.0,
                TerrainFeature::Channel {
                    path,
                    width_m,
                    bank_m,
                } => {
                    (2..=MAX_PATH).contains(&path.len())
                        && path.iter().all(|p| finite(p))
                        && finite(&[*width_m, *bank_m])
                        && *width_m > 0.0
                        && *bank_m >= 0.0
                }
            };
            if !ok {
                return Err(invalid(
                    &field,
                    "a feature's numbers must be finite, its radius, wavelength and width above 0, \
                     its grade within 10 and its path 2 to 256 points",
                ));
            }
        }
        for (n, l) in self.look.layers.iter().enumerate() {
            if !finite(&l.color)
                || !(0.0..=1.0).contains(&l.roughness)
                || l.steeper_than.is_some_and(|v| !v.is_finite())
                || l.below_m.is_some_and(|v| !v.is_finite())
            {
                return Err(invalid(
                    &format!("look.layers.{n}"),
                    "a layer's colour, roughness and limits must be finite, its roughness 0 to 1",
                ));
            }
        }
        if self.look.layers.is_empty() || self.look.layers.len() > 8 {
            return Err(invalid("look.layers", "a terrain draws 1 to 8 layers"));
        }
        Ok(())
    }

    /// Heights along x and z: one more than the cells.
    pub fn corners(&self) -> [usize; 2] {
        self.size.map(|n| n as usize + 1)
    }

    /// A hash of everything the ground is made from.
    pub fn hash(&self) -> u64 {
        let bytes = bincode::serialize(self).unwrap_or_default();
        amoris_sim::rng::fnv1a64_extend(0xcbf2_9ce4_8422_2325, &bytes)
    }

    /// The heights at the cells' corners, x fastest, metres above the entity.
    pub fn heights(&self) -> Vec<f64> {
        let [nx, nz] = self.corners();
        let mut h = vec![self.base_m; nx * nz];
        let at = |n: usize| [metres(n % nx, self.cell_m), metres(n / nx, self.cell_m)];
        for f in &self.features {
            match f {
                TerrainFeature::Slope { toward_deg, grade } => {
                    let (s, c) = math::sin_cos(toward_deg.to_radians());
                    for (n, h) in h.iter_mut().enumerate() {
                        let p = at(n);
                        *h += grade * (p[0] * c + p[1] * s);
                    }
                }
                TerrainFeature::Hill {
                    at: centre,
                    radius_m,
                    height_m,
                } => {
                    for (n, h) in h.iter_mut().enumerate() {
                        let p = at(n);
                        let r = math::hypot(p[0] - centre[0], p[1] - centre[1]) / radius_m;
                        if r < 1.0 {
                            let s = 1.0 - r * r;
                            *h += height_m * s * s;
                        }
                    }
                }
                TerrainFeature::Noise {
                    amplitude_m,
                    wavelength_m,
                    seed,
                } => {
                    // Three octaves of warped sines, each turned by the golden angle from the
                    // last, so no ridge runs straight across the ground.
                    let phase = phases(*seed);
                    let k = math::TAU / wavelength_m;
                    let octaves = [(1.0, 0.5), (1.9, 0.3), (3.7, 0.2)].map(|(f, w)| {
                        let (s, c) = math::sin_cos(phase[0] + 2.399_963 * f);
                        (k * f, w, s, c)
                    });
                    for (n, h) in h.iter_mut().enumerate() {
                        let [x, z] = at(n);
                        let mut sum = 0.0;
                        for (o, &(k, w, s, c)) in octaves.iter().enumerate() {
                            let (u, v) = ((x * c + z * s) * k, (z * c - x * s) * k);
                            let warp = 1.3 * math::sin(0.6 * v + phase[1 + o]);
                            sum += w
                                * math::sin(u + warp + phase[2 + o])
                                * math::sin(0.8 * v + phase[o]);
                        }
                        *h += amplitude_m * sum;
                    }
                }
                TerrainFeature::Channel { .. } => {}
            }
        }
        for f in &self.features {
            if let TerrainFeature::Channel {
                path,
                width_m,
                bank_m,
            } = f
            {
                carve(&mut h, [nx, nz], self.cell_m, path, *width_m, *bank_m);
            }
        }
        h
    }
}

/// `physics.terrain` (Forces, last): each terrain stands bodies and walkers on it as a fixed
/// triangle mesh of its heights, made again when the terrain changes (water.md 65.1). Not in the
/// paused edit world, so a scene keeps only the terrain's features.
pub(crate) fn ground(
    mut commands: Commands,
    terrains: Query<(Entity, &EntityId, &Terrain, Option<&TerrainGround>)>,
) {
    for (e, _, t, made) in amoris_sim::order::by_id(&terrains) {
        let hash = t.hash();
        if made.is_some_and(|m| m.terrain == hash) {
            continue;
        }
        commands.entity(e).insert((
            collider(t),
            RigidBody::fixed(),
            TerrainGround { terrain: hash },
        ));
    }
}

/// The terrain's ground as a triangle mesh in its entity's frame.
pub fn collider(t: &Terrain) -> Collider {
    let [nx, nz] = t.corners();
    let h = t.heights();
    let vertices = h
        .iter()
        .enumerate()
        .map(|(n, y)| [metres(n % nx, t.cell_m), *y, metres(n / nx, t.cell_m)])
        .collect();
    let row = u32::try_from(nx).unwrap_or(u32::MAX);
    let mut indices = Vec::with_capacity(2 * (nx - 1) * (nz - 1));
    for k in 0..t.size[1] {
        for i in 0..t.size[0] {
            let a = i + row * k;
            indices.push([a, a + row, a + 1]);
            indices.push([a + 1, a + row, a + row + 1]);
        }
    }
    Collider::new(Shape::TriMesh { vertices, indices }).with_friction(0.7, 0.0)
}

/// The coordinate of corner `i` of cells `cell` metres wide.
fn metres(i: usize, cell: f64) -> f64 {
    f64::from(u32::try_from(i).unwrap_or(u32::MAX)) * cell
}

/// The noise's phases for `seed`, from the golden ratio's fractions.
fn phases(seed: u32) -> [f64; 5] {
    std::array::from_fn(|i| {
        let x = (f64::from(seed) + 1.0) * (0.618_033_988_749_895 * metres(i + 1, 1.0));
        (x - x.floor()) * math::TAU
    })
}

/// The height under local `x`, `z` from corner heights `h`, bilinear; held at the edges.
pub fn height_at(h: &[f64], corners: [usize; 2], cell: f64, x: f64, z: f64) -> f64 {
    let [nx, nz] = corners;
    let gx = math::clamp(x / cell, 0.0, metres(nx - 1, 1.0));
    let gz = math::clamp(z / cell, 0.0, metres(nz - 1, 1.0));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to the grid
    let (i, k) = ((gx as usize).min(nx - 2), (gz as usize).min(nz - 2));
    let (tx, tz) = (gx - metres(i, 1.0), gz - metres(k, 1.0));
    let at = |i: usize, k: usize| h[i + nx * k];
    math::lerp(
        math::lerp(at(i, k), at(i + 1, k), tx),
        math::lerp(at(i, k + 1), at(i + 1, k + 1), tx),
        tz,
    )
}

/// Points along the centripetal Catmull-Rom spline through `path` (E. Catmull and R. Rom, "A
/// class of local interpolating splines", 1974; its centripetal form by C. Yuksel, S. Schaefer and
/// J. Keyser, "Parameterization and applications of Catmull-Rom curves", CAD 43(7), 2011, never
/// cusps or loops within a segment), at most `step` metres apart. The bed's height follows the
/// points' heights linearly along each segment, so a step between two points stays a step.
pub fn spline(path: &[[f64; 3]], step: f64) -> Vec<[f64; 3]> {
    let n = path.len();
    // Point `i - 1` of the path; past its ends a mirrored point, so the spline leaves each end
    // along its end segment.
    let p = |i: usize| -> [f64; 2] {
        let (a, b) = match i {
            0 => (path[0], path[1]),
            i if i > n => (path[n - 1], path[n - 2]),
            i => return [path[i - 1][0], path[i - 1][2]],
        };
        [2.0 * a[0] - b[0], 2.0 * a[2] - b[2]]
    };
    let mut out = vec![path[0]];
    for s in 0..n - 1 {
        let ps = [p(s), p(s + 1), p(s + 2), p(s + 3)];
        // Knots spaced by the square root of the chords' lengths.
        let mut t = [0.0; 4];
        for i in 1..4 {
            let d = math::hypot(ps[i][0] - ps[i - 1][0], ps[i][1] - ps[i - 1][1]);
            t[i] = t[i - 1] + math::max(math::sqrt(d), 1e-6);
        }
        let chord = math::hypot(ps[2][0] - ps[1][0], ps[2][1] - ps[1][1]);
        let steps = math::max((chord / step).ceil(), 1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a path's own size
        let steps = steps as usize;
        let (y0, y1) = (path[s][1], path[s + 1][1]);
        for j in 1..=steps {
            let u = metres(j, 1.0) / metres(steps, 1.0);
            let tt = math::lerp(t[1], t[2], u);
            let at = barry_goldman(&ps, &t, tt);
            out.push([at[0], math::lerp(y0, y1, u), at[1]]);
        }
    }
    out
}

/// The Catmull-Rom point at knot `tt` by the pyramid of P. J. Barry and R. N. Goldman (1988).
fn barry_goldman(p: &[[f64; 2]; 4], t: &[f64; 4], tt: f64) -> [f64; 2] {
    let mix = |a: [f64; 2], b: [f64; 2], t0: f64, t1: f64| {
        let w = (tt - t0) / (t1 - t0);
        [math::lerp(a[0], b[0], w), math::lerp(a[1], b[1], w)]
    };
    let a1 = mix(p[0], p[1], t[0], t[1]);
    let a2 = mix(p[1], p[2], t[1], t[2]);
    let a3 = mix(p[2], p[3], t[2], t[3]);
    let b1 = mix(a1, a2, t[0], t[2]);
    let b2 = mix(a2, a3, t[1], t[3]);
    mix(b1, b2, t[1], t[2])
}

/// Lowers the ground along a channel: to the bed within half its width of the line, then rising
/// smoothly to the ground over the banks. Never raises it.
fn carve(h: &mut [f64], corners: [usize; 2], cell: f64, path: &[[f64; 3]], width: f64, bank: f64) {
    let [nx, nz] = corners;
    let line = spline(path, 0.5 * cell);
    let (half, reach) = (0.5 * width, 0.5 * width + bank);
    // Per corner, the nearest point of the line found so far: its distance and bed.
    let mut best = vec![(f64::INFINITY, 0.0); h.len()];
    let index = |v: f64, n: usize| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to the grid
        let i = math::clamp((v / cell).floor(), 0.0, metres(n - 1, 1.0)) as usize;
        i
    };
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        let lo = [math::min(a[0], b[0]) - reach, math::min(a[2], b[2]) - reach];
        let hi = [math::max(a[0], b[0]) + reach, math::max(a[2], b[2]) + reach];
        if hi[0] < 0.0 || hi[1] < 0.0 {
            continue;
        }
        let d = [b[0] - a[0], b[2] - a[2]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        for k in index(lo[1], nz)..=index(hi[1] + cell, nz) {
            for i in index(lo[0], nx)..=index(hi[0] + cell, nx) {
                let (x, z) = (metres(i, cell), metres(k, cell));
                let u = if len2 > 0.0 {
                    math::clamp(((x - a[0]) * d[0] + (z - a[2]) * d[1]) / len2, 0.0, 1.0)
                } else {
                    0.0
                };
                let dist = math::hypot(x - a[0] - u * d[0], z - a[2] - u * d[1]);
                let n = i + nx * k;
                if dist < best[n].0 {
                    best[n] = (dist, math::lerp(a[1], b[1], u));
                }
            }
        }
    }
    for (h, (dist, bed)) in h.iter_mut().zip(best) {
        if dist >= reach {
            continue;
        }
        let ground = if dist <= half {
            bed
        } else {
            let t = math::clamp((dist - half) / math::max(bank, 1e-9), 0.0, 1.0);
            bed + (*h - bed) * t * t * (3.0 - 2.0 * t)
        };
        *h = math::min(*h, ground);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(size: [u32; 2], cell: f64, base: f64) -> Terrain {
        Terrain {
            size,
            cell_m: cell,
            base_m: base,
            features: vec![],
            look: TerrainLook::default(),
        }
    }

    #[test]
    fn a_channel_carves_its_bed_and_banks_and_follows_its_points() {
        let mut t = flat([64, 32], 0.5, 2.0);
        t.features.push(TerrainFeature::Channel {
            path: vec![[0.0, 1.0, 8.0], [32.0, 0.5, 8.0]],
            width_m: 4.0,
            bank_m: 2.0,
        });
        t.validate().unwrap();
        let h = t.heights();
        let at = |x: f64, z: f64| height_at(&h, t.corners(), t.cell_m, x, z);
        // On the line the bed, sloping from 1 to 0.5 m along it.
        assert!((at(0.0, 8.0) - 1.0).abs() < 1e-9);
        assert!((at(16.0, 8.0) - 0.75).abs() < 1e-9);
        // Within half the width still the bed; past the bank the ground.
        assert!((at(16.0, 9.5) - 0.75).abs() < 1e-9);
        assert!((at(16.0, 12.5) - 2.0).abs() < 1e-9);
        // On the bank between them.
        let bank = at(16.0, 11.0);
        assert!(bank > 0.75 && bank < 2.0, "{bank}");
    }

    #[test]
    fn a_step_in_the_bed_stays_a_step_and_a_hill_and_a_slope_add() {
        let mut t = flat([40, 20], 0.5, 5.0);
        t.features = vec![
            TerrainFeature::Slope {
                toward_deg: 0.0,
                grade: 0.1,
            },
            TerrainFeature::Hill {
                at: [5.0, 5.0],
                radius_m: 2.0,
                height_m: -1.0,
            },
            TerrainFeature::Channel {
                path: vec![
                    [0.0, 4.0, 2.0],
                    [10.0, 4.0, 2.0],
                    [10.5, 1.0, 2.0],
                    [20.0, 1.0, 2.0],
                ],
                width_m: 2.0,
                bank_m: 1.0,
            },
        ];
        let h = t.heights();
        let at = |x: f64, z: f64| height_at(&h, t.corners(), t.cell_m, x, z);
        assert!(
            (at(20.0, 9.0) - 7.0).abs() < 1e-9,
            "the slope: {}",
            at(20.0, 9.0)
        );
        assert!(
            (at(5.0, 5.0) - 4.5).abs() < 1e-9,
            "the basin: {}",
            at(5.0, 5.0)
        );
        assert!((at(9.0, 2.0) - 4.0).abs() < 1e-9 && (at(12.0, 2.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_spline_passes_through_its_points() {
        let path = [
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 3.0],
            [8.0, 0.0, -1.0],
            [12.0, 0.0, 0.0],
        ];
        let line = spline(&path, 0.25);
        for p in path {
            assert!(
                line.iter()
                    .any(|q| (q[0] - p[0]).abs() < 1e-9 && (q[2] - p[2]).abs() < 1e-9),
                "{p:?}"
            );
        }
        assert!(
            line.windows(2)
                .all(|w| math::hypot(w[1][0] - w[0][0], w[1][2] - w[0][2]) < 0.6)
        );
    }
}
