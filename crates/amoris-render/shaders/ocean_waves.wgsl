// The wind sea (water.md 60): the waves a wind raises over a fetch, synthesized as J. Tessendorf
// does ("Simulating ocean water", SIGGRAPH course notes, 2001) on three cascades of 256 x 256
// wave vectors, each tile holding its own band of wavelengths. A wave vector's amplitude follows
// the JONSWAP spectrum (K. Hasselmann et al., Deutsche Hydrographische Zeitschrift A8(12), 1973)
// spread about the wind as M. A. Donelan, J. Hamilton and W. H. Hui measured ("Directional
// spectra of wind-generated ocean waves", Phil. Trans. R. Soc. A 315, 1985), in the forms C. J.
// Horvath gathers for graphics ("Empirical directional wave spectra for computer graphics",
// DigiPro 2015). Each frame turns every wave by its deep-water frequency, inverse transforms the height,
// the horizontal displacement that sharpens the crests, the slopes and the displacement's
// derivatives, and writes them, with the foam the folding crests leave, into a texture array the
// sea and the regions sample (forward.wgsl, `wind_sea`): a workgroup makes a row of the spectrum
// and transforms it, a workgroup transforms a column and writes its texels, and two more make
// the mip levels.

const N: u32 = 256u;
const G: f32 = 9.81;
const TAU: f32 = 6.283185307179586;

struct Spectrum {
    // Per cascade: the tile's side (m), the band of wavenumbers it holds [low, high) (rad/m), and
    // the Jacobian under which its crests fold into whitecaps.
    cascades: array<vec4f, 3>,
    // Toward where the wind blows (x, z), its speed at 10 m (m/s), JONSWAP's peak frequency.
    wind: vec4f,
    // JONSWAP's alpha and gamma, the crests' choppiness lambda, the time (s).
    shape: vec4f,
    // The seconds since the last synthesis (0: paused, negative: rewound), the foam's decay a
    // second, unused, the seed.
    foam: vec4f,
    // Per cascade, the spread of its Jacobian about 1 (0: it makes no foam).
    spread: vec4f,
};
@group(0) @binding(0) var<uniform> s: Spectrum;
// Per cascade, four complex fields of N x N, x fastest: each packs two real fields as re + i im.
@group(0) @binding(1) var<storage, read_write> fields: array<vec2f>;
// The foam each texel holds, per cascade.
@group(0) @binding(2) var<storage, read_write> foam: array<f32>;
// Layers 0 to 2: each cascade's displacement (x, y, z) and foam; 3 to 5: its slopes (x, z) and
// the derivatives of its displacement along x and z.
@group(0) @binding(3) var waves: texture_storage_2d_array<rgba16float, write>;

fn hash(x: u32) -> u32 {
    var h = x * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return (h >> 22u) ^ h;
}
fn unit(x: u32) -> f32 { return (f32(hash(x) >> 8u) + 0.5) / 16777216.0; }
// A complex Gaussian of unit variance for the wave vector m of cascade c (Box-Muller).
fn gauss(c: u32, m: vec2i) -> vec2f {
    let key = hash(bitcast<u32>(s.foam.w) ^ hash(c * 65537u + bitcast<u32>(m.x) * 9781u
        + (bitcast<u32>(m.y) << 16u)));
    let r = sqrt(-2.0 * log(unit(key)));
    let a = TAU * unit(key ^ 0x9e3779b9u);
    return vec2f(cos(a), sin(a)) * r * 0.7071067811865476;
}

