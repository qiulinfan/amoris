// Test entry points for tests/hiz.rs, appended to the composed culling shader (cull.wgsl, whose
// `occluded` and `hiz` they use): fill a multisampled depth target with a pattern, one value per
// sample; dump every sample of it; and run the late pass's occlusion test on a list of boxes.

struct Probe {
    view_proj: mat4x4f,
    size: vec2f,
    levels: u32,
    count: u32,
    // 0: an independent random depth per sample; 1: walls and holes (`walls`).
    pattern: u32,
    seed: u32,
    near: f32,
    _pad: u32,
};

struct ProbeBox {
    c: vec4f,
    a0: vec4f,
    a1: vec4f,
    a2: vec4f,
};

@group(0) @binding(10) var<uniform> probe: Probe;
@group(0) @binding(11) var<storage, read> probe_boxes: array<ProbeBox>;
@group(0) @binding(12) var<storage, read_write> probe_answers: array<u32>;
@group(0) @binding(13) var probe_depth: texture_depth_multisampled_2d;
@group(0) @binding(14) var<storage, read_write> probe_samples: array<f32>;

fn probe_hash(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

fn probe_hash3(x: u32, y: u32, z: u32) -> u32 {
    return probe_hash(x ^ probe_hash(y ^ probe_hash(z ^ probe.seed)));
}

// Cells of 97 x 61 pixels (not aligned with any pyramid level) each hold a wall at 6, 10, 16 or
// 25 m, or, one in eight, nothing (depth 0). In a third of the cells one pixel in 48 is a hole to
// infinity, in all four samples or in one only.
fn walls(p: vec2u, s: u32) -> f32 {
    let ch = probe_hash3(p.x / 97u, p.y / 61u, 1000u);
    var dist = array<f32, 8>(6.0, 10.0, 16.0, 25.0, 6.0, 10.0, 16.0, 0.0);
    let m = dist[ch % 8u];
    var d = select(0.0, probe.near / m, m > 0.0);
    let ph = probe_hash3(p.x, p.y, 2000u);
    if ((ch >> 8u) % 3u == 0u && ph % 48u == 0u) {
        if ((ph >> 8u) % 2u == 0u || s == (ph >> 9u) % 4u) {
            d = 0.0;
        }
    }
    return d;
}

@vertex
fn probe_fill_vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(uv * 2.0 - 1.0, 0.5, 1.0);
}

// Runs per sample (it reads `sample_index`), so each sample gets its own depth.
@fragment
fn probe_fill_fs(@builtin(position) pos: vec4f, @builtin(sample_index) s: u32)
    -> @builtin(frag_depth) f32 {
    let p = vec2u(pos.xy);
    if (probe.pattern == 0u) {
        return f32(probe_hash3(p.x, p.y, s) >> 8u) / 16777216.0;
    }
    return walls(p, s);
}

@compute @workgroup_size(8, 8)
fn probe_dump(@builtin(global_invocation_id) id: vec3u) {
    let size = textureDimensions(probe_depth);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let n = textureNumSamples(probe_depth);
    for (var s = 0u; s < n; s++) {
        probe_samples[(id.y * size.x + id.x) * n + s] = textureLoad(probe_depth, id.xy, s);
    }
}

@compute @workgroup_size(64)
fn probe_occluded(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= probe.count) {
        return;
    }
    let b = probe_boxes[id.x];
    let hidden = occluded(probe.view_proj, probe.size, probe.levels, b.c.xyz, b.a0.xyz, b.a1.xyz,
        b.a2.xyz);
    probe_answers[id.x] = select(0u, 1u, hidden);
}
