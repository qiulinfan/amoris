//! Vessels (water.md 62): bodies the water holds up by their hull, the convex hull of their
//! collider, cut at the water's surface every tick. The water's weight presses on the hull under
//! the surface and its flow on every face meeting it; water comes in through the hull's holes and
//! over an open hull's rim, and the water inside weighs the hull down.

use bevy_ecs::prelude::Component;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use amoris_sim::math;

use crate::body::{PartShape, Shape};
use crate::geom::{self, Q4, V3};
use crate::sea::GRAVITY;

/// A hole in a vessel's hull: where it is, in the body's frame, and its area, m^2.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hole {
    pub at: V3,
    pub area: f64,
}

/// A body the water holds up by its hull (water.md 62): the convex hull of its collider, or of the
/// part of it under `rim`, a boat open above its rim. No points are placed by hand: each tick the
/// hull is cut at the water's surface, the water presses on the part under it, and its flow on
/// each face meeting it. Water comes in through `holes` and over the rim, and the water inside
/// weighs the hull down: a vessel that takes in enough sinks. Its body needs a stated mass, or a
/// density on a ball, cuboid or capsule.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Vessel {
    /// The height of an open hull's rim in the body's frame: the hull is the collider's convex
    /// hull under it, and water over it pours in. None: the hull is decked, closed all round.
    pub rim: Option<f64>,
    /// Holes in the hull, through which the water outside and the water inside level.
    pub holes: Vec<Hole>,
    /// The pressure on a face that meets the flow, in dynamic pressures of the flow across it: at
    /// 1 a flat plate meets the water face-on with a drag coefficient of 1.
    pub pressure: f64,
    /// Scales the skin friction of the ITTC 1957 line along the faces under the water; 0 leaves
    /// it out.
    pub friction: f64,
    /// The water inside the hull, m^3. Written by the engine; a scene may start a vessel with some.
    pub flooded: f64,
    /// Written by the engine: the share of the hull's volume under the water.
    pub submerged: f64,
}

impl Default for Vessel {
    fn default() -> Self {
        Vessel {
            rim: None,
            holes: Vec::new(),
            pressure: 1.0,
            friction: 1.0,
            flooded: 0.0,
            submerged: 0.0,
        }
    }
}

impl Vessel {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let numbers = [self.pressure, self.friction, self.flooded];
        if numbers.iter().any(|v| !v.is_finite() || *v < 0.0)
            || self.rim.is_some_and(|r| !r.is_finite())
            || self
                .holes
                .iter()
                .any(|h| !geom::finite(&h.at) || !h.area.is_finite() || h.area <= 0.0)
        {
            return Err(
                "a vessel's coefficients and water are finite and not negative, its rim \
                        finite, its holes finite with an area above 0"
                    .into(),
            );
        }
        Ok(())
    }
}

pub(crate) fn failed(message: impl Into<String>) -> amoris_contract::Problem {
    amoris_contract::Problem::new(
        "water.vessel_invalid",
        message.into(),
        amoris_contract::detail([]),
    )
}

/// Vessels' hulls, built from their colliders and kept while a collider and a rim stay as they
/// are: derived, never persisted.
#[derive(bevy_ecs::prelude::Resource, Default)]
pub(crate) struct Hulls(
    std::collections::BTreeMap<
        amoris_sim::EntityId,
        (Shape, Option<f64>, std::sync::Arc<HullMesh>),
    >,
);

impl Hulls {
    pub(crate) fn get(
        &mut self,
        id: amoris_sim::EntityId,
        shape: &Shape,
        rim: Option<f64>,
    ) -> Result<std::sync::Arc<HullMesh>, amoris_contract::Problem> {
        if let Some((s, r, hull)) = self.0.get(&id)
            && s == shape
            && *r == rim
        {
            return Ok(hull.clone());
        }
        let hull = std::sync::Arc::new(
            HullMesh::new(shape, rim).map_err(|e| failed(format!("entity {id}: {e}")))?,
        );
        self.0.insert(id, (shape.clone(), rim, hull.clone()));
        Ok(hull)
    }

    /// Forgets the hulls of bodies that are no longer vessels.
    pub(crate) fn retain(&mut self, live: &std::collections::BTreeSet<amoris_sim::EntityId>) {
        self.0.retain(|id, _| live.contains(id));
    }
}

/// The discharge coefficient of a sharp-edged hole (its vena contracta) and of the weir its rim
/// makes: Q = c a sqrt(2 g h) through a hole, (2/3) c l sqrt(2 g) h^(3/2) over a rim.
const DISCHARGE: f64 = 0.61;
/// The water's kinematic viscosity, m^2/s (fresh water at 20 C), for the skin friction's
/// Reynolds number.
const VISCOSITY: f64 = 1.0e-6;
/// The longest edge of a hull's triangles as a share of its largest extent, as far apart as the
/// lattice's nodes, and at least `LATTICE_STEP`: the water's surface is taken as a plane across
/// each triangle.
const EDGE_SHARE: f64 = 1.0 / 8.0;
/// Segments around a ball's or a capsule's polygonal hull, and rings from its equator to a pole.
const SEGMENTS: u32 = 24;
const RINGS: u32 = 6;
/// A vessel's lattice of water samples: at most this many nodes along each side...
const LATTICE_SIDE: usize = 9;
/// ...and no closer than this, m.
const LATTICE_STEP: f64 = 0.25;

