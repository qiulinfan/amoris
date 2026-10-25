// Sub-grid interactive ripples (water_ripples.rs, water.md 57): linear capillary-gravity waves
// on a window of 512 x 512 cells of 1/32 m to 1/27 m over a water region, finer than its columns
// can carry.
// The state is the surface's rise zeta and its velocity potential phi, packed as zeta + i phi in
// one complex field; the window is toroidal (a world cell lives at its index modulo 512), so it
// follows the camera without moving its data. A step:
// 1. `stir`: each body point splats the change of the volume it holds under the drawn surface,
//    between its place now and its last place carried on by the water, as a rise (a fixed-point
//    sum, so the result does not depend on the order of the adds);
// 2. `forward_rows`: the rise joins zeta and the rows go to Fourier space;
// 3. `turn_columns`, `turn_rows`: the columns go too; then, at each depth level the window's
//    water needs, each mode turns exactly by its own dispersion relation, omega^2 = (g k +
//    sigma/rho k^3) tanh(k h), decays by viscosity and a surface film, and comes back; the levels
//    blend per cell by the water's depth there;
// 4. `finish`: the field is carried by the water's current, masked off dry ground and absorbed
//    on shores and in a sponge along the window's edge;
// 5. `surface`, `mip`: the rise and its slopes, with their mean square for filtering, as a
//    mipmapped texture the surface shading samples.
// The exponential integrator follows J. Tessendorf, "eWave: Using an Exponential Solver on the
// iWave Problem" (2014), after his "Interactive Water Surfaces" (iWave, Game Programming Gems 4,
// 2004); blending depth levels follows S. Jeschke and C. Wojtan, "Generalizing Shallow Water
// Simulations with Dispersive Surface Waves" (ACM TOG 42(4), 2023) as water_waves.wgsl does.

struct Params {
    origin: vec2i,        // the window's first cell, world cells along x and z
    previous: vec2i,      // the last step's
    region_origin: vec2f, // the field's corner, world x and z
    region_cell: f32,     // metres a column
    dt: f32,              // simulated seconds this step
    count: vec4u,         // the field's columns along x and z, 1 when in tiles of 16 x 16, points
    cutoff: f32,          // wavenumber the field carries itself from, rad/m
    fresh: u32,           // 1: the step after a clear; every cell starts at rest
    cell: f32,            // metres a cell of the window
    joined: u32,          // the field's sides (bits -x, +x, -z, +z) joined to another region's
}

@group(0) @binding(0) var<uniform> p: Params;
// The field's columns: surface (world y), depth, velocity along x and z.
@group(0) @binding(1) var<storage, read> columns: array<vec4f>;
@group(0) @binding(2) var<storage, read_write> state: array<vec2f>;
@group(0) @binding(3) var<storage, read_write> spectrum: array<vec2f>;
// Each depth level's turned field, back along z: four of the window's size.
@group(0) @binding(4) var<storage, read_write> scratch: array<vec2f>;
@group(0) @binding(5) var<storage, read_write> next: array<vec2f>;
@group(0) @binding(6) var<storage, read_write> source: array<atomic<i32>>;
// Per cell: the water's depth, its velocity (x, z) and 1 where it is wet inside the field.
@group(0) @binding(7) var<storage, read_write> cells: array<vec4f>;
// Two words a point: its centre (world x, y, z) and height; its volume, slot, fresh flag.
@group(0) @binding(8) var<storage, read> points: array<vec4f>;
// Two words a slot: where the point was last (world x, z), the depths of its submerged part's top
// and bottom; its waterplane's area and radius.
@group(0) @binding(9) var<storage, read_write> memory: array<vec4f>;
@group(0) @binding(10) var<storage, read_write> depth_min: atomic<u32>;
@group(0) @binding(13) var surface_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(14) var mip_in: texture_2d<f32>;
@group(0) @binding(15) var mip_out: texture_storage_2d<rgba16float, write>;
// Per cell: where its depth falls among the depth levels, 0 to 3, continuous.
@group(0) @binding(16) var<storage, read_write> blend: array<f32>;

