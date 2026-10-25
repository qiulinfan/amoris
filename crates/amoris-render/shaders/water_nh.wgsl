// Depth-integrated non-hydrostatic pressure for the resident step (water.md 55), after Y.
// Yamazaki, Z. Kowalik and K. F. Cheung, "Depth-integrated, non-hydrostatic model for wave
// breaking and run-up", International Journal for Numerical Methods in Fluids 61(5), 2009,
// doi:10.1002/fld.1952. The pressure q at the bed (zero at the surface, linear between)
// accelerates the depth-averaged vertical velocity W by q/D and the horizontal flow by
// -grad(q)/2 - q grad(surface + bed)/(2D); continuity across the column, D/2 div U + W - w_b = 0
// with w_b = U . grad(bed), closes the system. Waves then disperse within a few percent of
// Airy's speed up to kh of about 2, and a swell shoals into crests and troughs rather than a bore.
//
// The discrete pressure force is the transpose of the discrete continuity B (per column,
// B U = D/2 div U - U . grad(bed)), so the pressure does no work: the correction is an orthogonal
// projection in the depth-weighted kinetic energy, and the system it solves,
//     (B D_f^-1 B^T + 1/D) q = (B U^n - B U~) / dt,
// is symmetric positive definite on any bed. Dropping the bed and surface slopes from the force
// alone, as for a flat bed, lets the pressure work against the swell on a sloping beach until it
// breaks the field.
//
// One correction a substep, between its hydrostatic momentum update (U^n to U~, the MacCormack
// faces) and its mass transport, which then carries the corrected flow, as Yamazaki et al. order
// it. The substep's source field still holds U^n, and W^n = -B U^n follows from its continuity,
// so no vertical velocity is stored and a step replays from its checkpoint. The solve is a fixed
// number of Chebyshev iterations on the Jacobi-preconditioned system from the previous substep's
// pressure (zero at a tick's first), so a replay computes the same pressure; a dispatch runs four
// of them on a tile in its workgroup's memory (`nh_block`). Dry columns, walls
// and the columns outside the window take no flux (Neumann); water too shallow or breaking, and
// the first column of an open side, stay hydrostatic (q = 0): a bore's front is a hydrostatic
// shock, as in Yamazaki et al. and SWASH.
struct Nh {
    size: vec2u,
    cell: f32,
    dt: f32,          // the substep
    open: vec4u,      // -x, +x, -z, +z: 1 where the side is open
    breaking: f32,    // 1: breaking water stays hydrostatic
    limit: f32,       // the face velocity limit of the substeps, m/s
    unused: vec2f,
}
struct Iter { index: u32, unused0: u32, unused1: u32, unused2: u32 }
@group(0) @binding(0) var<uniform> p: Nh;
// The substep's source field (bed, depth, -x and -z face velocities), which its transport has not
// yet replaced.
@group(0) @binding(1) var<storage, read> state: array<vec4f>;
// Per column: the right-hand side, the diagonal (zero where q is not solved) and the couplings
// through its -x and -z faces.
@group(0) @binding(2) var<storage, read_write> system: array<vec4f>;
// The pressure and the Chebyshev direction, in and out.
@group(0) @binding(3) var<storage, read_write> xd_in: array<vec2f>;
@group(0) @binding(4) var<storage, read_write> xd_out: array<vec2f>;
// The MacCormack faces' new velocities, -x then -z per column (water_maccormack.wgsl).
@group(0) @binding(5) var<storage, read_write> velocity: array<f32>;
// The preconditioned system's spectrum bounds, as the bits of positive f32s (which order as
// integers): 1 - min(diagonal share of 1/D) and the largest Gershgorin radius.
@group(0) @binding(6) var<storage, read_write> bounds: array<atomic<u32>, 4>;
// Whether a column is simulated (water_compute.wgsl `awake`): the others are walls.
@group(0) @binding(7) var<storage, read> awake: array<u32>;
@group(0) @binding(8) var<uniform> iter: Iter;
// Each iteration's Chebyshev coefficients, from the bounds: d = x d_prev + y residual.
@group(0) @binding(9) var<storage, read_write> chebyshev: array<vec2f, 64>;