/// The closed surface a vessel floats by, in its body's frame: triangles facing out.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HullMesh {
    pub triangles: Vec<[V3; 3]>,
    pub volume: f64,
    /// An open hull's rim, its corners in order round it; empty when the hull is decked.
    pub rim: Vec<V3>,
    /// The hull's largest extent, m: the length of its skin friction's Reynolds number.
    pub length: f64,
}

fn signed_volume(t: &[V3; 3]) -> f64 {
    geom::dot(t[0], geom::cross(t[1], t[2])) / 6.0
}

/// Points on a polygonal sphere of `radius`, its upper half raised by `half_height` and its lower
/// half lowered by it: a ball, or a capsule along +y.
fn round_points(radius: f64, half_height: f64) -> Vec<V3> {
    let mut points = Vec::new();
    for ring in 0..=RINGS {
        let (s, c) = math::sin_cos(math::FRAC_PI_2 * f64::from(ring) / f64::from(RINGS));
        let count = if ring == RINGS { 1 } else { SEGMENTS };
        for k in 0..count {
            let (sa, ca) = math::sin_cos(math::TAU * f64::from(k) / f64::from(SEGMENTS));
            let (x, y, z) = (radius * c * ca, radius * s, radius * c * sa);
            points.push([x, half_height + y, z]);
            points.push([x, -half_height - y, z]);
        }
    }
    points
}

/// The radius of a polygonal ball or capsule that holds the volume of the round one: its hull
/// holds k^3 v + k^2 2 h a for a radius k r, v the polygonal ball's volume and a its equator's
/// area at r.
fn round_radius(radius: f64, half_height: f64) -> Result<f64, String> {
    let ball: f64 = convex_hull(&round_points(radius, 0.0))?
        .iter()
        .map(signed_volume)
        .sum();
    let ring =
        0.5 * f64::from(SEGMENTS) * radius * radius * math::sin(math::TAU / f64::from(SEGMENTS));
    let target = math::PI * radius * radius * (4.0 / 3.0 * radius + 2.0 * half_height);
    let mut k = 1.0;
    for _ in 0..8 {
        let f = k * k * k * ball + k * k * 2.0 * half_height * ring - target;
        let slope = 3.0 * k * k * ball + 4.0 * k * half_height * ring;
        k -= f / slope;
    }
    Ok(k * radius)
}

fn part_points(shape: &PartShape) -> Result<Vec<V3>, String> {
    Ok(match shape {
        PartShape::Ball { radius } => round_points(round_radius(*radius, 0.0)?, 0.0),
        PartShape::Cuboid { half_extents: h } => {
            let mut corners = Vec::with_capacity(8);
            for i in [-1.0, 1.0] {
                for j in [-1.0, 1.0] {
                    for k in [-1.0, 1.0] {
                        corners.push([h[0] * i, h[1] * j, h[2] * k]);
                    }
                }
            }
            corners
        }
        PartShape::Capsule {
            half_height,
            radius,
        } => round_points(round_radius(*radius, *half_height)?, *half_height),
        PartShape::ConvexHull { points } => points.clone(),
    })
}

/// The points whose convex hull is a collider's hull, in its body's frame.
fn shape_points(shape: &Shape) -> Result<Vec<V3>, String> {
    Ok(match shape {
        Shape::Ball { radius } => part_points(&PartShape::Ball { radius: *radius })?,
        Shape::Cuboid { half_extents } => part_points(&PartShape::Cuboid {
            half_extents: *half_extents,
        })?,
        Shape::Capsule {
            half_height,
            radius,
        } => part_points(&PartShape::Capsule {
            half_height: *half_height,
            radius: *radius,
        })?,
        Shape::ConvexHull { points } => points.clone(),
        Shape::TriMesh { vertices, .. } => vertices.clone(),
        Shape::Compound { parts } => {
            let mut points = Vec::new();
            for p in parts {
                let rotation = geom::normalized(p.rotation).unwrap_or(geom::IDENTITY);
                for q in part_points(&p.shape)? {
                    points.push(geom::to_world(p.position, rotation, q));
                }
            }
            points
        }
    })
}

