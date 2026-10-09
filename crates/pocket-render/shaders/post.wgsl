// Post-processing on the HDR image: bloom (a 13-tap downsample chain with a Karis average on the
// first step against fireflies, then a tent-filtered upsample chain added back), and the display
// transform: exposure, AgX tone mapping with its punchy look, and dithering.

struct PostParams {
    texel: vec4f,            // 1/width, 1/height of the source; z: bloom strength; w: first step (1/0)
    exposure: vec4f,         // x: exposure multiplier; y: output is sRGB-encoded by the surface (1/0)
};

@group(0) @binding(0) var<uniform> pp: PostParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;
@group(0) @binding(3) var bloom: texture_2d<f32>;

struct FsIn {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FsIn {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    var o: FsIn;
    o.pos = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2f(p.x, 1.0 - p.y);
    return o;
}

fn karis(c: vec3f) -> f32 {
    return 1.0 / (1.0 + luminance(c));
}

@fragment
fn fs_down(in: FsIn) -> @location(0) vec4f {
    let t = pp.texel.xy;
    let uv = in.uv;
    let a = textureSample(src, src_sampler, uv + t * vec2f(-2.0, -2.0)).rgb;
    let b = textureSample(src, src_sampler, uv + t * vec2f(0.0, -2.0)).rgb;
    let c = textureSample(src, src_sampler, uv + t * vec2f(2.0, -2.0)).rgb;
    let d = textureSample(src, src_sampler, uv + t * vec2f(-2.0, 0.0)).rgb;
    let e = textureSample(src, src_sampler, uv).rgb;
    let f = textureSample(src, src_sampler, uv + t * vec2f(2.0, 0.0)).rgb;
    let g = textureSample(src, src_sampler, uv + t * vec2f(-2.0, 2.0)).rgb;
    let h = textureSample(src, src_sampler, uv + t * vec2f(0.0, 2.0)).rgb;
    let i = textureSample(src, src_sampler, uv + t * vec2f(2.0, 2.0)).rgb;
    let j = textureSample(src, src_sampler, uv + t * vec2f(-1.0, -1.0)).rgb;
    let k = textureSample(src, src_sampler, uv + t * vec2f(1.0, -1.0)).rgb;
    let l = textureSample(src, src_sampler, uv + t * vec2f(-1.0, 1.0)).rgb;
    let m = textureSample(src, src_sampler, uv + t * vec2f(1.0, 1.0)).rgb;
    if (pp.texel.w > 0.5) {
        // Karis average per group of four on the first downsample.
        let g0 = (a + b + d + e) * 0.25;
        let g1 = (b + c + e + f) * 0.25;
        let g2 = (d + e + g + h) * 0.25;
        let g3 = (e + f + h + i) * 0.25;
        let g4 = (j + k + l + m) * 0.25;
        let w0 = karis(g0) * 0.125;
        let w1 = karis(g1) * 0.125;
        let w2 = karis(g2) * 0.125;
        let w3 = karis(g3) * 0.125;
        let w4 = karis(g4) * 0.5;
        let s = g0 * w0 + g1 * w1 + g2 * w2 + g3 * w3 + g4 * w4;
        return vec4f(s / max(w0 + w1 + w2 + w3 + w4, 1e-4), 1.0);
    }
    var s = e * 0.125;
    s += (a + c + g + i) * 0.03125;
    s += (b + d + f + h) * 0.0625;
    s += (j + k + l + m) * 0.125;
    return vec4f(s, 1.0);
}

@fragment
fn fs_up(in: FsIn) -> @location(0) vec4f {
    let t = pp.texel.xy;
    let uv = in.uv;
    var s = textureSample(src, src_sampler, uv).rgb * 4.0;
    s += (textureSample(src, src_sampler, uv + vec2f(-t.x, 0.0)).rgb
        + textureSample(src, src_sampler, uv + vec2f(t.x, 0.0)).rgb
        + textureSample(src, src_sampler, uv + vec2f(0.0, -t.y)).rgb
        + textureSample(src, src_sampler, uv + vec2f(0.0, t.y)).rgb) * 2.0;
    s += textureSample(src, src_sampler, uv + vec2f(-t.x, -t.y)).rgb
        + textureSample(src, src_sampler, uv + vec2f(t.x, -t.y)).rgb
        + textureSample(src, src_sampler, uv + vec2f(-t.x, t.y)).rgb
        + textureSample(src, src_sampler, uv + vec2f(t.x, t.y)).rgb;
    return vec4f(s / 16.0, 1.0);
}

// AgX (Troy Sobotka), the minimal fitted form with the punchy look.
fn agx_contrast(x: vec3f) -> vec3f {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x
        + 0.4298 * x2 + 0.1191 * x - 0.00232;
}

fn agx(c: vec3f) -> vec3f {
    let inset = mat3x3f(
        vec3f(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
        vec3f(0.0784335999999992, 0.878468636469772, 0.0784336),
        vec3f(0.0792237451477643, 0.0791661274605434, 0.879142973793104));
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var v = inset * c;
    v = clamp(log2(max(v, vec3f(1e-10))), vec3f(min_ev), vec3f(max_ev));
    v = (v - min_ev) / (max_ev - min_ev);
    v = agx_contrast(v);
    // Punchy look: a little more saturation and contrast.
    let luma = luminance(v);
    v = pow(max(v, vec3f(0.0)), vec3f(1.35));
    v = luma + 1.4 * (v - luma);
    let outset = mat3x3f(
        vec3f(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
        vec3f(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
        vec3f(-0.0990297440797205, -0.0989611768448433, 1.15107367264116));
    v = outset * v;
    return clamp(v, vec3f(0.0), vec3f(1.0));
}

fn linear_to_srgb(c: vec3f) -> vec3f {
    return select(1.055 * pow(c, vec3f(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3f(0.0031308));
}

@fragment
fn fs_tonemap(in: FsIn) -> @location(0) vec4f {
    var c = textureSample(src, src_sampler, in.uv).rgb;
    // The upsample chain sums every level: divide by their count (texel.w) for an energy-preserving
    // blur, then blend it in.
    let b = textureSample(bloom, src_sampler, in.uv).rgb / max(pp.texel.w, 1.0);
    c = mix(c, b, pp.texel.z);
    c *= pp.exposure.x;
    // AgX's output is display-encoded (sRGB-like transfer built in).
    var o = agx(c);
    // Dither against banding in skies: a uniform value per pixel from an integer hash, identical on
    // every GPU and API (common.wgsl, `pcg_hash`).
    let px = vec2u(in.pos.xy);
    let n = unit_from_hash(pcg_hash(px.x + pcg_hash(px.y)));
    o += (n - 0.5) / 255.0;
    if (pp.exposure.y > 0.5) {
        // The surface encodes to sRGB: hand it linear values that encode to AgX's output.
        o = srgb_to_linear(clamp(o, vec3f(0.0), vec3f(1.0)));
    }
    return vec4f(o, 1.0);
}

@fragment
fn fs_copy(in: FsIn) -> @location(0) vec4f {
    return textureSample(src, src_sampler, in.uv);
}
