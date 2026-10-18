//! Shallow water on a height field (spike, docs/spikes/shallow-water.md): a grid of columns, each a
//! bed height and a water depth, moved by one of two schemes ([`Scheme`]):
//!
//! - **Advect** (the default): the shallow-water equations on a staggered grid, velocities on the
//!   columns' faces, after Chentanez and Mueller 2010. A step carries the velocities along
//!   themselves (semi-Lagrangian, bilinear), accelerates them down the surface's slope, damps them
//!   by bed friction, then moves the water through each face by its velocity times the upwind
//!   depth. Water keeps its momentum: it runs on from the foot of a fall and shoots through a gap.
//! - **Pipes**: the virtual pipe model (O'Brien and Hodgins 1995; Mei, Decaudin and Hu 2007). Each
//!   column keeps an outflow through a pipe to each neighbour, accelerated by the difference of the
//!   two surfaces. It carries no momentum from column to column, so water piles up where it lands.
//!
//! Both scale a column's outflows down so it never gives more water than it holds: water is
//! conserved to rounding.
//!
//! Deterministic: `f64`, `+ - * /` and `amoris_sim::math`'s `min` and `max` only, every loop in a
//! fixed order, so a step gives the same bits on every target, natively and in WebAssembly, in
//! debug and release (numeric.md 3.1 and 5). The terrain is anything that gives a bed height per
//! column; [`Shallow::from_voxels`] takes it from a block volume.

use amoris_sim::math;
use amoris_sim::voxels::{BlockShape, Voxels};
use rapier3d::prelude::{
    ColliderBuilder, ColliderHandle, PhysicsWorld, RigidBodyBuilder, RigidBodyHandle,
};

use crate::geom::{self, Q4, V3};

/// Standard gravity, m/s^2.
const G: f64 = 9.81;
/// A column with less water than this is dry, metres.
const DRY: f64 = 1e-4;

/// How [`Shallow::step`] moves the water (module documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Advect,
    Pipes,
}

/// The water over a grid of `nx` by `nz` columns of `cell_m` metres, column `(i, k)` at
/// `i + nx * k`. Its corner is the world's `(origin[0], origin[1])` in x and z; heights are world
/// y. The grid's edges are walls.
#[derive(Clone, Debug, PartialEq)]
pub struct Shallow {
    pub nx: usize,
    pub nz: usize,
    pub cell_m: f64,
    pub origin: [f64; 2],
    /// The ground's top, world y, per column.
    pub bed: Vec<f64>,
    /// Water above the bed, metres, per column (never below 0).
    pub depth: Vec<f64>,
    pub scheme: Scheme,
    /// Pipes: outflows, m^3/s, through the pipes toward -x, +x, -z and +z.
    flux: [Vec<f64>; 4],
    /// Advect: velocities, m/s, on the x faces (`nx + 1` by `nz`, face `(i, k)` at
    /// `i + (nx + 1) * k`, between columns `i - 1` and `i`) and the z faces (`nx` by `nz + 1`).
    u: Vec<f64>,
    w: Vec<f64>,
    /// Scratch: the advected velocities, each column's outflow scale, the new depths.
    u_next: Vec<f64>,
    w_next: Vec<f64>,
    scale: Vec<f64>,
    fresh: Vec<f64>,
    /// Bed friction, m/s: a pipe loses `friction / depth` of its flow per second, depth being the
    /// deeper of its two columns' (at least 5 cm), so shallow water drags and deep water runs.
    pub friction: f64,
    /// Steps taken.
    pub steps: u64,
}