/// The box around a collider's shape in its body's frame, a little wider than a polygonal ball's
/// or capsule's hull reaches.
fn shape_bounds(shape: &Shape) -> (V3, V3) {
    let part = |shape: &PartShape| -> (V3, V3) {
        match shape {
            PartShape::Ball { radius } => {
                let r = 1.02 * radius;
                ([-r; 3], [r; 3])
            }
            PartShape::Cuboid { half_extents: h } => ([-h[0], -h[1], -h[2]], *h),
            PartShape::Capsule {
                half_height,
                radius,
            } => {
                let r = 1.02 * radius;
                ([-r, -half_height - r, -r], [r, half_height + r, r])
            }
            PartShape::ConvexHull { points } => bounds(points.iter()),
        }
    };
    match shape {
        Shape::Ball { radius } => part(&PartShape::Ball { radius: *radius }),
        Shape::Cuboid { half_extents } => part(&PartShape::Cuboid {
            half_extents: *half_extents,
        }),
        Shape::Capsule {
            half_height,
            radius,
        } => part(&PartShape::Capsule {
            half_height: *half_height,
            radius: *radius,
        }),
        Shape::ConvexHull { points } => bounds(points.iter()),
        Shape::TriMesh { vertices, .. } => bounds(vertices.iter()),
        Shape::Compound { parts } => {
            let mut corners = Vec::with_capacity(8 * parts.len());
            for p in parts {
                let rotation = geom::normalized(p.rotation).unwrap_or(geom::IDENTITY);
                let (lo, hi) = part(&p.shape);
                for c in box_corners(lo, hi) {
                    corners.push(geom::to_world(p.position, rotation, c));
                }
            }
            bounds(corners.iter())
        }
    }
}

fn box_corners(lo: V3, hi: V3) -> [V3; 8] {
    std::array::from_fn(|n| {
        [
            if n & 1 == 0 { lo[0] } else { hi[0] },
            if n & 2 == 0 { lo[1] } else { hi[1] },
            if n & 4 == 0 { lo[2] } else { hi[2] },
        ]
    })
}

/// The corners of the box around points.
fn bounds<'a>(points: impl Iterator<Item = &'a V3>) -> (V3, V3) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in points {
        for a in 0..3 {
            lo[a] = math::min(lo[a], p[a]);
            hi[a] = math::max(hi[a], p[a]);
        }
    }
    (lo, hi)
}

/// The convex hull of `points`, its triangles facing out (Rapier's quickhull, in the solver's
/// `f32`, as it builds a hull collider).
fn convex_hull(points: &[V3]) -> Result<Vec<[V3; 3]>, String> {
    let input: Vec<_> = points.iter().map(|p| geom::vec_in(*p)).collect();
    let (vertices, indices) = rapier3d::parry::transformation::try_convex_hull(&input)
        .map_err(|e| format!("the collider's points span no volume ({e:?})"))?;
    let vertices: Vec<V3> = vertices.into_iter().map(geom::vec_out).collect();
    let mut triangles: Vec<[V3; 3]> = indices
        .iter()
        .map(|t| t.map(|i| vertices[i as usize]))
        .collect();
    let volume: f64 = triangles.iter().map(signed_volume).sum();
    if triangles.is_empty() || volume == 0.0 || !volume.is_finite() {
        return Err("the collider's points span no volume".into());
    }
    if volume < 0.0 {
        for t in &mut triangles {
            t.swap(1, 2);
        }
    }
    Ok(triangles)
}

