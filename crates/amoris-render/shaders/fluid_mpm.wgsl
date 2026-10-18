// A liquid in a box, by MLS-MPM (moving least squares material point method; Hu et al. 2018), as
// a weakly compressible fluid (fluid.rs, docs/spikes/fluid-box.md): particles carry position,
// velocity and an affine velocity field (APIC); each substep scatters their mass and momentum to a
// grid (P2G), adds the pressure and viscous forces the grid's density gives each particle, updates
// the grid's velocities under gravity and the box's walls, and gathers them back (G2P).
//
// Lengths are in cells (the grid spans [0, n)), time in seconds. WGSL has no float atomics, so
// the scatters add fixed-point integers (`SCALE`).

struct Particle {
    x: vec3f,
    _a: f32,
    v: vec3f,
    _b: f32,
    c: mat3x3f,
};

struct Params {
    grid: vec4u,          // xyz: cells per axis; w: particles
    dt: f32,              // seconds a substep
    gravity: f32,         // along y, cells per second squared (negative: down)
    rest_density: f32,    // particle mass per cell at rest
    stiffness: f32,       // equation of state: p = k ((rho / rho0)^4 - 1)
    viscosity: f32,       // dynamic viscosity
    wall: f32,            // cells of wall inside each face of the grid
    push: vec2f,          // a horizontal acceleration on the whole liquid (sloshing), cells / s^2
};

// 2^16 fixed-point units per unit of mass or momentum: a cell sums eight particles moving at a few
// hundred cells a second well inside an i32, at a precision of 1.5e-5.
const SCALE: f32 = 65536.0;
const MASS: f32 = 1.0;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> particles: array<Particle>;
// Four words per cell: momentum xyz and mass while scattering; velocity xyz (f32 bits) after the
// grid update.
@group(0) @binding(2) var<storage, read_write> cells: array<atomic<i32>>;

fn cell_index(c: vec3i) -> u32 {
    let n = params.grid.xyz;
    return (u32(c.z) * n.y * n.x + u32(c.y) * n.x + u32(c.x)) * 4u;
}

fn encode(f: f32) -> i32 {
    return i32(f * SCALE);
}

fn decode(i: i32) -> f32 {
    return f32(i) / SCALE;
}

// Quadratic B-spline weights of the 3 x 3 x 3 cells around a particle (column k: the k-th cell's
// weight on each axis; a matrix, not an array of vectors, which SPIRV-Cross cannot return to Metal),
// and the first cell.
struct Stencil {
    base: vec3i,
    w: mat3x3f,
};

fn stencil(x: vec3f) -> Stencil {
    var s: Stencil;
    s.base = vec3i(floor(x)) - vec3i(1);
    let d = x - floor(x) - 0.5;
    s.w[0] = 0.5 * (0.5 - d) * (0.5 - d);
    s.w[1] = 0.75 - d * d;
    s.w[2] = 0.5 * (0.5 + d) * (0.5 + d);
    return s;
}

@compute @workgroup_size(64)
fn clear_grid(@builtin(global_invocation_id) id: vec3u) {
    let n = params.grid.x * params.grid.y * params.grid.z;
    if (id.x >= n) {
        return;
    }
    for (var k = 0u; k < 4u; k++) {
        atomicStore(&cells[id.x * 4u + k], 0);
    }
}

// P2G, mass and momentum (with the affine term).
@compute @workgroup_size(64)
fn p2g_mass(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= params.grid.w) {
        return;
    }
    let p = particles[id.x];
    let s = stencil(p.x);
    for (var gx = 0; gx < 3; gx++) {
        for (var gy = 0; gy < 3; gy++) {
            for (var gz = 0; gz < 3; gz++) {
                let weight = s.w[gx].x * s.w[gy].y * s.w[gz].z;
                let c = s.base + vec3i(gx, gy, gz);
                let dist = vec3f(c) - p.x + 0.5;
                let q = p.c * dist;
                let m = weight * MASS;
                let mv = m * (p.v + q);
                let i = cell_index(c);
                atomicAdd(&cells[i], encode(mv.x));
                atomicAdd(&cells[i + 1u], encode(mv.y));
                atomicAdd(&cells[i + 2u], encode(mv.z));
                atomicAdd(&cells[i + 3u], encode(m));
            }
        }
    }
}

