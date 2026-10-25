// Dispersive surface waves over the shallow-water bulk flow, after S. Jeschke and C. Wojtan,
// "Generalizing Shallow Water Simulations with Dispersive Surface Waves", ACM TOG 42(4), 2023,
// doi:10.1145/3592098. Each step splits the free surface and face flux into a bulk part, which
// the ordinary shallow-water substeps advance, and Airy surface waves (their sections 3.5, 4.1),
// which advance with an exponential integrator per Fourier mode (their section 4.3, after
// J. Tessendorf's eWave, 2014). The surface waves are then carried by the bulk flow with the
// bulk's convergence (their section 4.4), and heights are rebuilt from fluxes, so volume is
// conserved (their section 4.5). Their filter's cutoff, the wavelength 2 pi h at which shallow-
// water and deep-water wave speeds agree, and its gradient penalty are kept.
//
// This version differs in four ways. The split is a stationary Gaussian pyramid read at each
// column's cutoff (each level's blur twice the last one's width, so any depth costs the same),
// instead of an explicit diffusion whose iteration count limits the cutoff in deep water, and the
// cutoff is set one depth level deeper than the column, leaving more to the exact Airy step and
// only long or steep waves to the bulk flow. The transforms
// mirror the region about its walls (even for the surface, odd for normal flux), so every mode is
// a discrete cosine/sine mode and walls reflect exactly. Each mode rotates exactly between
// surface and longitudinal flux with the staggered-grid derivative symbols, so the discrete wave
// runs at the exact Airy speed without a separate correction factor. The height update uses the
// mode's flux averaged over the step, which reproduces the exact rotation while keeping the
// update a conservative divergence.
struct Waves {
    size: vec2u,        // columns along x and z (powers of two)
    ext: vec2u,         // mirrored transform grid: 2 x size
    cell: f32,
    dt: f32,            // the whole step
    limit: f32,         // the face velocity limit of the bulk substeps, m/s
    breaking: f32,      // 1: the region breaks its waves (water_maccormack.wgsl's eddy viscosity)
    open: vec4u,        // -x, +x, -z, +z: 1 where the side is open (the split fades out)
    gamma: f32,         // damping of convergent amplification (Jeschke and Wojtan: 1/4)
    steep: f32,         // surface slope above which waves stay in the bulk flow
    unused1: f32,
    unused2: f32,
}
struct Level { depth: f32, index: u32, first: u32, unused: u32 }
@group(0) @binding(0) var<uniform> p: Waves;
@group(0) @binding(1) var<storage, read_write> state: array<vec4f>;
@group(0) @binding(2) var<storage, read_write> spec_q: array<vec2f>;   // qx + i qz
@group(0) @binding(3) var<storage, read_write> spec_s: array<vec2f>;   // surface wave height
@group(0) @binding(6) var<storage, read_write> surf: array<vec4f>;    // h~, qx~, qz~, gentle
@group(0) @binding(7) var<storage, read_write> bulk0: array<vec4f>;   // h-, u-, w-, 0 at start
@group(0) @binding(10) var<storage, read_write> out_avg: array<vec2f>;
@group(0) @binding(11) var<storage, read_write> out_end: array<vec2f>;
@group(0) @binding(12) var<storage, read_write> acc: array<vec4f>;    // avg x, avg z, end x, end z
@group(0) @binding(13) var<storage, read_write> flux: array<vec2f>;
@group(0) @binding(14) var<storage, read_write> donor: array<f32>;
@group(0) @binding(15) var<storage, read_write> heights: array<vec2f>; // new depth, bulk depth
@group(0) @binding(16) var<uniform> level: Level;
@group(0) @binding(17) var<storage, read_write> split: array<vec4f>;   // blended H-, qx-, qz-, 0
// Every pyramid level, finest first: H, qx and qz of wet columns and the wet mask, so the split
// can average water alone (a normalized convolution).
@group(0) @binding(18) var<storage, read_write> pyramid: array<vec4f>;
struct Floor { index: u32, unused0: u32, unused1: u32, unused2: u32 }
@group(0) @binding(19) var<uniform> floor_: Floor;