/// The 2D convex hull of points of a plane, by their x and z (A. M. Andrew's monotone chain).
fn plane_hull(mut points: Vec<V3>) -> Vec<V3> {
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[2].total_cmp(&b[2])));
    points.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-9 && (a[2] - b[2]).abs() < 1e-9);
    if points.len() < 3 {
        return points;
    }
    let turn = |o: V3, a: V3, b: V3| (a[0] - o[0]) * (b[2] - o[2]) - (a[2] - o[2]) * (b[0] - o[0]);
    let mut hull: Vec<V3> = Vec::with_capacity(2 * points.len());
    for pass in 0..2 {
        let start = hull.len();
        let order: Vec<V3> = if pass == 0 {
            points.clone()
        } else {
            points.iter().rev().copied().collect()
        };
        for p in order {
            while hull.len() >= start + 2
                && turn(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0
            {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// A point and a value linear over its triangle.
type Carried = [f64; 4];

fn carried(p: V3, value: f64) -> Carried {
    [p[0], p[1], p[2], value]
}

fn point(c: Carried) -> V3 {
    [c[0], c[1], c[2]]
}

/// The part of a triangle where its carried value is at most 0: none, all of it, or one or two
/// triangles cut along value 0, each keeping the triangle's winding.
fn cut(t: [Carried; 3]) -> ([[Carried; 3]; 2], usize) {
    let lerp = |a: Carried, b: Carried| {
        let w = a[3] / (a[3] - b[3]);
        std::array::from_fn::<f64, 4, _>(|k| a[k] + (b[k] - a[k]) * w)
    };
    let under = t.iter().filter(|c| c[3] <= 0.0).count();
    match under {
        0 => ([t; 2], 0),
        3 => ([t; 2], 1),
        1 => {
            let i = (0..3).find(|&i| t[i][3] <= 0.0).unwrap_or(0);
            let (a, b, c) = (t[i], t[(i + 1) % 3], t[(i + 2) % 3]);
            ([[a, lerp(a, b), lerp(a, c)], t], 1)
        }
        _ => {
            let i = (0..3).find(|&i| t[i][3] > 0.0).unwrap_or(0);
            let (c, a, b) = (t[i], t[(i + 1) % 3], t[(i + 2) % 3]);
            let bc = lerp(b, c);
            let ac = lerp(a, c);
            ([[a, b, bc], [a, bc, ac]], 2)
        }
    }
}

/// Splits each triangle into n^2 by its edges' n-ths, n making its longest edge at most `edge`.
fn subdivide(triangles: Vec<[V3; 3]>, edge: f64) -> Vec<[V3; 3]> {
    let mut out = Vec::with_capacity(triangles.len());
    for t in triangles {
        let longest = (0..3)
            .map(|i| geom::length(geom::sub(t[(i + 1) % 3], t[i])))
            .fold(0.0, math::max);
        // A hull's extent is finite and its edges at least `LATTICE_STEP` long: n is small.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = math::max(1.0, (longest / edge).ceil()) as u32;
        let at = |i: u32, j: u32| {
            let (u, v) = (f64::from(i) / f64::from(n), f64::from(j) / f64::from(n));
            geom::add(
                t[0],
                geom::add(
                    geom::scale(geom::sub(t[1], t[0]), u),
                    geom::scale(geom::sub(t[2], t[0]), v),
                ),
            )
        };
        for j in 0..n {
            for i in 0..n - j {
                out.push([at(i, j), at(i + 1, j), at(i, j + 1)]);
                if i + j + 2 <= n {
                    out.push([at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]);
                }
            }
        }
    }
    out
}

impl HullMesh {
    /// The hull of a collider's shape, under `rim` when it is open.
    pub(crate) fn new(shape: &Shape, rim: Option<f64>) -> Result<HullMesh, String> {
        let mut triangles = convex_hull(&shape_points(shape)?)?;
        let (lo, hi) = bounds(triangles.iter().flatten());
        let mut opening = Vec::new();
        if let Some(rim) = rim {
            if !(rim > lo[1] && rim < hi[1]) {
                return Err(format!(
                    "the rim at {rim} m lies outside the hull's height, {} to {} m",
                    lo[1], hi[1]
                ));
            }
            let mut below = Vec::with_capacity(triangles.len());
            for t in &triangles {
                let (parts, n) = cut(t.map(|p| carried(p, p[1] - rim)));
                for part in &parts[..n] {
                    below.push(part.map(point));
                    for c in part {
                        if c[3].abs() <= 1e-12 {
                            opening.push([c[0], rim, c[2]]);
                        }
                    }
                }
            }
            opening = plane_hull(opening);
            if opening.len() < 3 {
                return Err("the rim cuts no opening across the hull".into());
            }
            // The opening, closed for the water's pressure (water.md 62.3), its fan facing up.
            for i in 1..opening.len() - 1 {
                let t = [opening[0], opening[i], opening[i + 1]];
                let n = geom::cross(geom::sub(t[1], t[0]), geom::sub(t[2], t[0]));
                below.push(if n[1] >= 0.0 { t } else { [t[0], t[2], t[1]] });
            }
            triangles = below;
        }
        let length = (0..3).map(|a| hi[a] - lo[a]).fold(0.0, math::max);
        let triangles = subdivide(triangles, math::max(LATTICE_STEP, length * EDGE_SHARE));
        let volume = triangles.iter().map(signed_volume).sum();
        Ok(HullMesh {
            triangles,
            volume,
            rim: opening,
            length,
        })
    }
}

/// Where a vessel samples the water (water.md 62.1): a lattice over its footprint, aligned with
/// its heading, at most `LATTICE_SIDE` nodes along a side and `LATTICE_STEP` or more apart. The
/// water system asks for the same nodes, from the same pose, as the force system reads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Lattice {
    origin: V3,
    axes: [V3; 2],
    step: [f64; 2],
    count: [usize; 2],
}

impl Lattice {
    /// The lattice over the box around a collider's shape, at a pose.
    pub(crate) fn new(shape: &Shape, position: V3, rotation: Q4) -> Lattice {
        let (lo, hi) = shape_bounds(shape);
        let x = geom::rotate(rotation, [1.0, 0.0, 0.0]);
        let flat = math::sqrt(x[0] * x[0] + x[2] * x[2]);
        let ex = if flat > 1e-6 {
            [x[0] / flat, 0.0, x[2] / flat]
        } else {
            [1.0, 0.0, 0.0]
        };
        let axes = [ex, [-ex[2], 0.0, ex[0]]];
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for c in box_corners(lo, hi) {
            let w = geom::rotate(rotation, c);
            for a in 0..2 {
                let d = geom::dot(w, axes[a]);
                min[a] = math::min(min[a], d);
                max[a] = math::max(max[a], d);
            }
        }
        let mut step = [0.0; 2];
        let mut count = [2; 2];
        for a in 0..2 {
            let extent = math::max(max[a] - min[a], 0.0);
            // At most `LATTICE_SIDE` nodes: the cast takes a small whole number.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let n = math::clamp(
                (extent / LATTICE_STEP).ceil() + 1.0,
                2.0,
                geom::f64_of(LATTICE_SIDE),
            ) as usize;
            count[a] = n;
            step[a] = extent / geom::f64_of(n - 1);
        }
        let origin = geom::add(
            [position[0], position[1], position[2]],
            geom::add(geom::scale(axes[0], min[0]), geom::scale(axes[1], min[1])),
        );
        Lattice {
            origin,
            axes,
            step,
            count,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.count[0] * self.count[1]
    }

    /// Node `n`, at the height of the body's position.
    pub(crate) fn node(&self, n: usize) -> V3 {
        let (i, j) = (n % self.count[0], n / self.count[0]);
        geom::add(
            self.origin,
            geom::add(
                geom::scale(self.axes[0], self.step[0] * geom::f64_of(i)),
                geom::scale(self.axes[1], self.step[1] * geom::f64_of(j)),
            ),
        )
    }

    /// The node area, m^2: the water a node stands for.
    pub(crate) fn area(&self) -> f64 {
        math::max(self.step[0], LATTICE_STEP) * math::max(self.step[1], LATTICE_STEP)
    }

    /// The four nodes around a point, by their bilinear weights.
    pub(crate) fn around(&self, p: V3) -> [(usize, f64); 4] {
        let d = geom::sub(p, self.origin);
        let mut cell = [0usize; 2];
        let mut t = [0.0; 2];
        for a in 0..2 {
            let last = self.count[a] - 1;
            let u = if self.step[a] > 0.0 {
                math::clamp(
                    geom::dot(d, self.axes[a]) / self.step[a],
                    0.0,
                    geom::f64_of(last),
                )
            } else {
                0.0
            };
            // Clamped to the lattice: a small whole number.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let i = (u.floor() as usize).min(last - 1);
            cell[a] = i;
            t[a] = u - geom::f64_of(i);
        }
        let n = |i: usize, j: usize| (cell[0] + i) + self.count[0] * (cell[1] + j);
        [
            (n(0, 0), (1.0 - t[0]) * (1.0 - t[1])),
            (n(1, 0), t[0] * (1.0 - t[1])),
            (n(0, 1), (1.0 - t[0]) * t[1]),
            (n(1, 1), t[0] * t[1]),
        ]
    }
}

/// The water a node of a lattice takes the hull's volume into: a region with WaterDisplacement,
/// the node's place in it, the region's columns and their size.
pub(crate) struct Spread {
    pub region: amoris_sim::EntityId,
    pub grid: [f64; 2],
    pub size: [u32; 2],
    pub cell: f64,
}

/// The water at a lattice node: its surface (where the vessel's load takes it), its velocity and
/// its density.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NodeWater {
    pub surface: f64,
    pub velocity: V3,
    pub density: f64,
}

/// The water's surface at a point, between the nodes around it.
pub(crate) fn surface_at(lattice: &Lattice, nodes: &[Option<NodeWater>], p: V3) -> Option<f64> {
    water_at(lattice, nodes, p).map(|w| w.surface)
}

/// The water at a point between the nodes around it: their weights over the nodes with water.
fn water_at(lattice: &Lattice, nodes: &[Option<NodeWater>], p: V3) -> Option<NodeWater> {
    let mut weight = 0.0;
    let mut out = NodeWater {
        surface: 0.0,
        velocity: geom::ZERO,
        density: 0.0,
    };
    for (n, w) in lattice.around(p) {
        if w <= 0.0 {
            continue;
        }
        if let Some(Some(here)) = nodes.get(n) {
            weight += w;
            out.surface += w * here.surface;
            out.velocity = geom::add(out.velocity, geom::scale(here.velocity, w));
            out.density += w * here.density;
        }
    }
    (weight > 0.0).then(|| NodeWater {
        surface: out.surface / weight,
        velocity: geom::scale(out.velocity, 1.0 / weight),
        density: out.density / weight,
    })
}

/// The body's motion a vessel's load needs: its pose, its centre of mass in the world, and the
/// velocity of a point of it.
pub(crate) struct Motion<'a> {
    pub position: V3,
    pub rotation: Q4,
    pub com: V3,
    pub velocity: &'a dyn Fn(V3) -> V3,
}

/// The water's load on a hull (water.md 62.2): its force and torque about the centre of mass, the
/// hull's volume under the water, and the volume each lattice node holds under it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Load {
    pub force: V3,
    pub torque: V3,
    pub volume: f64,
    pub nodes: Vec<f64>,
    /// The flow's horizontal push on the water at each node over the tick, N s.
    pub push: Vec<[f64; 2]>,
}

