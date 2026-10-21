//! The substeps of one region's tick (water.md 4), the spike's advect scheme
//! (shallow-water.md, What was built) over the tiles that are stepped.
//!
//! The stepped tiles are the awake ones and the tiles beside one (4-neighbours), read from the
//! tick's starting state. They are gathered with a margin of one tile into a flat window
//! ([`Grid`]), stepped there and written back. A face between a stepped column and one that is not
//! stepped is a wall for the tick: the column beyond sleeps, so no water would cross it anyway, and
//! water is conserved exactly within the stepped set. Every phase of a substep computes from the
//! arrays as they stood before it, so the order columns are visited in changes no bit; columns and
//! faces that are not stepped never change during the tick, which lets the double buffers swap.

use amoris_sim::math;

use super::{
    CALM_RATE, CALM_SPEED, DRY_SPEED, MAX_SUBSTEPS, SLEEP_TICKS, STILL_DEPTH, STILL_SPEED, Stepped,
    TILE, WaterRegion,
};
use crate::geom;

/// Standard gravity, m/s^2.
pub(crate) const G: f64 = 9.81;
/// A column with less water than this is dry, metres.
pub(crate) const DRY: f64 = 1e-4;

/// The working window: `w` by `h` columns from region column `(x0, z0)`.
#[derive(Default)]
pub struct Grid {
    w: usize,
    h: usize,
    x0: usize,
    z0: usize,
    bed: Vec<f64>,
    depth: Vec<f64>,
    /// Face velocities: `u` on the x faces (`w + 1` by `h`, face `(i, k)` the -x face of column
    /// `(i, k)`), `v` on the z faces (`w` by `h + 1`).
    u: Vec<f64>,
    v: Vec<f64>,
    u_next: Vec<f64>,
    v_next: Vec<f64>,
    scale: Vec<f64>,
    fresh: Vec<f64>,
    /// Whether each column is stepped.
    on: Vec<bool>,
    pending: Option<Plan>,
}

struct Plan {
    tiles: Vec<usize>,
    count: u32,
}

fn reset(v: &mut Vec<f64>, n: usize, x: f64) {
    v.clear();
    v.resize(n, x);
}

/// The fewest substeps at which a wave of `speed` m/s crosses at most half a column of `cell_m`
/// in each, for a tick of `dt` (at most [`MAX_SUBSTEPS`]).
pub(crate) fn substeps_for(speed: f64, dt: f64, cell_m: f64) -> u32 {
    let need = 2.0 * dt * speed / cell_m;
    for n in 1..MAX_SUBSTEPS {
        if f64::from(n) >= need {
            return n;
        }
    }
    MAX_SUBSTEPS
}

/// One tick of `r` (water.md 4): substeps, drying, sleep.
pub(crate) fn tick(r: &mut WaterRegion, dt: f64, g: &mut Grid) -> Stepped {
    tick_with(r, dt, g, None).expect("CPU water step")
}

pub(crate) fn tick_with(
    r: &mut WaterRegion,
    dt: f64,
    g: &mut Grid,
    compute: Option<&mut dyn super::compute::WaterCompute>,
) -> Result<Stepped, amoris_contract::Problem> {
    match poll_tick(r, dt, g, compute)? {
        std::task::Poll::Ready(done) => Ok(done),
        std::task::Poll::Pending => Err(amoris_contract::Problem::new(
            "sim.external_pending",
            "Water awaits host computation",
            amoris_contract::detail([]),
        )),
    }
}

