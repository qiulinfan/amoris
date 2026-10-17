// Ground-truth ambient occlusion (Jimenez et al. 2016, "Practical Real-Time Strategies for Accurate
// Indirect Occlusion"; the horizon search and integration follow Intel's XeGTAO), at half
// resolution, applied to indirect light only (docs/spec/taa-gtao.md):
//   fs_depth   full-resolution depth -> half-resolution view distance (r32float)
//   fs_main    the horizon search: visibility in [0, 1] per half-resolution pixel (r8unorm)
//   fs_blur    a depth-aware 4x4 blur at half resolution (removes the 4x4 noise pattern)
//   fs_apply   a depth-aware upsample, multiplied into the scene's color through the indirect share
//              the forward shader wrote: color * (1 - share * (1 - visibility)).
// Fragment passes only (WebGPU core: render attachments, textureLoad).

struct Gtao {
    inv_proj: mat4x4f,       // this frame's (jittered) inverse projection
    proj: vec4f,             // P00, P11, P20, P21 of the projection (view position from NDC)
    ortho: vec4f,            // x: 1 for an orthographic camera; y, z: P30, P31
    full: vec4f,             // full resolution: width, height, 1/width, 1/height
    half: vec4f,             // half resolution: width, height, 1/width, 1/height
    settings: vec4f,         // x: radius (m), y: falloff share of the radius, z: final power, w: frame
    flags: vec4u,            // x: normals from the normal target, y: noise changes per frame (TAA),
                             // z: slices, w: steps per side
};

@group(0) @binding(0) var<uniform> g: Gtao;
@group(0) @binding(1) var depth: texture_depth_multisampled_2d;
@group(0) @binding(2) var half_depth: texture_2d<f32>;
@group(0) @binding(3) var ao: texture_2d<f32>;
@group(0) @binding(4) var normals: texture_2d<f32>;
@group(0) @binding(5) var share: texture_2d<f32>;

const FAR: f32 = 1e30;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4f(p, 0.0, 1.0);
}

// The view distance (positive, along the view axis) of full-resolution pixel `p`; FAR for the sky.
fn distance_at(p: vec2i) -> f32 {
    let d = textureLoad(depth, p, 0);
    let uv = (vec2f(p) + 0.5) * g.full.zw;
    let ndc = vec2f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let v = g.inv_proj * vec4f(ndc, d, 1.0);
    if (abs(v.w) < 1e-12) {
        return FAR;
    }
    let z = -v.z / v.w;
    return select(z, FAR, z <= 0.0 || z > 1e6);
}

// The view-space position of a point at half-resolution coordinate `uv` (0..1 over the image) and
// view distance `z`.
fn view_pos(uv: vec2f, z: f32) -> vec3f {
    let ndc = vec2f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    if (g.ortho.x > 0.5) {
        return vec3f((ndc - g.ortho.yz) / g.proj.xy, -z);
    }
    return vec3f((ndc + g.proj.zw) * z / g.proj.xy, -z);
}

@fragment
fn fs_depth(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let p = min(vec2i(pos.xy) * 2, vec2i(g.full.xy) - 1);
    return vec4f(distance_at(p), 0.0, 0.0, 0.0);
}

fn half_z(p: vec2i) -> f32 {
    let q = clamp(p, vec2i(0), vec2i(g.half.xy) - 1);
    return textureLoad(half_depth, q, 0).x;
}

fn half_uv(p: vec2i) -> vec2f {
    // The half-resolution texel's centre on the full-resolution image: full pixel 2p's centre.
    return (vec2f(p * 2) + 0.5) * g.full.zw;
}

fn fast_acos(x: f32) -> f32 {
    let a = abs(x);
    var r = -0.156583 * a + 1.570796;
    r *= sqrt(max(1.0 - a, 0.0));
    return select(PI - r, r, x >= 0.0);
}

// Interleaved gradient noise (Jimenez 2014).
fn ign(p: vec2f) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2f(0.06711056, 0.00583715))));
}

