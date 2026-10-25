// Ray-traced sun shadows (rt_shadows.rs, docs/spec/rt-shadows.md): appended after rt_common.wgsl
// when AMORIS_RT_SHADOWS=1 put ray queries on the device. It replaces the cascaded shadow lookup
// (renamed csm_shadow_factor) with one inline ray query toward the sun through the top-level
// structure of the static shadow casters: hard shadows, at any distance, no shadow map.

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
        RayDesc(RAY_FLAG_TERMINATE_ON_FIRST_HIT, RT_CASTS, 0.0, 1.0e5, origin, view.sun_dir.xyz));
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
