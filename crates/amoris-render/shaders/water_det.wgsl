// The deterministic water kernels' own arithmetic (water.md 66). tests/water_det.rs rounds every f32
// addition, subtraction, multiplication and negation of the kernels and of these functions once, to
// nearest even, behind a value no compiler can see through, so none is fused or reordered; these
// functions build what WGSL leaves to each device (division, square root, the transcendentals) from
// those operations and from integer operations alone. Each is within two units in the last place of
// the exact result over the range the kernels use it in.

// 1/b: a first guess from b's bits, then four Newton steps, each squaring the relative error.
// 0 for b = 0, which no kernel divides by.
fn det_rcp(b: f32) -> f32 {
    let m = abs(b);
    if (m == 0.0) { return 0.0; }
    var x = bitcast<f32>(0x7ef311c3u - bitcast<u32>(m));
    x = x * (2.0 - m * x);
    x = x * (2.0 - m * x);
    x = x * (2.0 - m * x);
    x = x * (2.0 - m * x);
    return select(x, -x, b < 0.0);
}
fn det_div(a: f32, b: f32) -> f32 { return a * det_rcp(b); }
fn det_div2(a: vec2f, b: vec2f) -> vec2f { return vec2f(det_div(a.x, b.x), det_div(a.y, b.y)); }
fn det_div3(a: vec3f, b: vec3f) -> vec3f {
    return vec3f(det_div(a.x, b.x), det_div(a.y, b.y), det_div(a.z, b.z));
}
fn det_div4(a: vec4f, b: vec4f) -> vec4f {
    return vec4f(det_div(a.x, b.x), det_div(a.y, b.y), det_div(a.z, b.z), det_div(a.w, b.w));
}

