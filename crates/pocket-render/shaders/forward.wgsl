// The opaque forward pass: physically based shading (GGX, Smith, Schlick) lit by the sun with
// cascaded shadows, punctual lights from the clustered light grid, and image-based light from the
// sky (prefiltered radiance cube for specular, spherical harmonics for diffuse), with height fog.
// Vertices are pulled from the shared mesh buffers; the instance comes from the culling pass's
// visible list.

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> instances: array<Instance>;
@group(0) @binding(2) var<storage, read> visible: array<u32>;
@group(0) @binding(3) var<storage, read> materials: array<Material>;
@group(0) @binding(4) var<storage, read> drawn: array<Drawn>;

@group(1) @binding(0) var shadow_map: texture_depth_2d_array;
@group(1) @binding(1) var shadow_sampler: sampler_comparison;
@group(1) @binding(2) var env_cube: texture_cube<f32>;
@group(1) @binding(3) var env_sampler: sampler;
@group(1) @binding(4) var<uniform> sh: array<vec4f, 9>;
@group(1) @binding(5) var<storage, read> lights: array<Light>;
@group(1) @binding(6) var<uniform> clusters: Clusters;
@group(1) @binding(7) var<storage, read> cluster_lights: array<u32>;   // per cell: count, then indices

@group(2) @binding(0) var tex_srgb: texture_2d_array<f32>;
@group(2) @binding(1) var tex_linear: texture_2d_array<f32>;
@group(2) @binding(2) var tex_sampler: sampler;

// The draw batch's base in the visible lists (batches.rs), added to `instance_index`: 0 when the
// base is the draw's `first_instance`; the base itself on WebGPU's baseline path, where
// `first_instance` must be 0. (Binding 0 of group 3 is the ocean's, in its own pipeline.)
@group(3) @binding(1) var<uniform> batch: vec4u;

struct VsIn {
    @location(0) position: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
    @location(3) tangent: vec4f,
};

struct VsOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
    @location(3) tangent: vec4f,
    @location(4) @interpolate(flat) material: u32,
};

@vertex
fn vs(v: VsIn, @builtin(instance_index) ii: u32) -> VsOut {
    let d = drawn[ii + batch.x];
    let world = d.pos + quat_rotate(d.rot, v.position * d.scale);
    // Normals under non-uniform scale: scale by the inverse, then rotate.
    let n = normalize(quat_rotate(d.rot, v.normal / d.scale));
    let t = normalize(quat_rotate(d.rot, v.tangent.xyz * d.scale));
    var o: VsOut;
    o.clip = view.view_proj * vec4f(world, 1.0);
    o.world = world;
    o.normal = n;
    o.uv = v.uv;
    o.tangent = vec4f(t, v.tangent.w);
    o.material = d.material;
    return o;
}

// --- Shading ----------------------------------------------------------------------------------

fn d_ggx(nh: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = nh * nh * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn v_smith(nv: f32, nl: f32, a: f32) -> f32 {
    let a2 = a * a;
    let gv = nl * sqrt(nv * nv * (1.0 - a2) + a2);
    let gl = nv * sqrt(nl * nl * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-5);
}

fn f_schlick(f0: vec3f, vh: f32) -> vec3f {
    return f0 + (1.0 - f0) * pow(1.0 - vh, 5.0);
}

// Karis' analytic approximation of the split-sum environment BRDF (no lookup table).
fn env_brdf(f0: vec3f, roughness: f32, nv: f32) -> vec3f {
    let c0 = vec4f(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4f(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * nv)) * r.x + r.y;
    let ab = vec2f(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

fn sh_irradiance(n: vec3f) -> vec3f {
    let c1 = 0.429043; let c2 = 0.511664; let c3 = 0.743125; let c4 = 0.886227; let c5 = 0.247708;
    return max(vec3f(0.0),
        c1 * sh[8].xyz * (n.x * n.x - n.y * n.y) + c3 * sh[6].xyz * n.z * n.z + c4 * sh[0].xyz
        - c5 * sh[6].xyz + 2.0 * c1 * (sh[4].xyz * n.x * n.y + sh[7].xyz * n.x * n.z + sh[5].xyz * n.y * n.z)
        + 2.0 * c2 * (sh[3].xyz * n.x + sh[1].xyz * n.y + sh[2].xyz * n.z));
}

fn shadow_factor(world: vec3f, n: vec3f, view_depth: f32) -> f32 {
    if (view.sky_color.w < 0.5) {
        return 1.0;
    }
    var c = 0u;
    for (var i = 0u; i < 4u; i++) {
        if (view_depth > view.cascade_splits[i]) {
            c = i + 1u;
        }
    }
    if (c >= 4u) {
        return 1.0;
    }
    // Normal offset scaled by the cascade's texel size keeps acne away without peter-panning.
    let texel = view.cascade_splits[c] / 1024.0;
    let p = view.cascades[c] * vec4f(world + n * texel * 1.5, 1.0);
    let uv = p.xy * vec2f(0.5, -0.5) + 0.5;
    if (any(uv < vec2f(0.0)) || any(uv > vec2f(1.0)) || p.z > 1.0) {
        return 1.0;
    }
    let dim = vec2f(textureDimensions(shadow_map));
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2f(f32(x), f32(y)) / dim;
            sum += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + o, c, p.z - 0.0005);
        }
    }
    let s = sum / 9.0;
    // Fade the last cascade out at its far end.
    let fade = clamp((view.cascade_splits[3] - view_depth) / (view.cascade_splits[3] * 0.1), 0.0, 1.0);
    return mix(1.0, s, select(1.0, fade, c == 3u));
}

