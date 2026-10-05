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
};

@group(0) @binding(0) var scene: acceleration_structure;
@group(0) @binding(1) var<storage, read> rays: array<RayInput>;
@group(0) @binding(2) var<storage, read_write> results: array<RayResult>;

@compute @workgroup_size(8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&rays) {
        return;
    }
    let ray = rays[id.x];
    var query: ray_query;
    rayQueryInitialize(&query, scene, RayDesc(
        0u, ray.mask, ray.origin_t_min.w, ray.direction_t_max.w,
        ray.origin_t_min.xyz, ray.direction_t_max.xyz,
    ));
    while rayQueryProceed(&query) {}
    let hit = rayQueryGetCommittedIntersection(&query);
    // Miss intersection fields other than kind have no defined meaning. Write explicit sentinels.
    var result = RayResult(0u, -1.0, 0u, 0u, 0u, 0u, vec2<f32>(0.0));
    result.kind = hit.kind;
    if hit.kind == RAY_QUERY_INTERSECTION_TRIANGLE {
        result.t = hit.t;
        result.instance_custom_data = hit.instance_custom_data;
        result.instance_index = hit.instance_index;
        result.primitive_index = hit.primitive_index;
        result.geometry_index = hit.geometry_index;
        result.barycentrics = hit.barycentrics;
    }
    results[id.x] = result;
}
