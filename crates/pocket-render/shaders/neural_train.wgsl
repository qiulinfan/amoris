// Trains a neural texture (docs/spec/neural-textures.md 4): latent grids and the MLP, by Adam on an
// L2 loss over random samples of every mip, with hand-written backpropagation. Composed after the
// profile constants (`NT_*`, neural.rs). The input assembly is the decoder's (neural_texture.wgsl,
// `pocket_assets::neural::Decoder`) on continuous latents: uniform noise of one quantization step
// while the steps are below `steps.z`, rounding with a straight-through gradient after it, and from
// `steps.w` on the latents are frozen and only the network trains.
//
// One step is five dispatches: `forward_backward` (a sample each), `mlp_partials` (a parameter of
// a chunk of samples each), `mlp_adam`, `latent_adam`, `finish_step`. No float atomics: a latent's
// gradient is summed in fixed point (stochastically rounded integers, whose sum does not depend on
// the order), the network's in a fixed order, so a seed gives the same result every run.

struct Train {
    // batch, chunks (the network's gradient is summed per chunk of batch / chunks samples), mips,
    // latent levels
    shape: vec4u,
    // seed, steps, first rounded step, first step with frozen latents
    steps: vec4u,
    // network learning rate, latent learning rate, share of samples at texel centres, the final
    // learning rate as a fraction of the first
    rates: vec4f,
    // latent values, unused
    sizes: vec4u,
    channel_weights: array<vec4f, 4>,
};

@group(0) @binding(0) var<uniform> train: Train;
// Per level l: 2l the fine grid (width, height, first value), 2l + 1 the coarse grid; then per mip
// m at 2 * levels + m: (width, height, first reference value, the sampling CDF's bits).
@group(0) @binding(1) var<storage, read> tables: array<vec4u>;
// Every mip's texels, NT_OUT channels each.
@group(0) @binding(2) var<storage, read> reference: array<f32>;
// Latent values in [0, 1], texel-major (a texel's features together).
@group(0) @binding(3) var<storage, read_write> latents: array<f32>;
@group(0) @binding(4) var<storage, read_write> latent_grads: array<atomic<i32>>;
@group(0) @binding(5) var<storage, read_write> latent_moments: array<vec2f>;
// The network in the file's order (pocket_assets::neural::NeuralLayout::offsets).
@group(0) @binding(6) var<storage, read_write> weights: array<f32>;
@group(0) @binding(7) var<storage, read_write> partials: array<f32>;
@group(0) @binding(8) var<storage, read_write> mlp_moments: array<vec2f>;
// Per sample: input, both hidden layers' activations, then every layer's pre-activation gradient.
@group(0) @binding(9) var<storage, read_write> records: array<f32>;
@group(0) @binding(10) var<storage, read_write> losses: array<f32>;
// [0]: the step.
@group(0) @binding(11) var<storage, read_write> state: array<u32>;
// The mean loss of each step.
@group(0) @binding(12) var<storage, read_write> history: array<f32>;

const NT_TW1: u32 = 0u;
const NT_TB1: u32 = NT_TW1 + NT_H1 * NT_INP;
const NT_TW2: u32 = NT_TB1 + NT_H1;
const NT_TB2: u32 = NT_TW2 + NT_H2 * NT_H1;
const NT_TW3: u32 = NT_TB2 + NT_H2;
const NT_TB3: u32 = NT_TW3 + NT_OUTP * NT_H2;
const NT_PARAMS: u32 = NT_TB3 + NT_OUTP;

const NT_RX: u32 = 0u;
const NT_RH1: u32 = NT_INP;
const NT_RH2: u32 = NT_RH1 + NT_H1;
const NT_RD1: u32 = NT_RH2 + NT_H2;
const NT_RD2: u32 = NT_RD1 + NT_H1;
const NT_RD3: u32 = NT_RD2 + NT_H2;
const NT_RECORD: u32 = NT_RD3 + NT_OUTP;

// Latent gradients are summed as integers of 1/FIXED, each sample's share clamped to +-32.
const FIXED: f32 = 16384.0;
const FIXED_LIMIT: f32 = 524288.0;
const NONE: u32 = 0xffffffffu;
const PI: f32 = 3.14159265358979;

fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

var<private> rng: u32;

fn rand() -> f32 {
    rng = pcg(rng);
    return f32(rng >> 8u) / 16777216.0;
}

fn tri(t: f32) -> f32 {
    return abs(2.0 * fract(t) - 1.0);
}

struct Taps {
    // Each tap's first value (a texel's features follow it).
    first: array<u32, 4>,
    weight: array<f32, 4>,
    p: vec2f,
};