// The view-space normal at half-resolution pixel `p` (centre `c` at distance `z`): from the
// normal target, or from the depth (on each axis the neighbour closer in depth, so edges do not
// bend it).
fn normal_at(p: vec2i, c: vec3f) -> vec3f {
    if (g.flags.x != 0u) {
        let q = min(p * 2, vec2i(g.full.xy) - 1);
        let n = textureLoad(normals, q, 0).xyz * 2.0 - 1.0;
        if (dot(n, n) > 1e-6) {
            return normalize(n);
        }
    }
    let l = view_pos(half_uv(p + vec2i(-1, 0)), half_z(p + vec2i(-1, 0)));
    let r = view_pos(half_uv(p + vec2i(1, 0)), half_z(p + vec2i(1, 0)));
    let u = view_pos(half_uv(p + vec2i(0, -1)), half_z(p + vec2i(0, -1)));
    let d = view_pos(half_uv(p + vec2i(0, 1)), half_z(p + vec2i(0, 1)));
    let dx = select(c - l, r - c, abs(r.z - c.z) < abs(c.z - l.z));
    let dy = select(c - u, d - c, abs(d.z - c.z) < abs(c.z - u.z));
    var n = cross(dy, dx);
    if (dot(n, n) < 1e-20) {
        return normalize(-c);
    }
    n = normalize(n);
    return select(n, -n, dot(n, -c) < 0.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let p = vec2i(pos.xy);
    let z = half_z(p);
    if (z >= FAR * 0.5) {
        return vec4f(1.0);
    }
    let c = view_pos(half_uv(p), z);
    let v = normalize(-c);
    let n = normal_at(p, c);
    let radius = g.settings.x;
    // The radius in half-resolution pixels.
    var px_per_m = g.proj.y * g.half.y * 0.5;
    if (g.ortho.x < 0.5) {
        px_per_m /= z;
    }
    let screen_radius = radius * px_per_m;
    if (screen_radius < 1.0) {
        return vec4f(1.0);
    }
    let falloff_range = g.settings.y * radius;
    let falloff_mul = -1.0 / falloff_range;
    let falloff_add = (radius - falloff_range) / falloff_range + 1.0;
    // Noise: a 4x4 pattern the blur removes, or with TAA a pattern that also changes every frame.
    var noise_slice: f32;
    var noise_step: f32;
    if (g.flags.y != 0u) {
        let f = g.settings.w;
        noise_slice = ign(pos.xy + 5.588238 * f);
        noise_step = ign(pos.yx + 7.137 * f + 13.0);
    } else {
        let q = vec2u(p) & vec2u(3u);
        let bayer = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0,
            15.0, 7.0, 13.0, 5.0);
        noise_slice = (bayer[q.y * 4u + q.x] + 0.5) / 16.0;
        noise_step = fract(noise_slice * 7.0 + 0.37);
    }
    let slices = g.flags.z;
    let steps = g.flags.w;
    var visibility = 0.0;
    for (var s = 0u; s < slices; s++) {
        let phi = (f32(s) + noise_slice) * PI / f32(slices);
        let dir = vec3f(cos(phi), sin(phi), 0.0);
        // Screen direction (y down) of the view-space direction.
        let omega = vec2f(dir.x, -dir.y) * screen_radius;
        let ortho_dir = dir - dot(dir, v) * v;
        let axis = normalize(cross(ortho_dir, v));
        let proj_n = n - axis * dot(n, axis);
        let proj_len = length(proj_n);
        let sign_n = sign(dot(ortho_dir, proj_n));
        let cos_n = clamp(dot(proj_n, v) / max(proj_len, 1e-6), 0.0, 1.0);
        let ang_n = sign_n * fast_acos(cos_n);
        let low0 = cos(ang_n + PI * 0.5);
        let low1 = cos(ang_n - PI * 0.5);
        var h0 = low0;
        var h1 = low1;
        for (var k = 0u; k < steps; k++) {
            var t = (f32(k) + noise_step) / f32(steps);
            t = t * t;
            // At least a pixel away, snapped to texel centres.
            let off = round(omega * t + normalize(omega) * 1.0);
            let q0 = p + vec2i(off);
            let q1 = p - vec2i(off);
            let s0 = view_pos(half_uv(q0), half_z(q0));
            let s1 = view_pos(half_uv(q1), half_z(q1));
            let d0 = s0 - c;
            let d1 = s1 - c;
            let l0 = length(d0);
            let l1 = length(d1);
            let w0 = clamp(l0 * falloff_mul + falloff_add, 0.0, 1.0);
            let w1 = clamp(l1 * falloff_mul + falloff_add, 0.0, 1.0);
            let c0 = mix(low0, dot(d0 / max(l0, 1e-6), v), w0);
            let c1 = mix(low1, dot(d1 / max(l1, 1e-6), v), w1);
            h0 = max(h0, c0);
            h1 = max(h1, c1);
        }
        // Side 0 (along omega) is the positive angle.
        var a1 = fast_acos(h0);
        var a0 = -fast_acos(h1);
        a0 = ang_n + clamp(a0 - ang_n, -PI * 0.5, PI * 0.5);
        a1 = ang_n + clamp(a1 - ang_n, -PI * 0.5, PI * 0.5);
        let sin_n = sin(ang_n);
        let arc0 = (cos_n + 2.0 * a0 * sin_n - cos(2.0 * a0 - ang_n)) * 0.25;
        let arc1 = (cos_n + 2.0 * a1 * sin_n - cos(2.0 * a1 - ang_n)) * 0.25;
        visibility += proj_len * (arc0 + arc1);
    }
    visibility = clamp(visibility / f32(slices), 0.0, 1.0);
    visibility = pow(visibility, g.settings.z);
    return vec4f(max(visibility, 0.03));
}