impl Shallow {
    /// A dry grid over a flat bed at `bed` metres.
    pub fn new(nx: usize, nz: usize, cell_m: f64, origin: [f64; 2], bed: f64) -> Shallow {
        let n = nx * nz;
        Shallow {
            nx,
            nz,
            cell_m,
            origin,
            bed: vec![bed; n],
            depth: vec![0.0; n],
            scheme: Scheme::Advect,
            flux: [vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![0.0; n]],
            u: vec![0.0; (nx + 1) * nz],
            w: vec![0.0; nx * (nz + 1)],
            u_next: vec![0.0; (nx + 1) * nz],
            w_next: vec![0.0; nx * (nz + 1)],
            scale: vec![0.0; n],
            fresh: vec![0.0; n],
            friction: 0.02,
            steps: 0,
        }
    }

    /// The columns over the block volume's cells `lo..=hi` in x and z (cell coordinates), `per`
    /// columns to a block's edge, with the volume's corner at `origin` (world x, y, z). A column's
    /// bed is the top of the highest solid block in it between `lo[1]` and `hi[1]` (overhangs and
    /// caves under it are not seen); its water fills up to the top of the highest liquid cell above
    /// that, and the liquid's cells are not part of the bed.
    pub fn from_voxels(
        v: &Voxels,
        origin: [f64; 3],
        lo: [i32; 3],
        hi: [i32; 3],
        per: usize,
    ) -> Shallow {
        let bx = usize::try_from(hi[0] - lo[0] + 1).unwrap_or(0);
        let bz = usize::try_from(hi[2] - lo[2] + 1).unwrap_or(0);
        let cell = v.cell_m / f64::from(u32::try_from(per).unwrap_or(1));
        let mut s = Shallow::new(
            bx * per,
            bz * per,
            cell,
            [
                origin[0] + f64::from(lo[0]) * v.cell_m,
                origin[2] + f64::from(lo[2]) * v.cell_m,
            ],
            origin[1] + f64::from(lo[1]) * v.cell_m,
        );
        for bk in 0..bz {
            for bi in 0..bx {
                let (bed, water) = column(v, origin[1], lo, hi, bi, bk);
                for k in 0..per {
                    for i in 0..per {
                        let at = bi * per + i + s.nx * (bk * per + k);
                        s.bed[at] = bed;
                        s.depth[at] = math::max(water - bed, 0.0);
                    }
                }
            }
        }
        s
    }

    /// Rereads the beds from the volume (after blocks changed): a column whose bed fell keeps its
    /// depth (the water then runs into the hole), one whose bed rose under water loses the water the
    /// new blocks took.
    pub fn rebed(&mut self, v: &Voxels, origin: [f64; 3], lo: [i32; 3], hi: [i32; 3], per: usize) {
        let bx = self.nx / per;
        let bz = self.nz / per;
        for bk in 0..bz {
            for bi in 0..bx {
                let (bed, _) = column(v, origin[1], lo, hi, bi, bk);
                for k in 0..per {
                    for i in 0..per {
                        let at = bi * per + i + self.nx * (bk * per + k);
                        if bed > self.bed[at] {
                            let surface = self.bed[at] + self.depth[at];
                            self.depth[at] = math::max(surface - bed, 0.0);
                        }
                        self.bed[at] = bed;
                    }
                }
            }
        }
    }

    /// The water's volume, m^3.
    pub fn volume(&self) -> f64 {
        let mut sum = 0.0;
        for d in &self.depth {
            sum += d;
        }
        sum * self.cell_m * self.cell_m
    }

    /// The water's surface (world y) of column `at`.
    pub fn surface(&self, at: usize) -> f64 {
        self.bed[at] + self.depth[at]
    }

    /// Advances `dt` seconds in `substeps` equal steps.
    pub fn step(&mut self, dt: f64, substeps: u32) {
        let h = dt / f64::from(substeps.max(1));
        for _ in 0..substeps.max(1) {
            match self.scheme {
                Scheme::Advect => self.substep_advect(h),
                Scheme::Pipes => self.substep_pipes(h),
            }
        }
    }