fn cluster_of(frag: vec4f) -> u32 {
    let g = clusters.grid;
    let x = min(u32(frag.x * view.viewport.z * f32(g.x)), g.x - 1u);
    let y = min(u32(frag.y * view.viewport.w * f32(g.y)), g.y - 1u);
    // Reversed infinite Z: view depth = near / ndc z.
    let depth = clusters.params.x / max(frag.z, 1e-7);
    let zf = log(max(depth, clusters.params.x) / clusters.params.x) * clusters.params.y;
    let z = min(u32(max(zf, 0.0)), g.z - 1u);
    return (z * g.y + y) * g.x + x;
}

struct Surface {
    albedo: vec3f,
    alpha: f32,
    metallic: f32,
    roughness: f32,
    n: vec3f,
    emissive: vec3f,
};

fn surface(in: VsOut, facing: bool) -> Surface {
    let m = materials[in.material];
    // Explicit gradients: the material (and so whether a texture exists) varies per pixel, and
    // implicit derivatives need uniform control flow.
    let du = dpdx(in.uv);
    let dv = dpdy(in.uv);
    var s: Surface;
    var base = m.base_color;
    if (m.base_color_tex != NO_TEXTURE) {
        base *= textureSampleGrad(tex_srgb, tex_sampler, in.uv, m.base_color_tex, du, dv);
    }
    s.albedo = base.rgb;
    s.alpha = base.a;
    s.metallic = m.metallic;
    s.roughness = m.roughness;
    if (m.metal_rough_tex != NO_TEXTURE) {
        let mr = textureSampleGrad(tex_linear, tex_sampler, in.uv, m.metal_rough_tex, du, dv);
        s.roughness *= mr.g;
        s.metallic *= mr.b;
    }
    var n = normalize(in.normal);
    let view_direction = normalize(view.camera_pos.xyz - in.world);
    var geometric = cross(dpdx(in.world), dpdy(in.world));
    if dot(geometric, geometric) > 1e-12 {
        geometric = normalize(geometric);
        if dot(geometric, view_direction) < 0.0 { geometric = -geometric; }
    } else {
        geometric = select(-n, n, facing);
    }
    if (!facing) { n = -n; }
    if dot(n, geometric) < 0.0 { n = -n; }
    if dot(n, view_direction) <= 0.001 { n = geometric; }
    if (m.normal_tex != NO_TEXTURE) {
        let t = normalize(in.tangent.xyz - n * dot(n, in.tangent.xyz));
        let b = cross(n, t) * in.tangent.w;
        let tn = textureSampleGrad(tex_linear, tex_sampler, in.uv, m.normal_tex, du, dv).xyz * 2.0 - 1.0;
        let candidate = t * tn.x + b * tn.y + n * tn.z;
        if dot(candidate, candidate) > 1e-12 {
            let mapped = normalize(candidate);
            // Match the path tracer: a malformed/inward normal map must not invert transport.
            if dot(mapped, geometric) > 0.001 && dot(mapped, view_direction) > 0.001 { n = mapped; }
        }
    }
    s.n = n;
    s.emissive = m.emissive;
    if (m.emissive_tex != NO_TEXTURE) {
        s.emissive *= textureSampleGrad(tex_srgb, tex_sampler, in.uv, m.emissive_tex, du, dv).rgb;
    }
    // Specular anti-aliasing (Kaplanyan-Hoffman): widen roughness where the normal varies fast.
    let dn = fwidth(n);
    let variance = 0.25 * dot(dn, dn);
    let a2 = s.roughness * s.roughness;
    s.roughness = sqrt(sqrt(clamp(a2 * a2 + min(2.0 * variance, 0.18), 0.0, 1.0)));
    s.roughness = clamp(s.roughness, 0.045, 1.0);
    return s;
}

