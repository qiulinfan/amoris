//! The engine's water (docs/spec/water.md): one deterministic shallow-water height field for every
//! game. A [`WaterRegion`] is a grid of columns over a bed, in tiles of 16 x 16 columns, each column
//! a bed height and a water depth and each column's -x and -z faces a velocity, all `f64` world
//! state. `physics.water` (Forces, row 4a of simulation.md 4.4) steps the tiles that are awake and
//! the tiles beside them with the spike's advect scheme ([`solve`]), dries thin slow films when the
//! region asks for it, and puts still tiles to sleep.
//!
//! Deterministic: `f64`, `+ - * /`, `floor`, comparisons and `amoris_sim::math` only; the active set
//! is read from the tick's starting state and every loop runs in a fixed order (water.md 4).

pub mod bed;
mod collider_bed;
pub use collider_bed::WaterColliderBed;
pub(crate) mod buoyancy;
pub mod compute;
pub mod displacement;
pub mod feedback;
pub mod field;
pub mod resident;
mod sea;
pub(crate) mod solve;
pub(crate) mod swimming;

use amoris_contract::{Problem, detail};
use amoris_sim::math;
use amoris_sim::order::by_id;
use amoris_sim::voxels::Voxels;
use amoris_sim::{EntityId, EntityIndex, EventKind, NewEvent, PlainData, SimClock};
use bevy_ecs::prelude::{Component, Query, Res, ResMut, Resource};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::body::Transform;
use crate::geom;

/// Columns along a tile's edge.
pub const TILE: usize = 16;
/// Columns in a tile.
pub const TILE_COLUMNS: usize = TILE * TILE;
/// The most columns along a region's side (water.md 3, decision 4).
pub const MAX_SIDE: u32 = 512;
/// The most substeps a tick takes (water.md 4).
pub const MAX_SUBSTEPS: u32 = 8;
/// Still ticks after which a tile sleeps (water.md 4).
pub const SLEEP_TICKS: u32 = 60;
/// The thickest film `dry_below` may dry, metres.
pub const MAX_DRY_BELOW: f64 = 0.05;
/// A tile is still when no depth changed by more than this over the tick, metres (water.md 4).
const STILL_DEPTH: f64 = 1e-5;
/// ... and no face ran faster than this, m/s.
const STILL_SPEED: f64 = 1e-3;
/// A film dries only when every face of its column runs slower than this, m/s.
const DRY_SPEED: f64 = 0.05;
/// Calm damping (water.md 4), besides bed friction: a face is damped at the region's
/// `calm_per_s` up to `CALM_SPEED` (m/s), and a faster one is decelerated by no more than
/// `calm_per_s * CALM_SPEED`
/// (0.1 m/s^2 at 0.2 a second). The sloshing and the eddies a flood leaves, which friction barely
/// damps in deep
/// water, die out, and the water sleeps again within about 2 minutes; a flow that gravity drives
/// (a breach, a fall) is hardly slowed.
const CALM_SPEED: f64 = 0.5;

/// One water observation in world units: free surface, depth, centred velocity x and z, the
/// free surface's slope along x and z, and its rate of rise (m/s). Floaters take Froude-Krylov
/// pressure along the slope and the surface's vertical motion from it (water.md 47).
pub type WaterColumn = [f64; 7];

/// A resident point observation in world units, for a region of `cell_m` columns.
pub fn column(raw: resident::RawSample, cell_m: f64) -> WaterColumn {
    [
        raw[0],
        raw[1],
        raw[2],
        raw[3],
        raw[4] / cell_m,
        raw[5] / cell_m,
        raw[6] / cell_m,
    ]
}

/// A region of water columns over a bed (water.md 3). Its corner is the entity's
/// `Transform.position` in x and z (y unused); the rotation must be the identity.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaterRegion {
    /// The execution path, persisted so a restore never silently changes precision.
    #[serde(default)]
    pub solver: Solver,
    /// A column's edge, metres (0.25 to 4).
    #[serde(default = "one")]
    pub cell_m: f64,
    /// Columns along x and z: multiples of 16, at most 512 each.
    pub size: [u32; 2],
    /// Where the beds come from (water.md 5).
    pub bed: BedSource,
    /// -x, +x, -z, +z (water.md 11).
    #[serde(default)]
    pub edges: [Edge; 4],
    /// Bed friction, m/s.
    #[serde(default = "friction")]
    pub friction: f64,
    /// Calm damping of slow faces, per second (water.md 4): 0.2 lets a disturbed sea sleep within
    /// minutes; waves meant to travel (a swell, a pool's ripples) take little or none (0 to 10).
    #[serde(default = "calm")]
    pub calm_per_s: f64,
    /// At least this many substeps a tick (1 to 8).
    #[serde(default = "two")]
    pub substeps: u32,
    /// Films thinner than this dry, metres (0: never; at most 0.05).
    #[serde(default)]
    pub dry_below: f64,
    /// Presentation, carried as the palette carries a liquid's fog (water.md 9).
    #[serde(default)]
    pub look: WaterLook,
    /// Physics beyond the hydrostatic bulk flow (water.md 46 to 49); resident regions only.
    #[serde(default)]
    pub physics: WaterPhysics,
    /// Every tile, in key order (z, then x); empty: all dry, the beds read from `bed` at the next
    /// `physics.water` (water.md 5).
    #[serde(default)]
    pub tiles: Vec<WaterTile>,
}

/// Physics beyond the hydrostatic bulk flow, for `gpu_resident` regions (water.md 46 to 49).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct WaterPhysics {
    /// Airy surface waves with depth-exact dispersion over the bulk flow (Jeschke and Wojtan
    /// 2023): ripples and wakes at any depth. Needs power-of-two sides.
    pub dispersion: bool,
    /// Eddy-viscosity breaking where the surface rises faster than half the local wave speed
    /// (Kennedy et al. 2000).
    pub breaking: bool,
    /// Ballistic falls: flow over a drop deeper than this many metres leaves its lip and lands
    /// where its trajectory meets water or bed (0: off; 0.25 to 100).
    pub falls_m: f64,
    /// Depth-integrated non-hydrostatic pressure (Yamazaki, Kowalik and Cheung 2009): long waves
    /// disperse and a swell shoals into crests rather than bores (water.md 55). Not with
    /// `dispersion`.
    pub nonhydrostatic: bool,
}
impl WaterPhysics {
    pub fn field(self) -> amoris_assets::water_field::FieldPhysics {
        amoris_assets::water_field::FieldPhysics {
            dispersion: self.dispersion,
            breaking: self.breaking,
            fall_min_m: self.falls_m,
            nonhydrostatic: self.nonhydrostatic,
        }
    }
}

/// CPU is bit-identical across targets; GPU requires a host compute service and uses f32.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Solver {
    #[default]
    Cpu,
    Gpu,
    GpuResident,
}

fn one() -> f64 {
    1.0
}

fn friction() -> f64 {
    0.02
}
fn calm() -> f64 {
    amoris_assets::water_field::DEFAULT_CALM_PER_S
}

fn two() -> u32 {
    2
}

/// 16 x 16 columns of a region (water.md 3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaterTile {
    /// The tile's column offset / 16, x and z.
    pub key: [u32; 2],
    /// 256 columns, x fastest: the bed's height, world y.
    pub bed: Vec<f64>,
    /// 256 columns: the water above the bed, metres, never below 0.
    pub depth: Vec<f64>,
    /// 256 columns: the velocity through each column's -x face, m/s.
    pub u: Vec<f64>,
    /// 256 columns: the velocity through each column's -z face, m/s.
    pub w: Vec<f64>,
    /// Ticks it has stayed still; it sleeps at 60.
    #[serde(default)]
    pub still: u32,
}

impl WaterTile {
    /// A dry tile over a flat bed at `bed`.
    pub fn dry(key: [u32; 2], bed: f64) -> WaterTile {
        WaterTile {
            key,
            bed: vec![bed; TILE_COLUMNS],
            depth: vec![0.0; TILE_COLUMNS],
            u: vec![0.0; TILE_COLUMNS],
            w: vec![0.0; TILE_COLUMNS],
            still: 0,
        }
    }

