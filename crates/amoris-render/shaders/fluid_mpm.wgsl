// Liquid particles by MLS-MPM (moving least squares material point method; Hu et al. 2018), as a
// weakly compressible fluid (fluid.rs; docs/spec/splashes.md 4, docs/spikes/fluid-box.md):
// particles carry position, velocity and an affine velocity field (APIC); each substep scatters
// their mass and momentum to a grid (P2G), adds the pressure and viscous forces the grid's density
// gives each particle, updates the grid's velocities under gravity and the solid blocks, and
// gathers them back (G2P).
//
// The grid is cut into regions of `params.dims` cells each, one per open region; a particle lives
// in one, in that region's cells. A region's mask holds a byte per block (`params.blocks.w` cells
// on an edge): 255 a solid block, 1 to 9 the surface of a splashing liquid in ninths of the block
// (a particle under it dies), 0 open. Lengths are in cells, time in seconds. WGSL has no float
// atomics, so the scatters add fixed-point integers (`SCALE`).

struct Particle {
    x: vec3f,
    age: f32,             // seconds
    v: vec3f,
    tag: u32,             // the region's slot (bits 0-2) and generation (the rest); DEAD: none
    c: mat3x3f,
};

struct Region {
    origin: vec4f,        // the region's corner in the world; w: metres per cell (0: closed)
    absorb: vec4f,        // the liquid's absorption per metre (rgb)
    generation: vec4u,    // x: the generation open in this slot
};

struct Params {
    dims: vec4u,          // xyz: cells per region and axis; w: the pool's capacity
    blocks: vec4u,        // xyz: mask bytes per region and axis; w: cells per block edge
    dt: f32,              // seconds a substep
    gravity: f32,         // m/s2, down
    rest_density: f32,    // particle mass per cell at rest
    stiffness: f32,       // equation of state: p = k ((rho / rho0)^4 - 1)
    viscosity: f32,       // dynamic viscosity
    lifetime: f32,        // seconds a particle lives (0: for ever)
    push: vec2f,          // a horizontal acceleration on every particle, m/s2 (sloshing)
};

const DEAD: u32 = 0xffffffffu;
const SLOTS: u32 = 4u;
// 2^16 fixed-point units per unit of mass or momentum: a cell sums eight particles moving at a few
// hundred cells a second well inside an i32, at a precision of 1.5e-5.
const SCALE: f32 = 65536.0;
const MASS: f32 = 1.0;
const SOLID: u32 = 255u;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> particles: array<Particle>;
// Four words per cell: momentum xyz and mass while scattering; velocity xyz (f32 bits) after the
// grid update.
@group(0) @binding(2) var<storage, read_write> cells: array<atomic<i32>>;
@group(0) @binding(3) var<uniform> regions: array<Region, 4>;
@group(0) @binding(4) var<storage, read> masks: array<u32>;

fn region_cells() -> u32 {
    return params.dims.x * params.dims.y * params.dims.z;
}

// The particle's slot, or SLOTS when it is dead or its region closed or reopened since.
fn slot_of(p: Particle) -> u32 {
    if (p.tag == DEAD) {
        return SLOTS;
    }
    let slot = p.tag & 7u;
    if (slot >= SLOTS || regions[slot].origin.w <= 0.0 || regions[slot].generation.x != (p.tag >> 3u)) {
        return SLOTS;
    }
    return slot;
}

fn cell_index(slot: u32, c: vec3i) -> u32 {
    let n = params.dims.xyz;
    return (slot * region_cells() + u32(c.z) * n.y * n.x + u32(c.y) * n.x + u32(c.x)) * 4u;
}

// The mask byte of the block holding cell `c` (outside the region: solid).
fn mask_at(slot: u32, c: vec3i) -> u32 {
    let b = c / i32(params.blocks.w);
    let n = vec3i(params.blocks.xyz);
    if (any(c < vec3i(0)) || any(b >= n)) {
        return SOLID;
    }
    let i = u32(b.x + n.x * (b.y + n.y * b.z));
    let words = (params.blocks.x * params.blocks.y * params.blocks.z + 3u) / 4u;
    return (masks[slot * words + i / 4u] >> ((i % 4u) * 8u)) & 0xffu;
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
    if (id.x >= region_cells() * SLOTS) {
        return;
    }
    if (regions[id.x / region_cells()].origin.w <= 0.0) {
        return;
    }
    for (var k = 0u; k < 4u; k++) {
        atomicStore(&cells[id.x * 4u + k], 0);
    }
}