pub(crate) fn poll_tick(
    r: &mut WaterRegion,
    dt: f64,
    g: &mut Grid,
    compute: Option<&mut dyn super::compute::WaterCompute>,
) -> Result<std::task::Poll<Stepped>, amoris_contract::Problem> {
    let Plan { tiles, count } = if let Some(plan) = g.pending.take() {
        plan
    } else {
        let [tx, tz] = r.tiles_along();
        // The stepped tiles: awake ones and their 4-neighbours.
        let mut stepped = vec![false; tx * tz];
        let mut fastest: f64 = 0.0;
        let mut deepest: f64 = 0.0;
        for (n, t) in r.tiles.iter().enumerate() {
            if t.asleep() {
                continue;
            }
            let (kx, kz) = (n % tx, n / tx);
            stepped[n] = true;
            if kx > 0 {
                stepped[n - 1] = true;
            }
            if kx + 1 < tx {
                stepped[n + 1] = true;
            }
            if kz > 0 {
                stepped[n - tx] = true;
            }
            if kz + 1 < tz {
                stepped[n + tx] = true;
            }
            for c in 0..t.depth.len() {
                fastest = math::max(fastest, math::max(t.u[c].abs(), t.w[c].abs()));
                deepest = math::max(deepest, t.depth[c]);
            }
        }
        let tiles: Vec<usize> = (0..stepped.len()).filter(|&n| stepped[n]).collect();
        if tiles.is_empty() {
            return Ok(std::task::Poll::Ready(Stepped::default()));
        }
        let speed = fastest + math::sqrt(G * deepest);
        let count = r.substeps.max(substeps_for(speed, dt, r.cell_m));

        // The window: the stepped tiles' box with a margin of one tile.
        let (mut lo, mut hi) = ([usize::MAX; 2], [0usize; 2]);
        for &n in &tiles {
            let k = [n % tx, n / tx];
            for a in 0..2 {
                lo[a] = lo[a].min(k[a]);
                hi[a] = hi[a].max(k[a]);
            }
        }
        let lo = [lo[0].saturating_sub(1), lo[1].saturating_sub(1)];
        let hi = [(hi[0] + 1).min(tx - 1), (hi[1] + 1).min(tz - 1)];
        g.gather(r, &stepped, lo, hi);
        Plan { tiles, count }
    };

    let h = dt / f64::from(count);
    match r.solver {
        super::Solver::Cpu => {
            for _ in 0..count {
                g.substep(h, r.cell_m, r.friction);
            }
        }
        super::Solver::Gpu => {
            let compute = compute.ok_or_else(|| {
                amoris_contract::Problem::new(
                    "water.compute_missing",
                    "GPU water needs a host compute service; this host has none",
                    amoris_contract::detail([]),
                )
            })?;
            let result = compute.poll_advance(
                super::compute::GridView {
                    size: [g.w, g.h],
                    bed: &g.bed,
                    depth: &mut g.depth,
                    u: &mut g.u,
                    w: &mut g.v,
                    active: &g.on,
                },
                r.cell_m,
                h,
                count,
                r.friction,
            )?;
            if result.is_pending() {
                g.pending = Some(Plan { tiles, count });
                return Ok(std::task::Poll::Pending);
            }
        }
        super::Solver::GpuResident => {
            return Err(super::resident::failed(
                "resident water cannot use the CPU-grid solve path",
            ));
        }
    }
    let dried = if r.dry_below > 0.0 {
        g.dry(r.dry_below) * r.cell_m * r.cell_m
    } else {
        0.0
    };
    g.scatter(r, &tiles);
    Ok(std::task::Poll::Ready(Stepped {
        substeps: count,
        tiles,
        dried_m3: dried,
    }))
}

impl Grid {
    fn gather(&mut self, r: &WaterRegion, stepped: &[bool], lo: [usize; 2], hi: [usize; 2]) {
        let tx = r.tiles_along()[0];
        let (w, h) = ((hi[0] - lo[0] + 1) * TILE, (hi[1] - lo[1] + 1) * TILE);
        (self.w, self.h, self.x0, self.z0) = (w, h, lo[0] * TILE, lo[1] * TILE);
        let n = w * h;
        // Every column is overwritten below. Retain the allocation and existing contents rather
        // than zero-filling the entire window before copying the same bytes again.
        self.bed.resize(n, 0.0);
        self.depth.resize(n, 0.0);
        self.u.resize((w + 1) * h, 0.0);
        self.v.resize(w * (h + 1), 0.0);
        self.on.resize(n, false);
        // The tiles own only minus faces. Initialize the two unowned outer boundaries explicitly,
        // including after a window reshapes while reusing a larger allocation.
        for k in 0..h {
            self.u[w + (w + 1) * k] = 0.0;
        }
        self.v[n..].fill(0.0);
        for kz in lo[1]..=hi[1] {
            for kx in lo[0]..=hi[0] {
                let t = &r.tiles[kx + tx * kz];
                let on = stepped[kx + tx * kz];
                let (bx, bz) = ((kx - lo[0]) * TILE, (kz - lo[1]) * TILE);
                for lz in 0..TILE {
                    let k = bz + lz;
                    let row = bx + w * k;
                    let from = TILE * lz;
                    self.bed[row..row + TILE].copy_from_slice(&t.bed[from..from + TILE]);
                    self.depth[row..row + TILE].copy_from_slice(&t.depth[from..from + TILE]);
                    let urow = bx + (w + 1) * k;
                    self.u[urow..urow + TILE].copy_from_slice(&t.u[from..from + TILE]);
                    self.v[row..row + TILE].copy_from_slice(&t.w[from..from + TILE]);
                    self.on[row..row + TILE].fill(on);
                }
            }
        }
        // Faces between a stepped column and one that is not, or the region's edge, are walls.
        for k in 0..h {
            for i in 0..w {
                let c = i + w * k;
                if !self.on[c] {
                    continue;
                }
                if i == 0 || !self.on[c - 1] {
                    self.u[i + (w + 1) * k] = 0.0;
                }
                if i + 1 == w || !self.on[c + 1] {
                    self.u[i + 1 + (w + 1) * k] = 0.0;
                }
                if k == 0 || !self.on[c - w] {
                    self.v[c] = 0.0;
                }
                if k + 1 == h || !self.on[c + w] {
                    self.v[c + w] = 0.0;
                }
            }
        }
        if r.solver == super::Solver::Cpu {
            self.u_next.clone_from(&self.u);
            self.v_next.clone_from(&self.v);
            self.fresh.clone_from(&self.depth);
            reset(&mut self.scale, n, 1.0);
        }
    }