const N: u32 = 512u;
const NI: i32 = 512;
const BITS: u32 = 9u;
const TAU: f32 = 6.283185307179586;
const G: f32 = 9.81;
// Surface tension over density, m^3/s^2 (clean water, 0.0728 N/m).
const SURFACE: f32 = 7.28e-5;
// Kinematic viscosity, m^2/s.
const NU: f32 = 1.0e-6;
// A rise of 2^-20 m (about a micrometre) per unit of the fixed-point sum.
const HEAD_SCALE: f32 = 1048576.0;
const SHALLOW_WET: f32 = 0.01;
// The sponge along the window's edge: its width in cells and its full rate, per second.
const SPONGE: i32 = 32;
const SPONGE_RATE: f32 = 12.0;
// Shores absorb ripples: the rate, per second, in water under 2 cm, fading out by 8 cm.
const SHORE_RATE: f32 = 6.0;
// Waves the field carries itself leave the window within a few frames.
const HANDOVER_RATE: f32 = 30.0;
// The shortest waves, near the grid's limit, die within a quarter second.
const GRID_RATE: f32 = 4.0;
// Depth levels: 0.064 m and every factor 2.5 above, to 1 m, deep for every wave the window holds
// shorter than 2 m.
const LEVELS: u32 = 4u;
const LEVEL_0: f32 = 0.064;

var<workgroup> line: array<vec2f, 512>;
var<workgroup> partial: array<f32, 256>;
var<workgroup> held: array<vec4f, 2>;

fn wrap(a: i32) -> u32 { return u32(((a % NI) + NI) % NI); }
fn at_cell(c: vec2i) -> u32 { return wrap(c.x) + N * wrap(c.y); }
// The world cell buffer cell `b` holds in the window whose first cell is `o`.
fn world_cell(b: vec2u, o: vec2i) -> vec2i {
    let d = (vec2i(b) - o) % vec2i(NI);
    return o + select(d, d + vec2i(NI), d < vec2i(0));
}
fn in_window(c: vec2i) -> bool { return all(c >= p.origin) && all(c < p.origin + vec2i(NI)); }

fn column(c: vec2i) -> vec4f {
    let n = vec2i(p.count.xy);
    let q = vec2u(clamp(c, vec2i(0), n - 1));
    if (p.count.z == 1u) {
        let tile = (q.x / 16u) + (p.count.x / 16u) * (q.y / 16u);
        return columns[tile * 256u + q.x % 16u + 16u * (q.y % 16u)];
    }
    return columns[q.x + p.count.x * q.y];
}
fn wet(c: vec4f) -> bool { return c.y > 0.002 && abs(c.x) < 1e10; }
// The water at a world point, bilinear between column centres: its surface (a dry column takes
// the point's own column's, so walls do not tilt it), depth and velocity, and whether the point
// lies inside the field.
struct Water { surface: f32, depth: f32, flow: vec2f, inside: bool }
fn water_at(xz: vec2f) -> Water {
    let g = (xz - p.region_origin) / p.region_cell;
    // Past a side joined to another region the water goes on as its edge columns (water.md 64.3).
    let open = vec4<bool>((p.joined & 1u) != 0u, (p.joined & 2u) != 0u, (p.joined & 4u) != 0u,
        (p.joined & 8u) != 0u);
    let inside = all((g >= vec2f(0.0)) | open.xz) && all((g < vec2f(p.count.xy)) | open.yw);
    let u = g - 0.5;
    let i0 = vec2i(floor(u));
    let t = u - floor(u);
    let home = column(vec2i(floor(g)));
    let a = column(i0);
    let b = column(i0 + vec2i(1, 0));
    let c = column(i0 + vec2i(0, 1));
    let d = column(i0 + vec2i(1, 1));
    let s = vec4f(select(home.x, a.x, wet(a)), select(home.x, b.x, wet(b)),
        select(home.x, c.x, wet(c)), select(home.x, d.x, wet(d)));
    let depth = vec4f(max(a.y, 0.0), max(b.y, 0.0), max(c.y, 0.0), max(d.y, 0.0));
    return Water(mix(mix(s.x, s.y, t.x), mix(s.z, s.w, t.x), t.y),
        mix(mix(depth.x, depth.y, t.x), mix(depth.z, depth.w, t.x), t.y),
        mix(mix(a.zw, b.zw, t.x), mix(c.zw, d.zw, t.x), t.y), inside);
}

// --- 1. Stirring bodies -------------------------------------------------------------------------