// P2G, mass and momentum (with the affine term).
@compute @workgroup_size(64)
fn p2g_mass(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= params.dims.w) {
        return;
    }
    let p = particles[id.x];
    let slot = slot_of(p);
    if (slot == SLOTS) {
        return;
    }
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
                let i = cell_index(slot, c);
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
    if (id.x >= params.dims.w) {
        return;
    }
    let p = particles[id.x];
    let slot = slot_of(p);
    if (slot == SLOTS) {
        return;
    }
    let s = stencil(p.x);
    var density = 0.0;
    for (var gx = 0; gx < 3; gx++) {
        for (var gy = 0; gy < 3; gy++) {
            for (var gz = 0; gz < 3; gz++) {
                let weight = s.w[gx].x * s.w[gy].y * s.w[gz].z;
                let i = cell_index(slot, s.base + vec3i(gx, gy, gz));
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
                let i = cell_index(slot, c);
                atomicAdd(&cells[i], encode(f.x));
                atomicAdd(&cells[i + 1u], encode(f.y));
                atomicAdd(&cells[i + 2u], encode(f.z));
            }
        }
    }
}

// Momentum to velocity, gravity and the push; no velocity in a solid block, none into one beside.
@compute @workgroup_size(64)
fn update_grid(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= region_cells() * SLOTS) {
        return;
    }
    let slot = id.x / region_cells();
    let r = regions[slot];
    if (r.origin.w <= 0.0) {
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
        let to_cells = params.dt / r.origin.w;
        v.y -= params.gravity * to_cells;
        v.x += params.push.x * to_cells;
        v.z += params.push.y * to_cells;
        let n = params.dims.xyz;
        let local = id.x % region_cells();
        let c = vec3i(i32(local % n.x), i32((local / n.x) % n.y), i32(local / (n.x * n.y)));
        if (mask_at(slot, c) == SOLID) {
            v = vec3f(0.0);
        }
        for (var a = 0; a < 3; a++) {
            var e = vec3i(0);
            e[a] = 1;
            if (v[a] < 0.0 && mask_at(slot, c - e) == SOLID) {
                v[a] = 0.0;
            }
            if (v[a] > 0.0 && mask_at(slot, c + e) == SOLID) {
                v[a] = 0.0;
            }
        }
    }
    atomicStore(&cells[i], bitcast<i32>(v.x));
    atomicStore(&cells[i + 1u], bitcast<i32>(v.y));
    atomicStore(&cells[i + 2u], bitcast<i32>(v.z));
}

// G2P: velocity and its affine field from the grid, then the move, axis by axis, kept out of solid
// blocks; then the particle's death: under a liquid's surface, near its region's edge, or old.
@compute @workgroup_size(64)
fn g2p(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= params.dims.w) {
        return;
    }
    var p = particles[id.x];
    let slot = slot_of(p);
    if (slot == SLOTS) {
        return;
    }
    let s = stencil(p.x);
    var v = vec3f(0.0);
    var b = mat3x3f(vec3f(0.0), vec3f(0.0), vec3f(0.0));
    for (var gx = 0; gx < 3; gx++) {
        for (var gy = 0; gy < 3; gy++) {
            for (var gz = 0; gz < 3; gz++) {
                let weight = s.w[gx].x * s.w[gy].y * s.w[gz].z;
                let c = s.base + vec3i(gx, gy, gz);
                let dist = vec3f(c) - p.x + 0.5;
                let i = cell_index(slot, c);
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
    // A move that would end in a solid block stays where it was on that axis and loses its
    // velocity into the block.
    for (var a = 0; a < 3; a++) {
        var next = p.x;
        next[a] += v[a] * params.dt;
        if (mask_at(slot, vec3i(floor(next))) == SOLID) {
            v[a] = 0.0;
        } else {
            p.x = next;
        }
    }
    p.v = v;
    p.age += params.dt;
    let n = vec3f(params.dims.xyz);
    let edge = any(p.x < vec3f(2.0)) || any(p.x >= n - 2.0);
    let old = params.lifetime > 0.0 && p.age >= params.lifetime;
    var under = false;
    let m = mask_at(slot, vec3i(floor(p.x)));
    if (p.age > 0.1 && m >= 1u && m <= 9u) {
        let k = f32(params.blocks.w);
        let height = p.x.y / k - floor(p.x.y / k);
        under = height * 9.0 < f32(m);
    }
    if (edge || old || under) {
        p.tag = DEAD;
    }
    particles[id.x] = p;
}
