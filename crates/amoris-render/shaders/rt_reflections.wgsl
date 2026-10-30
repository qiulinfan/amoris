// Ray-traced water reflections (rt_shadows.rs, water.md 67): appended after rt_common.wgsl when
// AMORIS_RT_REFLECTIONS=1 put ray queries on the device. The water's reflected ray meets the nearest
// opaque surface in the top level, as the screen-space march (renamed ssr_water_reflect) could only
// for what the screen shows; the hit is lit as `shade` lights an opaque surface by the sun (its own
// shadow lookup) and the sky's irradiance, without the specular term or the punctual lights, which
// is how hybrid renderers shade reflected hits (C. Barre-Brisebois et al., "Hybrid Rendering for
// Real-Time Ray Tracing", Ray Tracing Gems, 2019). A ray that meets nothing keeps the sky.

fn rt_vertex(base_vertex: i32, index: u32, word: u32) -> f32 {
    return rt_vertices[u32(base_vertex + i32(index)) * 12u + word];
}

// The reflected colour and the distance to the hit, or a negative distance when the ray meets
// nothing in RT_REFLECT_M.
struct RtHit { color: vec3f, t: f32 }
const RT_REFLECT_M: f32 = 1.0e4;

fn rt_reflect(origin: vec3f, direction: vec3f) -> RtHit {
    var query: ray_query;
    rayQueryInitialize(&query, rt_casters, RayDesc(0u, RT_SEEN, 0.0, RT_REFLECT_M, origin, direction));
    while (rayQueryProceed(&query)) {
        let candidate = rayQueryGetCandidateIntersection(&query);
        if (candidate.kind == RAY_QUERY_INTERSECTION_TRIANGLE && rt_blocks(candidate)) {
            rayQueryConfirmIntersection(&query);
        }
    }
    let hit = rayQueryGetCommittedIntersection(&query);
    if (hit.kind == RAY_QUERY_INTERSECTION_NONE) {
        return RtHit(vec3f(0.0), -1.0);
    }
    let caster = rt_caster_info[hit.instance_index];
    let m = materials[caster.material];
    let i = caster.first_index + hit.primitive_index * 3u;
    let b = vec3f(1.0 - hit.barycentrics.x - hit.barycentrics.y, hit.barycentrics);
    var normal_object = vec3f(0.0);
    var uv = vec2f(0.0);
    for (var c = 0u; c < 3u; c++) {
        let k = rt_indices[i + c];
        normal_object += b[c] * vec3f(rt_vertex(caster.base_vertex, k, 3u),
            rt_vertex(caster.base_vertex, k, 4u), rt_vertex(caster.base_vertex, k, 5u));
        uv += b[c] * vec2f(rt_vertex(caster.base_vertex, k, 6u), rt_vertex(caster.base_vertex, k, 7u));
    }
    // A normal goes through the inverse transpose of the instance's transform: the transpose of
    // world_to_object's columns.
    let w = hit.world_to_object;
    var n = normalize(vec3f(dot(w[0], normal_object), dot(w[1], normal_object),
        dot(w[2], normal_object)));
    if (dot(n, direction) > 0.0) {
        n = -n;
    }
    if ((m.flags & 32u) != 0u) {
        // A terrain layer's texture runs over the world (forward.wgsl `surface`).
        uv = (origin + direction * hit.t).xz;
    }
    var albedo = m.base_color.rgb;
    if (m.base_color_tex != NO_TEXTURE) {
        albedo *= textureSampleLevel(tex_srgb, tex_sampler, uv * m.uv_scale, m.base_color_tex, 0.0).rgb;
    }
    var emissive = m.emissive;
    if (m.emissive_tex != NO_TEXTURE) {
        emissive *= textureSampleLevel(tex_srgb, tex_sampler, uv * m.uv_scale, m.emissive_tex, 0.0).rgb;
    }
    let p = origin + direction * hit.t;
    var color = emissive;
    if ((m.flags & 4u) != 0u) {
        color += albedo;
    } else {
        let diffuse = albedo * (1.0 - m.metallic);
        if (view.sun_dir.w > 0.0) {
            let nl = dot(n, view.sun_dir.xyz);
            if (nl > 0.0) {
                // The hit's own depth in the view, as for a surface drawn there: with the cascades
                // it picks the cascade, and a hit behind the camera or past the last is lit.
                let depth = -(view.view * vec4f(p, 1.0)).z;
                color += diffuse / PI * view.sun_color.rgb * view.sun_dir.w * nl
                    * shadow_factor(p, n, depth);
            }
        }
        color += diffuse * sh_irradiance(n) * (view.sun_color.w / PI);
    }
    return RtHit(mix(color, view.fog.rgb * view.sun_color.w, fog_amount(p)), hit.t);
}

// forward.wgsl's water reflection, traced: the hit's colour, under water attenuated over the
// reflected path as the screen-space march attenuates its own; the sky where the ray meets nothing.
fn water_reflect(start: vec3f, direction: vec3f, fallback: vec3f, sigma: vec3f, source: vec3f,
    underwater: bool) -> vec3f {
    let hit = rt_reflect(start, direction);
    if (hit.t < 0.0) {
        return fallback;
    }
    var reflected = hit.color;
    if (underwater) {
        let trans = exp(-sigma * hit.t);
        reflected = reflected * trans + source / max(sigma, vec3f(1e-6)) * (1.0 - trans);
    }
    return reflected;
}
