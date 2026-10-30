//! Navigation over walkable ground (docs/spec/animation.md 8; Recast's first stage, a walkable
//! heightfield, without its polygons): a `NavGrid` names an area, a cell size and its agents; the
//! first query bakes it from the solver's fixed colliders (every cell's walkable floors, found by
//! downward rays, kept where the agent's capsule fits and the slope allows, linked to neighbours
//! within a step), and paths are A* over the floors, straightened where a straight line stays on
//! walkable ground. Everything is read from the world, so every host bakes the same grid.

use std::collections::{BTreeMap, BinaryHeap};
use std::sync::Arc;

use amoris_sim::math;
use amoris_sim::{EntityId, EntityIndex};
use bevy_ecs::prelude::{Component, Resource, World};
use rapier3d::prelude::{QueryFilter, Ray};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::geom::{self, V3};
use crate::solver::Physics;

/// An area agents find their way across.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct NavGrid {
    /// The area's lower and upper corners in the world, metres.
    pub min: V3,
    pub max: V3,
    /// A cell's edge, metres.
    pub cell_m: f64,
    /// The agents' radius and height, metres.
    pub radius_m: f64,
    pub height_m: f64,
    /// The highest step between neighbouring cells, metres, and the steepest walkable slope.
    pub step_m: f64,
    pub slope_deg: f64,
}

impl Default for NavGrid {
    fn default() -> Self {
        NavGrid {
            min: [-32.0, -8.0, -32.0],
            max: [32.0, 24.0, 32.0],
            cell_m: 0.5,
            radius_m: 0.35,
            height_m: 1.8,
            step_m: 0.45,
            slope_deg: 40.0,
        }
    }
}

/// The most floors a column keeps (bridges over courtyards, a stair's landings).
const LAYERS: usize = 4;

/// A baked grid: per column its floors' heights, per floor its links.
#[derive(Debug)]
pub struct Baked {
    grid: NavGrid,
    nx: usize,
    nz: usize,
    /// Per column, its floors' heights (ascending).
    floors: Vec<Vec<f64>>,
}

/// A floor of a column: (column, layer).
type Node = (usize, usize);