// The decoder's four wrapping taps (neural_texture.wgsl `nt_taps`) of a grid whose texels hold
// `width` values each.
fn taps(grid: vec4u, uv: vec2f, width: u32) -> Taps {
    let size = grid.xy;
    let p = uv * vec2f(size) - 0.5;
    let fl = floor(p);
    let f = p - fl;
    let i = vec2i(fl);
    let x0 = select(u32(i.x), size.x - 1u, i.x < 0);
    let y0 = select(u32(i.y), size.y - 1u, i.y < 0);
    let x1 = select(x0 + 1u, 0u, x0 + 1u >= size.x);
    let y1 = select(y0 + 1u, 0u, y0 + 1u >= size.y);
    var t: Taps;
    t.first = array<u32, 4>(
        grid.z + (y0 * size.x + x0) * width, grid.z + (y0 * size.x + x1) * width,
        grid.z + (y1 * size.x + x0) * width, grid.z + (y1 * size.x + x1) * width);
    t.weight = array<f32, 4>((1.0 - f.x) * (1.0 - f.y), f.x * (1.0 - f.y), (1.0 - f.x) * f.y, f.x * f.y);
    t.p = p;
    return t;
}

fn phase_of(step: u32) -> u32 {
    return select(select(0u, 1u, step >= train.steps.z), 2u, step >= train.steps.w);
}

// A latent value as the forward pass sees it in `phase`.
fn latent(i: u32, bits: u32, phase: u32) -> f32 {
    let z = latents[i];
    let levels = f32((1u << bits) - 1u);
    if phase == 0u {
        return z + (rand() - 0.5) / levels;
    }
    return round(clamp(z, 0.0, 1.0) * levels) / levels;
}

fn add_latent_grad(i: u32, g: f32) {
    let v = clamp(g * FIXED, -FIXED_LIMIT, FIXED_LIMIT);
    atomicAdd(&latent_grads[i], i32(floor(v + rand())));
}

fn channel_weight(c: u32) -> f32 {
    return train.channel_weights[c / 4u][c % 4u];
}

fn schedule(step: u32) -> f32 {
    let t = f32(step) / f32(max(train.steps.y, 1u));
    return train.rates.w + (1.0 - train.rates.w) * 0.5 * (1.0 + cos(PI * t));
}