// How much a tap at distance `zt` counts for a pixel at distance `z`.
fn depth_weight(zt: f32, z: f32) -> f32 {
    return exp(-abs(zt - z) / max(z, 1e-4) * 32.0);
}

@fragment
fn fs_blur(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let p = vec2i(pos.xy);
    let z = half_z(p);
    var sum = 0.0;
    var wsum = 0.0;
    for (var y = -2; y < 2; y++) {
        for (var x = -2; x < 2; x++) {
            let q = clamp(p + vec2i(x, y), vec2i(0), vec2i(g.half.xy) - 1);
            let w = depth_weight(half_z(q), z) + 1e-4;
            sum += textureLoad(ao, q, 0).x * w;
            wsum += w;
        }
    }
    return vec4f(sum / wsum);
}

@fragment
fn fs_apply(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let p = vec2i(pos.xy);
    let s = textureLoad(share, p, 0).rgb;
    if (all(s <= vec3f(0.0))) {
        return vec4f(1.0);
    }
    let z = distance_at(p);
    // The four half-resolution texels around this pixel, bilinear weights times depth agreement.
    let h = (vec2f(p) + 0.5) * 0.5 - 0.5;
    let base = vec2i(floor(h));
    let f = h - floor(h);
    var sum = 0.0;
    var wsum = 0.0;
    var nearest = 1.0;
    var best = 1e30;
    for (var k = 0; k < 4; k++) {
        let o = vec2i(k & 1, k >> 1);
        let q = clamp(base + o, vec2i(0), vec2i(g.half.xy) - 1);
        let zt = half_z(q);
        let a = textureLoad(ao, q, 0).x;
        let bw = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        let w = bw * depth_weight(zt, z);
        sum += a * w;
        wsum += w;
        let dz = abs(zt - z);
        if (dz < best) {
            best = dz;
            nearest = a;
        }
    }
    let vis = select(nearest, sum / wsum, wsum > 1e-3);
    return vec4f(1.0 - s * (1.0 - vis), 1.0);
}