    fn substep_advect(&mut self, dt: f64) {
        let (nx, nz, c) = (self.nx, self.nz, self.cell_m);
        let back = dt / c;
        // 1. Each interior face's velocity, carried back along itself (the other component
        //    averaged from the four faces around), sampled bilinearly.
        for k in 0..nz {
            for i in 1..nx {
                let at = i + (nx + 1) * k;
                if self.depth[at - k - 1] <= DRY && self.depth[at - k] <= DRY {
                    self.u_next[at] = 0.0;
                    continue;
                }
                let ww = (self.w[i - 1 + nx * k]
                    + self.w[i + nx * k]
                    + self.w[i - 1 + nx * (k + 1)]
                    + self.w[i + nx * (k + 1)])
                    * 0.25;
                let x = geom::f64_of(i) - self.u[at] * back;
                let z = geom::f64_of(k) + 0.5 - ww * back;
                self.u_next[at] = sample(&self.u, nx + 1, nz, x, z - 0.5);
            }
        }
        for k in 1..nz {
            for i in 0..nx {
                let at = i + nx * k;
                if self.depth[at - nx] <= DRY && self.depth[at] <= DRY {
                    self.w_next[at] = 0.0;
                    continue;
                }
                let uu = (self.u[i + (nx + 1) * (k - 1)]
                    + self.u[i + 1 + (nx + 1) * (k - 1)]
                    + self.u[i + (nx + 1) * k]
                    + self.u[i + 1 + (nx + 1) * k])
                    * 0.25;
                let x = geom::f64_of(i) + 0.5 - uu * back;
                let z = geom::f64_of(k) - self.w[at] * back;
                self.w_next[at] = sample(&self.w, nx, nz + 1, x - 0.5, z);
            }
        }
        std::mem::swap(&mut self.u, &mut self.u_next);
        std::mem::swap(&mut self.w, &mut self.w_next);
        // 2. Gravity down the surface's slope (the higher side pushes with no more than its own
        //    depth: a ledge's water falls, it is not shot off), bed friction, no flow into a bed
        //    above the water that would feed it, at most half a column a substep.
        let limit = 0.5 * c / dt;
        let (bed, depth, friction) = (&self.bed, &self.depth, self.friction);
        let face = |v: f64, a: usize, b: usize| -> f64 {
            let (ha, hb) = (depth[a], depth[b]);
            if ha <= DRY && hb <= DRY {
                return 0.0;
            }
            let (ea, eb) = (bed[a] + ha, bed[b] + hb);
            let mut d = eb - ea;
            d = if d > 0.0 {
                math::min(d, hb)
            } else {
                math::max(d, -ha)
            };
            let mut v = v - G * dt * d / c;
            v /= 1.0 + dt * friction / math::max(math::max(ha, hb), 0.05);
            if (v > 0.0 && (ha <= DRY || bed[b] >= ea)) || (v < 0.0 && (hb <= DRY || bed[a] >= eb))
            {
                return 0.0;
            }
            math::clamp(v, -limit, limit)
        };
        for k in 0..nz {
            for i in 1..nx {
                let at = i + (nx + 1) * k;
                let a = i - 1 + nx * k;
                self.u[at] = face(self.u[at], a, a + 1);
            }
        }
        for k in 1..nz {
            for i in 0..nx {
                let at = i + nx * k;
                self.w[at] = face(self.w[at], at - nx, at);
            }
        }
        // 3. Each column's outflow, scaled down to what it holds; then the water moves through the
        //    faces at their velocity times the upwind depth.
        let area = c * c;
        for k in 0..nz {
            for i in 0..nx {
                let at = i + nx * k;
                let h = self.depth[at];
                let left = self.u[i + (nx + 1) * k];
                let right = self.u[i + 1 + (nx + 1) * k];
                let down = self.w[at];
                let up = self.w[at + nx];
                let out = (math::max(-left, 0.0)
                    + math::max(right, 0.0)
                    + math::max(-down, 0.0)
                    + math::max(up, 0.0))
                    * h
                    * c
                    * dt;
                self.scale[at] = if out > h * area { h * area / out } else { 1.0 };
            }
        }
        let flow = |v: f64, a: usize, b: usize, depth: &[f64], scale: &[f64]| -> f64 {
            if v > 0.0 {
                v * depth[a] * scale[a]
            } else {
                v * depth[b] * scale[b]
            }
        };
        for k in 0..nz {
            for i in 0..nx {
                let at = i + nx * k;
                let mut net = 0.0;
                if i > 0 {
                    net += flow(
                        self.u[i + (nx + 1) * k],
                        at - 1,
                        at,
                        &self.depth,
                        &self.scale,
                    );
                }
                if i + 1 < nx {
                    net -= flow(
                        self.u[i + 1 + (nx + 1) * k],
                        at,
                        at + 1,
                        &self.depth,
                        &self.scale,
                    );
                }
                if k > 0 {
                    net += flow(self.w[at], at - nx, at, &self.depth, &self.scale);
                }
                if k + 1 < nz {
                    net -= flow(self.w[at + nx], at, at + nx, &self.depth, &self.scale);
                }
                // `net` is m^2/s through faces of width c: the column changes by net * dt / c.
                self.fresh[at] = math::max(self.depth[at] + net * dt / c, 0.0);
            }
        }
        std::mem::swap(&mut self.depth, &mut self.fresh);
        self.steps += 1;
    }