fn apply_fog(c: vec3f, world: vec3f) -> vec3f {
    let density = view.fog.w;
    if (density <= 0.0) {
        return c;
    }
    let d = distance(world, view.camera_pos.xyz);
    // Exponential height fog: thinner with altitude.
    let h = max(world.y, 0.0);
    let f = 1.0 - exp(-density * d * exp(-h * 0.02));
    return mix(c, view.fog.rgb * view.sun_color.w, clamp(f, 0.0, 1.0));
}

@fragment
fn fs(in: VsOut, @builtin(front_facing) facing: bool) -> @location(0) vec4f {
    return shade(in, surface(in, facing));
}

// Alpha-tested materials only: the one entry point with `discard`.
@fragment
fn fs_masked(in: VsOut, @builtin(front_facing) facing: bool) -> @location(0) vec4f {
    let s = surface(in, facing);
    if (s.alpha < materials[in.material].alpha_cutoff) {
        discard;
    }
    return shade(in, s);
}

fn shade(in: VsOut, s: Surface) -> vec4f {
    let frag = in.clip;
    let m = materials[in.material];
    if ((m.flags & 4u) != 0u) {
        return vec4f(s.albedo + s.emissive, 1.0);
    }
    let v = normalize(view.camera_pos.xyz - in.world);
    let n = s.n;
    let nv = max(dot(n, v), 1e-4);
    let a = s.roughness * s.roughness;
    let f0 = mix(vec3f(0.04), s.albedo, s.metallic);
    let diffuse_color = s.albedo * (1.0 - s.metallic);
    var color = vec3f(0.0);

    // Sun.
    if (view.sun_dir.w > 0.0) {
        let l = view.sun_dir.xyz;
        let nl = dot(n, l);
        if (nl > 0.0) {
            let h = normalize(l + v);
            let nh = max(dot(n, h), 0.0);
            let vh = max(dot(v, h), 0.0);
            let f = f_schlick(f0, vh);
            let spec = d_ggx(nh, a) * v_smith(nv, nl, a) * f;
            let diff = (1.0 - f) * diffuse_color / PI;
            let depth = dot(in.world - view.camera_pos.xyz, -vec3f(view.view[0].z, view.view[1].z, view.view[2].z));
            let sh = shadow_factor(in.world, n, depth);
            color += (diff + spec) * view.sun_color.rgb * view.sun_dir.w * nl * sh;
        }
    }

    // Punctual lights of this pixel's cluster.
    let cell = cluster_of(frag);
    let per = clusters.grid.w;
    let count = min(cluster_lights[cell * (per + 1u)], per);
    for (var i = 0u; i < count; i++) {
        let li = lights[cluster_lights[cell * (per + 1u) + 1u + i]];
        let to = li.pos - in.world;
        let d2 = dot(to, to);
        let l = to * inverseSqrt(max(d2, 1e-8));
        let nl = dot(n, l);
        if (nl <= 0.0) {
            continue;
        }
        // Inverse square with a smooth window to zero at the range.
        let x = d2 / (li.range * li.range);
        let window = clamp(1.0 - x * x, 0.0, 1.0);
        var att = window * window / max(d2, 0.01);
        if (li.kind == 2u) {
            let cd = dot(-l, li.dir);
            att *= smoothstep(li.cos_outer, li.cos_inner, cd);
        }
        let h = normalize(l + v);
        let f = f_schlick(f0, max(dot(v, h), 0.0));
        let spec = d_ggx(max(dot(n, h), 0.0), a) * v_smith(nv, nl, a) * f;
        color += ((1.0 - f) * diffuse_color / PI + spec) * li.color * att * nl;
    }

    // Image-based light from the sky.
    let ambient = view.sun_color.w;
    let r = reflect(-v, n);
    let levels = f32(textureNumLevels(env_cube) - 1);
    let prefiltered = textureSampleLevel(env_cube, env_sampler, r, s.roughness * levels).rgb;
    let brdf = env_brdf(f0, s.roughness, nv);
    // Horizon occlusion: reflections pointing below the surface fade.
    let horizon = clamp(1.0 + dot(r, normalize(in.normal)), 0.0, 1.0);
    let baked = baked_diffuse(in.world, n);
    // sh_irradiance returns E; both diffuse inputs need E / PI before multiplying albedo.
    let diffuse_light = select(sh_irradiance(n) * (ambient / PI), baked.xyz, baked.w > 0.0);
    color += diffuse_color * diffuse_light + prefiltered * brdf * horizon * horizon * ambient;

    color += s.emissive;
    color = apply_fog(color, in.world);
    return vec4f(color, 1.0);
}

