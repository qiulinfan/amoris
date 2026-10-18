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
pub mod field;
pub(crate) mod solve;

use amoris_contract::{Problem, detail};
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
/// A tile is still when no depth changed by more than this over the tick, metres.
const STILL_DEPTH: f64 = 1e-6;
/// ... and no face ran faster than this, m/s.
const STILL_SPEED: f64 = 1e-4;
/// A film dries only when every face of its column runs slower than this, m/s.
const DRY_SPEED: f64 = 0.05;

/// A region of water columns over a bed (water.md 3). Its corner is the entity's
/// `Transform.position` in x and z (y unused); the rotation must be the identity.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaterRegion {
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
    /// At least this many substeps a tick (1 to 8).
    #[serde(default = "two")]
    pub substeps: u32,
    /// Films thinner than this dry, metres (0: never; at most 0.05).
    #[serde(default)]
    pub dry_below: f64,
    /// Presentation, carried as the palette carries a liquid's fog (water.md 9).
    #[serde(default)]
    pub look: WaterLook,
    /// Every tile, in key order (z, then x); empty: all dry, the beds read from `bed` at the next
    /// `physics.water` (water.md 5).
    #[serde(default)]
    pub tiles: Vec<WaterTile>,
}

fn one() -> f64 {
    1.0
}

fn friction() -> f64 {
    0.02
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

/// How a region looks (water.md 9). Presentation only: the solver never reads it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct WaterLook {
    /// The view from under the surface: linear colour and fog density per metre.
    pub fog: [f32; 4],
    /// Absorption per metre, linear rgb (the drawing and the splashes).
    pub absorb: [f32; 3],
    pub style: Style,
}

impl Default for WaterLook {
    fn default() -> Self {
        WaterLook {
            fog: [0.05, 0.16, 0.45, 0.12],
            absorb: [0.9, 0.35, 0.2],
            style: Style::Smooth,
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

    /// The water's volume, m^3.
    pub fn volume(&self) -> f64 {
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
            if *e != Edge::Wall {
                return Err(invalid(
                    &format!("edges.{a}"),
                    format!(
                        "edge {e:?} is not built yet: this engine's regions have walls (water.md 2, \
                         slices C and D)"
                    ),
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
            BedSource::Colliders { .. } => {
                return Err(invalid(
                    "bed",
                    "beds from colliders are not built yet (water.md 2, slice D)".into(),
                ));
            }
        }
        let look = &self.look;
        if !(look.fog.iter().chain(&look.absorb).all(|x| x.is_finite())) {
            return Err(invalid("look", "the look's values must be finite".into()));
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

/// The solver's working buffers, reused from tick to tick. Every buffer is overwritten before it is
/// read, so it carries no game state.
#[derive(Resource, Default)]
pub struct WaterScratch(pub(crate) solve::Grid);

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
type RegionQuery<'w, 's> = Query<
    'w,
    's,
    (
        bevy_ecs::entity::Entity,
        &'static EntityId,
        Option<&'static Transform>,
        &'static mut WaterRegion,
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
pub(crate) fn water(
    clock: Res<SimClock>,
    index: Res<EntityIndex>,
    mut scratch: ResMut<WaterScratch>,
    mut steps: ResMut<WaterSteps>,
    mut regions: RegionQuery<'_, '_>,
    mut volumes: VolumeQuery<'_, '_>,
    mut emit: amoris_sim::Emit,
) {
    let dt = clock.dt();
    let tick = clock.tick.0;
    let since = tick.saturating_sub(1);
    let order: Vec<(bevy_ecs::entity::Entity, EntityId)> =
        by_id(&regions).map(|(e, id, _, _)| (e, *id)).collect();
    if order.is_empty() {
        return;
    }
    // 1. Beds at load, and each region's map over its volume.
    let mut maps: Vec<Option<bed::Map>> = Vec::with_capacity(order.len());
    for &(e, id) in &order {
        let Ok((_, _, t, mut region)) = regions.get_mut(e) else {
            maps.push(None);
            continue;
        };
        let p = position(t);
        let source = map_of(&region, [p[0], p[2]], &index, &volumes);
        if region.tiles.is_empty() {
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
        maps.push(source.and_then(Result::ok).filter(|_| region.well_formed()));
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
                let Ok((_, _, _, region)) = regions.get(e) else {
                    continue;
                };
                if volume_id(region) != Some(vid) {
                    continue;
                }
                if let Some(at) = field::covers(region, &m, c[0], c[2])
                    && let Ok((_, _, _, mut region)) = regions.get_mut(e)
                {
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
    // 3. Bed changes, then the substeps.
    for (n, &(e, id)) in order.iter().enumerate() {
        let Ok((_, _, _, mut region)) = regions.get_mut(e) else {
            continue;
        };
        // Read first, so a region asleep is not marked changed.
        if !region.well_formed() {
            continue;
        }
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
        if !awake(&region) {
            continue;
        }
        let done = step_region(&mut region, dt, &mut scratch.0);
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
}

#[cfg(test)]
mod tests;