    /// Whether it sleeps: skipped by the solver, its velocities 0.
    pub fn asleep(&self) -> bool {
        self.still >= SLEEP_TICKS
    }
}

/// A spring or a pipe: water added at the column under world `at` (x, z) of `region`, `rate_m3s`
/// cubic metres a second, before the substeps (water.md 8).
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaterSource {
    /// The region it pours into; none: nowhere.
    pub region: Option<EntityId>,
    pub at: [f64; 2],
    pub rate_m3s: f64,
}

/// A sink: water taken at the column under world `at` (x, z) of `region`, at most `rate_m3s` cubic
/// metres a second and never more than is there (water.md 8).
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaterDrain {
    /// The region it takes from; none: nowhere.
    pub region: Option<EntityId>,
    pub at: [f64; 2],
    pub rate_m3s: f64,
}

/// Checks a source's or a drain's numbers: `water.invalid`.
pub fn validate_flow(at: [f64; 2], rate_m3s: f64) -> Result<(), Problem> {
    if !(at[0].is_finite() && at[1].is_finite()) {
        return Err(invalid(
            "at",
            "a source's or drain's point must be finite".into(),
        ));
    }
    if !(rate_m3s.is_finite() && rate_m3s >= 0.0) {
        return Err(invalid(
            "rate_m3s",
            format!("a rate is finite and not negative, not {rate_m3s}"),
        ));
    }
    Ok(())
}

/// How a region looks (water.md 9). Presentation only: the solver never reads it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct WaterLook {
    /// The view from under the surface: linear colour and fog density per metre.
    pub fog: [f32; 4],
    /// Absorption per metre, linear rgb (the drawing and the splashes).
    pub absorb: [f32; 3],
    /// Scattering per metre, linear rgb: with `absorb`, the colour deep water scatters back,
    /// `scatter / (scatter + absorb)` of the light, and its extinction, `scatter + absorb` (water.md
    /// 9). The defaults make deep water the analytic sea's deep colour.
    pub scatter: [f32; 3],
    pub style: Style,
    /// Metres of still water drawn around the region, at its edge's level and shaded as deep
    /// water: the open water a game shows past its region (0: none; water.md 9).
    pub surround_m: f32,
    /// Seconds the bed stays damp after the water leaves it, darker and, a few seconds more,
    /// glossy, with the foam the water left on it (0: the bed is drawn as it is; water.md 61.2).
    /// A sandy beach takes a minute or two.
    pub wet_s: f32,
}

impl Default for WaterLook {
    fn default() -> Self {
        WaterLook {
            fog: [0.05, 0.16, 0.45, 0.12],
            absorb: [1.98, 0.38, 0.27],
            scatter: [0.02; 3],
            style: Style::Smooth,
            surround_m: 0.0,
            wet_s: 0.0,
        }
    }
}

/// How the surface is drawn (water.md 9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Style {
    /// A vertex at each column's centre.
    #[default]
    Smooth,
    /// Flat over each block of a `Voxels` bed (each column over a `Colliders` bed), with vertical
    /// sides where neighbouring blocks differ.
    Blocky,
}

/// A region's edge (water.md 11).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum Edge {
    /// No water crosses it.
    #[default]
    Wall,
    /// The sea's waves drive it (slice D).
    Sea,
    /// Water enters across it, m^3/s per metre of it (slice C).
    Inflow { m3s_per_m: f64 },
    /// Water leaves freely (slice C).
    Outflow,
}

/// Where a region's beds come from (water.md 5).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum BedSource {
    /// A block volume: `per` columns to a block's edge (1 or 2), its cells `y_lo..=y_hi` searched.
    Voxels {
        volume: EntityId,
        per: u32,
        y_lo: i32,
        y_hi: i32,
    },
    /// The fixed colliders: a ray cast down from `top` at each column's centre (slice D).
    Colliders { top: f64, bottom: f64 },
}

fn invalid(field: &str, message: String) -> Problem {
    Problem::new("water.invalid", message, detail([("field", json!(field))]))
}

fn finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}

impl WaterRegion {
    /// Tiles along x and z.
    pub fn tiles_along(&self) -> [usize; 2] {
        self.size.map(|n| usize::try_from(n).unwrap_or(0) / TILE)
    }

    /// Columns along x and z.
    pub fn columns_along(&self) -> [usize; 2] {
        self.size.map(|n| usize::try_from(n).unwrap_or(0))
    }

    /// The tile index and the column within it of region column `(i, k)`.
    pub fn locate(&self, i: usize, k: usize) -> (usize, usize) {
        let tx = self.tiles_along()[0];
        ((i / TILE) + tx * (k / TILE), (i % TILE) + TILE * (k % TILE))
    }

    /// The region column under the corner-relative point `(x, z)`, metres, if the region covers it.
    pub fn column_at(&self, x: f64, z: f64) -> Option<(usize, usize)> {
        let (fx, fz) = (x / self.cell_m, z / self.cell_m);
        if !(fx >= 0.0 && fz >= 0.0) {
            return None;
        }
        let [nx, nz] = self.columns_along();
        let (i, k) = (whole(fx)?, whole(fz)?);
        (i < nx && k < nz).then_some((i, k))
    }

    /// Whether the tiles are there and of the right shape: what the solver needs, checked cheaply
    /// each tick (`validate` checks the rest when the region enters the world).
    pub fn well_formed(&self) -> bool {
        let [tx, tz] = self.tiles_along();
        tx > 0
            && tz > 0
            && self.size.iter().all(|n| n % 16 == 0 && *n <= MAX_SIDE)
            && self.tiles.len() == tx * tz
            && self.tiles.iter().all(|t| {
                [&t.bed, &t.depth, &t.u, &t.w]
                    .iter()
                    .all(|v| v.len() == TILE_COLUMNS)
            })
    }

    /// The region column under world `(x, z)` for a region whose corner is `corner`, if the region
    /// covers it and has its tiles.
    pub fn column_of(&self, corner: [f64; 2], x: f64, z: f64) -> Option<(usize, usize)> {
        if self.solver != Solver::GpuResident && !self.well_formed() {
            return None;
        }
        self.column_at(x - corner[0], z - corner[1])
    }

    /// Column `(i, k)`'s depth, metres.
    pub fn depth_at(&self, i: usize, k: usize) -> f64 {
        assert!(
            self.solver != Solver::GpuResident,
            "resident field values require WaterProbe observations, not checkpoint tiles"
        );
        let (t, c) = self.locate(i, k);
        self.tiles[t].depth[c]
    }

    /// Column `(i, k)`'s surface, world y (a dry column's is its bed).
    pub fn surface_at(&self, i: usize, k: usize) -> f64 {
        assert!(
            self.solver != Solver::GpuResident,
            "resident field values require WaterProbe observations, not checkpoint tiles"
        );
        let (t, c) = self.locate(i, k);
        self.tiles[t].bed[c] + self.tiles[t].depth[c]
    }

    /// Column `(i, k)`'s velocity, m/s along x and z: the mean of its faces (water.md 8).
    pub fn velocity_at(&self, i: usize, k: usize) -> [f64; 2] {
        assert!(
            self.solver != Solver::GpuResident,
            "resident field values require WaterProbe observations, not checkpoint tiles"
        );
        let [nx, nz] = self.columns_along();
        let (t, c) = self.locate(i, k);
        let (u0, w0) = (self.tiles[t].u[c], self.tiles[t].w[c]);
        let u1 = if i + 1 < nx {
            let (t, c) = self.locate(i + 1, k);
            self.tiles[t].u[c]
        } else {
            0.0
        };
        let w1 = if k + 1 < nz {
            let (t, c) = self.locate(i, k + 1);
            self.tiles[t].w[c]
        } else {
            0.0
        };
        [(u0 + u1) * 0.5, (w0 + w1) * 0.5]
    }