// --- Shadow casters ----------------------------------------------------------------------------

struct ShadowOut {
    @builtin(position) clip: vec4f,
};

@vertex
fn vs_shadow(v: VsIn, @builtin(instance_index) ii: u32) -> ShadowOut {
    let i = ii + batch.x;
    let inst = instances[visible[i]];
    let pose = instance_pose(inst, view.params.x);
    let world = pose.pos + quat_rotate(pose.rot, v.position * inst.scale);
    var o: ShadowOut;
    o.clip = view.cascades[min(i / view.counts.x, 4u) - 1u] * vec4f(world, 1.0);
    return o;
}

struct MaskedShadowOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) @interpolate(flat) material: u32,
};

@vertex
fn vs_shadow_masked(v: VsIn, @builtin(instance_index) ii: u32) -> MaskedShadowOut {
    let i = ii + batch.x;
    let inst = instances[visible[i]];
    let pose = instance_pose(inst, view.params.x);
    let world = pose.pos + quat_rotate(pose.rot, v.position * inst.scale);
    var o: MaskedShadowOut;
    o.clip = view.cascades[min(i / view.counts.x, 4u) - 1u] * vec4f(world, 1.0);
    o.uv = v.uv;
    o.material = inst.material;
    return o;
}

@fragment
fn fs_shadow_masked(in: MaskedShadowOut) {
    let m = materials[in.material];
    var a = m.base_color.a;
    if (m.base_color_tex != NO_TEXTURE) {
        a *= textureSampleLevel(tex_srgb, tex_sampler, in.uv, m.base_color_tex, 0.0).a;
    }
    if (a < m.alpha_cutoff) {
        discard;
    }
}

// --- Entity ids (picking, and what an agent asks is at a pixel) --------------------------------

struct IdOut {
    @builtin(position) clip: vec4f,
    @location(0) @interpolate(flat) slot: u32,
};

@vertex
fn vs_id(v: VsIn, @builtin(instance_index) ii: u32) -> IdOut {
    let d = drawn[ii + batch.x];
    let world = d.pos + quat_rotate(d.rot, v.position * d.scale);
    var o: IdOut;
    o.clip = view.view_proj * vec4f(world, 1.0);
    o.slot = d.slot;
    return o;
}

@fragment
fn fs_id(in: IdOut) -> @location(0) u32 {
    return in.slot + 1u;
}

// --- The ocean ------------------------------------------------------------------------------------
// The sea pocket-physics floats boats on: the same sine waves (height a sin(k d.p - w t + phase),
// deep-water w = sqrt(g k)), on a camera-centred ring grid that reaches the horizon, with smaller
// wavelets in the normals only, so the geometry boats sit on is exactly the simulation's.

struct Ocean {
    level: f32,
    count: u32,
    time: f32,
    _p: f32,
    waves: array<vec4f, 16>,     // dir.x, dir.z, k, amplitude
    phases: array<vec4f, 16>,    // omega, phase
};

@group(3) @binding(0) var<uniform> ocean: Ocean;

fn ocean_height(p: vec2f, t: f32) -> vec3f {
    // Height and its gradient (d/dx, d/dz).
    var h = ocean.level;
    var g = vec2f(0.0);
    for (var i = 0u; i < ocean.count; i++) {
        let w = ocean.waves[i];
        let ph = ocean.phases[i];
        let th = w.z * dot(w.xy, p) - ph.x * t + ph.y;
        h += w.w * sin(th);
        g += w.w * cos(th) * w.z * w.xy;
    }
    return vec3f(h, g);
}

struct OceanOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) grad: vec2f,
};