const G: f32 = 9.81;
const DRY: f32 = 0.05;
const SHALLOW: f32 = 0.3;

fn at(i: u32, k: u32) -> u32 { return i + p.size.x * k; }
// Face velocities at the substep's start or after its hydrostatic update; the region's own sides
// are walls or hydrostatic columns, whose outer faces the pressure never reads.
fn u(i: u32, k: u32, updated: bool) -> f32 {
    if (i == 0u || i >= p.size.x) { return 0.0; }
    return select(state[at(i, k)].z, velocity[at(i, k) * 2u], updated);
}
fn w(i: u32, k: u32, updated: bool) -> f32 {
    if (k == 0u || k >= p.size.y) { return 0.0; }
    return select(state[at(i, k)].w, velocity[at(i, k) * 2u + 1u], updated);
}
fn bed(i: i32, k: i32) -> f32 {
    let c = clamp(vec2i(i, k), vec2i(0), vec2i(p.size) - 1);
    return state[at(u32(c.x), u32(c.y))].x;
}
// B's coefficients at a column on its -x, +x, -z and +z faces: D/2 div U - U . grad(bed), with
// central bed slopes, bounded: a cliff's rock is not a slope the flow climbs.
fn coefficients(i: u32, k: u32) -> vec4f {
    let a = i32(i); let b = i32(k);
    let slope = clamp(
        vec2f(bed(a + 1, b) - bed(a - 1, b), bed(a, b + 1) - bed(a, b - 1)) / (2.0 * p.cell),
        vec2f(-1.0), vec2f(1.0));
    let h = 0.5 * state[at(i, k)].y / p.cell;
    return vec4f(-h - 0.5 * slope.x, h - 0.5 * slope.x, -h - 0.5 * slope.y, h - 0.5 * slope.y);
}
// B U at a column whose coefficients are `a`.
fn continuity(a: vec4f, i: u32, k: u32, updated: bool) -> f32 {
    return a.x * u(i, k, updated) + a.y * u(i + 1u, k, updated)
        + a.z * w(i, k, updated) + a.w * w(i, k + 1u, updated);
}

// Whether a column holds water the pressure couples: simulated and not dry.
fn wet(i: u32, k: u32) -> bool {
    let n = at(i, k);
    return awake[n] != 0u && state[n].y > DRY;
}
fn solved(i: u32, k: u32) -> bool {
    if (!wet(i, k)) { return false; }
    let d = state[at(i, k)].y;
    if (d < SHALLOW) { return false; }
    if ((p.open.x != 0u && i == 0u) || (p.open.y != 0u && i + 1u == p.size.x)
        || (p.open.z != 0u && k == 0u) || (p.open.w != 0u && k + 1u == p.size.y)) {
        return false;
    }
    if (p.breaking > 0.0) {
        // The breaking model's onset (water_maccormack.wgsl `mac_breaking`): the surface rising
        // faster than half the long-wave speed.
        let rise = -(u(i + 1u, k, false) - u(i, k, false) + w(i, k + 1u, false) - w(i, k, false))
            / p.cell * d;
        if (rise > 0.5 * sqrt(G * d)) { return false; }
    }
    return true;
}
fn face_depth(n: u32, o: u32) -> f32 { return 0.5 * (state[n].y + state[o].y); }