/// A cell count or index as a real (grids are at most 1024 cells a side).
fn real(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// A signed cell count or index (grids are at most 1024 cells a side).
fn signed(n: usize) -> i64 {
    i64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// A whole, non-negative real as a cell count or index, capped at `cap`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0..=cap first
fn cells(x: f64, cap: usize) -> usize {
    math::clamp(x, 0.0, real(cap)) as usize
}

/// A cell index known to be in 0..grid size, from its signed form.
fn unsigned(n: i64) -> usize {
    usize::try_from(n).unwrap_or(0)
}

impl Baked {
    fn column(&self, x: f64, z: f64) -> Option<usize> {
        let i = ((x - self.grid.min[0]) / self.grid.cell_m).floor();
        let k = ((z - self.grid.min[2]) / self.grid.cell_m).floor();
        if i < 0.0 || k < 0.0 || i >= real(self.nx) || k >= real(self.nz) {
            return None;
        }
        Some(cells(i, self.nx) + self.nx * cells(k, self.nz))
    }

    fn centre(&self, c: usize) -> (f64, f64) {
        let (i, k) = (c % self.nx, c / self.nx);
        (
            self.grid.min[0] + (real(i) + 0.5) * self.grid.cell_m,
            self.grid.min[2] + (real(k) + 0.5) * self.grid.cell_m,
        )
    }

    fn point(&self, n: Node) -> V3 {
        let (x, z) = self.centre(n.0);
        [x, self.floors[n.0][n.1], z]
    }

    /// The floor of the column under `p` nearest its height (within a step and a body above), if
    /// any.
    fn node_at(&self, p: V3) -> Option<Node> {
        let c = self.column(p[0], p[2])?;
        let fs = &self.floors[c];
        let mut best: Option<(f64, usize)> = None;
        for (l, &h) in fs.iter().enumerate() {
            let d = (h - p[1]).abs();
            if h <= p[1] + self.grid.step_m + 0.25 && best.is_none_or(|b| d < b.0) {
                best = Some((d, l));
            }
        }
        best.map(|(_, l)| (c, l))
    }

    /// The neighbours of a floor with the step between them within reach; a diagonal needs both
    /// cells beside it walkable at that height (no corner cutting).
    fn links(&self, n: Node, out: &mut Vec<(Node, f64)>) {
        out.clear();
        let (i, k) = (signed(n.0 % self.nx), signed(n.0 / self.nx));
        let h = self.floors[n.0][n.1];
        let step = self.grid.step_m;
        let at = |di: i64, dk: i64| -> Option<Node> {
            let (a, b) = (i + di, k + dk);
            if a < 0 || b < 0 || a >= signed(self.nx) || b >= signed(self.nz) {
                return None;
            }
            let c = unsigned(a) + self.nx * unsigned(b);
            self.floors[c]
                .iter()
                .position(|&g| (g - h).abs() <= step)
                .map(|l| (c, l))
        };
        for (di, dk) in [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            if di != 0 && dk != 0 && (at(di, 0).is_none() || at(0, dk).is_none()) {
                continue;
            }
            if let Some(m) = at(di, dk) {
                let cost = if di != 0 && dk != 0 {
                    std::f64::consts::SQRT_2
                } else {
                    1.0
                };
                out.push((m, cost * self.grid.cell_m));
            }
        }
    }

    /// Whether a straight walk from `a` to `b` stays on floors within a step of each other.
    fn clear(&self, a: Node, b: Node) -> bool {
        let (pa, pb) = (self.point(a), self.point(b));
        let d = math::sqrt((pb[0] - pa[0]) * (pb[0] - pa[0]) + (pb[2] - pa[2]) * (pb[2] - pa[2]));
        let steps = cells((d / (0.5 * self.grid.cell_m)).ceil(), 1 << 20).max(1);
        let mut h = pa[1];
        for s in 1..=steps {
            let u = real(s) / real(steps);
            let (x, z) = (pa[0] + (pb[0] - pa[0]) * u, pa[2] + (pb[2] - pa[2]) * u);
            let Some(c) = self.column(x, z) else {
                return false;
            };
            // The cell's corners around the line must be walkable too (the agent's width).
            let Some(l) = self.floors[c]
                .iter()
                .position(|&g| (g - h).abs() <= self.grid.step_m)
            else {
                return false;
            };
            h = self.floors[c][l];
        }
        (h - pb[1]).abs() <= self.grid.step_m
    }

    /// A path from `from` to `to`: waypoints on the floors, the first where `from` stands, the
    /// last at `to`'s floor; none when either is off the grid or no way joins them.
    pub fn path(&self, from: V3, to: V3) -> Option<Vec<V3>> {
        let (s, g) = (self.node_at(from)?, self.node_at(to)?);
        let key = |n: Node| n.0 * LAYERS + n.1;
        let goal = self.point(g);
        let h = |n: Node| {
            let p = self.point(n);
            math::sqrt((p[0] - goal[0]) * (p[0] - goal[0]) + (p[2] - goal[2]) * (p[2] - goal[2]))
        };
        // Open set ordered by (f, then node key) for a deterministic tie-break.
        #[derive(PartialEq)]
        struct Open(f64, usize, Node);
        impl Eq for Open {}
        impl PartialOrd for Open {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        impl Ord for Open {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
            }
        }
        let mut best: BTreeMap<usize, (f64, Option<Node>)> = BTreeMap::new();
        let mut open = BinaryHeap::new();
        best.insert(key(s), (0.0, None));
        open.push(Open(h(s), key(s), s));
        let mut links = Vec::new();
        let mut found = false;
        let mut expanded = 0usize;
        while let Some(Open(_, k, n)) = open.pop() {
            if n == g {
                found = true;
                break;
            }
            expanded += 1;
            if expanded > 200_000 {
                break;
            }
            let gn = best.get(&k).map_or(f64::INFINITY, |b| b.0);
            self.links(n, &mut links);
            for &(m, cost) in &links {
                let km = key(m);
                let gm = gn + cost;
                if best.get(&km).is_none_or(|b| gm < b.0) {
                    best.insert(km, (gm, Some(n)));
                    open.push(Open(gm + h(m), km, m));
                }
            }
        }
        if !found {
            return None;
        }
        let mut nodes = vec![g];
        let mut at = g;
        while let Some(&(_, Some(p))) = best.get(&key(at)) {
            nodes.push(p);
            at = p;
        }
        nodes.reverse();
        // Straightened: from each kept node, the furthest node a straight walk reaches.
        let mut kept = vec![nodes[0]];
        let mut i = 0;
        while i + 1 < nodes.len() {
            let mut j = nodes.len() - 1;
            while j > i + 1 && !self.clear(nodes[i], nodes[j]) {
                j -= 1;
            }
            kept.push(nodes[j]);
            i = j;
        }
        let mut out: Vec<V3> = kept.iter().map(|&n| self.point(n)).collect();
        if let Some(last) = out.last_mut() {
            *last = [to[0], last[1], to[2]];
        }
        Some(out)
    }

    /// The walkable point nearest `at` within `radius` metres (its own column first).
    pub fn nearest(&self, at: V3, radius: f64) -> Option<V3> {
        if let Some(n) = self.node_at(at) {
            return Some(self.point(n));
        }
        let r = signed(cells((radius / self.grid.cell_m).ceil(), 1024)).max(1);
        let c0 = self.column(at[0], at[2])?;
        let (i0, k0) = (signed(c0 % self.nx), signed(c0 / self.nx));
        let mut best: Option<(f64, V3)> = None;
        for dk in -r..=r {
            for di in -r..=r {
                let (i, k) = (i0 + di, k0 + dk);
                if i < 0 || k < 0 || i >= signed(self.nx) || k >= signed(self.nz) {
                    continue;
                }
                let c = unsigned(i) + self.nx * unsigned(k);
                for l in 0..self.floors[c].len() {
                    let p = self.point((c, l));
                    let d = (p[0] - at[0]) * (p[0] - at[0])
                        + (p[1] - at[1]) * (p[1] - at[1])
                        + (p[2] - at[2]) * (p[2] - at[2]);
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, p));
                    }
                }
            }
        }
        best.map(|b| b.1)
    }
}