    fn substep_pipes(&mut self, dt: f64) {
        let (nx, nz) = (self.nx, self.nz);
        let area = self.cell_m * self.cell_m;
        // Pipes of cross-section `area` and length `cell_m`: a surface difference of dh speeds the
        // flow by g * area * dh / cell_m per second.
        let gain = dt * G * self.cell_m;
        let friction = self.friction * dt;
        let [fl, fr, fd, fu] = &mut self.flux;
        for k in 0..nz {
            for i in 0..nx {
                let at = i + nx * k;
                let d = self.depth[at];
                if d <= 0.0 {
                    fl[at] = 0.0;
                    fr[at] = 0.0;
                    fd[at] = 0.0;
                    fu[at] = 0.0;
                    continue;
                }
                let s = self.bed[at] + d;
                let pipe = |f: f64, n: usize| {
                    let deep = math::max(math::max(d, self.depth[n]), 0.05);
                    let keep = math::max(1.0 - friction / deep, 0.0);
                    math::max(f * keep + gain * (s - (self.bed[n] + self.depth[n])), 0.0)
                };
                let l = if i > 0 { pipe(fl[at], at - 1) } else { 0.0 };
                let r = if i + 1 < nx {
                    pipe(fr[at], at + 1)
                } else {
                    0.0
                };
                let dn = if k > 0 { pipe(fd[at], at - nx) } else { 0.0 };
                let up = if k + 1 < nz {
                    pipe(fu[at], at + nx)
                } else {
                    0.0
                };
                // Never more than the column holds.
                let out = (l + r + dn + up) * dt;
                let scale = if out > d * area { d * area / out } else { 1.0 };
                fl[at] = l * scale;
                fr[at] = r * scale;
                fd[at] = dn * scale;
                fu[at] = up * scale;
            }
        }
        for k in 0..nz {
            for i in 0..nx {
                let at = i + nx * k;
                let mut inflow = 0.0;
                if i > 0 {
                    inflow += fr[at - 1];
                }
                if i + 1 < nx {
                    inflow += fl[at + 1];
                }
                if k > 0 {
                    inflow += fu[at - nx];
                }
                if k + 1 < nz {
                    inflow += fd[at + nx];
                }
                let outflow = fl[at] + fr[at] + fd[at] + fu[at];
                self.depth[at] = math::max(self.depth[at] + (inflow - outflow) * dt / area, 0.0);
            }
        }
        self.steps += 1;
    }