@compute @workgroup_size(8, 8)
fn nh_system(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.size)) { return; }
    let i = id.x; let k = id.y; let n = at(i, k);
    if (!solved(i, k)) { system[n] = vec4f(0.0); xd_in[n] = vec2f(0.0); return; }
    // The previous substep's pressure starts the iteration; its direction does not.
    xd_in[n] = vec2f(xd_in[n].x, 0.0);
    let a = coefficients(i, k);
    let rhs = (continuity(a, i, k, false) - continuity(a, i, k, true)) / p.dt;
    // Each face to water adds its share; a face to a dry column or a wall carries no correction.
    var diag = 1.0 / state[n].y;
    var couple = vec2f(0.0);
    if (i > 0u && wet(i - 1u, k)) {
        let f = face_depth(n, n - 1u);
        diag += a.x * a.x / f;
        couple.x = a.x * coefficients(i - 1u, k).y / f;
    }
    if (i + 1u < p.size.x && wet(i + 1u, k)) { diag += a.y * a.y / face_depth(n, n + 1u); }
    if (k > 0u && wet(i, k - 1u)) {
        let f = face_depth(n, n - p.size.x);
        diag += a.z * a.z / f;
        couple.y = a.z * coefficients(i, k - 1u).w / f;
    }
    if (k + 1u < p.size.y && wet(i, k + 1u)) { diag += a.w * a.w / face_depth(n, n + p.size.x); }
    system[n] = vec4f(rhs, diag, couple);
}

// The Jacobi-preconditioned system's spectrum lies in [max(min_c (1/D_c) / A_cc, 1 - R), 1 + R]:
// above the first because B D_f^-1 B^T is positive semidefinite, and R is the largest Gershgorin
// radius of the symmetrically scaled matrix. Each workgroup reduces its columns before one atomic.
var<workgroup> largest: array<vec2f, 64>;
@compute @workgroup_size(8, 8)
fn nh_bounds(@builtin(global_invocation_id) id: vec3u, @builtin(local_invocation_index) lane: u32) {
    var mine = vec2f(0.0);
    if (all(id.xy < p.size)) {
        let i = id.x; let k = id.y; let n = at(i, k);
        let s = system[n];
        if (s.y > 0.0) {
            var radius = 0.0;
            if (i > 0u && system[n - 1u].y > 0.0) {
                radius += abs(s.z) / sqrt(s.y * system[n - 1u].y);
            }
            if (i + 1u < p.size.x && system[n + 1u].y > 0.0) {
                radius += abs(system[n + 1u].z) / sqrt(s.y * system[n + 1u].y);
            }
            if (k > 0u && system[n - p.size.x].y > 0.0) {
                radius += abs(s.w) / sqrt(s.y * system[n - p.size.x].y);
            }
            if (k + 1u < p.size.y && system[n + p.size.x].y > 0.0) {
                radius += abs(system[n + p.size.x].w) / sqrt(s.y * system[n + p.size.x].y);
            }
            mine = vec2f(1.0 - 1.0 / (state[n].y * s.y), radius);
        }
    }
    largest[lane] = mine;
    workgroupBarrier();
    for (var span = 32u; span > 0u; span >>= 1u) {
        if (lane < span) { largest[lane] = max(largest[lane], largest[lane + span]); }
        workgroupBarrier();
    }
    if (lane == 0u && largest[0].y + largest[0].x > 0.0) {
        atomicMax(&bounds[0], bitcast<u32>(largest[0].x));
        atomicMax(&bounds[1], bitcast<u32>(largest[0].y));
    }
}