// JONSWAP's S(omega) (m^2 s) at the wind's fetch.
fn jonswap(w: f32) -> f32 {
    let wp = s.wind.w;
    let sigma = select(0.09, 0.07, w <= wp);
    let r = exp(-(w - wp) * (w - wp) / (2.0 * sigma * sigma * wp * wp));
    return s.shape.x * G * G / pow(w, 5.0) * exp(-1.25 * pow(wp / w, 4.0)) * pow(s.shape.y, r);
}
// Donelan, Hamilton and Hui's spreading, beta/(2 tanh(beta pi)) sech^2(beta theta), which
// integrates to one over the circle; theta from the wind.
fn spread(w: f32, theta: f32) -> f32 {
    let ratio = w / s.wind.w;
    var beta: f32;
    if (ratio < 0.95) {
        beta = 2.61 * pow(max(ratio, 0.56), 1.3);
    } else if (ratio < 1.6) {
        beta = 2.28 * pow(ratio, -1.3);
    } else {
        let e = -0.4 + 0.8393 * exp(-0.567 * log(ratio * ratio));
        beta = pow(10.0, e);
    }
    let x = beta * theta;
    let sech = 2.0 / (exp(x) + exp(-x));
    return beta / (2.0 * tanh(beta * 3.14159265)) * sech * sech;
}
// The amplitude of the wave vector k (rad/m), |k| inside the band: sqrt(E(k)/2) dk, so that the
// field's variance sums the spectrum, E(k) = S(omega) D(theta) (d omega/dk) / k for deep water.
fn amplitude(k: vec2f, length_k: f32, dk: f32) -> f32 {
    let w = sqrt(G * length_k);
    let theta = atan2(k.x * s.wind.y - k.y * s.wind.x, k.x * s.wind.x + k.y * s.wind.y);
    let e = jonswap(w) * spread(w, theta) * (0.5 * G / w) / length_k;
    return sqrt(0.5 * e) * dk;
}
fn cmul(a: vec2f, b: vec2f) -> vec2f { return vec2f(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x); }
fn times_i(a: vec2f) -> vec2f { return vec2f(-a.y, a.x); }
fn at(c: u32, field: u32, ij: vec2u) -> u32 { return ((c * 4u + field) * N + ij.y) * N + ij.x; }

// A line of each of the four fields, in the transform's bit-reversed order.
var<workgroup> lines: array<array<vec2f, N>, 4>;

// The inverse transform of the four lines (unscaled: a field sums its waves), radix 2 in place,
// as water_fft.wgsl does it: each lane takes one butterfly of two of the lines a stage.
fn transform(lane: u32) {
    let b = lane % (N / 2u);
    let first = (lane / (N / 2u)) * 2u;
    for (var stage = 1u; stage <= 8u; stage++) {
        let half = 1u << (stage - 1u);
        let k = b % half;
        let i0 = (b / half) * (half << 1u) + k;
        let i1 = i0 + half;
        let angle = TAU * f32(k) / f32(half << 1u);
        let w = vec2f(cos(angle), sin(angle));
        for (var f = first; f < first + 2u; f++) {
            let a = lines[f][i0];
            let t = cmul(w, lines[f][i1]);
            lines[f][i0] = a + t;
            lines[f][i1] = a - t;
        }
        workgroupBarrier();
    }
}

fn reversed(lane: u32) -> u32 { return reverseBits(lane) >> (32u - 8u); }

// A row of the spectrum at time t, transformed along x. h(k) = h0(k) e^(i w t) + conj(h0(-k))
// e^(-i w t), so the transformed field is real. The fields it packs: Dx + i Dy, Dz + i dDy/dx,
// dDy/dz + i dDx/dx and dDz/dz + i dDx/dz, with D = -i k/|k| h horizontally (Tessendorf's choppy
// waves).
@compute @workgroup_size(256)
fn waves_rows(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lane: u32) {
    let c = group.z;
    let ij = vec2u(lane, group.x);
    // The wave vector in the transform's own order: index i stands for i, or i - N past N/2.
    let m = vec2i(ij) - select(vec2i(0), vec2i(i32(N)), ij >= vec2u(N / 2u));
    let cascade = s.cascades[c];
    let dk = TAU / cascade.x;
    let k = vec2f(m) * dk;
    let length_k = length(k);
    var h = vec2f(0.0);
    // The Nyquist wave vectors have no partner of their own: left out, the field stays real.
    let nyquist = any(m == vec2i(-i32(N / 2u)));
    // The band [low, high) in the lattice's own units, where wave vectors on a bound (|m| = 1 and
    // 16 in the first cascade, 2 in the others) lie exactly on it: |m|^2 is an integer, compared
    // with the bounds squared less 0.05, more than their f32 rounding (0.002 at the largest bound,
    // 128^2) and far below the gap of 1 between neighbouring |m|^2, so a vector on a bound belongs
    // to the cascade it begins, on every device. A float |k| against the bound went either way
    // with the vendor's rounding, so such a wave was drawn in two cascades or in none.
    let m2 = f32(m.x * m.x + m.y * m.y);
    let low = cascade.y / dk;
    let high = cascade.z / dk;
    if (m2 >= low * low - 0.05 && m2 < high * high - 0.05 && !nyquist) {
        let h0 = gauss(c, m) * amplitude(k, length_k, dk);
        let minus = gauss(c, -m) * amplitude(-k, length_k, dk);
        let phase = sqrt(G * length_k) * s.shape.w;
        let turn = vec2f(cos(phase), sin(phase));
        h = cmul(h0, turn) + cmul(vec2f(minus.x, -minus.y), vec2f(turn.x, -turn.y));
    }
    let unit_k = select(vec2f(0.0), k / length_k, length_k > 0.0);
    let dx = times_i(h) * -unit_k.x;
    let dz = times_i(h) * -unit_k.y;
    let sx = times_i(h) * k.x;
    let sz = times_i(h) * k.y;
    let jxx = h * (k.x * unit_k.x);
    let jzz = h * (k.y * unit_k.y);
    let jxz = h * (k.x * unit_k.y);
    let r = reversed(lane);
    lines[0][r] = dx + times_i(h);
    lines[1][r] = dz + times_i(sx);
    lines[2][r] = sz + times_i(jxx);
    lines[3][r] = jzz + times_i(jxz);
    workgroupBarrier();
    transform(lane);
    for (var f = 0u; f < 4u; f++) {
        fields[at(c, f, ij)] = lines[f][lane];
    }
}