@vertex
fn vs_ocean(@location(0) ring: vec2f) -> OceanOut {
    // `ring`: a point of the unit ring grid (radius grows geometrically outward); centred under the
    // camera, snapped to a coarse step so the grid does not swim.
    let cam = view.camera_pos.xz;
    let snap = 4.0;
    let centre = floor(cam / snap) * snap;
    let p = centre + ring;
    let s = ocean_height(p, ocean.time);
    var o: OceanOut;
    let world = vec3f(p.x, s.x, p.y);
    o.clip = view.view_proj * vec4f(world, 1.0);
    o.world = world;
    o.grad = s.yz;
    return o;
}

// Wavelets: a fixed set of short waves in many directions, in the normals only. Each fades out
// before its wavelength drops under four pixels (`footprint`: metres per pixel), so distant water
// does not alias into moire and sparkle.
fn wavelets(p: vec2f, t: f32, footprint: f32) -> vec2f {
    var g = vec2f(0.0);
    var amp = 0.05;
    var len = 3.1;
    for (var i = 0u; i < 14u; i++) {
        let a = f32(i) * 2.399963 + 0.3;  // golden angle
        let d = vec2f(cos(a), sin(a));
        let k = 6.2831853 / len;
        let phase = unit_from_hash(pcg_hash(i * 2654435769u + 7u)) * 6.28;
        let th = k * dot(d, p) - sqrt(9.81 * k) * t + phase;
        let keep = clamp(len / (footprint * 4.0) - 1.0, 0.0, 1.0);
        g += amp * k * cos(th) * d * keep;
        amp *= 0.8;
        len *= 0.74;
    }
    return g;
}

@fragment
fn fs_ocean(in: OceanOut) -> @location(0) vec4f {
    let v_world = view.camera_pos.xyz - in.world;
    let dist = length(v_world);
    let v = v_world / dist;
    let fade = clamp(1.0 - dist / 600.0, 0.0, 1.0);
    let footprint = max(length(fwidth(in.world.xz)), 1e-3);
    let g = in.grad + wavelets(in.world.xz, ocean.time, footprint);
    let n = normalize(vec3f(-g.x, 1.0, -g.y));
    let nv = max(dot(n, v), 1e-4);
    // Water: F0 0.02; slightly rough so the sun's glitter spreads with distance.
    let rough = clamp(0.05 + footprint * 0.04, 0.05, 0.3);
    let a = rough * rough;
    let f = 0.02 + 0.98 * pow(1.0 - nv, 5.0);
    let r = reflect(-v, n);
    let ry = vec3f(r.x, max(r.y, 0.02), r.z);
    let levels = f32(textureNumLevels(env_cube) - 1);
    let sky = textureSampleLevel(env_cube, env_sampler, ry, rough * levels).rgb * view.sun_color.w;
    // The water body: deep blue-green, lit by the sky and the sun.
    let sun = view.sun_dir.xyz;
    let sun_lux = view.sun_dir.w * view.sun_color.rgb;
    let deep = vec3f(0.01, 0.05, 0.07);
    let shallow_tint = vec3f(0.02, 0.12, 0.13);
    let body_light = sh_irradiance(vec3f(0.0, 1.0, 0.0)) * 0.08 + sun_lux * max(sun.y, 0.0) * 0.012;
    // Light through the crests facing away from the sun (subsurface).
    let crest = clamp((in.world.y - ocean.level) * 1.5, 0.0, 1.0);
    let sss = pow(max(dot(v, -sun), 0.0), 4.0) * crest * 0.08;
    var body = mix(deep, shallow_tint, crest * 0.6) * (body_light + sun_lux * sss);
    // Sun glints.
    var spec = vec3f(0.0);
    if (view.sun_dir.w > 0.0) {
        let h = normalize(sun + v);
        let nl = max(dot(n, sun), 0.0);
        spec = d_ggx(max(dot(n, h), 0.0), a) * v_smith(nv, nl, a) * f_schlick(vec3f(0.02), max(dot(v, h), 0.0))
            * sun_lux * nl * shadow_factor(in.world, n, dot(in.world - view.camera_pos.xyz,
                -vec3f(view.view[0].z, view.view[1].z, view.view[2].z)));
    }
    // Foam where the swell crests steeply.
    let steep = length(in.grad);
    let foam = smoothstep(0.32, 0.6, steep) * crest * fade;
    var c = mix(body, sky, f) + spec;
    c = mix(c, vec3f(0.8) * (body_light * 6.0 + sun_lux * max(sun.y, 0.0) * 0.25), foam * 0.6);
    c = apply_fog(c, in.world);
    return vec4f(c, 1.0);
}