const G: f32 = 9.81;
const DRY: f32 = 0.01;
// Airy depth levels: 0.4 m and every factor 2.5 above, to 6.25 m.
const LEVEL0: f32 = 0.4;
const LEVEL_RATIO: f32 = 2.5;

fn at(i: u32, k: u32) -> u32 { return i + p.size.x * k; }
fn upwind(v: f32, a: f32, b: f32) -> f32 { return select(b, a, v > 0.0); }
// The split fades out over 48 columns towards an open side, where the bulk flow's boundary
// treatment carries waves in and out. A swell entering there changes from a long wave to an Airy
// wave across the fade: over 12 columns a 10 s swell 6 m deep came in 9% higher than the sea's,
// over 48 columns (24 m at half-metre columns) 4%.
fn open_ramp(i: u32, k: u32) -> f32 {
    var d = 1.0e9;
    if (p.open.x != 0u) { d = min(d, f32(i)); }
    if (p.open.y != 0u) { d = min(d, f32(p.size.x - 1u - i)); }
    if (p.open.z != 0u) { d = min(d, f32(k)); }
    if (p.open.w != 0u) { d = min(d, f32(p.size.y - 1u - k)); }
    return clamp(d / 48.0, 0.0, 1.0);
}
// The free surface, carried level into dry columns from their wet neighbours so the filter does
// not read terrain as a wave; a column with no wet neighbour keeps its bed.
fn surface(i: u32, k: u32) -> f32 {
    let c = state[at(i, k)];
    if (c.y > DRY) { return c.x + c.y; }
    var sum = 0.0; var count = 0.0;
    if (i > 0u) { let o = state[at(i - 1u, k)]; if (o.y > DRY) { sum += o.x + o.y; count += 1.0; } }
    if (i + 1u < p.size.x) { let o = state[at(i + 1u, k)]; if (o.y > DRY) { sum += o.x + o.y; count += 1.0; } }
    if (k > 0u) { let o = state[at(i, k - 1u)]; if (o.y > DRY) { sum += o.x + o.y; count += 1.0; } }
    if (k + 1u < p.size.y) { let o = state[at(i, k + 1u)]; if (o.y > DRY) { sum += o.x + o.y; count += 1.0; } }
    return select(c.x, sum / count, count > 0.0);
}
fn face_flux(i: u32, k: u32, axis: u32) -> f32 {
    let n = at(i, k); let c = state[n];
    if (axis == 0u) {
        if (i == 0u) { return 0.0; }
        return c.z * upwind(c.z, state[n - 1u].y, c.y);
    }
    if (k == 0u) { return 0.0; }
    return c.w * upwind(c.w, state[n - p.size.x].y, c.y);
}
fn mirror(j: u32, n: u32) -> u32 { return select(2u * n - 1u - j, j, j < n); }

// ---- the split: a stationary Gaussian pyramid ---------------------------------------------