// A point under the drawn surface: its centre (world x, z), how deep under the surface the top
// and bottom of its submerged part lie, and the waterplane it stands for: a layer `size` high
// holding `volume` stands for volume / size square metres (its area, and a disk's radius).
struct Body { at: vec2f, top: f32, bottom: f32, area: f32, plane: f32 }
fn body(a: vec4f, volume: f32) -> Body {
    let w = water_at(a.xz);
    let size = max(a.w, 1e-3);
    let area = max(volume, 0.0) / size;
    var top = 0.0;
    var bottom = 0.0;
    if (w.inside && w.depth > SHALLOW_WET) {
        let high = min(a.y + 0.5 * size, w.surface);
        let low = max(a.y - 0.5 * size, w.surface - w.depth);
        if (high > low) {
            top = w.surface - high;
            bottom = w.surface - low;
        }
    }
    return Body(a.xz, top, bottom, area, sqrt(area / PI));
}
fn footprint(c: vec2i, at: vec2f, radius: f32) -> f32 {
    let d = ((vec2f(c) + 0.5) * p.cell - at) / radius;
    let s = max(1.0 - dot(d, d), 0.0);
    return s * s;
}
// Spreads `volume` over the smooth disk of `radius` around `at`: weights (1 - d^2/R^2)^2 at the
// covered cell centres, normalized over them all so the rise it adds holds the volume, then cut to
// the window. One workgroup a point, 64 lanes over the disk's cells.
fn splat(lane: u32, at: vec2f, radius: f32, volume: f32) {
    let lo = vec2i(floor((at - radius) / p.cell));
    let side = vec2i(floor((at + radius) / p.cell)) - lo + vec2i(1);
    let total = u32(side.x * side.y);
    var sum = 0.0;
    for (var n = lane; n < total; n += 64u) {
        sum += footprint(lo + vec2i(i32(n) % side.x, i32(n) / side.x), at, radius);
    }
    partial[lane] = sum;
    workgroupBarrier();
    for (var s = 32u; s > 0u; s >>= 1u) {
        if (lane < s) { partial[lane] += partial[lane + s]; }
        workgroupBarrier();
    }
    let weights = partial[0];
    workgroupBarrier();
    let scale = select(0.0, volume / (weights * p.cell * p.cell) * HEAD_SCALE, weights > 0.0);
    for (var n = lane; n < total; n += 64u) {
        let c = lo + vec2i(i32(n) % side.x, i32(n) / side.x);
        let units = i32(round(footprint(c, at, radius) * scale));
        if (units != 0 && in_window(c)) { atomicAdd(&source[at_cell(c)], units); }
    }
}
// What a body holds under the surface stirs the surface by its depth: the bump a source z deep
// raises spreads over about z, so a mode of wavenumber k feels it by exp(-k z). The submerged part
// goes in slices, finer near the surface, each spread over 2.5 times its depth past the waterplane;
// below 4 over the cutoff it would stir only waves the field carries. A swimmer's tall body stirs
// as its shoulders do, a duck's shallow hull with all it holds.
const SLICES: u32 = 4u;
fn stir_body(lane: u32, b: Body, at: vec2f, sign: f32) {
    let reach = min(b.bottom, 4.0 / p.cutoff);
    for (var i = 0u; i < SLICES; i++) {
        let f0 = f32(i) / f32(SLICES);
        let f1 = f32(i + 1u) / f32(SLICES);
        let z0 = b.top + (reach - b.top) * f0 * f0;
        let z1 = b.top + (reach - b.top) * f1 * f1;
        let depth = 0.5 * (z0 + z1);
        let radius = clamp(sqrt(b.plane * b.plane + 6.25 * depth * depth), 1.5 * p.cell, 2.0);
        splat(lane, at, radius, sign * b.area * max(z1 - z0, 0.0));
    }
}
@compute @workgroup_size(64)
fn stir(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lane: u32) {
    let a = points[2u * group.x];
    let b = points[2u * group.x + 1u];
    let slot = bitcast<u32>(b.y);
    // A point seen for the first time (or after a clear) records where it is and stirs nothing:
    // its volume was already in the water, and only its change from now on is a disturbance.
    let gain = select(1.0, 0.0, bitcast<u32>(b.z) != 0u || p.fresh != 0u);
    let now = body(a, b.x);
    // One lane reads and writes the slot; the others take its record from workgroup memory.
    if (lane == 0u) {
        held[0] = memory[2u * slot];
        held[1] = memory[2u * slot + 1u];
    }
    workgroupBarrier();
    let before = Body(held[0].xy, held[0].z, held[0].w, held[1].x, held[1].y);
    // Where the point would be had it moved with the water since: what it moves relative to
    // the water stirs it, so a duck drifting on a current makes no waves.
    let moved = before.at + water_at(before.at).flow * p.dt;
    stir_body(lane, now, now.at, gain);
    stir_body(lane, before, moved, -gain);
    if (lane == 0u) {
        memory[2u * slot] = vec4f(now.at, now.top, now.bottom);
        memory[2u * slot + 1u] = vec4f(now.area, now.plane, 0.0, 0.0);
    }
}