    /// Column `(i, k)`'s free surface rate of rise, m/s: the net inflow through its faces at
    /// centred depths over the column's width (continuity, as the resident point query).
    pub fn rise_at(&self, i: usize, k: usize) -> f64 {
        assert!(
            self.solver != Solver::GpuResident,
            "resident field values require WaterProbe observations, not checkpoint tiles"
        );
        let [nx, nz] = self.columns_along();
        let (t, c) = self.locate(i, k);
        let h = self.tiles[t].depth[c];
        let mut q = 0.0;
        if i > 0 {
            q += self.tiles[t].u[c] * 0.5 * (h + self.depth_at(i - 1, k));
        }
        if i + 1 < nx {
            let (t1, c1) = self.locate(i + 1, k);
            q -= self.tiles[t1].u[c1] * 0.5 * (h + self.tiles[t1].depth[c1]);
        }
        if k > 0 {
            q += self.tiles[t].w[c] * 0.5 * (h + self.depth_at(i, k - 1));
        }
        if k + 1 < nz {
            let (t1, c1) = self.locate(i, k + 1);
            q -= self.tiles[t1].w[c1] * 0.5 * (h + self.tiles[t1].depth[c1]);
        }
        q / self.cell_m
    }

    /// Changes column `(i, k)`'s water by `m3` (taking at most what it holds) and wakes its tile
    /// when that changed it; returns the change made, m^3.
    pub fn pour(&mut self, i: usize, k: usize, m3: f64) -> f64 {
        assert!(
            self.solver != Solver::GpuResident,
            "resident depth edits must be submitted to its GPU step"
        );
        let area = self.cell_m * self.cell_m;
        let (t, c) = self.locate(i, k);
        let tile = &mut self.tiles[t];
        let d = tile.depth[c];
        let new = math::max(d + m3 / area, 0.0);
        if new != d {
            tile.depth[c] = new;
            tile.still = 0;
        }
        (new - d) * area
    }

    /// The water's volume, m^3.
    pub fn volume(&self) -> f64 {
        assert!(
            self.solver != Solver::GpuResident,
            "resident volume is in WaterResident statistics"
        );
        let mut sum = 0.0;
        for t in &self.tiles {
            for d in &t.depth {
                sum += d;
            }
        }
        sum * self.cell_m * self.cell_m
    }

    /// Checks the region on its own (water.md 3): sizes and parameters in range, the tiles absent or
    /// complete and finite. `water.invalid`.
    pub fn validate(&self) -> Result<(), Problem> {
        if self.solver == Solver::GpuResident && self.dry_below != 0.0 {
            return Err(resident::failed(
                "resident drying is not connected yet; dry_below must be zero",
            ));
        }
        if self.physics != WaterPhysics::default() && self.solver != Solver::GpuResident {
            return Err(invalid(
                "physics",
                "dispersion, breaking and falls need the gpu_resident solver".into(),
            ));
        }
        if let Err(message) = self.physics.field().validate() {
            return Err(invalid("physics", message));
        }
        if self.physics.dispersion && self.size.iter().any(|n| !n.is_power_of_two()) {
            return Err(invalid(
                "physics.dispersion",
                format!(
                    "dispersive waves need power-of-two sides (16 to 512), not {:?}",
                    self.size
                ),
            ));
        }
        if !(self.cell_m >= 0.25 && self.cell_m <= 4.0) {
            return Err(invalid(
                "cell_m",
                format!("cell_m is 0.25 to 4 metres, not {}", self.cell_m),
            ));
        }
        for (a, n) in self.size.iter().enumerate() {
            if *n == 0 || n % 16 != 0 || *n > MAX_SIDE {
                return Err(invalid(
                    &format!("size.{a}"),
                    format!(
                        "a region's side is a multiple of 16 columns up to {MAX_SIDE}, not {n}"
                    ),
                ));
            }
        }
        if !(1..=MAX_SUBSTEPS).contains(&self.substeps) {
            return Err(invalid(
                "substeps",
                format!("substeps is 1 to {MAX_SUBSTEPS}, not {}", self.substeps),
            ));
        }
        if !(self.friction >= 0.0 && self.friction <= 10.0) {
            return Err(invalid(
                "friction",
                format!("friction is 0 to 10 m/s, not {}", self.friction),
            ));
        }
        if !(self.dry_below >= 0.0 && self.dry_below <= MAX_DRY_BELOW) {
            return Err(invalid(
                "dry_below",
                format!(
                    "dry_below is 0 to {MAX_DRY_BELOW} metres, not {}",
                    self.dry_below
                ),
            ));
        }
        for (a, e) in self.edges.iter().enumerate() {
            if let Edge::Inflow { m3s_per_m: q } = e
                && (!q.is_finite() || *q < 0.0 || *q > f64::from(f32::MAX))
            {
                return Err(invalid(
                    &format!("edges.{a}"),
                    "inflow discharge must be finite, nonnegative and fit f32".into(),
                ));
            }
            if *e != Edge::Wall && self.solver != Solver::GpuResident {
                return Err(invalid(
                    &format!("edges.{a}"),
                    format!("edge {e:?} requires the resident GPU path"),
                ));
            }
        }
        match self.bed {
            BedSource::Voxels {
                per, y_lo, y_hi, ..
            } => {
                if !(per == 1 || per == 2) {
                    return Err(invalid(
                        "bed.Voxels.per",
                        format!("per is 1 or 2 columns to a block, not {per}"),
                    ));
                }
                if y_lo > y_hi {
                    return Err(invalid(
                        "bed.Voxels.y_lo",
                        format!("the searched cells {y_lo}..={y_hi} are empty"),
                    ));
                }
            }
            BedSource::Colliders { top, bottom } => {
                if !top.is_finite()
                    || !bottom.is_finite()
                    || top <= bottom
                    || top.abs() > geom::SOLVER_MAX
                    || bottom.abs() > geom::SOLVER_MAX
                    || top - bottom > geom::SOLVER_MAX
                {
                    return Err(invalid(
                        "bed.Colliders",
                        "top/bottom must define a finite positive solver-range interval".into(),
                    ));
                }
            }
        }
        let look = &self.look;
        if !(look.surround_m >= 0.0 && look.surround_m <= 10_000.0) {
            return Err(invalid(
                "look.surround_m",
                format!("surround_m is 0 to 10000 metres, not {}", look.surround_m),
            ));
        }
        if !(look.wet_s >= 0.0 && look.wet_s <= 3600.0) {
            return Err(invalid(
                "look.wet_s",
                format!("wet_s is 0 to 3600 seconds, not {}", look.wet_s),
            ));
        }
        if !(look.fog.iter().chain(&look.absorb).all(|x| x.is_finite())
            && look
                .absorb
                .iter()
                .chain(&look.scatter)
                .all(|x| *x >= 0.0 && x.is_finite())
            && look
                .scatter
                .iter()
                .zip(&look.absorb)
                .all(|(s, a)| s + a > 0.0))
        {
            return Err(invalid(
                "look",
                "the look's values must be finite, absorb and scatter not negative and not both 0"
                    .into(),
            ));
        }
        if self.tiles.is_empty() {
            return Ok(());
        }
        let [tx, tz] = self.tiles_along();
        if self.tiles.len() != tx * tz {
            return Err(invalid(
                "tiles",
                format!(
                    "a region of {:?} columns has {} tiles, not {}",
                    self.size,
                    tx * tz,
                    self.tiles.len()
                ),
            ));
        }
        for (n, t) in self.tiles.iter().enumerate() {
            let want = [n % tx, n / tx].map(|k| u32::try_from(k).unwrap_or(u32::MAX));
            if t.key != want {
                return Err(invalid(
                    &format!("tiles.{n}.key"),
                    format!("tile {n} has key {:?}, not {want:?} (z, then x)", t.key),
                ));
            }
            for (name, v) in [
                ("bed", &t.bed),
                ("depth", &t.depth),
                ("u", &t.u),
                ("w", &t.w),
            ] {
                if v.len() != TILE_COLUMNS {
                    return Err(invalid(
                        &format!("tiles.{n}.{name}"),
                        format!("a tile holds {TILE_COLUMNS} columns, not {}", v.len()),
                    ));
                }
                if !finite(v) {
                    return Err(invalid(
                        &format!("tiles.{n}.{name}"),
                        "values must be finite".into(),
                    ));
                }
            }
            if t.depth.iter().any(|d| *d < 0.0) {
                return Err(invalid(
                    &format!("tiles.{n}.depth"),
                    "depths are never below 0".into(),
                ));
            }
        }
        Ok(())
    }
}