// Every level keeps every column (an undecimated, "a trous" pyramid), so the split is the same
// wherever a wave stands: sampling a decimated level between its columns let a wave near the
// cutoff, a few coarse columns long, swell and shrink with its position. Levels are stored one
// after another, finest first.
fn level_base(l: u32) -> u32 { return l * p.size.x * p.size.y; }
fn level_at(l: u32, c: vec2i) -> vec4f {
    let q = clamp(c, vec2i(0), vec2i(p.size) - 1);
    return pyramid[level_base(l) + u32(q.x) + p.size.x * u32(q.y)];
}
// The low pass averages water alone: each level blurs the wet columns' surface and fluxes and the
// wet mask, and the split divides one by the other. Blurring dry columns in, even filled one ring
// deep, averaged a cliff's rock into the water beside it: under a bay's 10 m headlands the bulk
// surface stood metres high, and the surface band held a trough as deep that the Airy step then
// sent across the bay. Jeschke and Wojtan diffuse within the wet domain for the same reason.
@compute @workgroup_size(8, 8)
fn pyramid_base(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.size)) { return; }
    let m = select(0.0, 1.0, state[at(id.x, id.y)].y > DRY);
    pyramid[at(id.x, id.y)] = vec4f(surface(id.x, id.y), face_flux(id.x, id.y, 0u), face_flux(id.x, id.y, 1u), 1.0) * m;
}
// One level up: a separable [1 4 6 4 1] / 16 binomial (variance 1) dilated to every 2^(l-1)-th
// column, which adds 4^(l-1) columns^2 of variance.
@compute @workgroup_size(8, 8)
fn pyramid_down(@builtin(global_invocation_id) id: vec3u) {
    let l = floor_.index;
    if (any(id.xy >= p.size)) { return; }
    let d = i32(1u << (l - 1u));
    let w = array<f32, 5>(0.0625, 0.25, 0.375, 0.25, 0.0625);
    var sum = vec4f(0.0);
    for (var b = 0; b < 5; b++) {
        for (var a = 0; a < 5; a++) {
            let c = vec2i(i32(id.x) + (a - 2) * d, i32(id.y) + (b - 2) * d);
            sum += w[a] * w[b] * level_at(l - 1u, c);
        }
    }
    pyramid[level_base(l) + at(id.x, id.y)] = sum;
}
// The pyramid's spread at level l, in columns^2: 1 + 4 + ... + 4^(l-1).
fn level_variance(l: u32) -> f32 {
    return f32((1u << (2u * l)) - 1u) / 3.0;
}
fn level_sample(l: u32, i: u32, k: u32) -> vec4f {
    return pyramid[level_base(l) + at(i, k)];
}
// The split's 3 dB cutoff is the wavelength 2 pi h one depth level deeper than the column
// (h' = 2.5 h): exp(-k^2 s^2 / 2) = exp(-k^2 ln2/2 h'^2), a variance of ln 2 (h' / cell)^2
// columns^2. As the depth falls to zero the blur does too and everything stays bulk.
@compute @workgroup_size(8, 8)
fn pyramid_split(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.size)) { return; }
    let n = at(id.x, id.y);
    let h = state[n].y;
    let target_ = 0.6931472 * (LEVEL_RATIO * h / p.cell) * (LEVEL_RATIO * h / p.cell);
    var l = 0u;
    loop {
        if (l + 1u >= floor_.index || level_variance(l + 1u) > target_) { break; }
        l += 1u;
    }
    let lo = level_variance(l);
    let hi = level_variance(min(l + 1u, floor_.index - 1u));
    let t = select(0.0, clamp((target_ - lo) / max(hi - lo, 1e-6), 0.0, 1.0), hi > lo);
    let blurred = mix(level_sample(l, id.x, id.y), level_sample(min(l + 1u, floor_.index - 1u), id.x, id.y), t);
    let own = vec4f(surface(id.x, id.y), face_flux(id.x, id.y, 0u), face_flux(id.x, id.y, 1u), 0.0);
    split[n] = select(own, vec4f(blurred.xyz / blurred.w, 0.0), blurred.w > 1e-3);
}

// ---- the Airy step in mirrored Fourier space -------------------------------------------------

