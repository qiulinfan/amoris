// Standard static-surface path integrator: textures, GGX VNDF, rough/smooth dielectric,
// emissive triangles, punctual/environment NEE with MIS, compensated RR and progressive HDR.
// Concatenate pt_bsdf.wgsl and online_nrc.wgsl before this file. No algorithm implementation is copied from PBRT.
// References: https://pbr-book.org/4ed/Light_Transport_I_Surface_Reflection/A_Better_Path_Tracer
// Nonopaque acceleration geometry is required: candidate intersections apply alpha and sidedness.
override NRC_ENABLED: bool = false;
const PT_INVALID: u32 = 0xffffffffu;
struct PtParameters {
    image: vec4<u32>, limits: vec4<u32>,
    camera_position: vec4<f32>, camera_right: vec4<f32>, camera_up: vec4<f32>, camera_forward: vec4<f32>,
    lighting_scale: vec4<f32>, scene_min: vec4<f32>, scene_extent: vec4<f32>,
    controls: vec4<u32>, ray: vec4<f32>,
};
struct PtTriangle {
    p: array<vec4<f32>, 3>, n: array<vec4<f32>, 3>,
    uv: array<vec4<f32>, 3>, tangent: array<vec4<f32>, 3>,
};
struct PtMaterial {
    base_color: vec4<f32>, emission: vec4<f32>, surface: vec4<f32>, tex: vec4<u32>, flags: vec4<u32>, extra: vec4<f32>,
};
struct PtLight { position_kind: vec4<f32>, direction_range: vec4<f32>, color_intensity: vec4<f32>, cone: vec4<f32> };
struct PtEmitter { triangle: u32, area: f32, power_cdf: f32, padding: u32 };
struct PtHit { valid: u32, primitive: u32, front_face: u32, distance: f32, bary: vec2<f32> };
struct PtSurface {
    position: vec3<f32>, normal: vec3<f32>, geometric: vec3<f32>, tangent: vec3<f32>, bitangent: vec3<f32>,
    emission: vec3<f32>, bsdf: PtBsdf,
};
struct PtLightSample { direction: vec3<f32>, radiance: vec3<f32>, distance: f32, pdf: f32, delta: u32 };
@group(0) @binding(0) var<uniform> pt_params: PtParameters;
@group(0) @binding(1) var pt_scene: acceleration_structure;
@group(0) @binding(2) var<storage, read> pt_triangles: array<PtTriangle>;
@group(0) @binding(3) var<storage, read> pt_materials: array<PtMaterial>;
@group(0) @binding(4) var<storage, read> pt_lights: array<PtLight>;
@group(0) @binding(5) var<storage, read> pt_emitters: array<PtEmitter>;
@group(0) @binding(6) var<storage, read_write> pt_image: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> pt_counters: array<atomic<u32>>;
@group(0) @binding(8) var pt_textures: texture_2d_array<f32>;
@group(0) @binding(9) var pt_sampler: sampler;
@group(0) @binding(10) var pt_sky: texture_cube<f32>;
@group(0) @binding(11) var pt_sky_sampler: sampler;
// Same texture array, viewed as sRGB only for color roles.
@group(0) @binding(12) var pt_color_textures: texture_2d_array<f32>;