/// The whole part of a non-negative `x` (below 2^31), by comparisons only.
fn whole(x: f64) -> Option<usize> {
    if x.is_nan() || x >= 2_147_483_648.0 {
        return None;
    }
    let mut i = 0usize;
    let mut step = 1usize << 30;
    while step > 0 {
        if f64::from(u32::try_from(i + step).unwrap_or(u32::MAX)) <= x {
            i += step;
        }
        step >>= 1;
    }
    Some(i)
}

/// FNV-1a over every tile's beds, depths, velocities (their bits) and still counts, in tile and
/// column order: equal hashes, equal states.
pub fn hash(r: &WaterRegion) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |x: u64| {
        for b in x.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for t in &r.tiles {
        for v in [&t.bed, &t.depth, &t.u, &t.w] {
            for x in v.iter() {
                eat(x.to_bits());
            }
        }
        eat(u64::from(t.still));
    }
    h
}

/// The spike's bench scene as a region of `tiles` x `tiles` tiles of 0.5 m columns: bumps up to
/// 0.8 m, a block of water 2 m deep in one quarter that breaks like a dam (shallow-water.md).
pub fn dam_scene(tiles: u32) -> WaterRegion {
    let mut r = WaterRegion {
        solver: Solver::Cpu,
        cell_m: 0.5,
        size: [tiles * 16, tiles * 16],
        bed: BedSource::Voxels {
            volume: EntityId::FIRST,
            per: 2,
            y_lo: 0,
            y_hi: 63,
        },
        edges: [Edge::Wall; 4],
        friction: 0.02,
        calm_per_s: calm(),
        substeps: 2,
        dry_below: 0.0,
        look: WaterLook::default(),
        physics: WaterPhysics::default(),
        tiles: Vec::new(),
    };
    for kz in 0..tiles {
        for kx in 0..tiles {
            r.tiles.push(WaterTile::dry([kx, kz], 0.0));
        }
    }
    let n = r.columns_along()[0];
    for k in 0..n {
        for i in 0..n {
            let (t, c) = r.locate(i, k);
            let bump = f64::from(u32::try_from((i * 7 + k * 13) % 5).unwrap_or(0));
            r.tiles[t].bed[c] = 0.2 * bump;
            r.tiles[t].depth[c] = if i < n / 2 && k < n / 2 { 2.0 } else { 0.0 };
        }
    }
    r
}

/// The web tests' water lines (`lib.rs` `web_report`): the dam break of an 8 x 8 tile region
/// (128 x 128 columns) at 60 Hz, its hash every 60 ticks for 10 s, and the substeps it took. The
/// WebAssembly build must print the same.
pub fn water_report() -> String {
    let mut r = dam_scene(8);
    let mut g = solve::Grid::default();
    let mut out = String::new();
    let mut substeps = 0u64;
    for t in 1..=600u32 {
        substeps += u64::from(step_region(&mut r, 1.0 / 60.0, &mut g).substeps);
        if t % 60 == 0 {
            out.push_str(&format!("water {} {:016x} {substeps}\n", t / 60, hash(&r)));
        }
    }
    out
}

/// What one region's tick did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stepped {
    /// The substeps taken (0: nothing was awake).
    pub substeps: u32,
    /// The tiles stepped, by index, ascending.
    pub tiles: Vec<usize>,
    /// The water films that dried this tick took, m^3.
    pub dried_m3: f64,
}

/// Reusable solver buffers and a suspended tick's continuation. No checkpoint can be taken while
/// that continuation exists; the physics persistence rebuild hook clears it on restoration.
#[derive(Resource, Default)]
pub struct WaterScratch(
    pub(crate) solve::Grid,
    Option<WaterRun>,
    Option<Vec<resident::ResidentPlan>>,
    collider_bed::Cache,
);

pub(crate) fn prepare_collider_beds(world: &mut bevy_ecs::prelude::World) -> Result<(), Problem> {
    if world.resource::<WaterScratch>().1.is_some() {
        return Ok(());
    }
    world.resource_scope(
        |world, mut scratch: bevy_ecs::prelude::Mut<'_, WaterScratch>| {
            collider_bed::prepare(world, &mut scratch.3)
        },
    )
}

struct WaterRun {
    order: Vec<(bevy_ecs::entity::Entity, EntityId)>,
    maps: Vec<Option<bed::Map>>,
    flows: Vec<(EntityId, EntityId, [f64; 2], f64)>,
    index: usize,
    buoys: Vec<(EntityId, buoyancy::Point)>,
    swimmers: Vec<(EntityId, swimming::Point)>,
    checkpoint_refs: std::collections::BTreeMap<EntityId, amoris_sim::ContentHash>,
}

/// `physics.water`'s report for the presenters: the tick each region's tiles were last stepped.
/// Bookkeeping only (a restore empties it and the feed resends every tile).
#[derive(Resource, Default, Debug)]
pub struct WaterSteps {
    /// Per region, the tick each tile was last stepped (0: not since the resource was made).
    pub regions: std::collections::BTreeMap<EntityId, Vec<u64>>,
}

/// One tick of one region (water.md 4): the substeps over the awake tiles and the tiles beside
/// them, then drying, then sleep. Sources, drains, edits and bed changes are applied before.
pub fn step_region(r: &mut WaterRegion, dt: f64, grid: &mut solve::Grid) -> Stepped {
    solve::tick(r, dt, grid)
}

/// Whether the region has work this tick: an awake tile.
pub fn awake(r: &WaterRegion) -> bool {
    r.tiles.iter().any(|t| !t.asleep())
}

fn event(kind: &str, subject: EntityId, data: Vec<(&str, PlainData)>) -> Option<NewEvent> {
    let kind = EventKind::new(kind).ok()?;
    let data =
        PlainData::object(data.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()).ok()?;
    Some(NewEvent::new(kind).subject(subject).data(data))
}

/// The corner of a region (or the origin of a volume) of an entity with this `Transform`.
fn position(t: Option<&Transform>) -> [f64; 3] {
    t.map_or([0.0; 3], |t| t.position)
}

/// How a region lies over the volume its beds come from: `None` for a bed that is not a volume's.
fn map_of(
    r: &WaterRegion,
    corner: [f64; 2],
    index: &EntityIndex,
    volumes: &VolumeQuery<'_, '_>,
) -> Option<Result<bed::Map, Problem>> {
    let BedSource::Voxels { volume, .. } = r.bed else {
        return None;
    };
    let found = index.get(volume).and_then(|e| volumes.get(e).ok());
    let Some((_, t, v)) = found else {
        return Some(Err(Problem::new(
            "water.misaligned",
            format!("the region's bed is volume {volume}, which has no Voxels"),
            detail([("volume", json!(volume))]),
        )));
    };
    bed::map(r, corner, v, position(t))
}

/// The volume of a region's `Voxels` bed.
fn volume_id(r: &WaterRegion) -> Option<EntityId> {
    match r.bed {
        BedSource::Voxels { volume, .. } => Some(volume),
        BedSource::Colliders { .. } => None,
    }
}