@compute @workgroup_size(64)
fn forward_backward(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let s = gid.x + gid.y * nwg.x * 64u;
    if s >= train.shape.x {
        return;
    }
    let step = state[0];
    let phase = phase_of(step);
    rng = pcg(pcg(pcg(s) ^ step) + train.steps.x);
    let levels = train.shape.w;
    let r = rand();
    var mip = 0u;
    for (var m = 0u; m + 1u < train.shape.z; m++) {
        if r > bitcast<f32>(tables[2u * levels + m].w) {
            mip = m + 1u;
        }
    }
    let md = tables[2u * levels + mip];
    let size = vec2f(md.xy);
    var uv: vec2f;
    if rand() < train.rates.z {
        uv = (floor(vec2f(rand(), rand()) * size) + 0.5) / size;
    } else {
        uv = vec2f(rand(), rand());
    }
    // The goal: the mip's reference, filtered bilinearly with wrapping.
    let rt = taps(vec4u(md.x, md.y, md.z, 0u), uv, NT_OUT);
    var goal: array<f32, NT_OUT>;
    for (var c = 0u; c < NT_OUT; c++) {
        goal[c] = rt.weight[0] * reference[rt.first[0] + c] + rt.weight[1] * reference[rt.first[1] + c]
            + rt.weight[2] * reference[rt.first[2] + c] + rt.weight[3] * reference[rt.first[3] + c];
    }

    // The input.
    let level = mip / 2u;
    let ft = taps(tables[2u * level], uv, NT_FINE_F);
    let ct = taps(tables[2u * level + 1u], uv, NT_COARSE_F);
    var x: array<f32, NT_INP>;
    if NT_SAMPLING == 0u {
        for (var f = 0u; f < NT_FINE_F; f++) {
            var v = 0.0;
            for (var k = 0u; k < 4u; k++) {
                v += ft.weight[k] * latent(ft.first[k] + f, NT_FINE_BITS, phase);
            }
            x[f] = v;
        }
    } else {
        for (var k = 0u; k < 4u; k++) {
            for (var f = 0u; f < NT_FINE_F; f++) {
                x[k * NT_FINE_F + f] = latent(ft.first[k] + f, NT_FINE_BITS, phase);
            }
        }
    }
    for (var f = 0u; f < NT_COARSE_F; f++) {
        var v = 0.0;
        for (var k = 0u; k < 4u; k++) {
            v += ct.weight[k] * latent(ct.first[k] + f, NT_COARSE_BITS, phase);
        }
        x[NT_FINE_IN + f] = v;
    }
    for (var o = 0u; o < NT_PE; o++) {
        let sc = f32(1u << o);
        let at = NT_FINE_IN + NT_COARSE_F + 4u * o;
        x[at] = tri(ft.p.x * sc);
        x[at + 1u] = tri(ft.p.x * sc + 0.25);
        x[at + 2u] = tri(ft.p.y * sc);
        x[at + 3u] = tri(ft.p.y * sc + 0.25);
    }
    x[NT_IN - 2u] = f32(mip & 1u);
    x[NT_IN - 1u] = f32(level) / 8.0;

    // Forward.
    var h1: array<f32, NT_H1>;
    for (var j = 0u; j < NT_H1; j++) {
        var v = weights[NT_TB1 + j];
        for (var i = 0u; i < NT_IN; i++) {
            v += weights[NT_TW1 + j * NT_INP + i] * x[i];
        }
        h1[j] = max(v, 0.0);
    }
    var h2: array<f32, NT_H2>;
    for (var j = 0u; j < NT_H2; j++) {
        var v = weights[NT_TB2 + j];
        for (var i = 0u; i < NT_H1; i++) {
            v += weights[NT_TW2 + j * NT_H1 + i] * h1[i];
        }
        h2[j] = max(v, 0.0);
    }
    var d3: array<f32, NT_OUT>;
    var loss = 0.0;
    for (var c = 0u; c < NT_OUT; c++) {
        var v = weights[NT_TB3 + c];
        for (var i = 0u; i < NT_H2; i++) {
            v += weights[NT_TW3 + c * NT_H2 + i] * h2[i];
        }
        let e = v - goal[c];
        let w = channel_weight(c);
        loss += w * e * e;
        d3[c] = 2.0 * w * e / f32(NT_OUT);
    }
    loss /= f32(NT_OUT);

    // Backward.
    var d2: array<f32, NT_H2>;
    for (var j = 0u; j < NT_H2; j++) {
        var v = 0.0;
        if h2[j] > 0.0 {
            for (var c = 0u; c < NT_OUT; c++) {
                v += weights[NT_TW3 + c * NT_H2 + j] * d3[c];
            }
        }
        d2[j] = v;
    }
    var d1: array<f32, NT_H1>;
    for (var j = 0u; j < NT_H1; j++) {
        var v = 0.0;
        if h1[j] > 0.0 {
            for (var k = 0u; k < NT_H2; k++) {
                v += weights[NT_TW2 + k * NT_H1 + j] * d2[k];
            }
        }
        d1[j] = v;
    }

    let r0 = s * NT_RECORD;
    for (var i = 0u; i < NT_INP; i++) {
        records[r0 + NT_RX + i] = x[i];
    }
    for (var j = 0u; j < NT_H1; j++) {
        records[r0 + NT_RH1 + j] = h1[j];
        records[r0 + NT_RD1 + j] = d1[j];
    }
    for (var j = 0u; j < NT_H2; j++) {
        records[r0 + NT_RH2 + j] = h2[j];
        records[r0 + NT_RD2 + j] = d2[j];
    }
    for (var c = 0u; c < NT_OUTP; c++) {
        records[r0 + NT_RD3 + c] = select(0.0, d3[min(c, NT_OUT - 1u)], c < NT_OUT);
    }
    losses[s] = loss;

    if phase >= 2u {
        return;
    }
    // The latents' gradients through the first layer (straight through the quantization).
    var dx: array<f32, NT_FINE_IN>;
    for (var i = 0u; i < NT_FINE_IN; i++) {
        var v = 0.0;
        for (var j = 0u; j < NT_H1; j++) {
            v += weights[NT_TW1 + j * NT_INP + i] * d1[j];
        }
        dx[i] = v;
    }
    if NT_SAMPLING == 0u {
        for (var f = 0u; f < NT_FINE_F; f++) {
            for (var k = 0u; k < 4u; k++) {
                add_latent_grad(ft.first[k] + f, ft.weight[k] * dx[f]);
            }
        }
    } else {
        for (var k = 0u; k < 4u; k++) {
            for (var f = 0u; f < NT_FINE_F; f++) {
                add_latent_grad(ft.first[k] + f, dx[k * NT_FINE_F + f]);
            }
        }
    }
    for (var f = 0u; f < NT_COARSE_F; f++) {
        var g = 0.0;
        for (var j = 0u; j < NT_H1; j++) {
            g += weights[NT_TW1 + j * NT_INP + NT_FINE_IN + f] * d1[j];
        }
        for (var k = 0u; k < 4u; k++) {
            add_latent_grad(ct.first[k] + f, ct.weight[k] * g);
        }
    }
}

