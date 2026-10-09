// Ray-traced sun shadows (rt_shadows.rs, docs/spec/rt-shadows.md): appended to the forward shader
// when POCKET_RT_SHADOWS=1 put ray queries on the device. It replaces the cascaded shadow lookup
// (renamed csm_shadow_factor) with one inline ray query toward the sun through the top-level
// structure of the static shadow casters: hard shadows, at any distance, no shadow map.

// One top-level instance: its material row and its mesh's range in the pool (rt_shadows.rs).
struct RtCaster {
    material: u32,
    first_index: u32,
    base_vertex: i32,
    _pad: u32,
};

@group(1) @binding(11) var rt_casters: acceleration_structure;
@group(1) @binding(12) var<storage, read> rt_caster_info: array<RtCaster>;
// The mesh pool: indices, and vertices as 12 floats (position, normal, uv, tangent).
@group(1) @binding(13) var<storage, read> rt_indices: array<u32>;
@group(1) @binding(14) var<storage, read> rt_vertices: array<f32>;

fn rt_uv(base_vertex: i32, index: u32) -> vec2f {
    let v = u32(base_vertex + i32(index)) * 12u;
    return vec2f(rt_vertices[v + 6u], rt_vertices[v + 7u]);
}

// Whether a candidate hit on an alpha-masked caster (non-opaque geometry) blocks the sun: the
// masked shadow pass's test (fs_shadow_masked) at the hit's interpolated texture coordinate.
fn rt_blocks(hit: RayIntersection) -> bool {
    let caster = rt_caster_info[hit.instance_index];
    let m = materials[caster.material];
    var a = m.base_color.a;
    if (m.base_color_tex != NO_TEXTURE) {
        let i = caster.first_index + hit.primitive_index * 3u;
        let b = vec3f(1.0 - hit.barycentrics.x - hit.barycentrics.y, hit.barycentrics);
        let uv = rt_uv(caster.base_vertex, rt_indices[i]) * b.x
            + rt_uv(caster.base_vertex, rt_indices[i + 1u]) * b.y
            + rt_uv(caster.base_vertex, rt_indices[i + 2u]) * b.z;
        a *= textureSampleLevel(tex_srgb, tex_sampler, uv, m.base_color_tex, 0.0).a;
    }
    return a >= m.alpha_cutoff;
}

fn shadow_factor(world: vec3f, n: vec3f, view_depth: f32) -> f32 {
    if (view.sky_color.w < 0.5) {
        return 1.0;
    }
    // Leave the surface along its normal: a few millimetres near the camera, more further away,
    // where the positions interpolated by the rasterizer and the acceleration structure's own
    // transform round further apart.
    let origin = world + n * (0.002 + 0.001 * max(view_depth, 0.0));
    var query: ray_query;
    rayQueryInitialize(&query, rt_casters,
        RayDesc(RAY_FLAG_TERMINATE_ON_FIRST_HIT, 0xffu, 0.0, 1.0e5, origin, view.sun_dir.xyz));
    // Opaque casters commit by themselves; masked ones arrive here as candidates.
    while (rayQueryProceed(&query)) {
        let candidate = rayQueryGetCandidateIntersection(&query);
        if (candidate.kind == RAY_QUERY_INTERSECTION_TRIANGLE && rt_blocks(candidate)) {
            rayQueryConfirmIntersection(&query);
        }
    }
    let hit = rayQueryGetCommittedIntersection(&query);
    return select(1.0, 0.0, hit.kind != RAY_QUERY_INTERSECTION_NONE);
}