// 1/sqrt(x) for x > 0: C. Lomont's constant ("Fast inverse square root", 2003) for the first
// guess, then three Newton steps.
fn det_rsqrt(x: f32) -> f32 {
    var y = bitcast<f32>(0x5f375a86u - (bitcast<u32>(x) >> 1u));
    let h = 0.5 * x;
    y = y * (1.5 - h * y * y);
    y = y * (1.5 - h * y * y);
    y = y * (1.5 - h * y * y);
    return y;
}
// sqrt(x), 0 for x <= 0: x/sqrt(x), then one Newton step on the root.
fn det_sqrt(x: f32) -> f32 {
    if (x <= 0.0) { return 0.0; }
    let y = det_rsqrt(x);
    let s = x * y;
    return s + 0.5 * y * (x - s * s);
}
fn det_length2(v: vec2f) -> f32 { return det_sqrt(v.x * v.x + v.y * v.y); }
fn det_length3(v: vec3f) -> f32 { return det_sqrt(v.x * v.x + v.y * v.y + v.z * v.z); }
fn det_dot2(a: vec2f, b: vec2f) -> f32 { return a.x * b.x + a.y * b.y; }
fn det_dot3(a: vec3f, b: vec3f) -> f32 { return a.x * b.x + a.y * b.y + a.z * b.z; }
fn det_dot4(a: vec4f, b: vec4f) -> f32 { return a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w; }
// WGSL's own definitions, a * (1 - t) + b * t and a * b + c, with each operation rounded.
fn det_mix(a: f32, b: f32, t: f32) -> f32 { return a * (1.0 - t) + b * t; }
fn det_mix2(a: vec2f, b: vec2f, t: vec2f) -> vec2f { return a * (1.0 - t) + b * t; }
fn det_mix3(a: vec3f, b: vec3f, t: vec3f) -> vec3f { return a * (1.0 - t) + b * t; }
fn det_mix4(a: vec4f, b: vec4f, t: vec4f) -> vec4f { return a * (1.0 - t) + b * t; }
fn det_fma(a: f32, b: f32, c: f32) -> f32 { return a * b + c; }
fn det_smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = clamp(det_div(x - low, high - low), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// e^x: W. J. Cody and W. Waite's reduction (Software Manual for the Elementary Functions, 1980),
// x = n ln 2 + r with ln 2 in two parts, so n ln 2 is exact; e^r by its Taylor polynomial to
// degree 7 for |r| <= ln(2)/2; then n added to the exponent. 0 below -69, where e^x is under the
// smallest value the kernels keep (2^-100).
fn det_exp(x: f32) -> f32 {
    if (x < -69.0) { return 0.0; }
    let c = min(x, 88.0);
    let n = round(c * 1.44269504);
    let r = (c - n * 0.693145752) - n * 1.42860677e-6;
    let p = 1.0 + r * (1.0 + r * (0.5 + r * (0.16666667 + r * (0.041666668 + r * (0.008333334
        + r * (0.0013888889 + r * 0.0001984127))))));
    return bitcast<f32>(bitcast<u32>(p) + bitcast<u32>(i32(n) << 23u));
}
// ln(x) for x > 0: x = 2^e m with m in [sqrt(1/2), sqrt(2)), and ln m = 2 atanh(s) for
// s = (m - 1)/(m + 1), |s| <= 0.172, by its series to s^11.
fn det_log(x: f32) -> f32 {
    let u = bitcast<u32>(x);
    var e = i32(u >> 23u) - 127;
    var m = bitcast<f32>((u & 0x007fffffu) | 0x3f800000u);
    if (m > 1.41421356) {
        m = m * 0.5;
        e = e + 1;
    }
    let s = det_div(m - 1.0, m + 1.0);
    let z = s * s;
    let p = s * (2.0 + z * (0.6666667 + z * (0.4 + z * (0.2857143 + z * (0.2222222
        + z * 0.18181818)))));
    let k = f32(e);
    return k * 0.693145752 + (k * 1.42860677e-6 + p);
}
// x^y for x > 0; 0 for x <= 0 (the kernels raise only positive bases).
fn det_pow(x: f32, y: f32) -> f32 {
    if (x <= 0.0) { return 0.0; }
    return det_exp(y * det_log(x));
}
// sin x and cos x: Cody and Waite's reduction by pi/2 in three parts (each exact when multiplied by
// the quadrant's index), then Taylor polynomials for |r| <= pi/4 and the quadrant's signs.
fn det_sincos(x: f32) -> vec2f {
    let k = round(x * 0.636619772);
    let r = ((x - k * 1.5703125) - k * 4.83751297e-4) - k * 7.54978995e-8;
    let z = r * r;
    let s = r * (1.0 + z * (-0.16666667 + z * (0.008333333 + z * (-0.0001984127
        + z * 2.7557319e-6))));
    let c = 1.0 + z * (-0.5 + z * (0.041666668 + z * (-0.0013888889 + z * (2.4801587e-5
        + z * -2.7557319e-7))));
    switch (i32(k) & 3) {
        case 0: { return vec2f(s, c); }
        case 1: { return vec2f(c, -s); }
        case 2: { return vec2f(-s, -c); }
        default: { return vec2f(-c, s); }
    }
}
fn det_sin(x: f32) -> f32 { return det_sincos(x).x; }
fn det_cos(x: f32) -> f32 { return det_sincos(x).y; }
// acosh x = ln(x + sqrt(x^2 - 1)) for x >= 1; 0 below.
fn det_acosh(x: f32) -> f32 {
    if (x <= 1.0) { return 0.0; }
    return det_log(x + det_sqrt(x * x - 1.0));
}
// tanh x = 1 - 2/(e^2|x| + 1), with x's sign; 1 beyond |x| = 9.
fn det_tanh(x: f32) -> f32 {
    let a = abs(x);
    var t = 1.0;
    if (a <= 9.0) { t = 1.0 - det_div(2.0, det_exp(2.0 * a) + 1.0); }
    return select(t, -t, x < 0.0);
}