// --- 2, 3. Fourier transforms and the exact dispersive step ----------------------------------

var<workgroup> other: array<vec2f, 512>;
var<workgroup> needed: u32;

fn reversed(j: u32) -> u32 { return reverseBits(j) >> (32u - BITS); }
fn butterfly(a: vec2f, b: vec2f, w: vec2f) -> array<vec2f, 2> {
    let wb = vec2f(w.x * b.x - w.y * b.y, w.x * b.y + w.y * b.x);
    return array<vec2f, 2>(a + wb, a - wb);
}
// The butterflies of radix-2 transforms of `line` and, with `both`, of `other` as well, each
// loaded in bit-reversed order (water_fft.wgsl): one barrier a stage for the pair.
fn transform(lane: u32, inverse: bool, both: bool) {
    let sign = select(-1.0, 1.0, inverse);
    for (var s = 1u; s <= BITS; s++) {
        let half = 1u << (s - 1u);
        let span = half << 1u;
        for (var t = lane; t < N / 2u; t += 256u) {
            let k = t % half;
            let i0 = (t / half) * span + k;
            let i1 = i0 + half;
            let angle = sign * TAU * f32(k) / f32(span);
            let w = vec2f(cos(angle), sin(angle));
            let x = butterfly(line[i0], line[i1], w);
            line[i0] = x[0];
            line[i1] = x[1];
            if (both) {
                let y = butterfly(other[i0], other[i1], w);
                other[i0] = y[0];
                other[i1] = y[1];
            }
        }
        workgroupBarrier();
    }
}

// One row a workgroup: a cell that entered the window since the last step starts at rest, the
// bodies' rise joins it (read and cleared), and the water under each cell is sampled once for
// the step. The shallowest wet water in the window decides which depth levels run.
@compute @workgroup_size(256)
fn forward_rows(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lane: u32) {
    let row = group.x;
    var low = 1e9;
    for (var j = lane; j < N; j += 256u) {
        let i = j + N * row;
        let cell = world_cell(vec2u(j, row), p.origin);
        let entered = any(cell < p.previous) || any(cell >= p.previous + vec2i(NI))
            || p.fresh != 0u;
        var z = select(state[i], vec2f(0.0), entered);
        z.x += f32(atomicExchange(&source[i], 0)) / HEAD_SCALE;
        let w = water_at((vec2f(cell) + 0.5) * p.cell);
        let here = w.inside && w.depth > SHALLOW_WET;
        cells[i] = vec4f(w.depth, w.flow, select(0.0, 1.0, here));
        blend[i] = clamp(log(max(w.depth, 1e-6) / LEVEL_0) / log(2.5), 0.0, 3.0);
        if (here) { low = min(low, w.depth); }
        line[reversed(j)] = z;
    }
    partial[lane] = low;
    workgroupBarrier();
    for (var s = 128u; s > 0u; s >>= 1u) {
        if (lane < s) { partial[lane] = min(partial[lane], partial[lane + s]); }
        workgroupBarrier();
    }
    // Positive floats order as their bits do.
    if (lane == 0u) { atomicMin(&depth_min, bitcast<u32>(partial[0])); }
    transform(lane, false, false);
    for (var j = lane; j < N; j += 256u) { spectrum[j + N * row] = line[j]; }
}

// The depth levels the window's water needs, a bit each, the same for the whole workgroup: the
// deepest always, each shallower one only when some wet cell lies below the level above it.
fn levels_needed(lane: u32) -> u32 {
    if (lane == 0u) {
        let low = bitcast<f32>(atomicLoad(&depth_min));
        var mask = 8u;
        for (var l = 0u; l < 3u; l++) {
            if (low < LEVEL_0 * pow(2.5, f32(l + 1u))) { mask = mask | (1u << l); }
        }
        needed = mask;
    }
    return workgroupUniformLoad(&needed);
}