/// The hydrodynamics of the faces meeting the flow: `pressure` and `friction` as `Vessel` has
/// them, the hull's length, the mass that moves with the hull and the tick.
pub(crate) struct Flow {
    pub pressure: f64,
    pub friction: f64,
    pub length: f64,
    pub mass: f64,
    pub dt: f64,
}

/// The skin friction coefficient of the ITTC 1957 model-ship correlation line, 0.075 / (log10 Re
/// - 2)^2, its Reynolds number taken no lower than 1e5, where the line begins.
fn ittc(speed: f64, length: f64) -> f64 {
    let re = math::max(speed * length / VISCOSITY, 1.0e5);
    let d = math::log10(re) - 2.0;
    0.075 / (d * d)
}

/// The water's load on the hull: the weight of the water above each face under the surface, each
/// vertex where it will be `ahead` seconds on against the surface the nodes give, and with `flow`
/// the pressure and skin friction of the flow over each face and the pushes on the water.
pub(crate) fn load(
    world: &[[V3; 3]],
    lattice: &Lattice,
    nodes: &[Option<NodeWater>],
    motion: &Motion<'_>,
    ahead: f64,
    flow: Option<&Flow>,
) -> Load {
    let mut out = Load {
        nodes: vec![0.0; lattice.len()],
        push: vec![[0.0; 2]; lattice.len()],
        ..Load::default()
    };
    struct Face {
        centre: V3,
        area: f64,
        normal: V3,
        water: NodeWater,
    }
    let mut faces = Vec::new();
    for world in world {
        let depths = world.map(|p| {
            water_at(lattice, nodes, p).map_or(f64::NEG_INFINITY, |w| {
                w.surface - (p[1] + ahead * (motion.velocity)(p)[1])
            })
        });
        if depths.iter().all(|d| *d <= 0.0) {
            continue;
        }
        let corners = std::array::from_fn(|i| carried(world[i], -math::max(depths[i], -1.0e6)));
        let (parts, n) = cut(corners);
        for part in &parts[..n] {
            let p = part.map(point);
            let d = part.map(|c| math::max(-c[3], 0.0));
            let normal2 = geom::cross(geom::sub(p[1], p[0]), geom::sub(p[2], p[0]));
            let twice = geom::length(normal2);
            if twice <= 0.0 {
                continue;
            }
            let area = 0.5 * twice;
            let normal = geom::scale(normal2, 1.0 / twice);
            let sum = d[0] + d[1] + d[2];
            let centre = geom::scale(geom::add(p[0], geom::add(p[1], p[2])), 1.0 / 3.0);
            let Some(water) = water_at(lattice, nodes, centre) else {
                continue;
            };
            // The water's weight over the face: rho g times the depth, linear over it, at its
            // centre of pressure, ((sum d)(sum p) + sum d p) / (4 sum d).
            let force = geom::scale(normal, -water.density * GRAVITY * area * sum / 3.0);
            let at = if sum > 0.0 {
                let mut weighted = geom::scale(geom::add(p[0], geom::add(p[1], p[2])), sum);
                for k in 0..3 {
                    weighted = geom::add(weighted, geom::scale(p[k], d[k]));
                }
                geom::scale(weighted, 1.0 / (4.0 * sum))
            } else {
                centre
            };
            out.force = geom::add(out.force, force);
            out.torque = geom::add(out.torque, geom::cross(geom::sub(at, motion.com), force));
            // The column of water the face holds under the surface: its projected area times the
            // mean depth, below the face for a face looking down, less above one looking up.
            let held = -normal[1] * area * sum / 3.0;
            out.volume += held;
            for (node, w) in lattice.around(centre) {
                out.nodes[node] += w * held;
                if let Some(flow) = flow {
                    out.push[node][0] -= w * force[0] * flow.dt;
                    out.push[node][1] -= w * force[2] * flow.dt;
                }
            }
            faces.push(Face {
                centre,
                area,
                normal,
                water,
            });
        }
    }
    let Some(flow) = flow else {
        return out;
    };
    let wetted: f64 = faces.iter().map(|f| f.area).sum();
    for f in &faces {
        let u = geom::sub((motion.velocity)(f.centre), f.water.velocity);
        let speed = geom::length(u);
        if speed <= 0.0 {
            continue;
        }
        // Each face's share of the moving mass takes its own drag implicitly, c / (1 + c dt / m),
        // so one tick never turns the flow over it around.
        let share = flow.mass * f.area / wetted;
        let implicit = |c: f64| c / (1.0 + c * flow.dt / share);
        let half_rho = 0.5 * f.water.density;
        let un = geom::dot(u, f.normal);
        let mut force = geom::ZERO;
        // A face moving into the water meets its pressure, half rho |u| (u . n) along its normal:
        // a plate's drag face-on, its lift and drag at an angle to the flow. A face moving away
        // leaves a wake that pulls it back hardly at all.
        if un > 0.0 && flow.pressure > 0.0 {
            let c = implicit(half_rho * flow.pressure * f.area * speed);
            force = geom::scale(f.normal, -c * un);
        }
        if flow.friction > 0.0 {
            let ut = geom::sub(u, geom::scale(f.normal, un));
            let tangential = geom::length(ut);
            let cf = flow.friction * ittc(speed, flow.length);
            let c = implicit(half_rho * cf * f.area * tangential);
            force = geom::add(force, geom::scale(ut, -c));
        }
        out.force = geom::add(out.force, force);
        out.torque = geom::add(
            out.torque,
            geom::cross(geom::sub(f.centre, motion.com), force),
        );
        for (node, w) in lattice.around(f.centre) {
            out.push[node][0] -= w * force[0] * flow.dt;
            out.push[node][1] -= w * force[2] * flow.dt;
        }
    }
    out
}

