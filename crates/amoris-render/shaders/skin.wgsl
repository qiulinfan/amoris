// Skinning: one thread per vertex of a skinned instance blends its rest vertex by up to four
// joint matrices and writes the result into the instance's own range of the shared vertex buffer,
// which the rest of the renderer then draws like any mesh. One dispatch skins every instance: each
// job (skinned instance) owns consecutive workgroups, and a workgroup finds its job by a binary
// search over the jobs' first workgroups (skinning.rs).
//
// Morph targets (docs/spec/animation.md 5a) come first: a vertex adds each of its job's active
// targets' offsets times the target's weight to its rest position, normal and tangent, then skins.
// A job without active targets reads its rest vertex as before.

// `jobs`: the job count, then per job eight words: its first vertex in the rest and weight buffers,
// the first vertex (base vertex) of its output range, its vertex count, its first joint matrix,
// its first workgroup, its source's first morph offset, its first active target and their count.
const JOB_WORDS: u32 = 8u;
// Workgroups per dispatch row (skinning.rs `ROW`).
const ROW: u32 = 65535u;

struct SkinVert {
    joints: vec2u,  // four u16 joint indices
    _pad: vec2u,
    weights: vec4f,
};

@group(0) @binding(0) var<storage, read> jobs: array<u32>;
@group(0) @binding(1) var<storage, read> rest: array<f32>;
@group(0) @binding(2) var<storage, read> skin: array<SkinVert>;
@group(0) @binding(3) var<storage, read> palette: array<mat4x4f>;
@group(0) @binding(4) var<storage, read_write> out_vertices: array<f32>;
// Per target, per vertex of a source: position, normal and tangent offsets (MorphTargets::STRIDE).
@group(0) @binding(5) var<storage, read> deltas: array<f32>;
// The frame's active targets: (target, weight's bits).
@group(0) @binding(6) var<storage, read> morph_active: array<vec2u>;

const STRIDE: u32 = 12u;   // position 3, normal 3, uv 2, tangent 4
const MORPH_STRIDE: u32 = 9u;

fn job_word(k: u32, word: u32) -> u32 {
    return jobs[1u + k * JOB_WORDS + word];
}

@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) wg: vec3u, @builtin(local_invocation_index) lane: u32) {
    let g = wg.x + wg.y * ROW;
    // The last job whose first workgroup is at or before this one.
    var lo = 0u;
    var hi = jobs[0];
    while (hi - lo > 1u) {
        let mid = (lo + hi) / 2u;
        if (job_word(mid, 4u) <= g) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let src = job_word(lo, 0u);
    let dst = job_word(lo, 1u);
    let count = job_word(lo, 2u);
    let first = job_word(lo, 3u);
    let i = (g - job_word(lo, 4u)) * 64u + lane;
    if (i >= count) {
        return;
    }
    let s = (src + i) * STRIDE;
    var p3 = vec3f(rest[s], rest[s + 1u], rest[s + 2u]);
    var n3 = vec3f(rest[s + 3u], rest[s + 4u], rest[s + 5u]);
    var t3 = vec3f(rest[s + 8u], rest[s + 9u], rest[s + 10u]);
    let actives = job_word(lo, 7u);
    if (actives > 0u) {
        let morph_at = job_word(lo, 5u);
        let first_active = job_word(lo, 6u);
        for (var k = 0u; k < actives; k++) {
            let a = morph_active[first_active + k];
            let w = bitcast<f32>(a.y);
            let d = morph_at + (a.x * count + i) * MORPH_STRIDE;
            p3 += w * vec3f(deltas[d], deltas[d + 1u], deltas[d + 2u]);
            n3 += w * vec3f(deltas[d + 3u], deltas[d + 4u], deltas[d + 5u]);
            t3 += w * vec3f(deltas[d + 6u], deltas[d + 7u], deltas[d + 8u]);
        }
    }
    let pos = vec4f(p3, 1.0);
    let nrm = vec4f(n3, 0.0);
    let tan = vec4f(t3, 0.0);
    let sv = skin[src + i];
    let j = array<u32, 4>(sv.joints.x & 0xffffu, sv.joints.x >> 16u, sv.joints.y & 0xffffu, sv.joints.y >> 16u);
    let w = sv.weights;
    var m = palette[first + j[0]] * w.x
        + palette[first + j[1]] * w.y
        + palette[first + j[2]] * w.z
        + palette[first + j[3]] * w.w;
    let total = w.x + w.y + w.z + w.w;
    if (total < 1e-5) {
        m = palette[first + j[0]];
    }
    let p = m * pos;
    let n = normalize((m * nrm).xyz);
    let t = normalize((m * tan).xyz);
    let d = (dst + i) * STRIDE;
    out_vertices[d] = p.x;
    out_vertices[d + 1u] = p.y;
    out_vertices[d + 2u] = p.z;
    out_vertices[d + 3u] = n.x;
    out_vertices[d + 4u] = n.y;
    out_vertices[d + 5u] = n.z;
    out_vertices[d + 6u] = rest[s + 6u];
    out_vertices[d + 7u] = rest[s + 7u];
    out_vertices[d + 8u] = t.x;
    out_vertices[d + 9u] = t.y;
    out_vertices[d + 10u] = t.z;
    out_vertices[d + 11u] = rest[s + 11u];
}