fn pt_random(state: ptr<function, u32>) -> f32 {
    *state ^= *state << 13u; *state ^= *state >> 17u; *state ^= *state << 5u;
    return f32(*state >> 8u)*(1.0/16777216.0);
}
fn pt_hash(input: u32) -> u32 {
    var x = input; x ^= x >> 16u; x *= 0x7feb352du; x ^= x >> 15u; x *= 0x846ca68bu;
    return x ^ (x >> 16u);
}
fn pt_srgb_decode(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + vec3<f32>(0.055))/1.055, vec3<f32>(2.4)), c/12.92, c <= vec3<f32>(0.04045));
}
fn pt_texture(index: u32, uv: vec2<f32>) -> vec4<f32> {
    if index == PT_INVALID { return vec4<f32>(1.0); }
    // Explicit LOD 0: ray footprints/mip selection are a separate filtering optimization.
    return textureSampleLevel(pt_textures, pt_sampler, uv, i32(index), 0.0);
}
fn pt_color_texture(index: u32, uv: vec2<f32>) -> vec4<f32> {
    if index == PT_INVALID { return vec4<f32>(1.0); }
    // The sRGB view decodes color texels before hardware bilinear filtering. Data textures
    // (normal, metallic/roughness, alpha, transmission) retain their linear UNORM view.
    return textureSampleLevel(pt_color_textures, pt_sampler, uv, i32(index), 0.0);
}
fn pt_uv(triangle: PtTriangle, bary: vec2<f32>) -> vec2<f32> {
    return triangle.uv[0].xy*(1.0 - bary.x - bary.y) + triangle.uv[1].xy*bary.x + triangle.uv[2].xy*bary.y;
}
fn pt_trace(origin: vec3<f32>, direction: vec3<f32>, maximum: f32, shadow: bool, state: ptr<function, u32>) -> PtHit {
    // Naga 30 MSL does not preserve assignment reset of loop-local ray queries.
    var query: ray_query;
    atomicAdd(&pt_counters[select(1u, 2u, shadow)], 1u);
    let flags = select(RAY_FLAG_NONE, RAY_FLAG_TERMINATE_ON_FIRST_HIT,
        shadow && pt_params.controls.w != 0u);
    rayQueryInitialize(&query, pt_scene, RayDesc(flags, 255u, pt_params.ray.x, maximum, origin, direction));
    while rayQueryProceed(&query) {
        let candidate = rayQueryGetCandidateIntersection(&query);
        if candidate.kind != RAY_QUERY_INTERSECTION_TRIANGLE { continue; }
        let triangle = pt_triangles[candidate.primitive_index];
        let material = pt_materials[u32(triangle.n[0].w)];
        if shadow && material.flags.w == 0u { continue; }
        // Transmission is a solid air/material interface and needs both entering and exit faces.
        if !shadow && !candidate.front_face && material.emission.w < 0.5 && material.surface.z <= 0.0 { continue; }
        let alpha = clamp(material.base_color.a * pt_texture(material.tex.x, pt_uv(triangle, candidate.barycentrics)).a, 0.0, 1.0);
        if material.flags.x == 1u && alpha < material.extra.x {
            atomicAdd(&pt_counters[9], 1u); continue;
        }
        if material.flags.x == 2u {
            atomicAdd(&pt_counters[10], 1u);
            if pt_random(state) >= alpha { atomicAdd(&pt_counters[9], 1u); continue; }
        }
        rayQueryConfirmIntersection(&query);
    }
    let hit = rayQueryGetCommittedIntersection(&query);
    if hit.kind != RAY_QUERY_INTERSECTION_TRIANGLE { return PtHit(0u, 0u, 0u, 0.0, vec2<f32>(0.0)); }
    return PtHit(1u, hit.primitive_index, select(0u, 1u, hit.front_face), hit.t, hit.barycentrics);
}
fn pt_surface(hit: PtHit, origin: vec3<f32>, direction: vec3<f32>) -> PtSurface {
    let t = pt_triangles[hit.primitive];
    let m = pt_materials[u32(t.n[0].w)];
    let w = vec3<f32>(1.0 - hit.bary.x - hit.bary.y, hit.bary);
    let uv = pt_uv(t, hit.bary);
    let base_tex = pt_color_texture(m.tex.x, uv);
    let base = m.base_color.rgb*base_tex.rgb;
    let mr = pt_texture(m.tex.y, uv);
    let metal = clamp(m.surface.x*mr.b, 0.0, 1.0);
    let rough = clamp(m.surface.y*mr.g, 0.0, 1.0);
    let transmission = clamp(m.surface.z*pt_texture(m.flags.z, uv).r, 0.0, 1.0);
    if any(m.tex != vec4<u32>(PT_INVALID)) || m.flags.z != PT_INVALID { atomicAdd(&pt_counters[15], 1u); }
    var geometric = normalize(cross(t.p[1].xyz - t.p[0].xyz, t.p[2].xyz - t.p[0].xyz));
    if dot(geometric, direction) > 0.0 { geometric = -geometric; }
    var normal = normalize(t.n[0].xyz*w.x + t.n[1].xyz*w.y + t.n[2].xyz*w.z);
    if dot(normal, geometric) < 0.0 { normal = -normal; }
    var tangent = t.tangent[0].xyz*w.x + t.tangent[1].xyz*w.y + t.tangent[2].xyz*w.z;
    tangent -= normal*dot(normal, tangent);
    if dot(tangent, tangent) < 1e-12 {
        tangent = cross(select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), abs(normal.z) > 0.9), normal);
    }
    tangent = normalize(tangent);
    let handedness = select(-1.0, 1.0, t.tangent[0].w*w.x + t.tangent[1].w*w.y + t.tangent[2].w*w.z >= 0.0);
    if m.tex.z != PT_INVALID {
        var map = 2.0*pt_texture(m.tex.z, uv).rgb - vec3<f32>(1.0);
        map = normalize(vec3<f32>(map.xy*m.extra.y, map.z));
        let mapped = normalize(tangent*map.x + cross(normal, tangent)*(map.y*handedness) + normal*map.z);
        if dot(mapped, geometric) > 0.001 && dot(mapped, -direction) > 0.001 { normal = mapped; }
    }
    if dot(normal, -direction) <= 0.001 { normal = geometric; }
    tangent -= normal*dot(normal, tangent);
    tangent = normalize(tangent);
    let bitangent = cross(normal, tangent);
    let emission_tex = pt_color_texture(m.tex.w, uv).rgb;
    var emission = m.emission.rgb*emission_tex * pt_params.lighting_scale.x;
    if m.emission.w < 0.5 && hit.front_face == 0u { emission = vec3<f32>(0.0); }
    let ior = max(1.0, m.surface.w);
    let eta = select(1.0/ior, ior, hit.front_face != 0u);
    // Occlusion textures are deliberately not multiplied into physical path transport;
    // the acceleration geometry already determines visibility.
    return PtSurface(origin + direction*hit.distance, normal, geometric, tangent, bitangent, emission,
        PtBsdf(max(vec3<f32>(0.0), base), metal, rough, transmission, eta));
}
fn pt_local(s: PtSurface, w: vec3<f32>) -> vec3<f32> { return vec3<f32>(dot(w, s.tangent), dot(w, s.bitangent), dot(w, s.normal)); }
fn pt_world(s: PtSurface, w: vec3<f32>) -> vec3<f32> { return normalize(s.tangent*w.x + s.bitangent*w.y + s.normal*w.z); }
fn pt_offset(s: PtSurface, direction: vec3<f32>) -> vec3<f32> {
    return s.position + s.geometric*(pt_params.ray.x*2.0*select(-1.0, 1.0, dot(s.geometric, direction) >= 0.0));
}
fn pt_environment(w: vec3<f32>) -> vec3<f32> {
    return textureSampleLevel(pt_sky, pt_sky_sampler, w, 0.0).rgb*pt_params.lighting_scale.z;
}
fn pt_light_family_count() -> u32 {
    return select(0u, 1u, pt_params.limits.z > 0u) + select(0u, 1u, pt_params.limits.w > 0u)
        + select(0u, 1u, pt_params.lighting_scale.z > 0.0);
}
fn pt_power_heuristic(a: f32, b: f32) -> f32 {
    // Scale first to avoid PDF overflow at smooth microfacets/grazing emitters.
    let scale = max(a, b);
    if scale <= 0.0 { return 1.0; }
    let aa = a/scale; let bb = b/scale;
    return aa*aa/max(1e-30, aa*aa + bb*bb);
}
fn pt_emitter_probability(index: u32) -> f32 {
    var previous = 0.0;
    if index > 0u { previous = pt_emitters[index - 1u].power_cdf; }
    return max(0.0, pt_emitters[index].power_cdf - previous) / max(1e-20, pt_emitters[pt_params.limits.w - 1u].power_cdf);
}
fn pt_emitter_pdf(primitive: u32, previous_position: vec3<f32>, hit_position: vec3<f32>, direction: vec3<f32>) -> f32 {
    if pt_params.controls.x == 0u || pt_params.limits.w == 0u { return 0.0; }
    var index = PT_INVALID;
    // Host can encode emitter_index+1 in n[1].w, leaving zero on nonemissive triangles.
    let encoded = u32(pt_triangles[primitive].n[1].w);
    if encoded > 0u { index = encoded - 1u; }
    if index == PT_INVALID { return 0.0; }
    let t = pt_triangles[primitive];
    let m = pt_materials[u32(t.n[0].w)];
    let ng = normalize(cross(t.p[1].xyz - t.p[0].xyz, t.p[2].xyz - t.p[0].xyz));
    let cosine = select(max(0.0, dot(ng, -direction)), abs(dot(ng, -direction)), m.emission.w > 0.5);
    if cosine <= 1e-12 { return 0.0; }
    let delta = hit_position - previous_position;
    return pt_emitter_probability(index)*dot(delta, delta)/(pt_emitters[index].area*cosine*f32(pt_light_family_count()));
}
fn pt_light_sample(s: PtSurface, state: ptr<function, u32>) -> PtLightSample {
    let none = PtLightSample(vec3<f32>(0.0), vec3<f32>(0.0), 0.0, 0.0, 0u);
    let count = pt_light_family_count();
    if count == 0u { return none; }
    var family = min(count - 1u, u32(pt_random(state)*f32(count)));
    let family_pdf = 1.0/f32(count);
    if pt_params.limits.z > 0u {
        if family == 0u {
            let index = min(pt_params.limits.z - 1u, u32(pt_random(state)*f32(pt_params.limits.z)));
            let light = pt_lights[index];
            let kind = u32(light.position_kind.w);
            var direction = -normalize(light.direction_range.xyz);
            var distance = pt_params.ray.y;
            var attenuation = 1.0;
            if kind != 0u {
                let delta = light.position_kind.xyz - s.position;
                let distance2 = dot(delta, delta);
                if distance2 < pt_params.ray.x*pt_params.ray.x { return none; }
                distance = sqrt(distance2); direction = delta/distance; attenuation = 1.0/distance2;
                if light.direction_range.w > 0.0 {
                    let ratio = distance/light.direction_range.w;
                    attenuation *= pow(clamp(1.0 - ratio*ratio*ratio*ratio, 0.0, 1.0), 2.0);
                }
                if kind == 2u {
                    let cosine = dot(normalize(light.direction_range.xyz), -direction);
                    let ramp = clamp((cosine - light.cone.y)/max(1e-6, light.cone.x - light.cone.y), 0.0, 1.0);
                    attenuation *= ramp*ramp;
                }
            }
            let radiance = light.color_intensity.rgb*light.color_intensity.w*attenuation*pt_params.lighting_scale.y;
            return PtLightSample(direction, radiance, distance, family_pdf/f32(pt_params.limits.z), 1u | select(0u, 2u, light.cone.z == 0.0));
        }
        family -= 1u;
    }
    if pt_params.limits.w > 0u {
        if family == 0u {
            let cdf_draw = pt_random(state)*pt_emitters[pt_params.limits.w - 1u].power_cdf;
            var low = 0u; var high = pt_params.limits.w - 1u;
            while low < high {
                let middle = (low + high)/2u;
                if cdf_draw < pt_emitters[middle].power_cdf { high = middle; } else { low = middle + 1u; }
            }
            let emitter = pt_emitters[low];
            let t = pt_triangles[emitter.triangle];
            let m = pt_materials[u32(t.n[0].w)];
            let root = sqrt(pt_random(state)); let v = pt_random(state);
            let bary = vec2<f32>(root*(1.0 - v), root*v);
            let point = t.p[0].xyz*(1.0 - bary.x - bary.y) + t.p[1].xyz*bary.x + t.p[2].xyz*bary.y;
            let delta = point - s.position;
            let distance2 = dot(delta, delta);
            if distance2 < 16.0*pt_params.ray.x*pt_params.ray.x { return none; }
            let distance = sqrt(distance2); let direction = delta/distance;
            let ng = normalize(cross(t.p[1].xyz - t.p[0].xyz, t.p[2].xyz - t.p[0].xyz));
            let cosine = select(max(0.0, dot(ng, -direction)), abs(dot(ng, -direction)), m.emission.w > 0.5);
            if cosine <= 1e-12 { return none; }
            let uv = pt_uv(t, bary);
            let tex = pt_color_texture(m.tex.w, uv).rgb;
            var radiance = m.emission.rgb*tex*pt_params.lighting_scale.x;
            let alpha = clamp(m.base_color.a*pt_texture(m.tex.x, uv).a, 0.0, 1.0);
            if m.flags.x == 1u && alpha < m.extra.x { radiance = vec3<f32>(0.0); }
            if m.flags.x == 2u { radiance *= alpha; }
            let pdf = family_pdf*pt_emitter_probability(low)*distance2/(emitter.area*cosine);
            return PtLightSample(direction, radiance, distance, pdf, 0u);
        }
        family -= 1u;
    }
    let z = 1.0 - 2.0*pt_random(state);
    let phi = 2.0*PT_PI*pt_random(state);
    let radius = sqrt(max(0.0, 1.0 - z*z));
    let direction = vec3<f32>(radius*cos(phi), radius*sin(phi), z);
    return PtLightSample(direction, pt_environment(direction), pt_params.ray.y, family_pdf/(4.0*PT_PI), 0u);
}
fn pt_direct(s: PtSurface, wo: vec3<f32>, continuation: bool, state: ptr<function, u32>) -> vec3<f32> {
    if pt_params.controls.x == 0u { return vec3<f32>(0.0); }
    atomicAdd(&pt_counters[8], 1u);
    let light = pt_light_sample(s, state);
    if light.pdf <= 0.0 || max(light.radiance.x, max(light.radiance.y, light.radiance.z)) <= 0.0 { return vec3<f32>(0.0); }
    let wi = pt_local(s, light.direction);
    // Keep reflection/transmission on their geometric side even with a normal map.
    if wi.z*dot(s.geometric, light.direction) <= 0.0 { return vec3<f32>(0.0); }
    let bsdf = pt_bsdf_eval(s.bsdf, wo, wi);
    if bsdf.pdf <= 0.0 { return vec3<f32>(0.0); }
    let maximum = max(pt_params.ray.x*2.0, light.distance - pt_params.ray.x*4.0);
    if (light.delta & 2u) == 0u {
        let blocker = pt_trace(pt_offset(s, light.direction), light.direction, maximum, true, state);
        if blocker.valid != 0u { return vec3<f32>(0.0); }
    }
    let mis = select(pt_power_heuristic(light.pdf, bsdf.pdf), 1.0, light.delta != 0u || !continuation);
    return bsdf.value*light.radiance*(mis/light.pdf);
}
fn pt_path(origin: vec3<f32>, direction: vec3<f32>, state: ptr<function, u32>, train_path: bool, record_training: bool) -> vec3<f32> {
    var ray_origin = origin; var ray_direction = direction;
    var throughput = vec3<f32>(1.0); var radiance = vec3<f32>(0.0);
    var previous_pdf = 0.0; var previous_delta = true; var previous_position = origin;
    var eta_scale = 1.0;
    // A separate local throughput avoids dividing by the camera throughput's zero channels.
    // Training paths never query the network, so the model never trains on its own predictions.
    var tail_active = false;
    var tail_beta = vec3<f32>(1.0);
    var tail_radiance = vec3<f32>(0.0);
    var training_record: NrcRecord;
    atomicAdd(&pt_counters[0], 1u);
    for (var depth = 0u; depth < pt_params.limits.x; depth++) {
        let hit = pt_trace(ray_origin, ray_direction, pt_params.ray.y, false, state);
        if hit.valid == 0u {
            atomicAdd(&pt_counters[7], 1u);
            var weight = 1.0;
            if depth > 0u && !previous_delta && pt_params.controls.x != 0u && pt_params.lighting_scale.z > 0.0 {
                weight = pt_power_heuristic(previous_pdf, 1.0/(4.0*PT_PI*f32(pt_light_family_count())));
            }
            let escaped = pt_environment(ray_direction)*weight;
            radiance += throughput*escaped;
            if tail_active { tail_radiance += tail_beta*escaped; }
            break;
        }
        atomicAdd(&pt_counters[3], 1u);
        let s = pt_surface(hit, ray_origin, ray_direction);
        let wo = pt_local(s, -ray_direction);
        if any(s.emission > vec3<f32>(0.0)) {
            atomicAdd(&pt_counters[6], 1u);
            var weight = 1.0;
            if depth > 0u && !previous_delta {
                weight = pt_power_heuristic(previous_pdf, pt_emitter_pdf(hit.primitive, previous_position, s.position, ray_direction));
            }
            let emitted = s.emission*weight;
            radiance += throughput*emitted;
            if tail_active { tail_radiance += tail_beta*emitted; }
        }
        let eligible = depth >= max(1u, nrc_params.query.x) && s.bsdf.roughness >= 0.2 && s.bsdf.transmission <= 0.001;
        // Own emission was already evaluated. The target and prediction both represent only
        // outgoing reflected light, including this vertex's direct NEE and all later path terms.
        if NRC_ENABLED && record_training && eligible && !tail_active {
            tail_active = true;
            training_record = nrc_feature_record(s.position, s.normal, -ray_direction, s.bsdf.base,
                s.bsdf.roughness, s.bsdf.metallic, vec3<f32>(0.0));
        }
        if NRC_ENABLED && !train_path && nrc_query_allowed(depth, s.bsdf.roughness, s.bsdf.transmission) {
            let record = nrc_feature_record(s.position, s.normal, -ray_direction, s.bsdf.base,
                s.bsdf.roughness, s.bsdf.metallic, vec3<f32>(0.0));
            radiance += throughput*nrc_infer(record);
            break;
        }
        let continuation = depth + 1u < pt_params.limits.x;
        let direct = pt_direct(s, wo, continuation, state);
        radiance += throughput*direct;
        if tail_active { tail_radiance += tail_beta*direct; }
        if !continuation { break; }
        let u = vec4<f32>(pt_random(state), pt_random(state), pt_random(state), pt_random(state));
        let sample = pt_bsdf_sample(s.bsdf, wo, u);
        if sample.pdf <= 0.0 {
            atomicAdd(&pt_counters[5], 1u); break;
        }
        let wi = pt_world(s, sample.direction);
        if sample.direction.z*dot(wi, s.geometric) <= 0.0 {
            atomicAdd(&pt_counters[5], 1u); break;
        }
        if sample.direction.z < 0.0 { atomicAdd(&pt_counters[13], 1u); }
        if s.bsdf.metallic > 0.5 { atomicAdd(&pt_counters[14], 1u); }
        throughput *= sample.weight;
        if tail_active { tail_beta *= sample.weight; }
        eta_scale *= sample.eta_scale;
        if any(throughput != throughput) || any(throughput > vec3<f32>(1e30)) {
            atomicAdd(&pt_counters[12], 1u); break;
        }
        previous_pdf = sample.pdf; previous_delta = sample.delta != 0u; previous_position = s.position;
        ray_origin = pt_offset(s, wi); ray_direction = wi;
        if pt_params.controls.y != 0u && depth + 1u >= pt_params.limits.y {
            let rr_beta = throughput*eta_scale;
            let survival = clamp(max(rr_beta.x, max(rr_beta.y, rr_beta.z)), 0.05, 0.95);
            if pt_random(state) >= survival { atomicAdd(&pt_counters[4], 1u); break; }
            throughput /= survival;
            if tail_active { tail_beta /= survival; }
        }
    }
    if tail_active {
        training_record.label = vec4<f32>(tail_radiance, 0.0);
        nrc_record(training_record);
    }
    return radiance*pt_params.lighting_scale.w;
}
override PT_WORKGROUP: u32 = 64u;
@compute @workgroup_size(PT_WORKGROUP)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.x;
    if pixel >= pt_params.image.x*pt_params.image.y { return; }
    let coordinates = vec2<u32>(pixel % pt_params.image.x, pixel / pt_params.image.x);
    var state = pt_hash(pixel ^ pt_hash(pt_params.image.w + pt_params.controls.z*1664525u));
    if state == 0u { state = 1u; }
    // Rotate an evenly spaced subset through a bijective pixel rotation each frame. Exactly
    // batch pixels are designated (at most one record each), and each pixel is equally likely.
    let pixel_count = pt_params.image.x*pt_params.image.y;
    let batch = min(pixel_count, max(1u, nrc_params.shape.x));
    let stride = max(1u, pixel_count/batch);
    let rotated = (pixel + pt_hash(pt_params.image.w + pt_params.controls.z)) % pixel_count;
    let training_pixel = NRC_ENABLED && nrc_train_enabled() && rotated % stride == 0u && rotated/stride < batch;
    var color = vec3<f32>(0.0);
    for (var sample = 0u; sample < pt_params.image.z; sample++) {
        let jitter = vec2<f32>(pt_random(&state), pt_random(&state));
        let uv = (vec2<f32>(coordinates) + jitter)/vec2<f32>(pt_params.image.xy);
        let ndc = vec2<f32>(2.0*uv.x - 1.0, 1.0 - 2.0*uv.y);
        let direction = normalize(pt_params.camera_forward.xyz + pt_params.camera_right.xyz*(ndc.x*pt_params.camera_right.w)
            + pt_params.camera_up.xyz*(ndc.y*pt_params.camera_up.w));
        color += pt_path(pt_params.camera_position.xyz, direction, &state, training_pixel, training_pixel && sample == 0u);
    }
    color /= f32(pt_params.image.z);
    if any(color != color) || any(color > vec3<f32>(1e30)) { atomicAdd(&pt_counters[12], 1u); color = vec3<f32>(0.0); }
    pt_image[pixel] += vec4<f32>(color, 1.0);
}