/// The baked grids, by the grid's entity, with the settings they were baked with: rebuilt when
/// those change. Derived from the world (its colliders), never persisted.
#[derive(Resource, Default)]
pub struct NavCache {
    grids: BTreeMap<EntityId, (NavGrid, Arc<Baked>)>,
}

/// Bakes `grid` from the solver's fixed colliders.
pub fn bake(physics: &Physics, grid: &NavGrid) -> Baked {
    let cell = if grid.cell_m > 0.01 { grid.cell_m } else { 0.5 };
    let nx = cells(((grid.max[0] - grid.min[0]) / cell).ceil(), 1024).max(1);
    let nz = cells(((grid.max[2] - grid.min[2]) / cell).ceil(), 1024).max(1);
    let min_up = math::cos(grid.slope_deg.to_radians());
    let filter = QueryFilter::only_fixed();
    // The agent's body above a step's height (what it steps over is no obstacle).
    let tall = math::max(grid.height_m - grid.step_m, 2.0 * grid.radius_m + 0.02);
    let body = rapier3d::parry::shape::Capsule::new_y(
        geom::f32_of(math::max(0.5 * tall - grid.radius_m, 0.01)),
        geom::f32_of(grid.radius_m),
    );
    let mut floors = vec![Vec::new(); nx * nz];
    for k in 0..nz {
        for i in 0..nx {
            let x = grid.min[0] + (real(i) + 0.5) * cell;
            let z = grid.min[2] + (real(k) + 0.5) * cell;
            let mut top = grid.max[1];
            let mut found: Vec<f64> = Vec::new();
            while found.len() < LAYERS && top > grid.min[1] {
                let ray = Ray::new(geom::vec_in([x, top, z]), geom::vec_in([0.0, -1.0, 0.0]));
                let Some((_, hit)) = physics.world.cast_ray_and_get_normal(
                    &ray,
                    geom::f32_of(top - grid.min[1]),
                    true,
                    filter,
                ) else {
                    break;
                };
                let t = f64::from(hit.time_of_impact);
                let y = top - t;
                let up = f64::from(hit.normal.y);
                if up >= min_up {
                    // The agent's capsule standing here must touch nothing.
                    let centre = [x, y + grid.step_m + 0.5 * tall, z];
                    let pose = geom::pose_in(centre, geom::IDENTITY);
                    let blocked = physics
                        .world
                        .intersect_shape(pose, &body, filter)
                        .next()
                        .is_some();
                    if !blocked {
                        found.push(y);
                    }
                }
                // Below this surface, at least an agent's height down for the next floor.
                top = y - math::max(grid.height_m, 0.5);
            }
            found.reverse();
            floors[i + nx * k] = found;
        }
    }
    Baked {
        grid: NavGrid {
            cell_m: cell,
            ..*grid
        },
        nx,
        nz,
        floors,
    }
}