fn damping(k: f32, omega: f32, axis: f32) -> f32 {
    // Viscosity over a clean surface (2 nu k^2) and the inextensible film nearly every real
    // surface carries, k sqrt(nu omega / 8), which takes ripples a few centimetres long in about
    // a second and decimetre ones in tens of seconds.
    let viscous = 2.0 * NU * k * k + k * sqrt(NU * omega / 8.0);
    let handover = HANDOVER_RATE * (1.0 - smoothstep(0.5 * p.cutoff, p.cutoff, k));
    let grid = GRID_RATE * smoothstep(0.75, 1.0, axis * p.cell / PI);
    return viscous + handover + grid;
}
// Mode (m, n) after the step in water `depth` deep: its rise and potential, recovered from the
// packed transform and its mirror, turn by omega dt as the exact solution of
// d(zeta)/dt = k tanh(k h) phi, d(phi)/dt = -(g + sigma/rho k^2) zeta.
fn turn(mn: vec2u, z: vec2f, mirror: vec2f, depth: f32) -> vec2f {
    let signed = vec2i(mn) - select(vec2i(0), vec2i(NI), mn >= vec2u(N / 2u));
    let kv = vec2f(signed) * (TAU / (f32(N) * p.cell));
    let k = length(kv);
    let conj = vec2f(mirror.x, -mirror.y);
    let rise = 0.5 * (z + conj);
    let d = z - conj;
    let potential = vec2f(0.5 * d.y, -0.5 * d.x);
    let lift = k * tanh(min(k * depth, 10.0));
    let pull = G + SURFACE * k * k;
    let omega = sqrt(lift * pull);
    var r = rise;
    var q = potential;
    if (omega > 1e-6) {
        let c = cos(omega * p.dt);
        let s = sin(omega * p.dt);
        r = c * rise + (lift / omega) * s * potential;
        q = c * potential - (pull / omega) * s * rise;
    }
    let keep = exp(-p.dt * damping(k, omega, max(abs(kv.x), abs(kv.y))));
    return keep * (r + vec2f(-q.y, q.x));
}
// One pair of columns a workgroup, m and its mirror 512 - m (0 and 256 alone): both go along z;
// then, for each depth level the window needs, every mode turns (its mirror is in the pair) and
// both columns come back along z into that level's part of the scratch.
@compute @workgroup_size(256)
fn turn_columns(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lane: u32) {
    let m = group.x;
    let partner = (N - m) % N;
    let both = m != partner;
    for (var j = lane; j < N; j += 256u) {
        line[reversed(j)] = spectrum[m + N * j];
        other[reversed(j)] = spectrum[partner + N * j];
    }
    let mask = levels_needed(lane);
    workgroupBarrier();
    transform(lane, false, true);
    // Each lane keeps what its two rows j need: mode (m, j) with its mirror (partner, -j), and
    // (partner, j) with (m, -j).
    var a: array<vec2f, 2>;
    var am: array<vec2f, 2>;
    var b: array<vec2f, 2>;
    var bm: array<vec2f, 2>;
    for (var r = 0u; r < 2u; r++) {
        let j = lane + 256u * r;
        let mirror = (N - j) % N;
        a[r] = line[j];
        am[r] = line[mirror];
        b[r] = other[j];
        bm[r] = other[mirror];
    }
    workgroupBarrier();
    for (var l = 0u; l < LEVELS; l++) {
        if ((mask & (1u << l)) != 0u) {
            let depth = LEVEL_0 * pow(2.5, f32(l));
            for (var r = 0u; r < 2u; r++) {
                let j = lane + 256u * r;
                line[reversed(j)] = turn(vec2u(m, j), a[r], bm[r], depth);
                other[reversed(j)] = turn(vec2u(partner, j), b[r], am[r], depth);
            }
            workgroupBarrier();
            transform(lane, true, both);
            for (var r = 0u; r < 2u; r++) {
                let j = lane + 256u * r;
                scratch[l * N * N + m + N * j] = line[j] / f32(N);
                if (both) { scratch[l * N * N + partner + N * j] = other[j] / f32(N); }
            }
            workgroupBarrier();
        }
    }
}
// One row a workgroup: each level's part comes back along x and joins the blend with the cell's
// share of that level, linear in the logarithm of its depth between the levels around it.
@compute @workgroup_size(256)
fn turn_rows(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lane: u32) {
    let row = group.x;
    let mask = levels_needed(lane);
    var sum = array<vec2f, 2>(vec2f(0.0), vec2f(0.0));
    for (var l = 0u; l < LEVELS; l++) {
        if ((mask & (1u << l)) != 0u) {
            for (var j = lane; j < N; j += 256u) {
                line[reversed(j)] = scratch[l * N * N + j + N * row];
            }
            workgroupBarrier();
            transform(lane, true, false);
            for (var r = 0u; r < 2u; r++) {
                let j = lane + 256u * r;
                let share = max(1.0 - abs(blend[j + N * row] - f32(l)), 0.0);
                sum[r] += line[j] / f32(N) * share;
            }
            workgroupBarrier();
        }
    }
    for (var r = 0u; r < 2u; r++) {
        next[lane + 256u * r + N * row] = sum[r];
    }
}