/// Checks every region against the world when a scene loads (water.md 5 and 6): a `Voxels` bed's
/// volume exists and the region sits on its grid (`water.misaligned`), and every field liquid cell
/// lies under a region whose bed is its volume (`water.uncovered`).
pub fn check_world(world: &bevy_ecs::prelude::World) -> Result<(), Problem> {
    let Some(index) = world.get_resource::<EntityIndex>() else {
        return Ok(());
    };
    let mut ids: Vec<(EntityId, bevy_ecs::entity::Entity)> = index.iter().collect();
    ids.sort_by_key(|(id, _)| *id);
    let mut maps: Vec<(EntityId, &WaterRegion, bed::Map)> = Vec::new();
    for (id, e) in &ids {
        let Some(r) = world.get::<WaterRegion>(*e) else {
            continue;
        };
        let p = position(world.get::<Transform>(*e));
        let Some(volume) = volume_id(r) else {
            continue;
        };
        let at = index.get(volume);
        let Some(v) = at.and_then(|e| world.get::<Voxels>(e)) else {
            return Err(Problem::new(
                "water.misaligned",
                format!("region {id}'s bed is volume {volume}, which has no Voxels"),
                detail([("region", json!(id)), ("volume", json!(volume))]),
            ));
        };
        let origin = position(at.and_then(|e| world.get::<Transform>(e)));
        match bed::map(r, [p[0], p[2]], v, origin) {
            Some(Err(mut p)) => {
                p.message = format!("region {id}: {}", p.message);
                return Err(p);
            }
            Some(Ok(m)) => maps.push((volume, r, m)),
            None => {}
        }
    }
    for (id, e) in &ids {
        let Some(v) = world.get::<Voxels>(*e) else {
            continue;
        };
        for (c, _) in field::cells(v, 0) {
            let covered = maps
                .iter()
                .any(|(vol, r, m)| vol == id && field::covers(r, m, c[0], c[2]).is_some());
            if !covered {
                return Err(field::uncovered(*id, c));
            }
        }
    }
    Ok(())
}

type VolumeQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static EntityId,
        Option<&'static Transform>,
        &'static mut Voxels,
    ),
>;
type FeedbackQuery<'w, 's> = Query<
    'w,
    's,
    (
        bevy_ecs::entity::Entity,
        &'static EntityId,
        &'static mut feedback::WaterFeedback,
    ),
>;
type DisplacementQuery<'w, 's> = Query<
    'w,
    's,
    (
        bevy_ecs::entity::Entity,
        &'static EntityId,
        &'static mut displacement::WaterDisplacement,
    ),
>;
type BuoyInput<'w, 's> = (
    Query<
        'w,
        's,
        (
            &'static EntityId,
            &'static Transform,
            &'static crate::boat::Floater,
        ),
    >,
    Query<
        'w,
        's,
        (
            &'static EntityId,
            &'static Transform,
            &'static crate::boat::Hull,
            &'static crate::boat::Boat,
        ),
    >,
    ResMut<'w, buoyancy::Samples>,
    FeedbackQuery<'w, 's>,
    DisplacementQuery<'w, 's>,
);
type ForcingInput<'w, 's> = (
    Query<'w, 's, (&'static EntityId, &'static WaterSource)>,
    Query<'w, 's, (&'static EntityId, &'static WaterDrain)>,
    Query<'w, 's, (&'static EntityId, &'static crate::Sea)>,
);
type RegionQuery<'w, 's> = Query<
    'w,
    's,
    (
        bevy_ecs::entity::Entity,
        &'static EntityId,
        Option<&'static Transform>,
        &'static mut WaterRegion,
        Option<&'static resident::WaterOutletStage>,
    ),
>;