/// The baked grid of entity `grid`: from the cache, or baked now (and kept).
pub fn baked(world: &mut World, grid: EntityId) -> Option<Arc<Baked>> {
    let e = world.get_resource::<EntityIndex>()?.get(grid)?;
    let settings = *world.get::<NavGrid>(e)?;
    if let Some((s, b)) = world
        .get_resource::<NavCache>()
        .and_then(|c| c.grids.get(&grid))
        && *s == settings
    {
        return Some(b.clone());
    }
    let b = Arc::new(bake(world.get_resource::<Physics>()?, &settings));
    world
        .get_resource_or_insert_with(NavCache::default)
        .grids
        .insert(grid, (settings, b.clone()));
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::{BodyKind, Collider, RigidBody, Shape, Transform};

    fn world_with(boxes: &[(V3, V3)]) -> amoris_sim::Sim {
        let mut sim = amoris_sim::Sim::new(amoris_sim::SimConfig {
            rate: amoris_sim::TickRate::DEFAULT,
            seed: 1,
        })
        .unwrap();
        crate::plugin(&mut sim).unwrap();
        let mut b = sim.boundary();
        for (at, half) in boxes {
            b.spawn((
                Transform {
                    position: *at,
                    rotation: geom::IDENTITY,
                },
                RigidBody {
                    kind: BodyKind::Fixed,
                    ..RigidBody::default()
                },
                Collider::new(Shape::Cuboid {
                    half_extents: *half,
                }),
            ))
            .unwrap();
        }
        sim.step(&mut amoris_sim::NoHooks).unwrap();
        sim
    }

    #[test]
    fn a_path_goes_round_a_wall_and_never_through_it() {
        // A floor 20 m square, a wall across its middle with a gap at x > 6.
        let sim = world_with(&[
            ([0.0, -0.5, 0.0], [10.0, 0.5, 10.0]),
            ([-2.0, 1.0, 0.0], [8.0, 1.0, 0.25]),
        ]);
        let grid = NavGrid {
            min: [-10.0, -2.0, -10.0],
            max: [10.0, 6.0, 10.0],
            ..NavGrid::default()
        };
        let physics = sim.world().get_resource::<Physics>().unwrap();
        let b = bake(physics, &grid);
        let path = b
            .path([-5.0, 0.0, -5.0], [-5.0, 0.0, 5.0])
            .expect("a way round");
        assert!(path.iter().any(|p| p[0] > 6.0), "{path:?}");
        for w in path.windows(2) {
            // No segment crosses the wall's line west of the gap.
            let (a, c) = (w[0], w[1]);
            if (a[2] < 0.0) != (c[2] < 0.0) {
                let u = (0.0 - a[2]) / (c[2] - a[2]);
                let x = a[0] + (c[0] - a[0]) * u;
                assert!(x > 6.0, "crosses at x {x}: {path:?}");
            }
        }
    }

    #[test]
    fn a_ramp_is_walked_and_a_cliff_is_not() {
        // A raised terrace 3 m up, reached by a ramp of boxes 0.3 m apart in height at x in 0..6.
        let mut boxes = vec![
            ([0.0, -0.5, 0.0], [20.0, 0.5, 20.0]),
            ([0.0, 1.5, 10.0], [20.0, 1.5, 4.0]),
        ];
        // Stairs 0.3 m a step, 0.6 m deep, from z 0 up to the terrace's edge at z 6.
        for s in 0..10 {
            let top = 0.3 * f64::from(s + 1);
            boxes.push((
                [3.0, 0.5 * top, 0.3 + 0.6 * f64::from(s)],
                [3.0, 0.5 * top, 0.3],
            ));
        }
        let sim = world_with(&boxes);
        let grid = NavGrid {
            min: [-10.0, -2.0, -6.0],
            max: [10.0, 8.0, 14.0],
            ..NavGrid::default()
        };
        let b = bake(sim.world().get_resource::<Physics>().unwrap(), &grid);
        let path = b.path([-5.0, 0.0, -2.0], [-5.0, 3.0, 11.0]);
        assert!(path.is_some(), "the terrace is reached by the ramp");
        let p = path.unwrap();
        // Straightened, the way runs up the stairs from their foot to the terrace's edge.
        assert!(
            p.iter().any(|q| q[0] > 0.0 && q[0] < 6.0 && q[2] < 1.0),
            "{p:?}"
        );
        // The terrace's face is no way up: from just below it, the way still goes by the stairs.
        let round = b
            .path([-5.0, 0.0, 5.0], [-5.0, 3.0, 7.0])
            .expect("by the stairs");
        assert!(round.iter().any(|q| q[0] > 0.0), "{round:?}");
    }
}