// The network's gradient over one chunk of samples: parameter `gid.x`, chunk `workgroup_id.y`.
@compute @workgroup_size(64)
fn mlp_partials(@builtin(global_invocation_id) gid: vec3u, @builtin(workgroup_id) wid: vec3u) {
    let p = gid.x;
    if p >= NT_PARAMS {
        return;
    }
    // The record words whose product over samples is the gradient (a bias has no activation).
    var d: u32;
    var a = NONE;
    if p < NT_TB1 {
        d = NT_RD1 + p / NT_INP;
        a = NT_RX + p % NT_INP;
    } else if p < NT_TW2 {
        d = NT_RD1 + (p - NT_TB1);
    } else if p < NT_TB2 {
        d = NT_RD2 + (p - NT_TW2) / NT_H1;
        a = NT_RH1 + (p - NT_TW2) % NT_H1;
    } else if p < NT_TW3 {
        d = NT_RD2 + (p - NT_TB2);
    } else if p < NT_TB3 {
        d = NT_RD3 + (p - NT_TW3) / NT_H2;
        a = NT_RH2 + (p - NT_TW3) % NT_H2;
    } else {
        d = NT_RD3 + (p - NT_TB3);
    }
    let per = train.shape.x / train.shape.y;
    let first = wid.y * per;
    var acc = 0.0;
    if a == NONE {
        for (var k = 0u; k < per; k++) {
            acc += records[(first + k) * NT_RECORD + d];
        }
    } else {
        for (var k = 0u; k < per; k++) {
            let r = (first + k) * NT_RECORD;
            acc += records[r + d] * records[r + a];
        }
    }
    partials[wid.y * NT_PARAMS + p] = acc;
}

// Padding weights (input columns past NT_IN, output rows past NT_OUT) stay zero.
fn is_padding(p: u32) -> bool {
    if p < NT_TB1 {
        return (p - NT_TW1) % NT_INP >= NT_IN;
    }
    if p >= NT_TW3 && p < NT_TB3 {
        return (p - NT_TW3) / NT_H2 >= NT_OUT;
    }
    if p >= NT_TB3 {
        return p - NT_TB3 >= NT_OUT;
    }
    return false;
}

fn adam(moments: vec2f, g: f32, step: u32) -> vec4f {
    let m = 0.9 * moments.x + 0.1 * g;
    let v = 0.999 * moments.y + 0.001 * g * g;
    let t = f32(step + 1u);
    let update = (m / (1.0 - pow(0.9, t))) / (sqrt(v / (1.0 - pow(0.999, t))) + 1e-12);
    return vec4f(m, v, update, 0.0);
}

@compute @workgroup_size(64)
fn mlp_adam(@builtin(global_invocation_id) gid: vec3u) {
    let p = gid.x;
    if p >= NT_PARAMS || is_padding(p) {
        return;
    }
    var g = 0.0;
    for (var c = 0u; c < train.shape.y; c++) {
        g += partials[c * NT_PARAMS + p];
    }
    g /= f32(train.shape.x);
    let step = state[0];
    let a = adam(mlp_moments[p], g, step);
    mlp_moments[p] = a.xy;
    weights[p] -= train.rates.x * schedule(step) * a.z;
}

@compute @workgroup_size(256)
fn latent_adam(@builtin(global_invocation_id) gid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let i = gid.x + gid.y * nwg.x * 256u;
    if i >= train.sizes.x {
        return;
    }
    let q = atomicExchange(&latent_grads[i], 0);
    let step = state[0];
    if step >= train.steps.w {
        return;
    }
    // The sum over the batch's samples (Adam is indifferent to its scale).
    let g = f32(q) / FIXED;
    let a = adam(latent_moments[i], g, step);
    latent_moments[i] = a.xy;
    latents[i] = clamp(latents[i] - train.rates.y * schedule(step) * a.z, 0.0, 1.0);
}

var<workgroup> loss_sum: array<f32, 256>;

@compute @workgroup_size(256)
fn finish_step(@builtin(local_invocation_index) lid: u32) {
    var sum = 0.0;
    for (var i = lid; i < train.shape.x; i += 256u) {
        sum += losses[i];
    }
    loss_sum[lid] = sum;
    workgroupBarrier();
    for (var w = 128u; w > 0u; w >>= 1u) {
        if lid < w {
            loss_sum[lid] += loss_sum[lid + w];
        }
        workgroupBarrier();
    }
    if lid == 0u {
        let step = state[0];
        history[step] = loss_sum[0] / f32(train.shape.x);
        state[0] = step + 1u;
    }
}
