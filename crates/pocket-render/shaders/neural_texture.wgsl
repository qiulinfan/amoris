// Neural texture decoding (docs/spec/neural-textures.md 3; `pocket_assets::neural::Decoder` is the
// CPU reference). Composed after the precision (neural_f16.wgsl or neural_f32.wgsl) and the
// profile constants (`NT_*`, neural.rs), and before a shader that
// declares the two resources this file reads:
//
//   nt_latents: texture_2d<u32>, rg32uint: every latent texel (two words) of every loaded neural
//               texture, addressed linearly NT_ROW texels per row;
//   nt_data:    a uniform array<vec4u>: per texture a descriptor (NT_DESC words) then its weights.
//
// Descriptor words: 0 (width, height, mips, levels); 1 (channel indices, see neural.rs); then per
// level l, word 4 + 2l (fine grid width, height, first texel) and 5 + 2l (the coarse grid's).
// Weights: per layer, a 4x4 block of halves per (output block, input block), column-major in two
// words, then a word per output block of biases (xy), padded to an even count so blocks stay on
// even words (the f16 view reads two words at a time). A texture's words start on an even word.
// The precision file defines `nt_t`, `nt_mat` and `nt_bias`; neural_mlp.wgsl, composed after this
// file, `nt_decode`.

const NT_ROW: u32 = 4096u;
const NT_DESC: u32 = 20u;
const NT_L1W: u32 = 0u;
const NT_L1B: u32 = NT_L1W + 2u * NT_H1_4 * NT_IN4;
const NT_L2W: u32 = NT_L1B + NT_H1_4 + NT_H1_4 % 2u;
const NT_L2B: u32 = NT_L2W + 2u * NT_H2_4 * NT_H1_4;
const NT_L3W: u32 = NT_L2B + NT_H2_4 + NT_H2_4 % 2u;
const NT_L3B: u32 = NT_L3W + 2u * NT_OUT4 * NT_H2_4;

struct NtTaps {
    t00: vec2u,
    t10: vec2u,
    t01: vec2u,
    t11: vec2u,
    // Bilinear weights of the four taps.
    weight: vec4f,
    // The position in this grid's texels (texel centres at integers).
    p: vec2f,
};

fn nt_load(index: u32) -> vec2u {
    return textureLoad(nt_latents, vec2u(index % NT_ROW, index / NT_ROW), 0).xy;
}

// The four wrapping taps of `grid` (width, height, first texel) around `uv` in [0, 1).
fn nt_taps(grid: vec4u, uv: vec2f) -> NtTaps {
    let size = grid.xy;
    let p = uv * vec2f(size) - 0.5;
    let fl = floor(p);
    let f = p - fl;
    let i = vec2i(fl);
    let x0 = select(u32(i.x), size.x - 1u, i.x < 0);
    let y0 = select(u32(i.y), size.y - 1u, i.y < 0);
    let x1 = select(x0 + 1u, 0u, x0 + 1u >= size.x);
    let y1 = select(y0 + 1u, 0u, y0 + 1u >= size.y);
    var t: NtTaps;
    t.t00 = nt_load(grid.z + y0 * size.x + x0);
    t.t10 = nt_load(grid.z + y0 * size.x + x1);
    t.t01 = nt_load(grid.z + y1 * size.x + x0);
    t.t11 = nt_load(grid.z + y1 * size.x + x1);
    t.weight = vec4f((1.0 - f.x) * (1.0 - f.y), f.x * (1.0 - f.y), (1.0 - f.x) * f.y, f.x * f.y);
    t.p = p;
    return t;
}

// Features 4b..4b+3 of a latent texel, dequantized.
fn nt_block(w: vec2u, b: u32, bits: u32) -> vec4f {
    if bits == 8u {
        return unpack4x8unorm(w[b]);
    }
    let word = w[b / 2u] >> ((b % 2u) * 16u);
    return vec4f(vec4u(word, word >> 4u, word >> 8u, word >> 12u) & vec4u(15u)) / 15.0;
}

fn nt_blend(t: NtTaps, b: u32, bits: u32) -> vec4f {
    return t.weight.x * nt_block(t.t00, b, bits) + t.weight.y * nt_block(t.t10, b, bits)
        + t.weight.z * nt_block(t.t01, b, bits) + t.weight.w * nt_block(t.t11, b, bits);
}

fn nt_tri(t: vec4f) -> vec4f {
    return abs(2.0 * fract(t) - 1.0);
}