// P2G, forces: each particle's density from the grid's mass, its pressure (Tait's equation of
// state) and viscous stress, scattered as momentum.
@compute @workgroup_size(64)
fn p2g_force(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= params.grid.w) {
        return;
    }
    let p = particles[id.x];
    let s = stencil(p.x);
    var density = 0.0;
    for (var gx = 0; gx < 3; gx++) {
        for (var gy = 0; gy < 3; gy++) {
            for (var gz = 0; gz < 3; gz++) {
                let weight = s.w[gx].x * s.w[gy].y * s.w[gz].z;
                let i = cell_index(s.base + vec3i(gx, gy, gz));
                density += decode(atomicLoad(&cells[i + 3u])) * weight;
            }
        }
    }
    let volume = MASS / max(density, 1e-3);
    let ratio = density / params.rest_density;
    let pressure = max(-0.1, params.stiffness * (ratio * ratio * ratio * ratio - 1.0));
    var stress = mat3x3f(-pressure, 0.0, 0.0, 0.0, -pressure, 0.0, 0.0, 0.0, -pressure);
    let strain = p.c + transpose(p.c);
    stress += params.viscosity * strain;
    let term = -volume * 4.0 * params.dt * stress;
    for (var gx = 0; gx < 3; gx++) {
        for (var gy = 0; gy < 3; gy++) {
            for (var gz = 0; gz < 3; gz++) {
                let weight = s.w[gx].x * s.w[gy].y * s.w[gz].z;
                let c = s.base + vec3i(gx, gy, gz);
                let dist = vec3f(c) - p.x + 0.5;
                let f = term * (weight * dist);
                let i = cell_index(c);
                atomicAdd(&cells[i], encode(f.x));
                atomicAdd(&cells[i + 1u], encode(f.y));
                atomicAdd(&cells[i + 2u], encode(f.z));
            }
        }
    }
}

// Momentum to velocity, gravity, the push, and the box: no velocity into a wall.
@compute @workgroup_size(64)
fn update_grid(@builtin(global_invocation_id) id: vec3u) {
    let n = params.grid.xyz;
    if (id.x >= n.x * n.y * n.z) {
        return;
    }
    let i = id.x * 4u;
    let mass = decode(atomicLoad(&cells[i + 3u]));
    var v = vec3f(0.0);
    if (mass > 0.0) {
        v = vec3f(
            decode(atomicLoad(&cells[i])),
            decode(atomicLoad(&cells[i + 1u])),
            decode(atomicLoad(&cells[i + 2u])),
        ) / mass;
        v.y += params.dt * params.gravity;
        v.x += params.dt * params.push.x;
        v.z += params.dt * params.push.y;
        let c = vec3i(i32(id.x % n.x), i32((id.x / n.x) % n.y), i32(id.x / (n.x * n.y)));
        let lo = vec3i(2);
        let hi = vec3i(n) - vec3i(3);
        if (c.x < lo.x || c.x > hi.x) {
            v.x = 0.0;
        }
        if (c.y < lo.y || c.y > hi.y) {
            v.y = 0.0;
        }
        if (c.z < lo.z || c.z > hi.z) {
            v.z = 0.0;
        }
    }
    atomicStore(&cells[i], bitcast<i32>(v.x));
    atomicStore(&cells[i + 1u], bitcast<i32>(v.y));
    atomicStore(&cells[i + 2u], bitcast<i32>(v.z));
}

// G2P: velocity and its affine field from the grid, then the move, kept inside the box with a soft
// predictive wall.
@compute @workgroup_size(64)
fn g2p(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= params.grid.w) {
        return;
    }
    var p = particles[id.x];
    let s = stencil(p.x);
    var v = vec3f(0.0);
    var b = mat3x3f(vec3f(0.0), vec3f(0.0), vec3f(0.0));
    for (var gx = 0; gx < 3; gx++) {
        for (var gy = 0; gy < 3; gy++) {
            for (var gz = 0; gz < 3; gz++) {
                let weight = s.w[gx].x * s.w[gy].y * s.w[gz].z;
                let c = s.base + vec3i(gx, gy, gz);
                let dist = vec3f(c) - p.x + 0.5;
                let i = cell_index(c);
                let cv = vec3f(
                    bitcast<f32>(atomicLoad(&cells[i])),
                    bitcast<f32>(atomicLoad(&cells[i + 1u])),
                    bitcast<f32>(atomicLoad(&cells[i + 2u])),
                );
                let wv = cv * weight;
                b += mat3x3f(wv * dist.x, wv * dist.y, wv * dist.z);
                v += wv;
            }
        }
    }
    p.c = b * 4.0;
    p.x += v * params.dt;
    let n = vec3f(params.grid.xyz);
    p.x = clamp(p.x, vec3f(1.0), n - 2.0);
    // A wall a few cells in: what would cross it next substep is turned back now.
    let next = p.x + v * params.dt * 3.0;
    let lo = vec3f(params.wall);
    let hi = n - 1.0 - params.wall;
    for (var a = 0; a < 3; a++) {
        if (next[a] < lo[a]) {
            v[a] += (lo[a] - next[a]) / (params.dt * 3.0);
        }
        if (next[a] > hi[a]) {
            v[a] += (hi[a] - next[a]) / (params.dt * 3.0);
        }
    }
    p.v = v;
    particles[id.x] = p;
}