@compute @workgroup_size(8, 8)
fn pack(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.ext)) { return; }
    let j = id.x; let l = id.y;
    let i = mirror(j, p.size.x); let r = mirror(l, p.size.y);
    let eta = surf[at(i, r)].x;
    // Normal flux is odd about each wall and zero on it.
    var qx = 0.0;
    if (j != 0u && j != p.size.x) {
        qx = select(-surf[at(p.ext.x - j, r)].y, surf[at(j, r)].y, j < p.size.x);
    }
    var qz = 0.0;
    if (l != 0u && l != p.size.y) {
        qz = select(-surf[at(i, p.ext.y - l)].z, surf[at(i, l)].z, l < p.size.y);
    }
    let e = j + p.ext.x * l;
    spec_q[e] = vec2f(qx, qz);
    spec_s[e] = vec2f(eta, 0.0);
}
fn cmul(a: vec2f, b: vec2f) -> vec2f { return vec2f(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x); }
fn cis(t: f32) -> vec2f { return vec2f(cos(t), sin(t)); }
fn times_i(a: vec2f) -> vec2f { return vec2f(-a.y, a.x); }
fn wavenumber(j: u32, n: u32) -> f32 {
    let m = select(f32(j) - f32(n), f32(j), j <= n / 2u);
    return 6.283185307179586 * m / (f32(n) * p.cell);
}
@compute @workgroup_size(8, 8)
fn rotate(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.ext)) { return; }
    let j = id.x; let l = id.y; let e = j + p.ext.x * l;
    let kx = wavenumber(j, p.ext.x); let kz = wavenumber(l, p.ext.y);
    let mj = (p.ext.x - j) % p.ext.x; let ml = (p.ext.y - l) % p.ext.y;
    let z = spec_q[e]; let zc = spec_q[mj + p.ext.x * ml] * vec2f(1.0, -1.0);
    // Two real fields packed as qx + i qz.
    let qx_fft = 0.5 * (z + zc);
    let qz_fft = times_i(0.5 * (zc - z));
    let half = 0.5 * p.cell;
    // Position-based coefficients: centres sit half a column in, faces on the column edge.
    let eta = cmul(spec_s[e], cis(-(kx + kz) * half));
    let qx = cmul(qx_fft, cis(-kz * half));
    let qz = cmul(qz_fft, cis(-kx * half));
    // The staggered derivative symbols: div q = i (Kx qx + Kz qz), grad eta = i K eta.
    let gx = sin(kx * half) / half; let gz = sin(kz * half) / half;
    let big = sqrt(gx * gx + gz * gz);
    var ax = qx; var az = qz; var ex = qx; var ez = qz;
    if (big > 1e-6) {
        let k = sqrt(kx * kx + kz * kz);
        // Some backends evaluate tanh through exp, which overflows to inf/inf for deep modes.
        let kh = k * level.depth;
        let omega = sqrt(G * k * select(tanh(kh), 1.0, kh > 9.0));
        let theta = omega * p.dt;
        let along = (gx * qx + gz * qz) / big;
        let tx = qx - along * (gx / big); let tz = qz - along * (gz / big);
        let swing = times_i(eta) * (omega / big);
        let end = along * cos(theta) - swing * sin(theta);
        var avg = along - swing * (0.5 * theta);
        if (theta > 1e-4) { avg = (along * sin(theta) - swing * (1.0 - cos(theta))) / theta; }
        ex = tx + end * (gx / big); ez = tz + end * (gz / big);
        ax = tx + avg * (gx / big); az = tz + avg * (gz / big);
    }
    out_avg[e] = cmul(ax, cis(kz * half)) + times_i(cmul(az, cis(kx * half)));
    out_end[e] = cmul(ex, cis(kz * half)) + times_i(cmul(ez, cis(kx * half)));
}
// The two levels around a face's bulk depth, linear in its logarithm.
fn level_weight(depth: f32, index: u32) -> f32 {
    let x = clamp(log(max(depth, 1e-4) / LEVEL0) / log(LEVEL_RATIO), 0.0, 3.0);
    return max(1.0 - abs(x - f32(index)), 0.0);
}
@compute @workgroup_size(8, 8)
fn accumulate(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.size)) { return; }
    let i = id.x; let k = id.y; let n = at(i, k);
    let h = bulk0[n].x;
    let wx = level_weight(0.5 * (h + bulk0[select(n, n - 1u, i > 0u)].x), level.index);
    let wz = level_weight(0.5 * (h + bulk0[select(n, n - p.size.x, k > 0u)].x), level.index);
    let e = i + p.ext.x * k;
    let a = out_avg[e]; let b = out_end[e];
    let value = vec4f(wx * a.x, wz * a.y, wx * b.x, wz * b.y);
    if (level.first != 0u) { acc[n] = value; } else { acc[n] = acc[n] + value; }
}

// ---- the split itself ------------------------------------------------------------------------