// --- 4. Transport, masks and absorption --------------------------------------------------------

fn catmull_rom(t: f32) -> vec4f {
    let t2 = t * t;
    let t3 = t2 * t;
    return vec4f(-0.5 * t3 + t2 - 0.5 * t, 1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t, 0.5 * t3 - 0.5 * t2);
}
// `next` at a fractional buffer position, bicubic: the bilinear kernel would blur the ripples a
// little at every step the current carries them.
fn carried(at: vec2f) -> vec2f {
    let base = vec2i(floor(at));
    let wx = catmull_rom(at.x - floor(at.x));
    let wz = catmull_rom(at.y - floor(at.y));
    var sum = vec2f(0.0);
    for (var k = 0; k < 4; k++) {
        var row = vec2f(0.0);
        for (var i = 0; i < 4; i++) { row += next[at_cell(base + vec2i(i - 1, k - 1))] * wx[i]; }
        sum += row * wz[k];
    }
    return sum;
}
@compute @workgroup_size(8, 8)
fn finish(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= vec2u(N))) { return; }
    let i = id.x + N * id.y;
    let c = cells[i];
    // Semi-Lagrangian: the value that the current brings to this cell over the step. Still water
    // (a thousandth of a cell a step) keeps its value, without the bicubic kernel's work.
    let back = c.yz * (p.dt / p.cell);
    var z = next[i];
    if (any(abs(back) > vec2f(1e-3))) { z = carried(vec2f(id.xy) - back); }
    let cell = world_cell(id.xy, p.origin);
    let far = p.origin + vec2i(NI - 1);
    let edge = min(min(cell.x - p.origin.x, far.x - cell.x),
        min(cell.y - p.origin.y, far.y - cell.y));
    let sponge = clamp(f32(SPONGE - edge) / f32(SPONGE), 0.0, 1.0);
    let shore = 1.0 - smoothstep(0.02, 0.08, c.x);
    z *= exp(-p.dt * (SPONGE_RATE * sponge * sponge + SHORE_RATE * shore));
    // Dry ground, walls and what lies past the field hold no ripples: waves reflect from them.
    if (c.w == 0.0 || edge <= 0) { z = vec2f(0.0); }
    state[i] = z;
}

// --- 5. What the shading samples -----------------------------------------------------------------

fn rise(c: vec2i) -> f32 { return state[at_cell(c)].x; }
// The rise and its slopes (fourth-order central differences) with their mean square: averaged
// down the mips, the square's excess over the mean slope's is the slope variance a pixel's
// footprint filters out, which the shading turns into roughness.
@compute @workgroup_size(8, 8)
fn surface(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= vec2u(N))) { return; }
    let c = vec2i(id.xy);
    let x = (8.0 * (rise(c + vec2i(1, 0)) - rise(c - vec2i(1, 0)))
        - (rise(c + vec2i(2, 0)) - rise(c - vec2i(2, 0)))) / (12.0 * p.cell);
    let z = (8.0 * (rise(c + vec2i(0, 1)) - rise(c - vec2i(0, 1)))
        - (rise(c + vec2i(0, 2)) - rise(c - vec2i(0, 2)))) / (12.0 * p.cell);
    textureStore(surface_out, c, vec4f(rise(c), x, z, x * x + z * z));
}
@compute @workgroup_size(8, 8)
fn mip(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= textureDimensions(mip_out))) { return; }
    let a = vec2i(id.xy) * 2;
    let v = textureLoad(mip_in, a, 0) + textureLoad(mip_in, a + vec2i(1, 0), 0)
        + textureLoad(mip_in, a + vec2i(0, 1), 0) + textureLoad(mip_in, a + vec2i(1, 1), 0);
    textureStore(mip_out, vec2i(id.xy), v * 0.25);
}