    /// The water's velocity in column `at`, m/s in x and z: the net flow through its faces over
    /// the face's wetted area (0 where it is dry).
    pub fn velocity(&self, at: usize) -> [f64; 2] {
        let d = self.depth[at];
        if d <= 1e-4 {
            return [0.0; 2];
        }
        if self.scheme == Scheme::Advect {
            let (i, k) = (at % self.nx, at / self.nx);
            let u = (self.u[i + (self.nx + 1) * k] + self.u[i + 1 + (self.nx + 1) * k]) * 0.5;
            let w = (self.w[at] + self.w[at + self.nx]) * 0.5;
            return [u, w];
        }
        // A film's flow over its own thin depth would read as a jet: at least 5 cm.
        let d = math::max(d, 0.05);
        let (nx, nz) = (self.nx, self.nz);
        let (i, k) = (at % nx, at / nx);
        let [fl, fr, fd, fu] = &self.flux;
        let from_left = if i > 0 { fr[at - 1] - fl[at] } else { 0.0 };
        let to_right = if i + 1 < nx { fr[at] - fl[at + 1] } else { 0.0 };
        let from_down = if k > 0 { fu[at - nx] - fd[at] } else { 0.0 };
        let to_up = if k + 1 < nz {
            fu[at] - fd[at + nx]
        } else {
            0.0
        };
        let face = self.cell_m * d;
        [
            (from_left + to_right) * 0.5 / face,
            (from_down + to_up) * 0.5 / face,
        ]
    }

    /// The column under world `(x, z)`, if the grid covers it.
    pub fn column_at(&self, x: f64, z: f64) -> Option<usize> {
        let fx = (x - self.origin[0]) / self.cell_m;
        let fz = (z - self.origin[1]) / self.cell_m;
        if !(fx >= 0.0 && fz >= 0.0) {
            return None;
        }
        let (i, k) = (to_index(fx), to_index(fz));
        (i < self.nx && k < self.nz).then_some(i + self.nx * k)
    }

    /// FNV-1a over every depth's and flow's bits, in column order: equal hashes, equal states.
    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |x: f64| {
            for b in x.to_bits().to_le_bytes() {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        for at in 0..self.depth.len() {
            eat(self.depth[at]);
            for f in &self.flux {
                eat(f[at]);
            }
        }
        for &v in self.u.iter().chain(&self.w) {
            eat(v);
        }
        h
    }
}

/// Water's density, kg/m^3.
const RHO: f64 = 1000.0;

/// Boxes that float on [`Shallow`] water (spike): Rapier bodies over a terrain of one fixed cuboid
/// per column, each box pushed up by the water it displaces at eight sample points (the centres of
/// its octants, each a column of water's depth over it) and dragged toward the water's velocity
/// there. One-way: the boxes do not move the water.
pub struct Floaters {
    pub world: PhysicsWorld,
    terrain: Vec<ColliderHandle>,
    boxes: Vec<(RigidBodyHandle, f64)>,
}

impl Floaters {
    /// An empty world stepping `dt` seconds under standard gravity.
    pub fn new(dt: f64) -> Floaters {
        let mut world = PhysicsWorld::new();
        world.gravity = geom::vec_in([0.0, -G, 0.0]);
        world.integration_parameters.dt = geom::f32_of(dt);
        Floaters {
            world,
            terrain: Vec::new(),
            boxes: Vec::new(),
        }
    }

    /// Rebuilds the terrain from the water's beds: a cuboid per column from `floor` (world y) up to
    /// its bed.
    pub fn set_terrain(&mut self, water: &Shallow, floor: f64) {
        for h in self.terrain.drain(..) {
            self.world.remove_collider(h);
        }
        let half = water.cell_m * 0.5;
        for k in 0..water.nz {
            for i in 0..water.nx {
                let bed = water.bed[i + water.nx * k];
                if bed <= floor {
                    continue;
                }
                let hy = (bed - floor) * 0.5;
                let x = water.origin[0] + (geom::f64_of(i) + 0.5) * water.cell_m;
                let z = water.origin[1] + (geom::f64_of(k) + 0.5) * water.cell_m;
                let c = ColliderBuilder::cuboid(
                    geom::f32_of(half),
                    geom::f32_of(hy),
                    geom::f32_of(half),
                )
                .translation(geom::vec_in([x, floor + hy, z]))
                .friction(0.6)
                .build();
                self.terrain.push(self.world.insert_collider(c, None));
            }
        }
    }