// The network's input for mip `mip` of the texture whose descriptor starts at `base`.
fn nt_inputs(base: u32, mip: u32, uv: vec2f) -> array<vec4<nt_t>, NT_IN4> {
    let level = mip / 2u;
    let u = uv - floor(uv);
    let fine = nt_taps(nt_data[base + 4u + 2u * level], u);
    let coarse = nt_taps(nt_data[base + 5u + 2u * level], u);
    var x: array<vec4<nt_t>, NT_IN4>;
    // Written without loops (naga's loop bounds keep drivers from unrolling them, see
    // tests/neural_mlp.rs): a statement per block a profile may have, behind constant conditions,
    // indices clamped to stay valid in every profile.
    let f4 = NT_FINE_F / 4u;
    if NT_SAMPLING == 0u {
        x[0] = vec4<nt_t>(nt_blend(fine, 0u, NT_FINE_BITS));
        x[min(1u, NT_IN4 - 1u)] = vec4<nt_t>(nt_blend(fine, 1u, NT_FINE_BITS));
        if f4 > 2u {
            x[min(2u, NT_IN4 - 1u)] = vec4<nt_t>(nt_blend(fine, 2u, NT_FINE_BITS));
            x[min(3u, NT_IN4 - 1u)] = vec4<nt_t>(nt_blend(fine, 3u, NT_FINE_BITS));
        }
    } else {
        nt_put_taps(&x, 0u, fine.t00);
        nt_put_taps(&x, f4, fine.t10);
        nt_put_taps(&x, 2u * f4, fine.t01);
        nt_put_taps(&x, 3u * f4, fine.t11);
    }
    x[min(NT_FINE_IN4, NT_IN4 - 1u)] = vec4<nt_t>(nt_blend(coarse, 0u, NT_COARSE_BITS));
    x[min(NT_FINE_IN4 + 1u, NT_IN4 - 1u)] = vec4<nt_t>(nt_blend(coarse, 1u, NT_COARSE_BITS));
    if NT_COARSE_F > 8u {
        x[min(NT_FINE_IN4 + 2u, NT_IN4 - 1u)] = vec4<nt_t>(nt_blend(coarse, 2u, NT_COARSE_BITS));
        x[min(NT_FINE_IN4 + 3u, NT_IN4 - 1u)] = vec4<nt_t>(nt_blend(coarse, 3u, NT_COARSE_BITS));
    }
    let pe = NT_FINE_IN4 + NT_COARSE_F / 4u;
    let p = fine.p;
    if NT_PE > 0u {
        x[min(pe, NT_IN4 - 1u)] = vec4<nt_t>(nt_tri(vec4f(p.x, p.x + 0.25, p.y, p.y + 0.25)));
    }
    if NT_PE > 1u {
        let q = 2.0 * p;
        x[min(pe + 1u, NT_IN4 - 1u)] = vec4<nt_t>(nt_tri(vec4f(q.x, q.x + 0.25, q.y, q.y + 0.25)));
    }
    if NT_PE > 2u {
        let q = 4.0 * p;
        x[min(pe + 2u, NT_IN4 - 1u)] = vec4<nt_t>(nt_tri(vec4f(q.x, q.x + 0.25, q.y, q.y + 0.25)));
    }
    x[NT_IN4 - 1u] = vec4<nt_t>(vec4f(f32(mip & 1u), f32(level) / 8.0, 0.0, 0.0));
    return x;
}

// One fine tap's blocks at `x[at..]` (concatenated sampling).
fn nt_put_taps(x: ptr<function, array<vec4<nt_t>, NT_IN4>>, at: u32, t: vec2u) {
    (*x)[min(at, NT_IN4 - 1u)] = vec4<nt_t>(nt_block(t, 0u, NT_FINE_BITS));
    (*x)[min(at + 1u, NT_IN4 - 1u)] = vec4<nt_t>(nt_block(t, 1u, NT_FINE_BITS));
    if NT_FINE_F > 8u {
        (*x)[min(at + 2u, NT_IN4 - 1u)] = vec4<nt_t>(nt_block(t, 2u, NT_FINE_BITS));
        (*x)[min(at + 3u, NT_IN4 - 1u)] = vec4<nt_t>(nt_block(t, 3u, NT_FINE_BITS));
    }
}

// `nt_decode` (the network) is neural_mlp.wgsl, generated unrolled by tests/neural_mlp.rs.

// Output channel `i` of a decode, as f32 (callers clamp).
fn nt_channel(y: array<vec4<nt_t>, NT_OUT4>, i: u32) -> f32 {
    return f32(y[i / 4u][i % 4u]);
}

// The texture's size and mip count (descriptor word 0).
fn nt_extent(base: u32) -> vec4u {
    return nt_data[base];
}