// Bulk = filtered; surface = the rest. Steep or open-side water stays bulk entirely, and so does
// the surface flux there. Each invocation writes only its own column.
fn gentle(n: u32, i: u32, k: u32) -> f32 {
    let c = state[n];
    var slope = 0.0;
    if (i > 0u && state[n - 1u].y > DRY) { slope = max(slope, abs(c.x + c.y - state[n - 1u].x - state[n - 1u].y)); }
    if (i + 1u < p.size.x && state[n + 1u].y > DRY) { slope = max(slope, abs(c.x + c.y - state[n + 1u].x - state[n + 1u].y)); }
    if (k > 0u && state[n - p.size.x].y > DRY) { slope = max(slope, abs(c.x + c.y - state[n - p.size.x].x - state[n - p.size.x].y)); }
    if (k + 1u < p.size.y && state[n + p.size.x].y > DRY) { slope = max(slope, abs(c.x + c.y - state[n + p.size.x].x - state[n + p.size.x].y)); }
    slope /= p.cell;
    // The split stays out of water under a metre deep and next to it, where the pyramid's blurred
    // bulk depth would exceed what a film on a beach or a rock holds. Waves there travel with the
    // bulk flow at the long-wave speed: right for swell and surf, a little fast for ripples
    // shorter than a few depths.
    var shallowest = c.y;
    if (i > 0u) { shallowest = min(shallowest, state[n - 1u].y); }
    if (i + 1u < p.size.x) { shallowest = min(shallowest, state[n + 1u].y); }
    if (k > 0u) { shallowest = min(shallowest, state[n - p.size.x].y); }
    if (k + 1u < p.size.y) { shallowest = min(shallowest, state[n + p.size.x].y); }
    return exp(-(slope / p.steep) * (slope / p.steep)) * open_ramp(i, k) * smoothstep(0.5, 1.0, shallowest)
        * (1.0 - breaking_share(i, k, c.y));
}
// Breaking water is a bore, which the bulk flow carries and the eddy viscosity dissipates: the
// split stays out of it, by the breaking model's own onset (water_maccormack.wgsl
// `mac_breaking`: the surface rising faster than half the long-wave speed, fully at twice that).
// Left in the surface band, a breaking crest's front became free Airy waves that the viscosity
// never reached.
fn breaking_share(i: u32, k: u32, h: f32) -> f32 {
    if (p.breaking <= 0.0 || h <= DRY) { return 0.0; }
    var net = face_flux(i, k, 0u) + face_flux(i, k, 1u);
    if (i + 1u < p.size.x) { net -= face_flux(i + 1u, k, 0u); }
    if (k + 1u < p.size.y) { net -= face_flux(i, k + 1u, 1u); }
    let rise = net / p.cell;
    return clamp(rise / (0.5 * sqrt(G * h)) - 1.0, 0.0, 1.0);
}
fn bulk_depth(n: u32) -> f32 {
    let c = state[n];
    if (c.y <= DRY) { return c.y; }
    let g = gentle(n, n % p.size.x, n / p.size.x);
    return mix(c.y, clamp(split[n].x - c.x, 0.0, 4.0 * c.y + 1.0), g);
}
@compute @workgroup_size(8, 8)
fn split_finish(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.size)) { return; }
    let i = id.x; let k = id.y; let n = at(i, k);
    let c = state[n];
    let g = select(0.0, gentle(n, i, k), c.y > DRY);
    let bulk = bulk_depth(n);
    var u = c.z; var w = c.w; var sx = 0.0; var sz = 0.0;
    if (i > 0u) {
        let q = face_flux(i, k, 0u);
        let gx = min(g, select(0.0, gentle(n - 1u, i - 1u, k), state[n - 1u].y > DRY));
        let qb = mix(q, split[n].y, gx);
        let up = upwind(qb, bulk_depth(n - 1u), bulk);
        if (up > 1e-3) { u = qb / up; sx = q - qb; } else { u = 0.0; sx = q; }
    }
    if (k > 0u) {
        let q = face_flux(i, k, 1u);
        let gz = min(g, select(0.0, gentle(n - p.size.x, i, k - 1u), state[n - p.size.x].y > DRY));
        let qb = mix(q, split[n].z, gz);
        let up = upwind(qb, bulk_depth(n - p.size.x), bulk);
        if (up > 1e-3) { w = qb / up; sz = q - qb; } else { w = 0.0; sz = q; }
    }
    u = clamp(u, -p.limit, p.limit); w = clamp(w, -p.limit, p.limit);
    surf[n] = vec4f(c.y - bulk, sx, sz, g);
    bulk0[n] = vec4f(bulk, u, w, 0.0);
}
// The bulk state replaces the field for the substeps; a separate pass, so the split above reads
// every neighbour's original state.
@compute @workgroup_size(8, 8)
fn split_store(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.size)) { return; }
    let n = at(id.x, id.y);
    let b = bulk0[n];
    state[n] = vec4f(state[n].x, b.x, select(state[n].z, b.y, id.x > 0u), select(state[n].w, b.z, id.y > 0u));
}