/// The water inside a hull (water.md 62.3): its level, a world height, and its centre, from the
/// columns between the hull under that level and the level.
pub(crate) struct Inside {
    pub level: f64,
    pub centre: V3,
}

/// The volume of the hull under the world height `level`, and the moments of the water that
/// would fill it: x z weighted by depth, and the column's centre height.
fn under_level(world: &[[V3; 3]], level: f64) -> (f64, V3) {
    let mut volume = 0.0;
    let mut moment = geom::ZERO;
    for t in world {
        let (parts, n) = cut(t.map(|p| carried(p, p[1] - level)));
        for part in &parts[..n] {
            let p = part.map(point);
            let d = part.map(|c| math::max(-c[3], 0.0));
            let normal2 = geom::cross(geom::sub(p[1], p[0]), geom::sub(p[2], p[0]));
            // The projected area, signed: positive for a face looking down.
            let projected = -0.5 * normal2[1];
            let s1 = d[0] + d[1] + d[2];
            if s1 <= 0.0 {
                continue;
            }
            let held = projected * s1 / 3.0;
            let mut weighted = geom::scale(geom::add(p[0], geom::add(p[1], p[2])), s1);
            for k in 0..3 {
                weighted = geom::add(weighted, geom::scale(p[k], d[k]));
            }
            let across = geom::scale(weighted, 1.0 / (4.0 * s1));
            // A column's centre is half its depth under the level: the depth-weighted mean of
            // d / 2 over the face is (sum d^2 + sum_{i<j} d_i d_j) / (4 sum d).
            let s2 =
                d[0] * d[0] + d[1] * d[1] + d[2] * d[2] + d[0] * d[1] + d[1] * d[2] + d[2] * d[0];
            let height = level - s2 / (4.0 * s1);
            volume += held;
            moment = geom::add(moment, geom::scale([across[0], height, across[2]], held));
        }
    }
    (volume, moment)
}