// Chebyshev acceleration of the Jacobi-preconditioned system (Y. Saad, "Iterative Methods for
// Sparse Linear Systems", 2nd ed., SIAM 2003, Algorithm 12.1) on the interval [lo, hi]:
// d_0 = r_0 / centre, d_j = rho_j rho_{j-1} d_{j-1} + 2 rho_j / width r_j, with
// rho_j = T_j(s) / T_{j+1}(s) and s = cosh(alpha) = centre / width (the interval's half-width),
// in a form that neither overflows nor cancels. A thread writes each iteration's coefficients.
fn rho(j: u32, alpha: f32) -> f32 {
    let a = exp(-alpha);
    let e = exp(-2.0 * alpha * f32(j));
    return a * (1.0 + e) / (1.0 + e * a * a);
}
@compute @workgroup_size(64)
fn nh_chebyshev(@builtin(local_invocation_index) j: u32) {
    let share = 1.0 - bitcast<f32>(atomicLoad(&bounds[0]));
    let radius = bitcast<f32>(atomicLoad(&bounds[1]));
    let lo = max(share, 1.0 - radius);
    let hi = 1.0 + radius;
    let centre = 0.5 * (hi + lo);
    let width = max(0.5 * (hi - lo), 1e-6);
    let alpha = acosh(centre / width);
    if (j == 0u) {
        chebyshev[0] = vec2f(0.0, 1.0 / centre);
    } else {
        let now = rho(j, alpha);
        chebyshev[j] = vec2f(now * rho(j - 1u, alpha), 2.0 * now / width);
    }
}

// One iteration a dispatch: the reference `nh_block` matches bit for bit (water_nh.rs `REFERENCE`).
// A x at a solved column, from the neighbours' current pressure (zero where it is not solved).
fn apply(n: u32, i: u32, k: u32) -> f32 {
    let s = system[n];
    var y = s.y * xd_in[n].x;
    if (i > 0u) { y += s.z * xd_in[n - 1u].x; }
    if (i + 1u < p.size.x) { y += system[n + 1u].z * xd_in[n + 1u].x; }
    if (k > 0u) { y += s.w * xd_in[n - p.size.x].x; }
    if (k + 1u < p.size.y) { y += system[n + p.size.x].w * xd_in[n + p.size.x].x; }
    return y;
}
@compute @workgroup_size(8, 8)
fn nh_iterate(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.size)) { return; }
    let i = id.x; let k = id.y; let n = at(i, k);
    let s = system[n];
    if (s.y <= 0.0) { xd_out[n] = vec2f(0.0); return; }
    let c = chebyshev[iter.index];
    let d = c.x * xd_in[n].y + c.y * (s.x - apply(n, i, k)) / s.y;
    xd_out[n] = vec2f(xd_in[n].x + d, d);
}