// ---- transport by the bulk flow and recombination -----------------------------------------

fn wet_face(a: u32, b: u32) -> bool { return bulk0[a].x > DRY && bulk0[b].x > DRY; }
// The share of a face's surface-wave flux it carries: none as its water thins below 0.6 m.
// The Airy step propagates every mode across the whole region at its column's blended depths, so
// a wave from deep water would otherwise pour its deep-water flux into the thin water on a shore,
// many times what a film there can carry. The next split hands what such a wave brings to the
// bulk flow.
fn wave_face(a: u32, b: u32) -> f32 { return smoothstep(0.05, 0.6, min(bulk0[a].x, bulk0[b].x)); }
@compute @workgroup_size(8, 8)
fn surface_flux(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.size)) { return; }
    let i = id.x; let k = id.y; let n = at(i, k);
    var f = vec2f(0.0);
    if (i > 0u && wet_face(n - 1u, n)) {
        let u = bulk0[n].y;
        f.x = acc[n].x * wave_face(n - 1u, n) + upwind(u, surf[n - 1u].x, surf[n].x) * u;
    }
    if (k > 0u && wet_face(n - p.size.x, n)) {
        let w = bulk0[n].z;
        f.y = acc[n].y * wave_face(n - p.size.x, n) + upwind(w, surf[n - p.size.x].x, surf[n].x) * w;
    }
    flux[n] = f;
}
fn out_x(i: u32, k: u32) -> f32 { if (i >= p.size.x) { return 0.0; } return flux[at(i, k)].x; }
fn out_z(i: u32, k: u32) -> f32 { if (k >= p.size.y) { return 0.0; } return flux[at(i, k)].y; }
// Surface fluxes may take no more than the column holds after its bulk step.
@compute @workgroup_size(8, 8)
fn donors(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.size)) { return; }
    let i = id.x; let k = id.y; let n = at(i, k);
    let have = state[n].y + surf[n].x;
    let leave = (max(-out_x(i, k), 0.0) + max(out_x(i + 1u, k), 0.0)
        + max(-out_z(i, k), 0.0) + max(out_z(i, k + 1u), 0.0)) * p.dt / p.cell;
    var s = 1.0;
    if (leave > max(have, 0.0)) { s = max(have, 0.0) / leave; }
    donor[n] = s;
}
fn limited(f: f32, a: u32, b: u32) -> f32 { return f * select(donor[b], donor[a], f > 0.0); }
@compute @workgroup_size(8, 8)
fn rebuild(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.size)) { return; }
    let i = id.x; let k = id.y; let n = at(i, k);
    var net = 0.0;
    if (i > 0u) { net += limited(flux[n].x, n - 1u, n); }
    if (i + 1u < p.size.x) { net -= limited(flux[n + 1u].x, n, n + 1u); }
    if (k > 0u) { net += limited(flux[n].y, n - p.size.x, n); }
    if (k + 1u < p.size.y) { net -= limited(flux[n + p.size.x].y, n, n + p.size.x); }
    let bulk = state[n].y;
    heights[n] = vec2f(max(bulk + surf[n].x + net * p.dt / p.cell, 0.0), bulk);
}
// The end-of-step surface flux, carried along the bulk flow (semi-Lagrangian, bilinear and
// clamped to its four donors) and amplified where the bulk flow converges.
fn end_flux(i: i32, k: i32, axis: u32) -> f32 {
    let c = clamp(vec2i(i, k), vec2i(0), vec2i(p.size) - 1);
    let v = acc[at(u32(c.x), u32(c.y))];
    return select(v.w, v.z, axis == 0u);
}
fn divergence(i: u32, k: u32) -> f32 {
    let n = at(i, k);
    let u1 = select(0.0, bulk0[min(n + 1u, p.size.x * p.size.y - 1u)].y, i + 1u < p.size.x);
    let w1 = select(0.0, bulk0[min(n + p.size.x, p.size.x * p.size.y - 1u)].z, k + 1u < p.size.y);
    return (u1 - bulk0[n].y + w1 - bulk0[n].z) / p.cell;
}
fn carried(i: u32, k: u32, axis: u32, velocity: vec2f) -> f32 {
    let back = vec2f(f32(i), f32(k)) - velocity * (p.dt / p.cell);
    let lo = vec2i(floor(back)); let t = back - vec2f(lo);
    let a = end_flux(lo.x, lo.y, axis); let b = end_flux(lo.x + 1, lo.y, axis);
    let c = end_flux(lo.x, lo.y + 1, axis); let d = end_flux(lo.x + 1, lo.y + 1, axis);
    let value = mix(mix(a, b, t.x), mix(c, d, t.x), t.y);
    return clamp(value, min(min(a, b), min(c, d)), max(max(a, b), max(c, d)));
}
fn amplified(value: f32, div: f32) -> f32 {
    let growth = min(-div, -p.gamma * div);
    return value * exp(clamp(growth * p.dt, -4.0, 1.0));
}
// A face between two columns that hold water after the step.
fn awake_face(a: u32, b: u32) -> bool { return heights[a].x > 0.0 || heights[b].x > 0.0; }
@compute @workgroup_size(8, 8)
fn velocities(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.size)) { return; }
    let i = id.x; let k = id.y; let n = at(i, k);
    let c = state[n];
    let h = heights[n];
    var u = c.z; var w = c.w;
    if (i > 0u && awake_face(n - 1u, n)) {
        let cross = 0.25 * (bulk0[n].z + bulk0[n - 1u].z
            + select(0.0, bulk0[n + p.size.x].z, k + 1u < p.size.y)
            + select(0.0, bulk0[n - 1u + p.size.x].z, k + 1u < p.size.y));
        var s = 0.0;
        if (wet_face(n - 1u, n)) {
            s = amplified(carried(i, k, 0u, vec2f(bulk0[n].y, cross)),
                0.5 * (divergence(i - 1u, k) + divergence(i, k))) * wave_face(n - 1u, n);
        }
        let q = c.z * upwind(c.z, heights[n - 1u].y, h.y) + s;
        let up = upwind(q, heights[n - 1u].x, h.x);
        u = select(0.0, clamp(q / up, -p.limit, p.limit), up > 1e-4);
    }
    if (k > 0u && awake_face(n - p.size.x, n)) {
        let cross = 0.25 * (bulk0[n].y + bulk0[n - p.size.x].y
            + select(0.0, bulk0[n + 1u].y, i + 1u < p.size.x)
            + select(0.0, bulk0[n + 1u - p.size.x].y, i + 1u < p.size.x));
        var s = 0.0;
        if (wet_face(n - p.size.x, n)) {
            s = amplified(carried(i, k, 1u, vec2f(cross, bulk0[n].z)),
                0.5 * (divergence(i, k - 1u) + divergence(i, k))) * wave_face(n - p.size.x, n);
        }
        let q = c.w * upwind(c.w, heights[n - p.size.x].y, h.y) + s;
        let up = upwind(q, heights[n - p.size.x].x, h.x);
        w = select(0.0, clamp(q / up, -p.limit, p.limit), up > 1e-4);
    }
    state[n] = vec4f(c.x, h.x, u, w);
}
