// The transparent stage's two-sided index lists (transparent.rs, docs/spec/transparency.md): a mesh
// row's indices copied twice into one list, the first copy's triangles reversed here. Drawn with a
// pipeline that culls back faces, the reversed copy draws the mesh's back faces (they now face the
// camera) and the second copy its front faces, so one instanced draw draws every instance's back
// faces before its front faces.

struct Job {
    first: u32,       // the reversed copy's first index in `indices`
    triangles: u32,
};

@group(0) @binding(0) var<storage, read_write> indices: array<u32>;
@group(0) @binding(1) var<storage, read> jobs: array<Job>;

// One job per workgroup row (y); a job's triangles strided over the row's invocations.
@compute @workgroup_size(64)
fn reverse(@builtin(global_invocation_id) id: vec3u, @builtin(num_workgroups) groups: vec3u) {
    let job = jobs[id.y];
    for (var t = id.x; t < job.triangles; t += groups.x * 64u) {
        let i = job.first + t * 3u;
        let b = indices[i + 1u];
        indices[i + 1u] = indices[i + 2u];
        indices[i + 2u] = b;
    }
}