    /// Adds a box of half-edge `half` metres and `density` kg/m^3 at `at`.
    pub fn add_box(&mut self, at: V3, half: f64, density: f64) {
        let rb = RigidBodyBuilder::dynamic()
            .translation(geom::vec_in(at))
            .build();
        let co =
            ColliderBuilder::cuboid(geom::f32_of(half), geom::f32_of(half), geom::f32_of(half))
                .density(geom::f32_of(density))
                .friction(0.6)
                .build();
        let (h, _) = self.world.insert(rb, co);
        self.boxes.push((h, half));
    }

    /// One step: the water's forces at the start, then Rapier's step.
    pub fn step(&mut self, water: &Shallow) {
        let dt = f64::from(self.world.integration_parameters.dt);
        for &(h, half) in &self.boxes {
            let Some(b) = self.world.bodies.get_mut(h) else {
                continue;
            };
            b.reset_forces(false);
            let pos = geom::vec_out(b.translation());
            let rot = geom::quat_out(*b.rotation());
            let mass = f64::from(b.mass());
            let q = half * 0.5;
            let part = 8.0 * q * q * q;
            for o in 0..8u8 {
                let sign = |bit: u8| if o & bit == 0 { -q } else { q };
                let p = geom::to_world(pos, rot, [sign(1), sign(2), sign(4)]);
                let Some(at) = water.column_at(p[0], p[2]) else {
                    continue;
                };
                if water.depth[at] <= 0.0 {
                    continue;
                }
                let wet = math::clamp((water.surface(at) - (p[1] - q)) / (2.0 * q), 0.0, 1.0);
                if wet <= 0.0 {
                    continue;
                }
                let u = water.velocity(at);
                let vp = geom::vec_out(b.velocity_at_point(geom::vec_in(p)));
                let rel = [u[0] - vp[0], -vp[1], u[1] - vp[2]];
                let speed = geom::length(rel);
                // Quadratic drag (Cd 1 on the octant's face), never more than stops the octant's
                // share of the mass relative to the water in one step.
                let drag = 0.5 * RHO * 4.0 * q * q * speed * wet;
                let most = mass / 8.0 / dt;
                let k = math::min(drag, most);
                let f = [rel[0] * k, RHO * G * part * wet + rel[1] * k, rel[2] * k];
                b.add_force_at_point(geom::vec_in(f), geom::vec_in(p), true);
            }
        }
        self.world.step();
    }