/// Where the water inside a hull stands: the level under which the hull holds `flooded`, found
/// between its lowest and highest points by false position with the Illinois step (M. Dowell and P.
/// Jarratt, "A modified regula falsi method for computing the root of an equation", BIT 11(2),
/// 1971), to a millionth of the hull's volume.
pub(crate) fn inside(world: &[[V3; 3]], flooded: f64, volume: f64) -> Option<Inside> {
    if flooded <= 0.0 || world.is_empty() {
        return None;
    }
    let (lo, hi) = bounds(world.iter().flatten());
    let (mut a, mut b) = (lo[1], hi[1]);
    let (mut fa, mut fb) = (-flooded, volume - flooded);
    let mut level = b;
    let mut side = 0;
    for _ in 0..40 {
        if fb - fa <= 0.0 {
            break;
        }
        level = math::clamp(b - fb * (b - a) / (fb - fa), a, b);
        let f = under_level(world, level).0 - flooded;
        if f.abs() <= 1.0e-6 * volume {
            break;
        }
        if f < 0.0 {
            a = level;
            fa = f;
            if side == -1 {
                fb *= 0.5;
            }
            side = -1;
        } else {
            b = level;
            fb = f;
            if side == 1 {
                fa *= 0.5;
            }
            side = 1;
        }
    }
    let (held, moment) = under_level(world, level);
    (held > 0.0).then(|| Inside {
        level,
        centre: geom::scale(moment, 1.0 / held),
    })
}

/// The water that comes in over a tick, m^3/s (out when negative): through each hole, a sharp
/// orifice between the heads of the water outside and inside over it, and over an open hull's
/// rim, a weir taking the difference of those heads over each edge.
pub(crate) fn inflow(
    vessel: &Vessel,
    hull: &HullMesh,
    motion: &Motion<'_>,
    outside: &dyn Fn(V3) -> Option<f64>,
    level: Option<f64>,
) -> f64 {
    let head = |p: V3| {
        let out = outside(p).map_or(0.0, |s| math::max(s - p[1], 0.0));
        let inn = level.map_or(0.0, |l| math::max(l - p[1], 0.0));
        out - inn
    };
    let mut q = 0.0;
    for hole in &vessel.holes {
        let p = geom::to_world(motion.position, motion.rotation, hole.at);
        let h = head(p);
        q += DISCHARGE * hole.area * h.signum() * math::sqrt(2.0 * GRAVITY * h.abs());
    }
    let rim: Vec<V3> = hull
        .rim
        .iter()
        .map(|p| geom::to_world(motion.position, motion.rotation, *p))
        .collect();
    for (k, a) in rim.iter().enumerate() {
        let b = rim[(k + 1) % rim.len()];
        let length = geom::length(geom::sub(b, *a));
        let weir = |h: f64| h.signum() * math::pow(h.abs(), 1.5);
        let mean = 0.5 * (weir(head(*a)) + weir(head(b)));
        q += 2.0 / 3.0 * DISCHARGE * length * math::sqrt(2.0 * GRAVITY) * mean;
    }
    q
}

