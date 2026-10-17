// API reference: wgpu-hal 30.0.1 examples/ray-traced-triangle/shader.wgsl.
// This probe uses its own buffered rays and readback results, rather than a rendered image.
enable wgpu_ray_query;

struct RayInput {
    origin_t_min: vec4<f32>,
    direction_t_max: vec4<f32>,
    mask: u32,
    padding: array<u32, 3>,
};

struct RayResult {
    kind: u32,
    t: f32,
    instance_custom_data: u32,
    instance_index: u32,
    primitive_index: u32,
    geometry_index: u32,
    barycentrics: vec2<f32>,
    front_face: u32,
    candidates: u32,
};

@group(0) @binding(0) var scene: acceleration_structure;
@group(0) @binding(1) var<storage, read> rays: array<RayInput>;
@group(0) @binding(2) var<storage, read_write> results: array<RayResult>;
// First candidate of each ray in the non-opaque pass.
@group(0) @binding(3) var<storage, read_write> first_candidates: array<RayResult>;

fn describe(ray: RayInput) -> RayDesc {
    return RayDesc(0u, ray.mask, ray.origin_t_min.w, ray.direction_t_max.w,
        ray.origin_t_min.xyz, ray.direction_t_max.xyz);
}

// Takes the intersection, not the query: naga 30's SPIR-V and HLSL writers panic on a
// ray_query pointer passed to a function (MSL accepts it).
fn committed(hit: RayIntersection) -> RayResult {
    // Miss intersection fields other than kind have no defined meaning. Write explicit sentinels.
    var result = RayResult(0u, -1.0, 0u, 0u, 0u, 0u, vec2<f32>(0.0), 0u, 0u);
    result.kind = hit.kind;
    if hit.kind == RAY_QUERY_INTERSECTION_TRIANGLE {
        result.t = hit.t;
        result.instance_custom_data = hit.instance_custom_data;
        result.instance_index = hit.instance_index;
        result.primitive_index = hit.primitive_index;
        result.geometry_index = hit.geometry_index;
        result.barycentrics = hit.barycentrics;
        result.front_face = select(0u, 1u, hit.front_face);
    }
    return result;
}

// Opaque geometry: traversal commits the closest hit without the shader.
@compute @workgroup_size(8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&rays) {
        return;
    }
    var query: ray_query;
    rayQueryInitialize(&query, scene, describe(rays[id.x]));
    while rayQueryProceed(&query) {}
    results[id.x] = committed(rayQueryGetCommittedIntersection(&query));
}

// Non-opaque geometry: every hit is a candidate that only the shader can commit, as in the path
// tracer's alpha and sidedness tests.
@compute @workgroup_size(8)
fn confirm_candidates(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&rays) {
        return;
    }
    var query: ray_query;
    rayQueryInitialize(&query, scene, describe(rays[id.x]));
    var first = RayResult(0u, -1.0, 0u, 0u, 0u, 0u, vec2<f32>(0.0), 0u, 0u);
    var seen = 0u;
    while rayQueryProceed(&query) {
        let candidate = rayQueryGetCandidateIntersection(&query);
        if seen == 0u {
            first = RayResult(candidate.kind, candidate.t, candidate.instance_custom_data,
                candidate.instance_index, candidate.primitive_index, candidate.geometry_index,
                candidate.barycentrics, select(0u, 1u, candidate.front_face), 0u);
        }
        seen += 1u;
        rayQueryConfirmIntersection(&query);
    }
    first.candidates = seen;
    first_candidates[id.x] = first;
    results[id.x] = committed(rayQueryGetCommittedIntersection(&query));
}