    /// Each box's half-edge, position and rotation.
    pub fn poses(&self) -> Vec<(f64, V3, Q4)> {
        self.boxes
            .iter()
            .filter_map(|&(h, half)| {
                let b = self.world.bodies.get(h)?;
                Some((
                    half,
                    geom::vec_out(b.translation()),
                    geom::quat_out(*b.rotation()),
                ))
            })
            .collect()
    }
}

/// The spike's bench scene: `n` by `n` columns of 0.5 m over a bed with bumps up to 0.8 m, a
/// block of water 2 m deep in one quarter that breaks like a dam.
pub fn bench_scene(n: usize) -> Shallow {
    let mut s = Shallow::new(n, n, 0.5, [0.0, 0.0], 0.0);
    for k in 0..n {
        for i in 0..n {
            let at = i + n * k;
            s.bed[at] = 0.2 * f64::from(u32::try_from((i * 7 + k * 13) % 5).unwrap_or(0));
            if i < n / 2 && k < n / 2 {
                s.depth[at] = 2.0;
            }
        }
    }
    s
}

/// Bilinear sample of a face grid of `n0` by `n1` values (`v[a + n0 * b]`) at grid coordinates
/// `(x, y)`, clamped to the grid.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn sample(v: &[f64], n0: usize, n1: usize, x: f64, y: f64) -> f64 {
    let x = math::clamp(x, 0.0, geom::f64_of(n0 - 1));
    let y = math::clamp(y, 0.0, geom::f64_of(n1 - 1));
    // Clamped to the grid and not negative: `as` takes the whole part exactly.
    let (i, j) = (x.floor() as usize, y.floor() as usize);
    let (i, j) = (i.min(n0.saturating_sub(2)), j.min(n1.saturating_sub(2)));
    let (tx, ty) = (x - geom::f64_of(i), y - geom::f64_of(j));
    let at = |a: usize, b: usize| v[(a.min(n0 - 1)) + n0 * b.min(n1 - 1)];
    let top = at(i, j) + (at(i + 1, j) - at(i, j)) * tx;
    let bottom = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * tx;
    top + (bottom - top) * ty
}

/// The whole part of a non-negative `x` as an index (`x` below 2^52).
fn to_index(x: f64) -> usize {
    let mut i = 0usize;
    let mut step = 1usize << 20;
    // Comparisons only: no float-to-integer cast whose rounding a lint would question.
    while step > 0 {
        if f64::from(u32::try_from(i + step).unwrap_or(u32::MAX)) <= x {
            i += step;
        }
        step >>= 1;
    }
    i
}

/// The bed (top of the highest solid block) and the water's top (of the highest liquid cell above
/// the bed, or the bed) of block column `(lo[0] + bi, lo[2] + bk)`, world y.
fn column(v: &Voxels, y0: f64, lo: [i32; 3], hi: [i32; 3], bi: usize, bk: usize) -> (f64, f64) {
    let x = lo[0] + i32::try_from(bi).unwrap_or(0);
    let z = lo[2] + i32::try_from(bk).unwrap_or(0);
    let top = |y: i32| y0 + f64::from(y + 1) * v.cell_m;
    let mut bed = y0 + f64::from(lo[1]) * v.cell_m;
    let mut ground = lo[1] - 1;
    for y in (lo[1]..=hi[1]).rev() {
        if v.solid(v.get([x, y, z])) {
            bed = top(y);
            ground = y;
            break;
        }
    }
    let mut water = bed;
    for y in (ground + 1..=hi[1]).rev() {
        if v.shape(v.get([x, y, z])) == BlockShape::Liquid {
            water = top(y);
            break;
        }
    }
    (bed, water)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_is_conserved_and_settles_level() {
        let mut s = bench_scene(32);
        let v0 = s.volume();
        s.step(240.0, 14_400);
        let v1 = s.volume();
        assert!(((v1 - v0) / v0).abs() < 1e-9, "volume {v0} then {v1}");
        let wet: Vec<f64> = (0..s.depth.len())
            .filter(|&at| s.depth[at] > 0.05)
            .map(|at| s.surface(at))
            .collect();
        let (lo, hi) = wet.iter().fold((f64::MAX, f64::MIN), |(a, b), &x| {
            (math::min(a, x), math::max(b, x))
        });
        assert!(hi - lo < 0.02, "the surface settles level: {lo} to {hi}");
    }

    #[test]
    fn a_step_is_the_same_bits_from_the_same_state() {
        let mut a = bench_scene(24);
        let mut b = a.clone();
        a.step(1.0, 60);
        b.step(1.0, 60);
        assert_eq!(a.hash(), b.hash());
        assert_ne!(a.hash(), bench_scene(24).hash());
    }

    #[test]
    fn indices_are_whole_parts() {
        for (x, i) in [(0.0, 0), (0.99, 0), (1.0, 1), (7.5, 7), (1023.999, 1023)] {
            assert_eq!(to_index(x), i);
        }
    }
}