    /// Whether the x face `(i, k)` (between columns `i - 1` and `i`) is stepped.
    fn u_on(&self, i: usize, k: usize) -> bool {
        let c = i + self.w * k;
        i > 0 && i < self.w && self.on[c] && self.on[c - 1]
    }

    /// Whether the z face `(i, k)` (between columns `k - 1` and `k`) is stepped.
    fn v_on(&self, i: usize, k: usize) -> bool {
        let c = i + self.w * k;
        k > 0 && k < self.h && self.on[c] && self.on[c - self.w]
    }

    fn substep(&mut self, dt: f64, c: f64, friction: f64) {
        let (w, h) = (self.w, self.h);
        let back = dt / c;
        // 1. Each stepped face's velocity, carried back along itself (the other component averaged
        //    from the four faces around), sampled bilinearly.
        for k in 0..h {
            for i in 0..w {
                if !self.on[i + w * k] {
                    continue;
                }
                if self.u_on(i, k) {
                    let at = i + (w + 1) * k;
                    let col = i + w * k;
                    self.u_next[at] = if self.depth[col - 1] <= DRY && self.depth[col] <= DRY {
                        0.0
                    } else {
                        let vv =
                            (self.v[col - 1] + self.v[col] + self.v[col - 1 + w] + self.v[col + w])
                                * 0.25;
                        let x = geom::f64_of(i) - self.u[at] * back;
                        let z = geom::f64_of(k) + 0.5 - vv * back;
                        sample(&self.u, w + 1, h, x, z - 0.5)
                    };
                }
                if self.v_on(i, k) {
                    let at = i + w * k;
                    self.v_next[at] = if self.depth[at - w] <= DRY && self.depth[at] <= DRY {
                        0.0
                    } else {
                        let uu = (self.u[i + (w + 1) * (k - 1)]
                            + self.u[i + 1 + (w + 1) * (k - 1)]
                            + self.u[i + (w + 1) * k]
                            + self.u[i + 1 + (w + 1) * k])
                            * 0.25;
                        let x = geom::f64_of(i) + 0.5 - uu * back;
                        let z = geom::f64_of(k) - self.v[at] * back;
                        sample(&self.v, w, h + 1, x - 0.5, z)
                    };
                }
            }
        }
        std::mem::swap(&mut self.u, &mut self.u_next);
        std::mem::swap(&mut self.v, &mut self.v_next);
        // 2. Gravity down the surface's slope (the higher side pushes with no more than its own
        //    depth), bed friction and calm damping, no flow into a bed above the giving surface or
        //    out of a dry column, at most half a column a substep.
        let limit = 0.5 * c / dt;
        let (bed, depth) = (&self.bed, &self.depth);
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
            // Bed friction, and calm damping of slow faces (water.md 4).
            let speed = v.abs();
            let calm = if speed > CALM_SPEED {
                CALM_RATE * CALM_SPEED / speed
            } else {
                CALM_RATE
            };
            v /= 1.0 + dt * (friction / math::max(math::max(ha, hb), 0.05) + calm);
            if (v > 0.0 && (ha <= DRY || bed[b] >= ea)) || (v < 0.0 && (hb <= DRY || bed[a] >= eb))
            {
                return 0.0;
            }
            math::clamp(v, -limit, limit)
        };
        for k in 0..h {
            for i in 0..w {
                let col = i + w * k;
                if !self.on[col] {
                    continue;
                }
                if i > 0 && self.on[col - 1] {
                    let at = i + (w + 1) * k;
                    self.u[at] = face(self.u[at], col - 1, col);
                }
                if k > 0 && self.on[col - w] {
                    self.v[col] = face(self.v[col], col - w, col);
                }
            }
        }
        // 3. Each column's outflow, scaled down to what it holds; then the water moves through the
        //    faces at their velocity times the upwind depth.
        let area = c * c;
        for k in 0..h {
            for i in 0..w {
                let at = i + w * k;
                if !self.on[at] {
                    continue;
                }
                let d = self.depth[at];
                let left = self.u[i + (w + 1) * k];
                let right = self.u[i + 1 + (w + 1) * k];
                let down = self.v[at];
                let up = self.v[at + w];
                let out = (math::max(-left, 0.0)
                    + math::max(right, 0.0)
                    + math::max(-down, 0.0)
                    + math::max(up, 0.0))
                    * d
                    * c
                    * dt;
                self.scale[at] = if out > d * area { d * area / out } else { 1.0 };
            }
        }
        let (depth, scale) = (&self.depth, &self.scale);
        let flow = |v: f64, a: usize, b: usize| -> f64 {
            if v > 0.0 {
                v * depth[a] * scale[a]
            } else {
                v * depth[b] * scale[b]
            }
        };
        for k in 0..h {
            for i in 0..w {
                let at = i + w * k;
                if !self.on[at] {
                    continue;
                }
                let mut net = 0.0;
                if i > 0 {
                    net += flow(self.u[i + (w + 1) * k], at - 1, at);
                }
                if i + 1 < w {
                    net -= flow(self.u[i + 1 + (w + 1) * k], at, at + 1);
                }
                if k > 0 {
                    net += flow(self.v[at], at - w, at);
                }
                if k + 1 < h {
                    net -= flow(self.v[at + w], at, at + w);
                }
                // `net` is m^2/s through faces of width c: the column changes by net * dt / c.
                self.fresh[at] = math::max(depth[at] + net * dt / c, 0.0);
            }
        }
        std::mem::swap(&mut self.depth, &mut self.fresh);
    }

    /// Dries every stepped column shallower than `below` whose faces all run slower than
    /// [`DRY_SPEED`]; returns the depth taken, summed (metres times columns).
    fn dry(&mut self, below: f64) -> f64 {
        let (w, h) = (self.w, self.h);
        let mut taken = 0.0;
        for k in 0..h {
            for i in 0..w {
                let at = i + w * k;
                let d = self.depth[at];
                if !self.on[at] || d <= 0.0 || d >= below {
                    continue;
                }
                let faces = [
                    self.u[i + (w + 1) * k],
                    self.u[i + 1 + (w + 1) * k],
                    self.v[at],
                    self.v[at + w],
                ];
                if faces.iter().all(|f| f.abs() < DRY_SPEED) {
                    taken += d;
                    self.depth[at] = 0.0;
                }
            }
        }
        taken
    }

    /// Writes the stepped tiles back, counting each one's still ticks and putting it to sleep at
    /// [`SLEEP_TICKS`].
    fn scatter(&self, r: &mut WaterRegion, tiles: &[usize]) {
        let tx = r.tiles_along()[0];
        let w = self.w;
        for &n in tiles {
            let (kx, kz) = (n % tx, n / tx);
            let (bx, bz) = (kx * TILE - self.x0, kz * TILE - self.z0);
            let t = &mut r.tiles[n];
            let mut still = true;
            for lz in 0..TILE {
                let k = bz + lz;
                let row = bx + w * k;
                let urow = bx + (w + 1) * k;
                let from = TILE * lz;
                for l in 0..TILE {
                    let (c, f) = (row + l, from + l);
                    let (u, v) = (self.u[urow + l], self.v[c]);
                    // The region has not been written back yet: its own depth is the tick's
                    // starting value, so a second full-size copy is unnecessary.
                    if (self.depth[c] - t.depth[f]).abs() > STILL_DEPTH
                        || u.abs() > STILL_SPEED
                        || v.abs() > STILL_SPEED
                    {
                        still = false;
                    }
                    t.depth[f] = self.depth[c];
                    t.u[f] = u;
                    t.w[f] = v;
                }
            }
            t.still = if still {
                (t.still + 1).min(SLEEP_TICKS)
            } else {
                0
            };
            if t.asleep() {
                t.u.fill(0.0);
                t.w.fill(0.0);
            }
        }
    }
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