// A column of the four fields transformed along z, into the texture array, and the foam: where
// the displaced surface folds (its Jacobian under the threshold) a crest breaks and leaves foam
// that fades.
@compute @workgroup_size(256)
fn waves_columns(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lane: u32) {
    let c = group.z;
    let ij = vec2u(group.x, lane);
    let r = reversed(lane);
    for (var f = 0u; f < 4u; f++) {
        lines[f][r] = fields[at(c, f, ij)];
    }
    workgroupBarrier();
    transform(lane);
    let a = lines[0][lane];
    let b = lines[1][lane];
    let d = lines[2][lane];
    let e = lines[3][lane];
    let lambda = s.shape.z;
    let jacobian = (1.0 + lambda * d.y) * (1.0 + lambda * e.x) - lambda * lambda * e.y * e.y;
    let n = (c * N + ij.x) * N + ij.y;
    let dt = s.foam.x;
    var foamy = foam[n];
    if (dt > 0.0) {
        // Full foam half a spread under the threshold.
        let spread = s.spread[c];
        let fresh = select(0.0, clamp((s.cascades[c].w - jacobian) / (0.5 * spread), 0.0, 1.0),
            spread > 0.0);
        foamy = max(foamy * exp(-s.foam.y * dt), fresh);
    } else if (dt < 0.0) {
        foamy = 0.0;
    }
    foam[n] = foamy;
    textureStore(waves, ij, c, vec4f(lambda * a.x, a.y, lambda * b.x, foamy));
    textureStore(waves, ij, c + 3u, vec4f(b.y, d.x, lambda * d.y, lambda * e.x));
}

// Four mip levels from the one above them, two by two each: a workgroup reads a block of 16 x 16
// texels and writes 8 x 8, 4 x 4, 2 x 2 and one. Levels 1 to 4 from level 0, then 5 to 8 from 4.
@group(0) @binding(0) var above: texture_2d_array<f32>;
@group(0) @binding(1) var level1: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(2) var level2: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(3) var level3: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(4) var level4: texture_storage_2d_array<rgba16float, write>;
var<workgroup> block: array<array<vec4f, 8>, 8>;
@compute @workgroup_size(8, 8)
fn waves_mips(@builtin(workgroup_id) group: vec3u, @builtin(local_invocation_id) local: vec3u) {
    let layer = group.z;
    let at = group.xy * 8u + local.xy;
    let p = vec2i(at * 2u);
    let l = i32(layer);
    var v = (textureLoad(above, p, l, 0) + textureLoad(above, p + vec2i(1, 0), l, 0)
        + textureLoad(above, p + vec2i(0, 1), l, 0) + textureLoad(above, p + vec2i(1, 1), l, 0))
        * 0.25;
    textureStore(level1, at, layer, v);
    block[local.y][local.x] = v;
    workgroupBarrier();
    for (var level = 2u; level <= 4u; level++) {
        let side = 8u >> (level - 1u);
        let inside = all(local.xy < vec2u(side));
        if (inside) {
            let q = local.xy * 2u;
            v = (block[q.y][q.x] + block[q.y][q.x + 1u] + block[q.y + 1u][q.x]
                + block[q.y + 1u][q.x + 1u]) * 0.25;
        }
        workgroupBarrier();
        if (inside) {
            block[local.y][local.x] = v;
            let texel = group.xy * side + local.xy;
            switch level {
                case 2u: { textureStore(level2, texel, layer, v); }
                case 3u: { textureStore(level3, texel, layer, v); }
                default: { textureStore(level4, texel, layer, v); }
            }
        }
        workgroupBarrier();
    }
}