/// The hull's triangles in the world.
pub(crate) fn in_world(hull: &HullMesh, position: V3, rotation: Q4) -> Vec<[V3; 3]> {
    hull.triangles
        .iter()
        .map(|t| t.map(|p| geom::to_world(position, rotation, p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(level: f64, lattice: &Lattice) -> Vec<Option<NodeWater>> {
        vec![
            Some(NodeWater {
                surface: level,
                velocity: geom::ZERO,
                density: 1000.0,
            });
            lattice.len()
        ]
    }

    fn still(_: V3) -> V3 {
        geom::ZERO
    }

    #[test]
    fn hulls_hold_their_shapes_volumes() {
        let ball = HullMesh::new(&Shape::Ball { radius: 0.5 }, None).unwrap();
        let exact = 4.0 / 3.0 * math::PI * 0.125;
        assert!((ball.volume / exact - 1.0).abs() < 1e-6, "{}", ball.volume);
        let capsule = HullMesh::new(
            &Shape::Capsule {
                half_height: 0.6,
                radius: 0.3,
            },
            None,
        )
        .unwrap();
        let exact = math::PI * 0.09 * (4.0 / 3.0 * 0.3 + 1.2);
        assert!(
            (capsule.volume / exact - 1.0).abs() < 1e-6,
            "{}",
            capsule.volume
        );
        let cuboid = HullMesh::new(
            &Shape::Cuboid {
                half_extents: [1.0, 0.5, 2.0],
            },
            None,
        )
        .unwrap();
        assert!((cuboid.volume - 8.0).abs() < 1e-9, "{}", cuboid.volume);
        // Open at y = 0: the lower half, its opening the box's section.
        let open = HullMesh::new(
            &Shape::Cuboid {
                half_extents: [1.0, 0.5, 2.0],
            },
            Some(0.0),
        )
        .unwrap();
        assert!((open.volume - 4.0).abs() < 1e-9, "{}", open.volume);
        assert_eq!(open.rim.len(), 4);
        assert!(HullMesh::new(&Shape::Ball { radius: 0.5 }, Some(0.6)).is_err());
    }

    #[test]
    fn a_hull_under_a_flat_surface_feels_archimedes_at_its_centre_of_buoyancy() {
        let shape = Shape::Cuboid {
            half_extents: [1.0, 0.5, 2.0],
        };
        let hull = HullMesh::new(&shape, None).unwrap();
        // Rolled 20 degrees about z, its centre at the surface.
        let (s, c) = math::sin_cos(math::PI / 18.0);
        let rotation = [0.0, 0.0, s, c];
        let lattice = Lattice::new(&shape, geom::ZERO, rotation);
        let nodes = flat(0.0, &lattice);
        let motion = Motion {
            position: geom::ZERO,
            rotation,
            com: geom::ZERO,
            velocity: &still,
        };
        let world = in_world(&hull, geom::ZERO, rotation);
        let out = load(&world, &lattice, &nodes, &motion, 0.0, None);
        // A box half under, rolled: still half its volume.
        assert!((out.volume - 4.0).abs() < 1e-9, "{}", out.volume);
        let lift = 1000.0 * GRAVITY * 4.0;
        assert!((out.force[1] - lift).abs() < 1e-6 * lift, "{:?}", out.force);
        assert!(out.force[0].abs() < 1e-6 * lift && out.force[2].abs() < 1e-6 * lift);
        // The centre of buoyancy, from the torque: the centroid of the wedge under the surface.
        let (v, m) = under_level(&world, 0.0);
        assert!((v - 4.0).abs() < 1e-9);
        let x = m[0] / v;
        let expected = out.torque[2] / out.force[1];
        assert!((x - expected).abs() < 1e-9, "{x} against {expected}");
        assert!((out.nodes.iter().sum::<f64>() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn water_inside_levels_and_comes_in_through_a_hole() {
        let shape = Shape::Cuboid {
            half_extents: [1.0, 0.5, 1.0],
        };
        let hull = HullMesh::new(&shape, Some(0.4)).unwrap();
        let world = in_world(&hull, geom::ZERO, geom::IDENTITY);
        // 1.2 m^3 in a 2 x 2 m hull: 0.3 m deep over its bottom at -0.5.
        let w = inside(&world, 1.2, hull.volume).unwrap();
        assert!((w.level + 0.2).abs() < 1e-9, "{}", w.level);
        assert!((w.centre[1] + 0.35).abs() < 1e-9, "{:?}", w.centre);
        let mut vessel = Vessel {
            rim: Some(0.4),
            holes: vec![Hole {
                at: [0.0, -0.5, 0.0],
                area: 0.01,
            }],
            ..Vessel::default()
        };
        let motion = Motion {
            position: geom::ZERO,
            rotation: geom::IDENTITY,
            com: geom::ZERO,
            velocity: &still,
        };
        // The surface outside 0.3 m over the hole: Torricelli's speed through it.
        let q = inflow(&vessel, &hull, &motion, &|_| Some(-0.2), None);
        let expected = 0.61 * 0.01 * math::sqrt(2.0 * GRAVITY * 0.3);
        assert!((q - expected).abs() < 1e-12, "{q}");
        // Level inside and out: nothing flows.
        let q = inflow(&vessel, &hull, &motion, &|_| Some(-0.2), Some(-0.2));
        assert!(q.abs() < 1e-12);
        // Over the rim: a weir 8 m round, 5 cm under the water.
        vessel.holes.clear();
        let q = inflow(&vessel, &hull, &motion, &|_| Some(0.45), None);
        let expected = 2.0 / 3.0 * 0.61 * 8.0 * math::sqrt(2.0 * GRAVITY) * math::pow(0.05, 1.5);
        assert!((q - expected).abs() < 1e-9, "{q} against {expected}");
    }
}
