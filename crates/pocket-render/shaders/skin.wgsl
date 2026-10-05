// Skinning: one thread per vertex of one skinned instance blends its rest vertex by up to four
// joint matrices and writes the result into the instance's own range of the shared vertex buffer,
// which the rest of the renderer then draws like any mesh.

struct Job {
    src: u32,       // first vertex in the rest and weight buffers
    dst: u32,       // first vertex (base vertex) of the output range
    count: u32,
    palette: u32,   // first joint matrix
};

struct SkinVert {
    joints: vec2u,  // four u16 joint indices
    _pad: vec2u,
    weights: vec4f,
};

@group(0) @binding(0) var<uniform> job: Job;
@group(0) @binding(1) var<storage, read> rest: array<f32>;
@group(0) @binding(2) var<storage, read> skin: array<SkinVert>;
@group(0) @binding(3) var<storage, read> palette: array<mat4x4f>;
@group(0) @binding(4) var<storage, read_write> out_vertices: array<f32>;

const STRIDE: u32 = 12u;   // position 3, normal 3, uv 2, tangent 4

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if (i >= job.count) {
        return;
    }
    let s = (job.src + i) * STRIDE;
    let pos = vec4f(rest[s], rest[s + 1u], rest[s + 2u], 1.0);
    let nrm = vec4f(rest[s + 3u], rest[s + 4u], rest[s + 5u], 0.0);
    let tan = vec4f(rest[s + 8u], rest[s + 9u], rest[s + 10u], 0.0);
    let sv = skin[job.src + i];
    let j = array<u32, 4>(sv.joints.x & 0xffffu, sv.joints.x >> 16u, sv.joints.y & 0xffffu, sv.joints.y >> 16u);
    let w = sv.weights;
    var m = palette[job.palette + j[0]] * w.x
        + palette[job.palette + j[1]] * w.y
        + palette[job.palette + j[2]] * w.z
        + palette[job.palette + j[3]] * w.w;
    let total = w.x + w.y + w.z + w.w;
    if (total < 1e-5) {
        m = palette[job.palette + j[0]];
    }
    let p = m * pos;
    let n = normalize((m * nrm).xyz);
    let t = normalize((m * tan).xyz);
    let d = (job.dst + i) * STRIDE;
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