/// `physics.water` (water.md 4), in three passes over the regions and volumes in id order:
///
/// 1. a region spawned without tiles takes its beds from its volume (or reports
///    `water.misaligned` once and stays flat);
/// 2. each volume's field liquid cells edited since the last tick's run become the water of the
///    first region over them whose bed is the volume, or are reported `water.uncovered`, and are
///    cleared;
/// 3. each region rereads the beds over the chunks edited since the last tick's run (any chunk
///    edited at the previous tick or this one: the change rule gives the same beds where nothing
///    changed), then its awake tiles are stepped.
#[allow(clippy::too_many_arguments)]
pub(crate) fn water(
    clock: Res<SimClock>,
    index: Res<EntityIndex>,
    mut scratch: ResMut<WaterScratch>,
    mut steps: ResMut<WaterSteps>,
    mut regions: RegionQuery<'_, '_>,
    mut volumes: VolumeQuery<'_, '_>,
    forcing: ForcingInput<'_, '_>,
    mut emit: amoris_sim::Emit,
    mut compute: Option<ResMut<compute::Backend>>,
    resident_states: Query<(&EntityId, &resident::WaterResident)>,
    mut checkpoints: ResMut<resident::WaterCheckpoints>,
    mut probes: Query<(
        bevy_ecs::entity::Entity,
        &EntityId,
        &mut resident::WaterProbe,
    )>,
    swimmers_input: (
        Query<(&EntityId, &Transform, &crate::walker::Walker)>,
        ResMut<swimming::Samples>,
    ),
    buoy_input: BuoyInput<'_, '_>,
    mut commands: bevy_ecs::prelude::Commands,
) -> Result<bool, Problem> {
    for event in std::mem::take(&mut scratch.3.events) {
        emit.emit(event);
    }
    let (floaters, hulls, mut buoy_samples, mut feedbacks, mut displacements) = buoy_input;
    let (walkers, mut swim_samples) = swimmers_input;
    let (sources, drains, seas) = forcing;
    let dt = clock.dt();
    let tick = clock.tick.0;
    let since = tick.saturating_sub(1);
    let resuming = scratch.1.is_some();
    let run = if let Some(run) = scratch.1.take() {
        run
    } else {
        // Sources and drains, in id order: (region, point, signed rate).
        let mut flows: Vec<(EntityId, EntityId, [f64; 2], f64)> = by_id(&sources)
            .filter_map(|(id, s)| Some((*id, s.region?, s.at, s.rate_m3s)))
            .collect();
        flows
            .extend(by_id(&drains).filter_map(|(id, d)| Some((*id, d.region?, d.at, -d.rate_m3s))));
        flows.sort_by_key(|f| f.0);
        let order: Vec<(bevy_ecs::entity::Entity, EntityId)> =
            by_id(&regions).map(|(e, id, _, _, _)| (e, *id)).collect();
        let checkpoint_refs: std::collections::BTreeMap<_, _> = by_id(&resident_states)
            .map(|(id, state)| (*id, state.recipe.checkpoint()))
            .collect();
        for (e, _, f) in by_id(&feedbacks) {
            f.validate_source(tick)?;
            if !regions
                .get(e)
                .is_ok_and(|(_, _, _, r, _)| r.solver == Solver::GpuResident)
            {
                return Err(Problem::new(
                    "water.feedback_requires_resident",
                    "WaterFeedback belongs on a gpu_resident WaterRegion",
                    detail([]),
                ));
            }
        }
        for (e, _, d) in by_id(&displacements) {
            d.validate_source(tick)?;
            if !regions
                .get(e)
                .is_ok_and(|(_, _, _, r, _)| r.solver == Solver::GpuResident)
            {
                return Err(Problem::new(
                    "water.displacement_requires_resident",
                    "WaterDisplacement belongs on a gpu_resident WaterRegion",
                    detail([]),
                ));
            }
        }
        let resident_ids: std::collections::BTreeSet<_> = by_id(&regions)
            .filter(|(_, _, _, r, _)| r.solver == Solver::GpuResident)
            .map(|(_, id, _, _, _)| *id)
            .collect();
        for (_, _, mut probe) in amoris_sim::order::by_id_mut(&mut probes) {
            if probe.region.is_none_or(|id| !resident_ids.contains(&id)) {
                probe.ready = false;
                probe.tick = tick;
            }
        }
        if order.is_empty() {
            let used: std::collections::BTreeSet<_> = checkpoint_refs.into_values().collect();
            let unused: Vec<_> = checkpoints
                .fields
                .keys()
                .filter(|key| !used.contains(key))
                .copied()
                .collect();
            for key in unused {
                checkpoints.fields.remove(&key);
            }
            return Ok(false);
        }
        if compute.is_none()
            && order.iter().any(|&(e, _)| {
                regions
                    .get(e)
                    .is_ok_and(|(_, _, _, r, _)| r.solver != Solver::Cpu)
            })
        {
            return Err(Problem::new(
                "water.compute_missing",
                "GPU water needs a host compute service; this host has none",
                detail([]),
            ));
        }
        // 1. Beds at load, and each region's map over its volume.
        let mut maps: Vec<Option<bed::Map>> = Vec::with_capacity(order.len());
        for &(e, id) in &order {
            let Ok((_, _, t, mut region, _)) = regions.get_mut(e) else {
                maps.push(None);
                continue;
            };
            let p = position(t);
            if region.solver != Solver::GpuResident && resident_states.get(e).is_ok() {
                return Err(resident::failed(
                    "materialize or explicitly reset resident state before changing solver mode",
                ));
            }
            let source = map_of(&region, [p[0], p[2]], &index, &volumes);
            if region.tiles.is_empty()
                && !(region.solver == Solver::GpuResident && resident_states.get(e).is_ok())
            {
                match &source {
                    Some(Ok(m)) => {
                        let found = volume_id(&region)
                            .and_then(|v| index.get(v))
                            .and_then(|e| volumes.get(e).ok());
                        if let Some((_, _, v)) = found {
                            bed::load(&mut region, m, v);
                        }
                    }
                    Some(Err(err)) => {
                        // Reported once: the region takes flat dry tiles and keeps no bed.
                        if let Some(ev) = event(
                            "water.misaligned",
                            id,
                            vec![("message", PlainData::String(err.message.clone()))],
                        ) {
                            emit.emit(ev);
                        }
                        let [tx, tz] = region.tiles_along();
                        region.tiles = (0..tx * tz)
                            .map(|n| {
                                let key = [n % tx, n / tx].map(|k| u32::try_from(k).unwrap_or(0));
                                WaterTile::dry(key, 0.0)
                            })
                            .collect();
                    }
                    None => {}
                }
            }
            maps.push(
                source
                    .and_then(Result::ok)
                    .filter(|_| region.well_formed() || resident_states.get(e).is_ok()),
            );
        }
        // 2. Field liquids into the regions.
        let vols: Vec<(bevy_ecs::entity::Entity, EntityId)> = by_id(&volumes)
            .filter_map(|(id, _, _)| index.get(*id).map(|e| (e, *id)))
            .collect();
        for (ve, vid) in vols {
            let Ok((_, _, v)) = volumes.get(ve) else {
                continue;
            };
            let cells = field::cells(v, since);
            if cells.is_empty() {
                continue;
            }
            for &(c, part) in &cells {
                let mut taken = false;
                for (n, &(e, _)) in order.iter().enumerate() {
                    let Some(m) = maps[n] else {
                        continue;
                    };
                    let Ok((_, _, _, region, _)) = regions.get(e) else {
                        continue;
                    };
                    if volume_id(region) != Some(vid) {
                        continue;
                    }
                    if let Some(at) = field::covers(region, &m, c[0], c[2])
                        && let Ok((_, _, _, mut region, _)) = regions.get_mut(e)
                    {
                        if region.solver == Solver::GpuResident {
                            return Err(resident::failed(
                                "field-cell conversion is not connected to resident water yet",
                            ));
                        }
                        field::pour(&mut region, &m, at, part);
                        taken = true;
                        break;
                    }
                }
                if !taken
                    && let Some(ev) = event(
                        "water.uncovered",
                        vid,
                        vec![(
                            "cell",
                            PlainData::Array(c.map(|x| PlainData::Number(f64::from(x))).to_vec()),
                        )],
                    )
                {
                    emit.emit(ev);
                }
            }
            if let Ok((_, _, mut v)) = volumes.get_mut(ve) {
                for (c, _) in cells {
                    // The cell was read from the volume: clearing it cannot fail.
                    let _ = v.set(c, 0, tick);
                }
            }
        }
        let mut buoys = Vec::new();
        let feedback_ids: std::collections::BTreeSet<_> =
            by_id(&feedbacks).map(|(_, id, _)| *id).collect();
        let mut live = std::collections::BTreeSet::new();
        let covers: Vec<_> = if floaters.is_empty() && walkers.is_empty() && hulls.is_empty() {
            Vec::new()
        } else {
            order
                .iter()
                .filter_map(|&(e, id)| {
                    let (_, _, t, r, _) = regions.get(e).ok()?;
                    buoyancy::Cover::new(id, position(t), r)
                })
                .collect()
        };
        // Nonresident fields are sampled directly by the force system; prepare no duplicate
        // body point lists for them. Keep CPU work independent of tile count per volume point.
        let resident_cover = covers.iter().any(|c| c.resident);
        for (body, tf, floater) in by_id(&floaters).filter(|_| resident_cover) {
            for (point, b) in floater.points.iter().enumerate() {
                let key = (*body, buoyancy::Role::Floater(point));
                live.insert(key);
                let at = geom::to_world(tf.position, tf.rotation, b.at);
                // First covering region owns this point, including a dry region over a sea.
                for cover in &covers {
                    if let Some(grid) = cover.grid_at(at) {
                        if cover.resident {
                            buoys.push((
                                cover.id,
                                buoyancy::Point::floater(
                                    *body,
                                    point,
                                    at,
                                    grid,
                                    feedback_ids.contains(&cover.id),
                                ),
                            ));
                        }
                        break;
                    }
                }
            }
        }
        for (body, tf, hull, _) in by_id(&hulls).filter(|_| resident_cover) {
            // Reject nonfinite poses even if no region would claim their undefined coordinates.
            if tf
                .position
                .iter()
                .chain(&tf.rotation)
                .any(|v| !v.is_finite())
            {
                return Err(buoyancy::hull_failed("Hull sampling pose must be finite"));
            }
            for role in buoyancy::HullPoint::ALL {
                let at = geom::to_world(tf.position, tf.rotation, role.local(hull));
                if at.iter().any(|v| !v.is_finite()) {
                    return Err(buoyancy::hull_failed("Hull sampling point must be finite"));
                }
                live.insert((*body, buoyancy::Role::Hull(role)));
                for cover in &covers {
                    if let Some(grid) = cover.grid_at(at) {
                        if cover.resident {
                            buoys.push((
                                cover.id,
                                buoyancy::Point::hull(
                                    *body,
                                    role,
                                    *tf,
                                    at,
                                    grid,
                                    feedback_ids.contains(&cover.id),
                                    cover,
                                )?,
                            ));
                        }
                        break;
                    }
                }
            }
        }
        buoy_samples.retain(&live);
        let mut swimmers = Vec::new();
        let mut live_swimmers = std::collections::BTreeSet::new();
        for (actor, pose, _) in by_id(&walkers).filter(|_| resident_cover) {
            swimming::validate_pose(pose)?;
            for &(e, id) in &order {
                let (_, _, tf, region, _) = regions
                    .get(e)
                    .map_err(|_| resident::failed("Walker region disappeared"))?;
                if region.solver != Solver::GpuResident && !region.well_formed() {
                    continue;
                }
                if let Some(point) = swimming::Point::new(*actor, *pose, position(tf), region)? {
                    // The first covering region owns the centre column, including dry water.
                    if region.solver == Solver::GpuResident {
                        live_swimmers.insert(*actor);
                        swimmers.push((id, point));
                    }
                    break;
                }
            }
        }
        swim_samples.retain(&live_swimmers);
        WaterRun {
            order,
            maps,
            flows,
            index: 0,
            buoys,
            swimmers,
            checkpoint_refs,
        }
    };
    let WaterRun {
        order,
        maps,
        flows,
        index: start,
        buoys,
        swimmers,
        mut checkpoint_refs,
    } = run;
    // 3. Bed changes, then the substeps. A resumed region has already received its inputs.
    let mut next_index = start;
    for n in start..order.len() {
        if n < next_index {
            continue;
        }
        let (e, id) = order[n];
        let Ok((_, _, t, region, _)) = regions.get(e) else {
            continue;
        };
        let p = position(t);
        if region.solver == Solver::GpuResident {
            use amoris_assets::water_field::{
                ColumnDelta, FieldStep, MAX_BATCH_COLUMNS, MAX_BATCH_REGIONS,
            };
            // A cohort never crosses a CPU/grid region or the transient host's bounded budget.
            // Include local-ready sleeping plans so their writes share receipt validation.
            let mut end = n;
            while end < order.len() && end - n < MAX_BATCH_REGIONS {
                let Ok((_, _, _, r, _)) = regions.get(order[end].0) else {
                    break;
                };
                if r.solver != Solver::GpuResident {
                    break;
                }
                end += 1;
            }
            let plans = if let Some(plans) = scratch.2.take() {
                plans
            } else {
                let mut plans = Vec::with_capacity(end - n);
                let mut columns = 0usize;
                for &(e, id) in &order[n..end] {
                    let (_, _, t, region, outlet) = regions
                        .get(e)
                        .map_err(|_| resident::failed("resident region disappeared"))?;
                    let p = position(t);
                    let mut controls = resident::ResidentControls::from_region(region, dt)?;
                    if let Some(outlet) = outlet {
                        outlet.validate()?;
                        for i in 0..4 {
                            if region.edges[i] == Edge::Outflow {
                                controls.outlet_levels[i] = outlet.levels[i];
                            }
                        }
                    }
                    let state = match resident_states.get(e) {
                        Ok((_, s)) => s.clone(),
                        Err(_) => resident::initial(region, &mut checkpoints)?,
                    };
                    let checkpoint = checkpoints
                        .fields
                        .get(&state.recipe.checkpoint())
                        .cloned()
                        .ok_or_else(|| resident::failed("resident checkpoint is unavailable"))?;
                    if checkpoint.size != region.size {
                        return Err(resident::failed(
                            "resident dimensions changed without a new seed",
                        ));
                    }
                    if state.completed_tick > 0
                        && let Some(ve) = volume_id(region).and_then(|v| index.get(v))
                        && let Ok((_, _, v)) = volumes.get(ve)
                        && !bed::edited_since(v, since).is_empty()
                    {
                        return Err(resident::failed("resident bed edits are not connected yet"));
                    }
                    let sea = if region.edges.contains(&Edge::Sea) {
                        let input = by_id(&seas).next().map(|(_, s)| s).ok_or_else(|| {
                            Problem::new(
                                "water.no_sea",
                                "a Sea boundary needs a Sea component",
                                detail([]),
                            )
                        })?;
                        Some(sea::prepare(
                            input,
                            region,
                            &checkpoint,
                            [p[0], p[2]],
                            crate::forces::start_time(&clock),
                        )?)
                    } else {
                        None
                    };
                    let mut edits = Vec::new();
                    for &(_, rid, at, rate) in &flows {
                        if rid == id
                            && rate != 0.0
                            && let Some((i, k)) = region.column_of([p[0], p[2]], at[0], at[1])
                        {
                            let column = u32::try_from(i + k * region.size[0] as usize)
                                .map_err(|_| resident::failed("resident source index overflow"))?;
                            edits.push(ColumnDelta {
                                column,
                                depth_m: rate * dt / (region.cell_m * region.cell_m),
                            });
                        }
                    }
                    let mut point_rows: Vec<_> = by_id(&probes)
                        .filter(|(_, _, probe)| probe.region == Some(id))
                        .map(|(e, _, probe)| (e, probe.at))
                        .collect();
                    let mut points = Vec::new();
                    let mut selected = Vec::new();
                    let mut reusable = true;
                    let mut invalid_probes = Vec::new();
                    for (probe_e, at) in point_rows.drain(..) {
                        if region.column_of([p[0], p[2]], at[0], at[1]).is_none() {
                            invalid_probes.push(probe_e);
                            continue;
                        }
                        let grid = [
                            (at[0] - p[0]) / region.cell_m,
                            (at[1] - p[2]) / region.cell_m,
                        ];
                        if let Ok((_, _, probe)) = probes.get(probe_e) {
                            reusable &= probe.matches(id, at, grid, &state.recipe);
                        }
                        points.push(grid);
                        selected.push((probe_e, at));
                    }
                    let selected_swimmers: Vec<_> = swimmers
                        .iter()
                        .filter(|(rid, _)| *rid == id)
                        .map(|(_, point)| point.clone())
                        .collect();
                    for point in &selected_swimmers {
                        // The local_ready gate below also requires a sleeping field with no
                        // advance, edits, impulses or changed controls. The ordinary column query
                        // then depends only on these unchanged field/column identities.
                        reusable &= swim_samples.can_rebind(point, id, &state.recipe);
                        points.push(point.grid);
                    }
                    let selected_buoys: Vec<_> = buoys
                        .iter()
                        .filter(|(rid, _)| *rid == id)
                        .map(|(_, point)| point.clone())
                        .collect();
                    for point in &selected_buoys {
                        reusable &= buoy_samples.matches(point, id, &state.recipe);
                        points.push(point.grid);
                    }
                    if points.len() > amoris_assets::water_field::MAX_SAMPLES {
                        return Err(resident::failed(
                            "resident water supports at most 4096 explicit probes, Walker, Floater and Hull points per region",
                        ));
                    }
                    let impulses = feedbacks
                        .get(e)
                        .map_or_else(|_| vec![], |(_, _, f)| f.pending.clone());
                    let displaced = displacements
                        .get(e)
                        .map_or_else(|_| vec![], |(_, _, d)| d.pending.clone());
                    let local_ready = state.still >= SLEEP_TICKS
                        && state.controls == controls
                        && !controls.open()
                        && edits.is_empty()
                        && impulses.is_empty()
                        && displaced.is_empty()
                        && reusable;
                    let mut positive = std::collections::BTreeMap::<u32, f64>::new();
                    for edit in &edits {
                        if edit.depth_m > 0.0 {
                            *positive.entry(edit.column).or_default() += edit.depth_m;
                        }
                    }
                    let extra = positive.values().copied().fold(0.0, math::max);
                    let inlet_depth =
                        controls
                            .boundaries
                            .iter()
                            .map(|e| match e {
                                amoris_assets::water_field::FieldBoundary::Inflow {
                                    m3s_per_m: q,
                                } if *q > 0.0 => math::pow(q * q / solve::G, 1.0 / 3.0),
                                _ => 0.0,
                            })
                            .fold(0.0, math::max);
                    let inlet_speed = math::sqrt(solve::G * inlet_depth);
                    let count = region.substeps.max(solve::substeps_for(
                        math::max(
                            math::max(state.stats.max_speed, inlet_speed),
                            sea.as_ref().map_or(0.0, |s| s.speed),
                        ) + math::sqrt(
                            solve::G
                                * math::max(
                                    math::max(state.stats.max_depth + extra, inlet_depth),
                                    sea.as_ref().map_or(0.0, |s| s.depth),
                                ),
                        ),
                        dt,
                        region.cell_m,
                    ));
                    if inlet_speed * dt / f64::from(count) / region.cell_m > 0.5 {
                        return Err(resident::failed(
                            "inflow exceeds the boundary CFL limit at eight substeps",
                        ));
                    }
                    if sea
                        .as_ref()
                        .is_some_and(|s| s.speed * dt / f64::from(count) / region.cell_m > 0.5)
                    {
                        return Err(resident::failed(
                            "sea forcing exceeds the boundary CFL limit at eight substeps",
                        ));
                    }
                    let step = if state.still >= SLEEP_TICKS
                        && state.controls == controls
                        && !controls.open()
                        && edits.is_empty()
                        && impulses.is_empty()
                        && displaced.is_empty()
                    {
                        None
                    } else {
                        Some(FieldStep {
                            sea: sea.map(|s| s.forcing),
                            boundaries: std::array::from_fn(|i| match controls.outlet_levels[i] {
                                Some(level) => amoris_assets::water_field::FieldBoundary::Stage {
                                    surface_relative_m: level - checkpoint.datum,
                                },
                                None => controls.boundaries[i],
                            }),
                            cell_m: region.cell_m,
                            substep_dt: dt / f64::from(count),
                            substeps: count,
                            friction: region.friction,
                            calm_per_s: region.calm_per_s,
                            dry_below: region.dry_below,
                            edits,
                            impulses,
                            displacements: displaced,
                            physics: region.physics.field(),
                        })
                    };
                    if !local_ready {
                        let count =
                            (region.size[0] as usize).saturating_mul(region.size[1] as usize);
                        if columns.saturating_add(count) > MAX_BATCH_COLUMNS {
                            break;
                        }
                        columns += count;
                    }
                    plans.push(resident::ResidentPlan {
                        entity: e,
                        region: id,
                        controls,
                        area: region.cell_m * region.cell_m,
                        local_ready,
                        feedback: feedbacks.get(e).ok().map(|(_, _, f)| f.clone()),
                        displacement: displacements.get(e).ok().map(|(_, _, d)| d.clone()),
                        invalid_probes,
                        state,
                        checkpoint,
                        step,
                        points,
                        probes: selected,
                        buoys: selected_buoys,
                        swimmers: selected_swimmers,
                    });
                }
                plans
            };
            if plans.is_empty() {
                return Err(resident::failed(
                    "resident region exceeds the batch column budget",
                ));
            }
            let end = n + plans.len();
            let requests: Vec<_> = plans
                .iter()
                .filter(|p| !p.local_ready)
                .map(resident::ResidentPlan::request)
                .collect();
            let outputs = if requests.is_empty() {
                Vec::new()
            } else {
                resident::validate_batch(&requests)?;
                let backend = compute
                    .as_mut()
                    .ok_or_else(|| resident::failed("resident compute service is unavailable"))?;
                match backend.0.poll_resident_batch(&requests)? {
                    std::task::Poll::Pending => {
                        scratch.2 = Some(plans);
                        scratch.1 = Some(WaterRun {
                            order,
                            maps,
                            flows,
                            index: n,
                            buoys,
                            swimmers,
                            checkpoint_refs,
                        });
                        return Ok(true);
                    }
                    std::task::Poll::Ready(outputs) => {
                        resident::validate_results(&requests, &outputs)?;
                        outputs
                    }
                }
            };
            let mut receipts = outputs.into_iter();
            let mut checked = Vec::with_capacity(plans.len());
            for mut plan in plans {
                let output = if plan.local_ready {
                    None
                } else {
                    Some(receipts.next().expect("validated receipt count").output)
                };
                plan.finish(output.as_ref(), tick)?;
                checked.push((plan, output));
            }
            // Only owned copies were changed above. No receipt, observation or feedback packet
            // enters the world until every member (including sleeping reuse) passed validation.
            for (plan, output) in checked {
                let id = plan.region;
                let e = plan.entity;
                for probe_e in plan.invalid_probes {
                    if let Ok((_, _, mut probe)) = probes.get_mut(probe_e) {
                        probe.ready = false;
                        probe.tick = tick;
                    }
                }
                if let Some(output) = output {
                    if let Some(checkpoint) = output.checkpoint {
                        checkpoints
                            .fields
                            .insert(plan.state.recipe.checkpoint(), checkpoint);
                    }
                    let probe_count = plan.probes.len();
                    let cell_m = plan.controls.cell_m;
                    for (((probe_e, at), grid), sample) in plan
                        .probes
                        .into_iter()
                        .zip(plan.points)
                        .zip(output.samples.iter().map(|s| column(*s, cell_m)))
                    {
                        if let Ok((_, _, mut probe)) = probes.get_mut(probe_e) {
                            probe.ready = true;
                            probe.tick = tick;
                            probe.sampled_at = at;
                            probe.state_digest = plan.state.recipe.digest;
                            probe.sampled_region = Some(id);
                            probe.sampled_grid = grid;
                            probe.sampled_checkpoint = plan.state.recipe.checkpoint().0;
                            probe.surface = sample[0];
                            probe.depth = sample[1];
                            probe.velocity = [sample[2], sample[3]];
                        }
                    }
                    let swimmer_count = plan.swimmers.len();
                    for (point, sample) in plan.swimmers.into_iter().zip(
                        output
                            .samples
                            .iter()
                            .map(|s| column(*s, cell_m))
                            .skip(probe_count),
                    ) {
                        swim_samples.record(point, id, &plan.state.recipe, tick, sample);
                    }
                    for (point, sample) in plan.buoys.into_iter().zip(
                        output
                            .samples
                            .iter()
                            .map(|s| column(*s, cell_m))
                            .skip(probe_count + swimmer_count),
                    ) {
                        buoy_samples.record(point, id, &plan.state.recipe, tick, sample);
                    }
                } else {
                    for (probe_e, _) in plan.probes {
                        if let Ok((_, _, mut probe)) = probes.get_mut(probe_e) {
                            probe.tick = tick;
                        }
                    }
                    for point in plan.swimmers {
                        swim_samples.reuse(&point, id, &plan.state.recipe, tick)?;
                    }
                    for point in plan.buoys {
                        buoy_samples.reuse(&point, id, &plan.state.recipe, tick);
                    }
                }
                if let Some(feedback) = plan.feedback
                    && let Ok((_, _, mut f)) = feedbacks.get_mut(e)
                {
                    *f = feedback;
                }
                if let Some(displacement) = plan.displacement
                    && let Ok((_, _, mut d)) = displacements.get_mut(e)
                {
                    *d = displacement;
                }
                if let Ok((_, _, _, mut r, _)) = regions.get_mut(e)
                    && !r.tiles.is_empty()
                {
                    r.tiles.clear();
                }
                checkpoint_refs.insert(id, plan.state.recipe.checkpoint());
                commands.entity(e).insert(plan.state);
            }
            next_index = end;
            continue;
        }
        let Ok((_, _, _, mut region, _)) = regions.get_mut(e) else {
            continue;
        };
        // Read first, so a region asleep is not marked changed.
        if !region.well_formed() {
            continue;
        }
        if !(resuming && n == start) {
            if let Some(m) = maps[n]
                && let Some(ve) = volume_id(&region).and_then(|v| index.get(v))
                && let Ok((_, _, v)) = volumes.get(ve)
            {
                let changed = bed::edited_since(v, since);
                let beds = bed::rebeds(&region, &m, v, &changed);
                if !beds.is_empty() {
                    for d in bed::apply(&mut region, &beds) {
                        let column = d.column.map(|c| PlainData::Number(geom::f64_of(c)));
                        if let Some(e) = event(
                            "water.displaced",
                            id,
                            vec![
                                ("column", PlainData::Array(column.to_vec())),
                                ("m3", PlainData::Number(d.m3)),
                            ],
                        ) {
                            emit.emit(e);
                        }
                    }
                }
            }
            for &(_, rid, at, rate) in &flows {
                if rid != id || rate == 0.0 || !rate.is_finite() {
                    continue;
                }
                // A drain over a dry column changes nothing: the region is not marked changed.
                if let Some((i, k)) = region.column_of([p[0], p[2]], at[0], at[1])
                    && (rate > 0.0 || region.depth_at(i, k) > 0.0)
                {
                    region.pour(i, k, rate * dt);
                }
            }
        }
        if !awake(&region) {
            continue;
        }
        let done = match compute.as_mut() {
            Some(c) => solve::poll_tick(&mut region, dt, &mut scratch.0, Some(c.0.as_mut()))?,
            None => solve::poll_tick(&mut region, dt, &mut scratch.0, None)?,
        };
        let done = match done {
            std::task::Poll::Ready(done) => done,
            std::task::Poll::Pending => {
                scratch.1 = Some(WaterRun {
                    order,
                    maps,
                    flows,
                    index: n,
                    buoys,
                    swimmers,
                    checkpoint_refs,
                });
                return Ok(true);
            }
        };
        let marks = steps
            .regions
            .entry(id)
            .or_insert_with(|| vec![0; region.tiles.len()]);
        marks.resize(region.tiles.len(), 0);
        for &t in &done.tiles {
            marks[t] = tick;
        }
        if done.dried_m3 > 0.0
            && let Some(e) = event(
                "water.dried",
                id,
                vec![("m3", PlainData::Number(done.dried_m3))],
            )
        {
            emit.emit(e);
        }
    }
    let used: std::collections::BTreeSet<_> = checkpoint_refs.into_values().collect();
    let unused: Vec<_> = checkpoints
        .fields
        .keys()
        .filter(|key| !used.contains(key))
        .copied()
        .collect();
    for key in unused {
        checkpoints.fields.remove(&key);
    }
    Ok(false)
}

#[cfg(test)]
pub(crate) mod tests;