// BLOCK iterations a dispatch (temporal blocking): a workgroup holds a tile of TILE x TILE columns
// with BLOCK columns of halo on every side, the span, iterates in its own memory and writes the
// tile back. An iteration needs a column's neighbours, so each one leaves the outermost ring it
// computed unknown: iteration j computes the columns at least j + 1 from the span's edge, and after
// BLOCK of them exactly the tile remains. Every column computes what `nh_iterate` does, term by
// term in its order, so a dispatch here is BLOCK of those, bit for bit; the halo is computed twice,
// by its own tile and by its neighbour's, and the field is read once for BLOCK iterations, by 12
// dispatches a substep instead of 48. Deeper blocks lose more to their halo: 16 x 16 tiles of 8
// iterations ran slower than these on the M5.
const TILE: u32 = 24u;
const BLOCK: u32 = 4u;
const SPAN: u32 = TILE + 2u * BLOCK;
const_assert SPAN * SPAN == 1024u;
var<workgroup> pressures: array<array<f32, 1024>, 2>;
// A column of the span as a thread holds it: its pressure and direction, its system, and the
// couplings its +x and +z neighbours keep for their faces to it. Columns outside the region are
// unsolved: no pressure, no coupling.
struct Held { x: f32, d: f32, s: vec4f, right: f32, up: f32 }
fn hold(c: u32, origin: vec2i) -> Held {
    var h = Held(0.0, 0.0, vec4f(0.0), 0.0, 0.0);
    let g = origin + vec2i(i32(c % SPAN), i32(c / SPAN));
    if (all(g >= vec2i(0)) && all(g < vec2i(p.size))) {
        let i = u32(g.x); let k = u32(g.y); let n = at(i, k);
        let xd = xd_in[n];
        h.x = xd.x; h.d = xd.y;
        h.s = system[n];
        if (i + 1u < p.size.x) { h.right = system[n + 1u].z; }
        if (k + 1u < p.size.y) { h.up = system[n + p.size.x].w; }
    }
    pressures[0][c] = h.x;
    return h;
}
// Iteration j at column c of the span, if it is at least j + 1 from the span's edge.
fn iterate(h: ptr<function, Held>, c: u32, j: u32, weights: vec2f) {
    let e = vec2u(c % SPAN, c / SPAN);
    if (any(e < vec2u(j + 1u)) || any(e + j + 2u > vec2u(SPAN))) { return; }
    let before = j % 2u;
    if ((*h).s.y > 0.0) {
        var y = (*h).s.y * (*h).x;
        y += (*h).s.z * pressures[before][c - 1u];
        y += (*h).right * pressures[before][c + 1u];
        y += (*h).s.w * pressures[before][c - SPAN];
        y += (*h).up * pressures[before][c + SPAN];
        (*h).d = weights.x * (*h).d + weights.y * ((*h).s.x - y) / (*h).s.y;
        (*h).x = (*h).x + (*h).d;
    } else {
        (*h).x = 0.0;
        (*h).d = 0.0;
    }
    pressures[1u - before][c] = (*h).x;
}
fn release(h: Held, c: u32, origin: vec2i) {
    let e = vec2u(c % SPAN, c / SPAN);
    let g = origin + vec2i(e);
    if (all(e >= vec2u(BLOCK)) && all(e < vec2u(BLOCK + TILE)) && all(g < vec2i(p.size))) {
        xd_out[at(u32(g.x), u32(g.y))] = vec2f(h.x, h.d);
    }
}
@compute @workgroup_size(256)
fn nh_block(@builtin(workgroup_id) tile: vec3u, @builtin(local_invocation_index) lane: u32) {
    // The span's 1024 columns, four a thread in named variables: indexed arrays of them would live
    // in the thread's memory, not its registers, and ran slower than one dispatch an iteration.
    let origin = vec2i(tile.xy * TILE) - vec2i(i32(BLOCK));
    var a = hold(lane, origin);
    var b = hold(lane + 256u, origin);
    var c = hold(lane + 512u, origin);
    var d = hold(lane + 768u, origin);
    workgroupBarrier();
    for (var j = 0u; j < BLOCK; j++) {
        let weights = chebyshev[iter.index + j];
        iterate(&a, lane, j, weights);
        iterate(&b, lane + 256u, j, weights);
        iterate(&c, lane + 512u, j, weights);
        iterate(&d, lane + 768u, j, weights);
        workgroupBarrier();
    }
    release(a, lane, origin);
    release(b, lane + 256u, origin);
    release(c, lane + 512u, origin);
    release(d, lane + 768u, origin);
}

// U = U~ + dt D_f^-1 B^T q on the faces between wet columns inside the region.
@compute @workgroup_size(8, 8)
fn nh_correct(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.size)) { return; }
    let i = id.x; let k = id.y; let n = at(i, k);
    if (!wet(i, k)) { return; }
    let q = xd_in[n].x;
    let a = coefficients(i, k);
    if (i > 0u && wet(i - 1u, k)) {
        let force = a.x * q + coefficients(i - 1u, k).y * xd_in[n - 1u].x;
        let v = velocity[n * 2u] + p.dt * force / face_depth(n, n - 1u);
        velocity[n * 2u] = clamp(v, -p.limit, p.limit);
    }
    if (k > 0u && wet(i, k - 1u)) {
        let force = a.z * q + coefficients(i, k - 1u).w * xd_in[n - p.size.x].x;
        let v = velocity[n * 2u + 1u] + p.dt * force / face_depth(n, n - p.size.x);
        velocity[n * 2u + 1u] = clamp(v, -p.limit, p.limit);
    }
}
