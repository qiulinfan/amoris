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
// Skinned parts' vertices as last frame drew them (skinning.rs `History`): the table's length in
// meshes, per mesh two words (the part's first vertex in the pool, or NO_TEXTURE when it has no
// history, and where its copy starts), then the copies, 12 words a vertex. Vertex stage only
// (TAA's object motion).
@group(0) @binding(5) var<storage, read> skin_prev: array<u32>;
// One bit per instance slot: moved this tick or skinned (renderer.rs `moving_bits`); only those
// read last frame's pose. Vertex stage only.
@group(0) @binding(6) var<storage, read> moving: array<u32>;

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

// The clip position is `@invariant` here and in the depth-only stages below (`DepthOut`): the depth
// prepass draws a depth that the forward pipelines then test for equality (renderer.rs
// `DepthPrepass`, docs/spec/prepass.md), so both must compute it identically. naga writes it as
// HLSL's `precise`, SPIR-V's `Invariant` decoration and MSL's `[[invariant]]`; the expression must
// stay the same in every one of them (the instance's `Drawn` pose, then `view.view_proj`).
struct VsOut {
    @builtin(position) @invariant clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
    @location(3) tangent: vec4f,
    @location(4) @interpolate(flat) material: u32,
    // Where this point was last frame (TAA's object motion; `world` when nothing moved).
    @location(5) prev_world: vec3f,
};

// A vertex's local position as last frame drew it: a skinned part's from its copy (skin_prev).
fn prev_local(position: vec3f, vertex: u32, mesh: u32) -> vec3f {
    if (mesh >= skin_prev[0]) {
        return position;
    }
    let first = skin_prev[1u + mesh * 2u];
    if (first == NO_TEXTURE || vertex < first) {
        return position;
    }
    let k = skin_prev[2u + mesh * 2u] + (vertex - first) * 12u;
    if (k + 2u >= arrayLength(&skin_prev)) {
        return position;
    }
    return vec3f(bitcast<f32>(skin_prev[k]), bitcast<f32>(skin_prev[k + 1u]),
        bitcast<f32>(skin_prev[k + 2u]));
}

@vertex
fn vs(v: VsIn, @builtin(instance_index) ii: u32, @builtin(vertex_index) vi: u32) -> VsOut {
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
    o.prev_world = world;
    // Only a pipeline with the motion target reads last frame's pose (`SCENE_MOTION`, below).
    if (SCENE_MOTION) {
        let word = d.slot >> 5u;
        let moved = word < arrayLength(&moving) && ((moving[word] >> (d.slot & 31u)) & 1u) != 0u;
        if (view.motion.y > 0.5 && moved) {
            // Last frame's pose: the instance's two tick poses at last frame's alpha.
            let inst = instances[d.slot];
            let pose = instance_pose(inst, view.motion.x);
            o.prev_world = pose.pos + quat_rotate(pose.rot, prev_local(v.position, vi, inst.mesh) * d.scale);
        }
    }
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
    // A point behind the eye is no receiver: with MSAA a partly covered pixel shades at its centre,
    // `world` extrapolated along the triangle's plane, which can land behind the camera. The
    // cascades' slice culling (shadows.rs `slice_planes`) covers view depths from 0 on.
    if (view.sky_color.w < 0.5 || view_depth < 0.0) {
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
    // Ambient occlusion of the sky's light (1 for materials without it; neural textures carry it).
    occlusion: f32,
};

// The shading normal: the interpolated normal facing the viewer, perturbed by the tangent-space
// normal `tn` when `mapped` (a malformed or inward map is ignored, as in the path tracer).
fn shading_normal(in: VsOut, facing: bool, tn: vec3f, mapped: bool) -> vec3f {
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
    if (mapped) {
        let t = normalize(in.tangent.xyz - n * dot(n, in.tangent.xyz));
        let b = cross(n, t) * in.tangent.w;
        let candidate = t * tn.x + b * tn.y + n * tn.z;
        if dot(candidate, candidate) > 1e-12 {
            let mapped_n = normalize(candidate);
            if dot(mapped_n, geometric) > 0.001 && dot(mapped_n, view_direction) > 0.001 { n = mapped_n; }
        }
    }
    return n;
}

// Specular anti-aliasing (Kaplanyan-Hoffman): widen roughness where the normal varies fast over
// an output pixel. Below a render scale of 1 the opaque pass's pixel spans 1 / scale of them per
// axis, so its derivatives are scaled as the textures' are (the larger axis: one isotropic width).
fn antialiased_roughness(n: vec3f, roughness: f32) -> f32 {
    let dn = fwidth(n) * max(view.texture_lod.x, view.texture_lod.y);
    let variance = 0.25 * dot(dn, dn);
    let a2 = roughness * roughness;
    return clamp(sqrt(sqrt(clamp(a2 * a2 + min(2.0 * variance, 0.18), 0.0, 1.0))), 0.045, 1.0);
}

fn surface(in: VsOut, facing: bool) -> Surface {
    let m = materials[in.material];
    // Explicit gradients: the material (and so whether a texture exists) varies per pixel, and
    // implicit derivatives need uniform control flow. Per output pixel at a render scale below 1.
    let du = dpdx(in.uv) * view.texture_lod.x;
    let dv = dpdy(in.uv) * view.texture_lod.y;
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
    var tn = vec3f(0.0);
    let mapped = m.normal_tex != NO_TEXTURE;
    if (mapped) {
        tn = textureSampleGrad(tex_linear, tex_sampler, in.uv, m.normal_tex, du, dv).xyz * 2.0 - 1.0;
    }
    let n = shading_normal(in, facing, tn, mapped);
    s.n = n;
    s.emissive = m.emissive;
    if (m.emissive_tex != NO_TEXTURE) {
        s.emissive *= textureSampleGrad(tex_srgb, tex_sampler, in.uv, m.emissive_tex, du, dv).rgb;
    }
    s.roughness = antialiased_roughness(n, s.roughness);
    s.occlusion = 1.0;
    return s;
}

// How much of the color at `world` the fog replaces.
fn fog_amount(world: vec3f) -> f32 {
    let density = view.fog.w;
    let d = distance(world, view.camera_pos.xyz);
    // A negative density: even fog, as under a liquid (docs/spec/liquids.md 6).
    if (density < 0.0) {
        return clamp(1.0 - exp(density * d), 0.0, 1.0);
    }
    if (density == 0.0) {
        return 0.0;
    }
    // Exponential height fog: thinner with altitude.
    let h = max(world.y, 0.0);
    return clamp(1.0 - exp(-density * d * exp(-h * 0.02)), 0.0, 1.0);
}

fn apply_fog(c: vec3f, world: vec3f) -> vec3f {
    return mix(c, view.fog.rgb * view.sun_color.w, fog_amount(world));
}

// What the opaque pass's forward pipelines write; the targets after the color exist only when TAA
// or GTAO need them (post.rs `SceneFormat`), and a pipeline without them ignores those outputs.
struct SceneOut {
    @location(0) color: vec4f,
    // The share of each channel of `color` that is indirect light (GTAO darkens only that).
    @location(1) share: vec4f,
    // Object motion (TAA): last frame's position of this point minus where it would be had the
    // object not moved, in last frame's normalized device coordinates.
    @location(2) motion: vec4f,
    // The view-space shading normal, `n * 0.5 + 0.5` (GTAO's optional normal source).
    @location(3) normal: vec4f,
};

// Whether the pipeline has the motion and the normal target (pipeline-overridable constants set
// from post.rs `SceneFormat::constants`; on when a pipeline sets none). The outputs of a missing
// target are left zero instead of computed: not every driver drops that work itself (NVIDIA's
// Direct3D 12 and AMD's Vulkan do not; docs/bench/dx12.md 10). The indirect share is computed with
// or without its target: without it NVIDIA's Direct3D 12 compiler rounds the color itself
// differently, and the images would differ from those before by a level here and there
// (docs/bench/dx12.md 10.5).
override SCENE_MOTION: bool = true;
override SCENE_NORMALS: bool = true;

fn scene_out(in: VsOut, n: vec3f, color: vec3f, indirect: vec3f) -> SceneOut {
    var o: SceneOut;
    o.color = vec4f(color, 1.0);
    o.share = vec4f(clamp(indirect / max(color, vec3f(1e-6)), vec3f(0.0), vec3f(1.0)), 1.0);
    if (SCENE_MOTION) {
        let moved = view.prev_view_proj * vec4f(in.prev_world, 1.0);
        let still = view.prev_view_proj * vec4f(in.world, 1.0);
        if (moved.w > 1e-6 && still.w > 1e-6) {
            o.motion = vec4f(moved.xy / moved.w - still.xy / still.w, 0.0, 0.0);
        }
    }
    if (SCENE_NORMALS) {
        let vn = (view.view * vec4f(n, 0.0)).xyz;
        o.normal = vec4f(vn * 0.5 + 0.5, 1.0);
    }
    return o;
}

@fragment
fn fs(in: VsOut, @builtin(front_facing) facing: bool) -> SceneOut {
    return shade(in, surface(in, facing));
}

// Alpha-tested materials only: the one entry point with `discard`.
@fragment
fn fs_masked(in: VsOut, @builtin(front_facing) facing: bool) -> SceneOut {
    let s = surface(in, facing);
    if (s.alpha < materials[in.material].alpha_cutoff) {
        discard;
    }
    return shade(in, s);
}

fn shade(in: VsOut, s: Surface) -> SceneOut {
    let frag = in.clip;
    let m = materials[in.material];
    if ((m.flags & 4u) != 0u) {
        return scene_out(in, s.n, s.albedo + s.emissive, vec3f(0.0));
    }
    let v = normalize(view.camera_pos.xyz - in.world);
    let n = s.n;
    let nv = max(dot(n, v), 1e-4);
    let a = s.roughness * s.roughness;
    let f0 = mix(vec3f(0.04), s.albedo, s.metallic);
    let diffuse_color = s.albedo * (1.0 - s.metallic);
    var color = vec3f(0.0);

    // Sun, refracted under water (water_caustics_light.wgsl `sun_path`).
    if (view.sun_dir.w > 0.0) {
        let sun = sun_path(in.world);
        let l = sun.dir;
        let nl = dot(n, l);
        if (nl > 0.0) {
            let h = normalize(l + v);
            let nh = max(dot(n, h), 0.0);
            let vh = max(dot(v, h), 0.0);
            let f = f_schlick(f0, vh);
            let spec = d_ggx(nh, a) * v_smith(nv, nl, a) * f;
            let diff = (1.0 - f) * diffuse_color / PI;
            let depth = dot(in.world - view.camera_pos.xyz, -vec3f(view.view[0].z, view.view[1].z, view.view[2].z));
            let sh = shadow_factor(sun.exit, n, depth);
            color += (diff + spec) * view.sun_color.rgb * view.sun_dir.w * nl * sun.beam * sh
                * caustic_light(in.world);
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
    let indirect = (diffuse_color * diffuse_light + prefiltered * brdf * horizon * horizon * ambient)
        * s.occlusion;
    color += indirect;

    color += s.emissive;
    let fog = fog_amount(in.world);
    color = mix(color, view.fog.rgb * view.sun_color.w, fog);
    return scene_out(in, n, color, indirect * (1.0 - fog));
}

// --- Transparent surfaces -------------------------------------------------------------------------
// The transparent stage (transparent.rs, docs/spec/transparency.md): alpha-blended (glTF BLEND, a
// look's alpha below 1) and transmissive (KHR_materials_transmission) surfaces, drawn back to front
// after the opaque pass and the sky (with GTAO on, after GTAO), premultiplied, depth tested and not
// written; the extra targets are left as they are (what is behind keeps them). Lit as `shade`
// lights an opaque surface; the dielectric's transmission takes the place of its diffuse light and
// lets what is behind through, less what the surface reflects, without refraction. The blend
// multiplies everything behind by one factor (no dual-source blending, an optional WebGPU feature),
// so the base color's luminance tints what shows through and its hue only the light the surface
// reflects and emits. A double-sided surface draws a two-sided index list (transparent.rs), its
// back faces first as reversed triangles: they face the camera, so the one pipeline, which culls
// back faces, draws them, and `surface` turns their normal toward the viewer.

// The output is the light the surface adds (premultiplied by its coverage) and, in alpha, 1 minus
// the share of the light behind that passes through (the blend multiplies the target by that share).
@fragment
fn fs_transparent(in: VsOut, @builtin(front_facing) facing: bool) -> @location(0) vec4f {
    // Gradients and the surface before anything depends on the material (uniform control flow).
    let du = dpdx(in.uv) * view.texture_lod.x;
    let dv = dpdy(in.uv) * view.texture_lod.y;
    return transparent(in, surface(in, facing), du, dv);
}

@group(3) @binding(4) var water_surface_depth: texture_depth_2d;
@group(3) @binding(5) var<uniform> water_layer: vec4u;

@fragment
fn fs_transparent_water(in: VsOut, @builtin(front_facing) facing: bool) -> @location(0) vec4f {
    let du = dpdx(in.uv) * view.texture_lod.x;
    let dv = dpdy(in.uv) * view.texture_lod.y;
    let s = surface(in,facing);
    let p = clamp(vec2i(in.clip.xy),vec2i(0),vec2i(textureDimensions(water_surface_depth))-1);
    let z = textureLoad(water_surface_depth,p,0);
    let behind = z > 0.0 && in.clip.z < z;
    if ((water_layer.x == 1u && !behind) || (water_layer.x == 2u && behind)) { discard; }
    return transparent(in,s,du,dv);
}

// A transparent surface's light and coverage (`fs_transparent`), from its `surface` and its uv's
// gradients.
fn transparent(in: VsOut, s: Surface, du: vec2f, dv: vec2f) -> vec4f {
    let m = materials[in.material];
    var coverage = clamp(s.alpha, 0.0, 1.0);
    if ((m.flags & 1u) != 0u) {
        // Alpha-masked and transmissive: the alpha cuts, it does not blend.
        if (s.alpha < m.alpha_cutoff) {
            discard;
        }
        coverage = 1.0;
    }
    var transmission = m.transmission;
    if (m.transmission_tex != NO_TEXTURE) {
        transmission *= textureSampleGrad(tex_linear, tex_sampler, in.uv, m.transmission_tex, du,
            dv).r;
    }
    let dielectric = 1.0 - s.metallic;
    var color = s.albedo + s.emissive;
    var through = 0.0;
    if ((m.flags & 4u) == 0u) {
        let v = normalize(view.camera_pos.xyz - in.world);
        let n = s.n;
        let nv = max(dot(n, v), 1e-4);
        let a = s.roughness * s.roughness;
        let r0 = (max(m.ior, 1.0) - 1.0) / (max(m.ior, 1.0) + 1.0);
        let f0 = mix(vec3f(r0 * r0), s.albedo, s.metallic);
        // What the dielectric does not transmit it diffuses.
        let diffuse_color = s.albedo * dielectric * (1.0 - transmission);
        color = vec3f(0.0);
        // Sun, refracted under water.
        if (view.sun_dir.w > 0.0) {
            let sun = sun_path(in.world);
            let l = sun.dir;
            let nl = dot(n, l);
            if (nl > 0.0) {
                let h = normalize(l + v);
                let f = f_schlick(f0, max(dot(v, h), 0.0));
                let spec = d_ggx(max(dot(n, h), 0.0), a) * v_smith(nv, nl, a) * f;
                let diff = (1.0 - f) * diffuse_color / PI;
                let depth = dot(in.world - view.camera_pos.xyz,
                    -vec3f(view.view[0].z, view.view[1].z, view.view[2].z));
                let sh = shadow_factor(sun.exit, n, depth);
                color += (diff + spec) * view.sun_color.rgb * view.sun_dir.w * nl * sun.beam * sh
                    * caustic_light(in.world);
            }
        }
        // Punctual lights of this pixel's cluster.
        let cell = cluster_of(in.clip);
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
            let x = d2 / (li.range * li.range);
            let window = clamp(1.0 - x * x, 0.0, 1.0);
            var att = window * window / max(d2, 0.01);
            if (li.kind == 2u) {
                att *= smoothstep(li.cos_outer, li.cos_inner, dot(-l, li.dir));
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
        let horizon = clamp(1.0 + dot(r, normalize(in.normal)), 0.0, 1.0);
        let baked = baked_diffuse(in.world, n);
        let diffuse_light = select(sh_irradiance(n) * (ambient / PI), baked.xyz, baked.w > 0.0);
        color += diffuse_color * diffuse_light
            + prefiltered * env_brdf(f0, s.roughness, nv) * horizon * horizon * ambient;
        color += s.emissive;
        // The light behind passes the dielectric's transmitting share, less what its specular
        // lobe reflects (the lobe's directional albedo), tinted by the base color's luminance.
        let reflected = env_brdf(vec3f(r0 * r0), s.roughness, nv).x;
        through = dielectric * transmission * (1.0 - reflected) * luminance(s.albedo);
    }
    let behind = clamp(1.0 - coverage + coverage * through, 0.0, 1.0);
    // Fogged as an opaque surface where it hides what is behind; what shows through was fogged
    // at its own depth.
    let fog = fog_amount(in.world);
    let lit = mix(coverage * color, view.fog.rgb * view.sun_color.w * (1.0 - behind), fog);
    return vec4f(lit, 1.0 - behind);
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
    // A transparent material casts as alpha-masked (materials.rs `variant`): what transmits is
    // not opaque. 0 for every other material.
    a *= 1.0 - m.transmission;
    if (a < m.alpha_cutoff) {
        discard;
    }
}

// --- Depth only: the depth prepass, and the depth without TAA's jitter ---------------------------
// The depth prepass (renderer.rs `DepthPrepass`, docs/spec/prepass.md) draws the opaque instances'
// depth with the frame group the forward pipelines then draw with; with TAA and splats the renderer
// also draws it from the unjittered frame group (renderer.rs `UnjitteredDepth`): splats are drawn
// after TAA without the jitter and test against it. The same positions as `vs` (`@invariant`, as
// there), the same alpha test as `fs_masked`.

struct DepthOut {
    @builtin(position) @invariant clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) @interpolate(flat) material: u32,
};

@vertex
fn vs_depth(v: VsIn, @builtin(instance_index) ii: u32) -> @builtin(position) @invariant vec4f {
    let d = drawn[ii + batch.x];
    let world = d.pos + quat_rotate(d.rot, v.position * d.scale);
    return view.view_proj * vec4f(world, 1.0);
}

@vertex
fn vs_depth_masked(v: VsIn, @builtin(instance_index) ii: u32) -> DepthOut {
    let d = drawn[ii + batch.x];
    let world = d.pos + quat_rotate(d.rot, v.position * d.scale);
    var o: DepthOut;
    o.clip = view.view_proj * vec4f(world, 1.0);
    o.uv = v.uv;
    o.material = d.material;
    return o;
}

// `fs_masked`'s alpha test (`surface`: the mip level of the uv's gradients, so distant alpha-tested
// surfaces open up as they do in the opaque pass).
@fragment
fn fs_depth_masked(in: DepthOut) {
    let m = materials[in.material];
    let du = dpdx(in.uv) * view.texture_lod.x;
    let dv = dpdy(in.uv) * view.texture_lod.y;
    var a = m.base_color.a;
    if (m.base_color_tex != NO_TEXTURE) {
        a *= textureSampleGrad(tex_srgb, tex_sampler, in.uv, m.base_color_tex, du, dv).a;
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
// The sea amoris-physics floats boats on: the same sine waves (height a sin(k d.p - w t + phase),
// deep-water w = sqrt(g k)), on a camera-centred ring grid that reaches the horizon, with smaller
// wavelets in the normals only, so the geometry boats sit on is exactly the simulation's.

override OCEAN_EXCLUDE_REGIONS:bool=false;
struct Ocean {
    level: f32,
    count: u32,
    time: f32,
    region_count: u32,
    waves: array<vec4f, 16>,     // dir.x, dir.z, k, amplitude
    phases: array<vec4f, 16>,    // omega, phase
    absorb: vec4f,               // the sea's look, as a region's (w: 1 when it has one)
    scatter: vec4f,
    fog: vec4f,
};

@group(3) @binding(0) var<uniform> ocean: Ocean;
// Entry-point-disjoint with shallow-water columns at this same binding.
@group(3) @binding(2) var<storage,read> sea_regions:array<vec4f>;
fn ocean_excluded(p:vec2f)->bool {
    for(var i=0u;i<ocean.region_count;i++) {
        let r=sea_regions[i];if(all(p>=r.xy)&&all(p<r.zw)){return true;}
    }
    return false;
}
@fragment
fn fs_ocean_depth(in:OceanOut) {
    if(ocean_excluded(in.world.xz)){discard;}
}


// The wind sea (ocean_waves.wgsl, water.md 60): three cascades of displacement and foam (layers
// 0 to 2) and of slopes and the displacement's derivatives (3 to 5), each tile repeating, read
// by the sea and by the regions beside it, so its detail runs on across their seam.
struct WindSea {
    cascades: array<vec4f, 3>,   // 1 / tile, texel (m), angular frequency at the band's middle
    lost: array<vec4f, 9>,       // per mip level, each cascade's mean square slope finer than it
    params: vec4f,               // x: on, y: the mean square slope finer than the cascades, z: foam
};
@group(3) @binding(14) var wind_waves: texture_2d_array<f32>;
@group(3) @binding(15) var wind_sampler: sampler;
@group(3) @binding(16) var<uniform> wind_sea: WindSea;

// The wind sea's displacement at p, for vertices `spacing` metres apart: each cascade at the mip
// level whose texels lie that far apart.
fn wind_displacement(p: vec2f, spacing: f32) -> vec3f {
    var d = vec3f(0.0);
    for (var c = 0u; c < 3u; c++) {
        let cascade = wind_sea.cascades[c];
        let lod = clamp(log2(max(spacing / cascade.y, 1.0)), 0.0, 8.0);
        d += textureSampleLevel(wind_waves, wind_sampler, p * cascade.x, c, lod).xyz;
    }
    return d;
}

// The share of a wave's height that reaches water `depth` deep: the square root of the TMA
// spectrum's depth factor (S. A. Kitaigorodskii, V. P. Krasitskii and M. M. Zaslavskii, J. Phys.
// Oceanogr. 5, 1975; E. Bouws et al., J. Geophys. Res. 90(C1), 1985) at angular frequency w.
fn wind_depth(w: f32, depth: f32) -> f32 {
    let wh = w * sqrt(max(depth, 0.0) / 9.81);
    let phi = select(select(1.0, 1.0 - 0.5 * (2.0 - wh) * (2.0 - wh), wh < 2.0), 0.5 * wh * wh,
        wh <= 1.0);
    return sqrt(phi);
}

struct WindSurface { slope: vec2f, variance: f32, foam: f32 };
// The wind sea at rest point p over water `depth` deep, seen by a pixel whose footprint `dx` and
// `dy` span: the slopes the pixel resolves, stretched where the crests' horizontal displacement
// crowds them, the mean square slope it does not (which widens the glints' lobe), and the foam of
// the folded crests (the first cascade's, the only one that makes any). A cascade the pixel sees
// only through its last mip levels, its waves all finer than the pixel, adds its slopes to the
// lobe and is not sampled.
fn wind_surface(p: vec2f, dx: vec2f, dy: vec2f, depth: f32) -> WindSurface {
    var w: WindSurface;
    let footprint = max(length(dx), length(dy));
    for (var c = 0u; c < 3u; c++) {
        let cascade = wind_sea.cascades[c];
        let a = wind_depth(cascade.z, depth);
        let lod = clamp(log2(max(footprint / cascade.y, 1e-6)), 0.0, 8.0);
        let level = u32(lod);
        w.variance += a * a * mix(wind_sea.lost[level][c], wind_sea.lost[min(level + 1u, 8u)][c],
            fract(lod));
        if (lod >= 7.0) { continue; }
        let uv = p * cascade.x;
        let g = textureSampleGrad(wind_waves, wind_sampler, uv, c + 3u, dx * cascade.x,
            dy * cascade.x);
        w.slope += a * g.xy / max(vec2f(1.0) + a * g.zw, vec2f(0.25));
        if (c == 0u) {
            w.foam = a * textureSampleGrad(wind_waves, wind_sampler, uv, 0u, dx * cascade.x,
                dy * cascade.x).w;
        }
    }
    w.variance += wind_sea.params.y;
    w.foam *= wind_sea.params.z;
    return w;
}

// How much of the wind sea's displacement the sea keeps at p: none within 24 m of a region,
// whose surface takes the wind sea's detail but not its height, so the two meet on the swell.
fn wind_kept(p: vec2f) -> f32 {
    var d = 1e9;
    for (var i = 0u; i < ocean.region_count; i++) {
        let r = sea_regions[i];
        let outside = max(max(r.xy - p, p - r.zw), vec2f(0.0));
        d = min(d, length(outside));
    }
    return smoothstep(0.0, 24.0, d);
}

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
    // The point at rest the wind sea moved here: its texels are indexed by it.
    @location(2) rest: vec2f,
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
    var world = vec3f(p.x, s.x, p.y);
    if (wind_sea.params.x > 0.0) {
        // The ring grid's rings lie 4 percent of their radius apart.
        let spacing = max(length(p - cam) * 0.04, 0.4);
        world += wind_displacement(p, spacing) * wind_kept(p);
    }
    o.clip = view.view_proj * vec4f(world, 1.0);
    o.world = world;
    o.grad = s.yz;
    o.rest = p;
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

// The sea writes every target of the opaque pass, since it writes depth over what lies under it:
// no indirect share (GTAO leaves it as it is), no object motion (it reprojects as a still surface
// under the camera), its own normal.
@fragment
fn fs_ocean(in: OceanOut) -> SceneOut {
    // Gradients first, in uniform control flow.
    let lod_scale = max(view.texture_lod.x, view.texture_lod.y);
    let rest_dx = dpdx(in.rest) * lod_scale;
    let rest_dy = dpdy(in.rest) * lod_scale;
    if(OCEAN_EXCLUDE_REGIONS&&ocean_excluded(in.world.xz)){discard;}
    let v_world = view.camera_pos.xyz - in.world;
    let dist = length(v_world);
    // Toward the eye, or along the view's axis for an orthographic camera, as a region has it.
    let v = select(vec3f(view.view[0].z, view.view[1].z, view.view[2].z), v_world / dist,
        abs(view.proj[3].w) < 0.5);
    let fade = clamp(1.0 - dist / 600.0, 0.0, 1.0);
    // Metres per output pixel (per the opaque pass's pixel times the render scale, as the
    // textures' gradients are).
    let footprint = max(length(fwidth(in.world.xz)) * lod_scale, 1e-3);
    let windy = wind_sea.params.x > 0.0;
    var g = in.grad;
    // Water: F0 0.02; slightly rough so the sun's glitter spreads with distance. With a wind sea
    // the lobe takes the mean square slope of the waves finer than the pixel.
    var rough = clamp(0.05 + footprint * 0.04, 0.05, 0.3);
    var whitecaps = 0.0;
    if (windy) {
        let w = wind_surface(in.rest, rest_dx, rest_dy, 1e4);
        g += w.slope;
        rough = sqrt(sqrt(0.05 * 0.05 * 0.05 * 0.05 + w.variance));
        whitecaps = foam_cover(in.rest, in.rest, 0.0, footprint, w.foam);
    } else {
        g += wavelets(in.world.xz, ocean.time, footprint);
    }
    let n = normalize(vec3f(-g.x, 1.0, -g.y));
    let nv = max(dot(n, v), 1e-4);
    let a = rough * rough;
    // A look shades the sea as a region's water: its Fresnel too.
    let looked = ocean.absorb.w > 0.0;
    let f = select(0.02 + 0.98 * pow(1.0 - nv, 5.0), shallow_fresnel(nv, 0.75), looked);
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
    if (looked) {
        // Deep water as a region's medium sees it: what its scattering sends back.
        let haze = max(ocean.fog.w, 0.0);
        let extinction = ocean.absorb.rgb + ocean.scatter.rgb + vec3f(haze);
        body = (ocean.scatter.rgb * body_light + ocean.fog.rgb * haze * view.sun_color.w)
            / max(extinction, vec3f(1e-6));
    }
    // Sun glints.
    let shadow = shadow_factor(in.world, n, dot(in.world - view.camera_pos.xyz,
        -vec3f(view.view[0].z, view.view[1].z, view.view[2].z)));
    var spec = vec3f(0.0);
    if (view.sun_dir.w > 0.0) {
        let h = normalize(sun + v);
        let nl = max(dot(n, sun), 0.0);
        spec = d_ggx(max(dot(n, h), 0.0), a) * v_smith(nv, nl, a) * f_schlick(vec3f(0.02), max(dot(v, h), 0.0))
            * sun_lux * nl * shadow;
    }
    // Foam where the swell crests steeply, or where the wind sea's crests fold.
    let steep = length(in.grad);
    let foam = max(smoothstep(0.32, 0.6, steep) * crest * fade, whitecaps);
    var c = mix(body, sky, f) + spec;
    if (looked) {
        // A region's foam (fs_shallow), so the two meet unseen.
        let white = vec3f(0.85, 0.9, 0.92) * (body_light * 6.0 + sun_lux * max(sun.y, 0.0) * 0.25
            * shadow);
        c = mix(c, white, foam);
    } else {
        c = mix(c, vec3f(0.8) * (body_light * 6.0 + sun_lux * max(sun.y, 0.0) * 0.25), foam * 0.6);
    }
    c = apply_fog(c, in.world);
    var o: SceneOut;
    o.color = vec4f(c, 1.0);
    o.share = vec4f(0.0);
    o.motion = vec4f(0.0);
    if (SCENE_NORMALS) {
        o.normal = vec4f((view.view * vec4f(n, 0.0)).xyz * 0.5 + 0.5, 1.0);
    }
    return o;
}

// --- Water regions (docs/spec/water.md 9; the spike, docs/spikes/shallow-water.md) --------------
// A height field of water columns (amoris-physics `water`): Smooth draws a vertex at each column's
// centre at its surface and two triangles between four neighbouring centres; a dry column's vertex
// takes the mean surface of its wet neighbours, so a shore stays flat and runs under the bank, and
// water beside water lower down (a ledge over a pool) slopes down as a sheet. Blocky draws each
// block of `per` x `per` columns flat at its columns' mean surface, with vertical sides where
// neighbouring blocks differ. Both are drawn in the transparent layer.

@group(3) @binding(3) var surface_foam: texture_2d<f32>; // row-major presentation history
@group(3) @binding(4) var water_scene_color: texture_2d<f32>;
@group(3) @binding(5) var water_scene_depth: texture_2d<f32>;
@group(3) @binding(6) var water_scene_sampler: sampler;
@group(3) @binding(7) var water_normal_depth:texture_2d<f32>;
@group(3) @binding(8) var water_flow_height:texture_2d<f32>;
@group(3) @binding(9) var<storage,read> water_geometry:array<vec4u>;
// The ripples the bodies stir (water_ripples.rs, water.md 57): rise, slopes and their mean square,
// mipmapped, texel i holding world cell i modulo 512 of the window; sampled with wrapping.
@group(3) @binding(12) var ripple_map: texture_2d<f32>;
@group(3) @binding(13) var ripple_sampler: sampler;
const RIPPLE_SIDE: f32 = 512.0;
const RIPPLE_LODS: f32 = 7.0;
// The sponge along the window's edge, as a share of its side: the ripples fade out over it.
const RIPPLE_EDGE: f32 = 0.0625;

struct ShallowOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) depth: f32,
    @location(3) speed: f32,
    // How much it falls: white where the water drops steeply (a sheet over a ledge, a fall).
    @location(4) steep: f32,
    @location(5) velocity: vec2f,
    @location(6) @interpolate(flat) field_surface:u32,
};

struct SurfaceAttributes { normal_depth:vec4f, flow:vec4f };
fn morph_height(ids:vec3u,weights:vec3f,base:vec2u)->f32 {
    let a=vec2i(base+vec2u(ids.x%17u,ids.x/17u));let b=vec2i(base+vec2u(ids.y%17u,ids.y/17u));let c=vec2i(base+vec2u(ids.z%17u,ids.z/17u));
    return dot(vec3f(shallow_column(a.x,a.y).x,shallow_column(b.x,b.y).x,shallow_column(c.x,c.y).x),weights);
}
fn shallow_morph_vertex(vi:u32)->ShallowOut {
    let header=water_geometry[0];let encoded=vi & 0x7fffffffu;let tile=encoded/header.x;let vertex=encoded%header.x;
    let tiles=(shallow.count.x-2u)/16u+1u;let base=vec2u(tile%tiles,tile/tiles)*16u;
    let row=1u+vertex*4u;let a=water_geometry[row];let b=water_geometry[row+1u];let c=water_geometry[row+2u];let d=water_geometry[row+3u];
    let state=water_geometry[header.y+tile*2u];let t=bitcast<f32>(state.z);
    let h_a=morph_height(vec3u(a.zw,b.x),bitcast<vec3f>(b.yzw),base);
    let h_b=morph_height(c.xyz,bitcast<vec3f>(vec3u(c.w,d.xy)),base);
    let local=bitcast<vec2f>(a.xy);let grid=vec2f(base)+local;var height=mix(h_a,h_b,t);
    if(any(local==vec2f(0.0))||any(local==vec2f(16.0))){let at=vec2i(base+vec2u(local));height=shallow_column(at.x,at.y).x;}
    let world=vec3f(shallow.origin.x+(grid.x+0.5)*shallow.cell,height,shallow.origin.y+(grid.y+0.5)*shallow.cell);
    var o:ShallowOut;o.clip=shallow_clip(world);o.world=world;o.normal=vec3f(0.0,1.0,0.0);o.depth=1.0;o.speed=0.0;o.steep=0.0;o.velocity=vec2f(0.0);o.field_surface=1u;return o;
}
// Explicit native accuracy probe of the same function used by the vertex entry point.
@group(0) @binding(7) var<storage,read> morph_probe_indices:array<u32>;
@group(0) @binding(8) var<storage,read_write> morph_probe_positions:array<vec4f>;
@compute @workgroup_size(64)
fn probe_water_vertices(@builtin(global_invocation_id) id:vec3u) {
    if(id.x>=arrayLength(&morph_probe_indices)){return;}
    let p=shallow_morph_vertex(morph_probe_indices[id.x]);morph_probe_positions[id.x*2u]=vec4f(p.world,1.0);morph_probe_positions[id.x*2u+1u]=p.clip;
}
fn shallow_attributes(world:vec2f)->SurfaceAttributes {
    let grid=clamp((world-shallow.origin)/shallow.cell-0.5,vec2f(0.0),vec2f(shallow.count.xy-vec2u(1u)));
    let at=min(vec2u(floor(grid)),shallow.count.xy-vec2u(2u));let q=grid-vec2f(at);
    let a=vec2i(at);var b=a+vec2i(0,1);let c=a+vec2i(1,1);
    var weights=vec3f(1.0-q.y,q.y-q.x,q.x);
    if(q.y<q.x){b=a+vec2i(1,0);weights=vec3f(1.0-q.x,q.x-q.y,q.y);}
    return SurfaceAttributes(
        textureLoad(water_normal_depth,a,0)*weights.x+textureLoad(water_normal_depth,b,0)*weights.y+textureLoad(water_normal_depth,c,0)*weights.z,
        textureLoad(water_flow_height,a,0)*weights.x+textureLoad(water_flow_height,b,0)*weights.y+textureLoad(water_flow_height,c,0)*weights.z);
}

// One vertex per column (`vi`, x fastest); an index buffer makes the triangles, so each column is
// shaded once.
@vertex
fn vs_shallow(@builtin(vertex_index) vi: u32) -> ShallowOut {
    if((vi & 0x80000000u)!=0u){return shallow_morph_vertex(vi);}
    let at = vec2i(i32(vi % shallow.count.x), i32(vi / shallow.count.x));
    let c = shallow_vertex(at.x, at.y);
    let hl = shallow_vertex(at.x - 1, at.y).x;
    let hr = shallow_vertex(at.x + 1, at.y).x;
    let hd = shallow_vertex(at.x, at.y - 1).x;
    let hu = shallow_vertex(at.x, at.y + 1).x;
    let world = vec3f(shallow.origin.x + (f32(at.x) + 0.5) * shallow.cell, c.x,
                      shallow.origin.y + (f32(at.y) + 0.5) * shallow.cell);
    var o: ShallowOut;
    o.clip = shallow_clip(world);
    o.world = world;
    o.normal = shallow_normal(at);
    o.depth = c.y;
    o.speed = length(c.zw);
    o.steep = 1.0 - normalize(o.normal).y;
    o.velocity = c.zw;
    o.field_surface=1u;
    return o;
}

// Block (bi, bk) of `per` x `per` columns: its level (the mean surface of its wet columns, else of
// its beds), mean depth and mean velocity; a level far below everything past the grid.
// Eighteen vertices a block: its top (two triangles), its side toward +x and its side toward +z
// (two triangles each, collapsed where the blocks meet level or the higher one is dry).
@vertex
fn vs_shallow_blocky(@builtin(vertex_index) vi: u32) -> ShallowOut {
    let per = i32(max(shallow.count.w, 1u));
    let nbx = max(i32(shallow.count.x) / per, 1);
    let b = i32(vi / 18u);
    let part = vi % 18u;
    let bi = b % nbx;
    let bk = b / nbx;
    let me = shallow_block(bi, bk);
    let size = shallow.cell * f32(per);
    let x0 = shallow.origin.x + f32(bi) * size;
    let z0 = shallow.origin.y + f32(bk) * size;
    // The quad's corner: (0, 0) (1, 0) (1, 1), (0, 0) (1, 1) (0, 1).
    let q = part % 6u;
    let cu = select(0.0, 1.0, q == 1u || q == 2u || q == 4u);
    let cv = select(0.0, 1.0, q == 2u || q == 4u || q == 5u);
    var world = vec3f(x0 + cu * size, me.x, z0 + cv * size);
    var normal = vec3f(0.0, 1.0, 0.0);
    var depth = me.y;
    var speed = length(me.zw);
    var steep = 0.0;
    if (part >= 6u) {
        let along_x = part < 12u;
        let other = select(shallow_block(bi, bk + 1), shallow_block(bi + 1, bk), along_x);
        let mine = me.x >= other.x;
        let hi = select(other, me, mine);
        let lo = select(me, other, mine);
        let drawn = other.x > -1e8 && hi.y > 0.002 && hi.x - lo.x > 0.001;
        let y = mix(lo.x, hi.x, cv);
        let toward = select(-1.0, 1.0, mine);
        if (along_x) {
            world = vec3f(x0 + size, y, z0 + cu * size);
            normal = vec3f(toward, 0.0, 0.0);
        } else {
            world = vec3f(x0 + cu * size, y, z0 + size);
            normal = vec3f(0.0, 0.0, toward);
        }
        depth = select(0.0, hi.y, drawn);
        speed = length(hi.zw);
        steep = smoothstep(0.25, 1.0, hi.x - lo.x);
        if (!drawn) {
            world = vec3f(x0, me.x, z0);
        }
    }
    var o: ShallowOut;
    o.clip = shallow_clip(world);
    o.world = world;
    o.normal = normal;
    o.depth = depth;
    o.speed = speed;
    o.steep = steep;
    o.velocity = me.zw;
    o.field_surface=0u;
    return o;
}

// The still water drawn around a region (water.md 9), at `surround.x` out to `surround.y` metres,
// as deep water (so `fs_shallow` gives it the body colour, opaque), with no flow. Two parts:
// - the stitch (`vs_shallow_stitch`): a strip `surround.z` metres wide along each edge, from the
//   region's last surface (each edge column's own vertex, so the two meet exactly however the edge
//   moves) out to the level, the corners closed by fans;
// - the bands (`vs_shallow_surround`): from a quarter metre inside the stitch's outer rim (they
//   overlap at the same level) out to the reach; twenty-four vertices.

fn shallow_out(world: vec3f, depth: f32, speed: f32) -> ShallowOut {
    var o: ShallowOut;
    o.clip = shallow_clip(world);
    o.world = world;
    o.normal = vec3f(0.0, 1.0, 0.0);
    o.depth = depth;
    o.speed = speed;
    o.steep = 0.0;
    o.velocity = vec2f(0.0);
    o.field_surface=0u;
    return o;
}

@vertex
fn vs_shallow_surround(@builtin(vertex_index) vi: u32) -> ShallowOut {
    let size = vec2f(shallow.count.xy) * shallow.cell;
    let rim = shallow.surround.z - 0.25;
    let lo = shallow.origin - vec2f(rim);
    let hi = shallow.origin + size + vec2f(rim);
    let far = shallow.surround.y;
    // Bands: -z and +z across the whole width, -x and +x between them.
    let band = vi / 6u;
    var a = vec2f(lo.x - far, lo.y - far);
    var b = vec2f(hi.x + far, lo.y);
    if (band == 1u) {
        a = vec2f(lo.x - far, hi.y);
        b = vec2f(hi.x + far, hi.y + far);
    } else if (band == 2u) {
        a = vec2f(lo.x - far, lo.y);
        b = vec2f(lo.x, hi.y);
    } else if (band == 3u) {
        a = vec2f(hi.x, lo.y);
        b = vec2f(hi.x + far, hi.y);
    }
    let q = vi % 6u;
    let cu = select(0.0, 1.0, q == 1u || q == 2u || q == 4u);
    let cv = select(0.0, 1.0, q == 2u || q == 4u || q == 5u);
    return shallow_out(vec3f(mix(a.x, b.x, cu), shallow.surround.x, mix(a.y, b.y, cv)), 1000.0, 0.0);
}

// Edge column (i, k)'s last surface as the region draws it: Smooth, its vertex at the column's
// centre; Blocky, its block's level, at the region's edge. Returns (height, depth, speed), and
// where across the edge it lies (`inner`, metres from the region's rectangle, inward).
fn shallow_edge(i: i32, k: i32) -> vec4f {
    if (shallow.surround.w > 0.5) {
        let per = i32(max(shallow.count.w, 1u));
        let b = shallow_block(i / per, k / per);
        return vec4f(b.x, b.y, length(b.zw), 0.0);
    }
    let c = shallow_vertex(i, k);
    return vec4f(c.x, c.y, length(c.zw), 0.5 * shallow.cell);
}

@vertex
fn vs_shallow_stitch(@builtin(vertex_index) vi: u32) -> ShallowOut {
    let n = vec2i(shallow.count.xy);
    let cell = shallow.cell;
    let o = shallow.origin;
    let size = vec2f(n) * cell;
    let w = shallow.surround.z;
    let level = shallow.surround.x;
    let quad = i32(vi / 6u);
    let q = vi % 6u;
    let cu = select(0, 1, q == 1u || q == 2u || q == 4u);
    let outer = q == 2u || q == 4u || q == 5u;
    let along_x = n.x - 1;
    let along_z = n.y - 1;
    // Edges: -z, +z (along x), -x, +x (along z); then the four corners.
    if (quad < 2 * along_x) {
        let plus = quad >= along_x;
        let i = quad % along_x + cu;
        let k = select(0, n.y - 1, plus);
        let e = shallow_edge(i, k);
        let x = o.x + (f32(i) + 0.5) * cell;
        let side = select(-1.0, 1.0, plus);
        let edge_z = select(o.y, o.y + size.y, plus);
        if (outer) {
            return shallow_out(vec3f(x, level, edge_z + side * w), 1000.0, 0.0);
        }
        return shallow_out(vec3f(x, e.x, edge_z - side * e.w), e.y, e.z);
    }
    let rest = quad - 2 * along_x;
    if (rest < 2 * along_z) {
        let plus = rest >= along_z;
        let k = rest % along_z + cu;
        let i = select(0, n.x - 1, plus);
        let e = shallow_edge(i, k);
        let z = o.y + (f32(k) + 0.5) * cell;
        let side = select(-1.0, 1.0, plus);
        let edge_x = select(o.x, o.x + size.x, plus);
        if (outer) {
            return shallow_out(vec3f(edge_x + side * w, level, z), 1000.0, 0.0);
        }
        return shallow_out(vec3f(edge_x - side * e.w, e.x, z), e.y, e.z);
    }
    // Corner m: the corner column's surface, fanned out to the stitch's two outer rims and the
    // corner between them: (0, 0) the column, (1, 0) the z edge's rim, (1, 1) the corner, (0, 1)
    // the x edge's rim.
    let m = rest - 2 * along_z;
    let px = (m & 1) == 1;
    let pz = (m & 2) == 2;
    let i = select(0, n.x - 1, px);
    let k = select(0, n.y - 1, pz);
    let e = shallow_edge(i, k);
    let sx = select(-1.0, 1.0, px);
    let sz = select(-1.0, 1.0, pz);
    let ex = select(o.x, o.x + size.x, px);
    let ez = select(o.y, o.y + size.y, pz);
    let cx = o.x + (f32(i) + 0.5) * cell;
    let cz = o.y + (f32(k) + 0.5) * cell;
    let inner = vec3f(select(cx, ex, shallow.surround.w > 0.5), e.x, select(cz, ez, shallow.surround.w > 0.5));
    let cv = select(0, 1, outer);
    if (cu == 0 && cv == 0) {
        return shallow_out(inner, e.y, e.z);
    }
    var p = vec2f(ex + sx * w, ez + sz * w);
    if (cu == 1 && cv == 0) {
        p = vec2f(inner.x, ez + sz * w);
    } else if (cu == 0 && cv == 1) {
        p = vec2f(ex + sx * w, inner.z);
    }
    return shallow_out(vec3f(p.x, level, p.y), 1000.0, 0.0);
}

// The ripples' slope at a world point (x, y: along x and z) and the slope variance the pixel's
// `footprint` (metres) filters out of them (z), faded toward the window's edges and past its
// coarsest level; all zero where there are none.
fn ripple_at(world: vec2f, footprint: f32) -> vec3f {
    if (shallow.ripple.w < 0.5) { return vec3f(0.0); }
    let local = (world - shallow.ripple.xy) / shallow.ripple.z;
    if (any(local < vec2f(0.0)) || any(local >= vec2f(1.0))) { return vec3f(0.0); }
    let edge = min(min(local.x, local.y), min(1.0 - local.x, 1.0 - local.y));
    let lod = log2(max(footprint * RIPPLE_SIDE / shallow.ripple.z, 1.0));
    let fade = smoothstep(0.0, RIPPLE_EDGE, edge)
        * (1.0 - smoothstep(RIPPLE_LODS - 2.0, RIPPLE_LODS - 1.0, lod));
    let r = textureSampleLevel(ripple_map, ripple_sampler, world / shallow.ripple.z, lod);
    return vec3f(r.yz, max(r.w - dot(r.yz, r.yz), 0.0)) * fade;
}

fn shallow_foam_at(world: vec2f) -> f32 {
    let pos = (world - shallow.origin) / shallow.cell - 0.5;
    let hi = shallow.count.xy - vec2u(1u);
    if (any(pos < vec2f(0.0)) || any(pos > vec2f(hi))) { return 0.0; }
    let a = vec2u(floor(pos));
    let b = min(a + vec2u(1u), hi);
    let t = fract(pos);
    return mix(mix(textureLoad(surface_foam, vec2i(a), 0).x,
                   textureLoad(surface_foam, vec2i(vec2u(b.x,a.y)), 0).x, t.x),
               mix(textureLoad(surface_foam, vec2i(vec2u(a.x,b.y)), 0).x,
                   textureLoad(surface_foam, vec2i(b), 0).x, t.x), t.y);
}

fn shallow_noise(q: vec2f) -> f32 {
    let cell = floor(q);
    let t = q - cell;
    let s2 = t * t * (3.0 - 2.0 * t);
    let lattice = vec2u(vec2i(cell) + vec2i(4096));
    let n00 = unit_from_hash(pcg_hash((lattice.x * 73856093u) ^ (lattice.y * 19349663u)));
    let n10 = unit_from_hash(pcg_hash(((lattice.x + 1u) * 73856093u) ^ (lattice.y * 19349663u)));
    let n01 = unit_from_hash(pcg_hash((lattice.x * 73856093u) ^ ((lattice.y + 1u) * 19349663u)));
    let n11 = unit_from_hash(pcg_hash(((lattice.x + 1u) * 73856093u) ^ ((lattice.y + 1u) * 19349663u)));
    return mix(mix(n00, n10, s2.x), mix(n01, n11, s2.x), s2.y);
}

// A whitewater texture in [0, 1] at flow-map point `q`: four octaves of value noise, each turned
// by 37 degrees and shifted so no lattice lines up, the coarse ones carrying most of it. An octave
// finer than six of the pixel's `footprint` (metres) gives way to its mean, so foam never breaks
// into specks that alias into flicker at a distance. The flow map's phases carry it with the
// water; turning or stretching `q` by the local flow instead turned the whole pattern about the
// world's origin, so 200 m out a flow turning by a fifth of a degree swept the texture through a
// cycle and the foam drew the flow's contour lines.
fn foam_texture(q: vec2f, footprint: f32) -> f32 {
    var p = q;
    var sum = 0.0;
    var total = 0.0;
    var amp = 1.0;
    var freq = 1.3;
    for (var i = 0u; i < 4u; i++) {
        let keep = clamp(1.0 / (freq * footprint * 6.0) - 1.0, 0.0, 1.0);
        let shift = vec2f(f32(i) * 17.3, f32(i) * 7.1);
        sum += amp * mix(0.5, shallow_noise(p * freq + shift), keep);
        total += amp;
        amp *= 0.5;
        freq *= 2.1;
        p = vec2f(0.8 * p.x - 0.6 * p.y, 0.6 * p.x + 0.8 * p.y);
    }
    // Summed octaves crowd their mean: spread them back over a single octave's range.
    return clamp(0.5 + (sum / total - 0.5) * 1.6, 0.0, 1.0);
}

// How much of a point foam covers, from the `density` (0 to 1) the presentation field carries
// there (water_foam.wgsl). Thick foam takes the coverage mapping real-time ocean foam uses: the
// density sets a threshold on the texture, so its troughs fill in as the density grows until the
// white water is whole, with soft grey hollows on an edge that widens with the pixel's footprint.
// Thin foam is a faint film the texture shades, so a streak of it does not break into hard specks
// and lines on the texture's peaks. Scaling a single octave's grain by the density had left even
// breaking white water a camouflage of grey blocks. The flow map's two phases (`blend`) mix with
// their variance kept, so the pattern does not fade and return each time the phases swap.
fn foam_cover(q0: vec2f, q1: vec2f, blend: f32, footprint: f32, density: f32) -> f32 {
    let w = vec2f(1.0 - blend, blend);
    let t = clamp(0.5 + ((foam_texture(q0, footprint) - 0.5) * w.x
        + (foam_texture(q1, footprint) - 0.5) * w.y) / max(length(w), 1e-3), 0.0, 1.0);
    let c = clamp(density, 0.0, 1.0);
    let soft = 0.3 + clamp(footprint * 2.0, 0.0, 0.5);
    let thick = smoothstep(1.0 - c - soft, 1.0 - c + soft * 0.33, t);
    let thin = c * mix(0.4, 1.0, t);
    return mix(thin, thick, smoothstep(0.3, 0.8, c)) * smoothstep(0.0, 0.05, c);
}

// ---- The wet bed (water.md 61.2) ---------------------------------------------------------------
// Where a region's water stands or has been, its bed is damp and darker; where the water left it
// bare, for a few seconds it is glossy under a film of water, with the foam the swash left on it.
// Drawn over the opaque pass from the scene's depth, before the water's background is resolved,
// in one blend: its alpha, the darkening, multiplies the colour, and its colour, the film's
// reflection and the foam, adds to it.
struct WetOut { @builtin(position) clip: vec4f };
@vertex
fn vs_wet(@builtin(vertex_index) i: u32) -> WetOut {
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    var o: WetOut;
    o.clip = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    return o;
}
// The world point behind a pixel, and whether there is one (the sky is not).
fn wet_world(pixel: vec2f) -> vec4f {
    let size = vec2f(textureDimensions(water_scene_depth));
    let depth = textureLoad(water_scene_depth, vec2i(clamp(pixel, vec2f(0.0), size - 1.0)), 0).x;
    let ndc = vec4f(pixel.x / size.x * 2.0 - 1.0, 1.0 - pixel.y / size.y * 2.0, depth, 1.0);
    let w = view.inv_view_proj * ndc;
    return vec4f(w.xyz / w.w, select(0.0, 1.0, depth > 0.0 && abs(w.w) > 1e-12));
}
// One column's part in the wet bed at a world point: its damp (whole under water) where the point
// lies within a few decimetres of the column's bed (as the terrain's mesh lies over the columns),
// and its film and the foam left behind where under 2 millimetres of water stand on it, the bed
// lying bare. The depth and drawn height come from the surface's shading attributes (a dry column
// beside water is drawn at its wet neighbours' level, a few centimetres over its bed).
fn wet_column(c: vec2u, y: f32) -> vec3f {
    let depth = textureLoad(water_normal_depth, vec2i(c), 0).w;
    let height = textureLoad(water_flow_height, vec2i(c), 0).w;
    if (!(depth >= 0.0 && abs(height) < 1e10)) { return vec3f(0.0); }
    let near = 1.0 - smoothstep(0.15, 0.4, abs(y - (height - depth)));
    let bare = 1.0 - smoothstep(0.002, 0.004, depth);
    return textureLoad(surface_foam, vec2i(c), 0).yzw * near * vec3f(1.0, bare, bare);
}
// The bed's damp, film and foam left behind at a world point, between its four columns, so that
// the wet bed has no cell edges. Smooth regions only, whose surface keeps the attributes read here.
fn wet_bed(world: vec3f) -> vec3f {
    let pos = (world.xz - shallow.origin) / shallow.cell - 0.5;
    let hi = vec2f(shallow.count.xy - vec2u(1u));
    if (any(pos < vec2f(-0.5)) || any(pos > hi + 0.5)) { return vec3f(0.0); }
    let q = clamp(pos, vec2f(0.0), hi);
    let a = vec2u(floor(q));
    let b = min(a + vec2u(1u), vec2u(hi));
    let t = fract(q);
    return mix(mix(wet_column(a, world.y), wet_column(vec2u(b.x, a.y), world.y), t.x),
        mix(wet_column(vec2u(a.x, b.y), world.y), wet_column(b, world.y), t.x), t.y);
}
// A wet rough surface is darker: light it lets into the water filling its pores is reflected back
// at the water's surface, inside the critical angle and partly beyond, and absorbed again (J.
// Lekner and M. C. Dorf, "Why some things are darker when wet", Applied Optics 27(7), 1988). With
// the water's mean external reflectance 0.066 and its internal 1 - (1 - 0.066) / 1.333^2 = 0.47,
// an albedo of a half keeps 0.934 x 0.53 / (1 - 0.47 x 0.5) = 0.65 of its light. The blends cannot
// read the colour they darken, so every bed takes the half's factor.
const WET_DARKENING: f32 = 0.65;
@fragment
fn fs_wet(in: WetOut) -> @location(0) vec4f {
    let w = wet_world(in.clip.xy);
    let dx = dpdx(w.xyz);
    let dy = dpdy(w.xyz);
    let footprint = max(length(fwidth(w.xz)), 1e-3);
    if (w.w == 0.0) { discard; }
    let s = wet_bed(w.xyz);
    if (all(s == vec3f(0.0))) { discard; }
    let foam = foam_cover(w.xz, w.xz, 0.0, footprint, s.z);
    // Fog hides the darkening as it hides the bed.
    let fog = fog_amount(w.xyz);
    let dark = mix(mix(1.0, WET_DARKENING, s.x) * (1.0 - foam), 1.0, fog);
    let film = s.y;
    if (film <= 0.0 && foam <= 0.0) { return vec4f(0.0, 0.0, 0.0, dark); }
    var n = normalize(cross(dy, dx));
    if (n.y < 0.0) { n = -n; }
    let v_world = view.camera_pos.xyz - w.xyz;
    let v = select(vec3f(view.view[0].z, view.view[1].z, view.view[2].z),
        v_world / max(length(v_world), 1e-6), abs(view.proj[3].w) < 0.5);
    let nv = max(dot(n, v), 1e-4);
    // The film is a thin sheet of water over the grains: a mirror rough with them.
    let f = shallow_fresnel(nv, 0.75);
    let r = reflect(-v, n);
    let ry = vec3f(r.x, max(r.y, 0.02), r.z);
    let levels = f32(textureNumLevels(env_cube) - 1);
    let rough = 0.12;
    let sky = textureSampleLevel(env_cube, env_sampler, ry, rough * levels).rgb * view.sun_color.w;
    let sun = view.sun_dir.xyz;
    let sun_lux = view.sun_dir.w * view.sun_color.rgb;
    let shadow = shadow_factor(w.xyz, n, dot(w.xyz - view.camera_pos.xyz,
        -vec3f(view.view[0].z, view.view[1].z, view.view[2].z)));
    var spec = vec3f(0.0);
    if (view.sun_dir.w > 0.0) {
        let h = normalize(sun + v);
        let nl = max(dot(n, sun), 0.0);
        let a = rough * rough;
        spec = d_ggx(max(dot(n, h), 0.0), a) * v_smith(nv, nl, a)
            * f_schlick(vec3f(0.02), max(dot(v, h), 0.0)) * sun_lux * nl * shadow;
    }
    let body_light = sh_irradiance(vec3f(0.0, 1.0, 0.0)) * 0.08 + sun_lux * max(sun.y, 0.0) * 0.012;
    let white = vec3f(0.85, 0.9, 0.92) * (body_light * 6.0 + sun_lux * max(sun.y, 0.0) * 0.25 * shadow);
    let c = film * (sky * f + spec) + white * foam;
    return vec4f(c * (1.0 - fog), dark);
}

// ---- Plunging lips and falling sheets (water.md 63.2) ------------------------------------------
// A ribbon a face wide along the parabola of the water the kernel throws over the face
// (water_lip.wgsl), from where it leaves to where it meets the water or the bed again, shaded as a
// sheet of the region's water as thick as the water leaving, thinning as the jet speeds up, and
// whitening with the air it takes in.
const LIP_SEGMENTS: u32 = 12u;
struct LipOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    // How far along its flight, the sheet's thickness there, and its kind (0 a lip, 1 a fall).
    @location(2) along: vec3f,
};
@vertex
fn vs_lip(@builtin(vertex_index) vi: u32, @location(0) launch: vec4f,
    @location(1) velocity: vec4f, @location(2) side: vec4f) -> LipOut {
    let corner = vi % 6u;
    let s = f32(vi / 6u + array<u32, 6>(0u, 1u, 1u, 0u, 1u, 0u)[corner]) / f32(LIP_SEGMENTS);
    let across = array<f32, 6>(-1.0, -1.0, 1.0, -1.0, 1.0, 1.0)[corner];
    let t = s * velocity.w;
    let g = vec3f(0.0, -9.81, 0.0);
    let moving = velocity.xyz + g * t;
    let world = launch.xyz + velocity.xyz * t + 0.5 * g * t * t + side.xyz * across;
    var o: LipOut;
    o.clip = view.view_proj * vec4f(world, 1.0);
    o.world = world;
    o.normal = normalize(cross(moving, side.xyz));
    let thickness = launch.w * length(velocity.xyz) / max(length(moving), 0.1);
    o.along = vec3f(s, thickness, side.w);
    return o;
}
@fragment
fn fs_lip(in: LipOut) -> @location(0) vec4f {
    let v_world = view.camera_pos.xyz - in.world;
    let v = select(vec3f(view.view[0].z, view.view[1].z, view.view[2].z),
        v_world / max(length(v_world), 1e-6), abs(view.proj[3].w) < 0.5);
    var n = normalize(in.normal);
    if (dot(n, v) < 0.0) { n = -n; }
    let nv = max(dot(n, v), 1e-3);
    let f = shallow_fresnel(nv, 0.75);
    let r = reflect(-v, n);
    let levels = f32(textureNumLevels(env_cube) - 1);
    let sky = textureSampleLevel(env_cube, env_sampler, vec3f(r.x, max(r.y, 0.02), r.z),
        0.08 * levels).rgb * view.sun_color.w;
    let sun = view.sun_dir.xyz;
    let sun_lux = view.sun_dir.w * view.sun_color.rgb;
    let body_light = sh_irradiance(vec3f(0.0, 1.0, 0.0)) * 0.08 + sun_lux * max(sun.y, 0.0) * 0.012;
    let shadow = shadow_factor(in.world, n, dot(in.world - view.camera_pos.xyz,
        -vec3f(view.view[0].z, view.view[1].z, view.view[2].z)));
    var spec = vec3f(0.0);
    if (view.sun_dir.w > 0.0) {
        let h = normalize(sun + v);
        let nl = max(dot(n, sun), 0.0);
        let a = 0.01;
        spec = d_ggx(max(dot(n, h), 0.0), a) * v_smith(nv, nl, a)
            * f_schlick(vec3f(0.02), max(dot(v, h), 0.0)) * sun_lux * nl * shadow;
    }
    // The light crossing the sheet along the view, and the light it scatters toward the eye.
    let haze = max(shallow.fog.w, 0.0);
    let extinction = shallow.absorb.rgb + shallow.scatter.rgb + vec3f(haze);
    let through = exp(-extinction * in.along.y / nv);
    let source = shallow.scatter.rgb * body_light + shallow.fog.rgb * haze * view.sun_color.w;
    let body = source / max(extinction, vec3f(1e-6)) * (1.0 - through);
    // The air the jet takes in: a lip stays clear as it leaves the crest and whitens as it pitches
    // over and lands; a fall whitens from its lip on.
    let air = select(0.04 + 0.7 * smoothstep(0.45, 1.0, in.along.x), 0.35 + 0.55 * in.along.x,
        in.along.z > 0.5);
    let white = vec3f(0.85, 0.9, 0.92) * (body_light * 6.0 + sun_lux * max(sun.y, 0.0) * 0.25
        * shadow);
    let clear = (body * (1.0 - f) + sky * f + spec) * (1.0 - air) + white * air;
    // What the sheet lets through of what is behind it, which the blend keeps.
    let behind = dot(through, vec3f(1.0 / 3.0)) * (1.0 - f) * (1.0 - air);
    let c = mix(clear, view.fog.rgb * view.sun_color.w * (1.0 - behind), fog_amount(in.world));
    return vec4f(c, 1.0 - behind);
}

@fragment
fn fs_water_depth(in: ShallowOut) -> @builtin(frag_depth) f32 {
    if(in.field_surface==1u&&ledge_at(in.world.xz)){discard;}
    var depth=in.depth;var normal=in.normal;
    if(in.field_surface==1u){let a=shallow_attributes(in.world.xz);depth=a.normal_depth.w;normal=a.normal_depth.xyz;}
    if (depth < 0.002 || -(view.view*vec4f(in.world,1.0)).z <= WATER_VIEW_EPSILON ||
        abs(dot(normalize(normal),view.camera_pos.xyz-in.world)) <= WATER_VIEW_EPSILON) { discard; }
    if(in.field_surface==1u&&water_behind_port(in.world)){discard;}
    if(in.field_surface==2u){return ledge_depth(in.world);}
    return shallow_depth(in.world);
}

fn water_world(uv: vec2f, z: f32) -> vec3f {
    let h = view.inv_view_proj * vec4f(uv * vec2f(2.0,-2.0) + vec2f(-1.0,1.0),z,1.0);
    return h.xyz / h.w;
}
fn water_project(p: vec3f) -> vec3f {
    let h = view.view_proj * vec4f(p,1.0);
    if (h.w <= 0.0) { return vec3f(-1.0,-1.0,0.0); }
    return vec3f(h.xy / h.w * vec2f(0.5,-0.5) + 0.5,h.z / h.w);
}
fn water_depth(uv: vec2f) -> f32 {
    let size = textureDimensions(water_scene_depth);
    let p = clamp(vec2i(uv * vec2f(size)),vec2i(0),vec2i(size)-1);
    return textureLoad(water_scene_depth,p,0).x;
}
fn water_inside(uv: vec2f) -> bool { return all(uv > vec2f(0.001)) && all(uv < vec2f(0.999)); }
// Scene color is resolved but its depth is nearest-sample. Ordinary bilinear color filtering
// across a depth discontinuity can pull a foreground texel into an otherwise valid floor hit.
fn water_color(uv: vec2f) -> vec3f {
    let size = textureDimensions(water_scene_depth);
    let q = uv * vec2f(size) - 0.5;
    let lo = vec2i(floor(q));
    let t = fract(q);
    let weights = vec4f((1.0-t.x)*(1.0-t.y), t.x*(1.0-t.y), (1.0-t.x)*t.y, t.x*t.y);
    let z = water_depth(uv);
    var depth: vec4f;
    for (var i=0u; i<4u; i++) {
        let p = clamp(lo+vec2i(i32(i%2u),i32(i/2u)),vec2i(0),vec2i(size)-1);
        depth[i] = textureLoad(water_scene_depth,p,0).x;
    }
    let valid = abs(depth-vec4f(z)) <= vec4f(max(z*0.02,1e-7));
    if (all(valid)) { return textureSampleLevel(water_scene_color,water_scene_sampler,uv,0.0).rgb; }
    var sum = vec3f(0.0); var weight = 0.0;
    for (var i=0u; i<4u; i++) {
        if (!valid[i]) { continue; }
        let p = clamp(lo+vec2i(i32(i%2u),i32(i/2u)),vec2i(0),vec2i(size)-1);
        sum += textureLoad(water_scene_color,p,0).rgb * weights[i];
        weight += weights[i];
    }
    return sum / max(weight,1e-6);
}
// Bounded depth-plane refinement of a Snell ray. Flat beds converge immediately; reject pulls
// that do not approach the actual ray on a curved/vertical depth surface.
fn water_refract(start: vec3f, direction: vec3f, uv0: vec2f, column_depth: f32, footprint: f32, enabled: bool) -> vec4f {
    let z0 = water_depth(uv0);
    var path = column_depth / max(abs(direction.y),0.15);
    if (z0 <= 0.0) { return vec4f(uv0,path,0.0); }
    let original = water_world(uv0,z0);
    if (!enabled) { return vec4f(uv0,length(original-start),0.0); }
    // Missing refracted scene color does not straighten the transmitted ray. Keep its optical
    // distance to the original depth plane; using the camera-ray distance casts an absorption
    // silhouette behind above-water occluders on an otherwise uniformly lit bed.
    var fallback_path=path;
    if((original.y-start.y)*direction.y>0.0){fallback_path=abs(start.y-original.y)/max(abs(direction.y),0.15);}
    var target_y = original.y;
    var uv = uv0;
    var destination = original;
    for (var i=0u; i<4u; i++) {
        path = abs(start.y-target_y) / max(abs(direction.y),0.15);
        let projected = water_project(start + direction * path);
        if (!water_inside(projected.xy)) { return vec4f(uv0,fallback_path,0.0); }
        let z = water_depth(projected.xy);
        if (z <= 0.0) { return vec4f(uv0,fallback_path,0.0); }
        // Depth is a nearest-texel load. Reconstruct that texel's centre, not the fractional
        // ray UV: mixing those coordinates creates a false height even on a planar floor.
        let size=vec2f(textureDimensions(water_scene_depth));
        let sample_uv=(floor(projected.xy*size)+0.5)/size;
        let hit = water_world(sample_uv,z);
        if ((direction.y < 0.0 && hit.y >= start.y-0.005) ||
            (direction.y > 0.0 && hit.y <= start.y+0.005)) {
            return vec4f(uv0,fallback_path,0.0);
        }
        uv = projected.xy;
        let converged = abs(hit.y-target_y) < 0.005;
        target_y = hit.y;
        destination = hit;
        if (converged) { break; }
    }
    // A curved/vertical depth surface need not converge to either horizontal-plane estimate.
    // Fade unconverged pulls rather than showing a detached image of a submerged obstacle.
    let residual = length(destination - (start + direction * path));
    // Scene depth is sampled per pixel: a fixed centimetre tolerance would reject ordinary
    // quantization at grazing/distant views and turn small bed gaps into surface speckles.
    let confidence = 1.0 - smoothstep(max(0.01,footprint*1.5),max(0.05,footprint*3.0),residual);
    let edge = smoothstep(0.0,0.08,min(min(uv.x,uv.y),min(1.0-uv.x,1.0-uv.y)));
    return vec4f(mix(uv0,uv,edge),mix(fallback_path,path,edge*confidence),confidence);
}
// Screen-space opaque reflection. A miss retains the environment reflection.
fn water_reflect(start: vec3f, direction: vec3f, fallback: vec3f, sigma:vec3f, source:vec3f, underwater:bool) -> vec3f {
    var previous = 0.05;
    var distance = 0.12;
    for (var i=0u; i<18u; i++) {
        let p = start + direction * distance;
        let projected = water_project(p);
        if (!water_inside(projected.xy)) { break; }
        let z = water_depth(projected.xy);
        if (z > 0.0) {
            let hit = water_world(projected.xy,z);
            let ray_z = -(view.view * vec4f(p,1.0)).z;
            let scene_z = -(view.view * vec4f(hit,1.0)).z;
            let thickness = max(0.08,(distance-previous)*0.7);
            if (ray_z >= scene_z && ray_z-scene_z < thickness) {
                var lo = previous; var hi = distance;
                var uv = projected.xy;
                for (var j=0u; j<4u; j++) {
                    let mid = (lo+hi)*0.5;
                    let q = start+direction*mid;
                    let test = water_project(q);
                    let d = water_depth(test.xy);
                    let world = water_world(test.xy,max(d,1e-7));
                    if (d > 0.0 && -(view.view*vec4f(q,1.0)).z >= -(view.view*vec4f(world,1.0)).z) {
                        hi=mid;uv=test.xy;
                    } else {lo=mid;}
                }
                let edge = smoothstep(0.0,0.06,min(min(uv.x,uv.y),min(1.0-uv.x,1.0-uv.y)));
                var reflected = water_color(uv);
                if(underwater){
                    let trans = exp(-sigma*hi);
                    reflected = reflected*trans+source/max(sigma,vec3f(1e-6))*(1.0-trans);
                }
                return mix(fallback,reflected,edge);
            }
        }
        previous=distance;distance=distance*1.38+0.05;
    }
    return fallback;
}

// How far a point has turned to the sea's water: 1 on a side open on the sea, 0 past its band.
// The band starts a column in: the sea runs under the region's first column (water.rs
// `owned_rectangles`), and over it the water must not show the sea's surface behind it where
// the next column shows its bed.
fn sea_opening(p: vec2f) -> f32 {
    let lo = p - shallow.origin;
    let hi = shallow.origin + vec2f(shallow.count.xy) * shallow.cell - p;
    let inside = array<f32, 4>(lo.x, hi.x, lo.y, hi.y);
    var open = 0.0;
    for (var side = 0u; side < 4u; side++) {
        let band = shallow.sea[side];
        if (band > 0.0) {
            open = max(open, 1.0 - smoothstep(shallow.cell, band, inside[side]));
        }
    }
    return open;
}

// Unpolarized dielectric Fresnel. Schlick alone misses the rapid rise just below the
// water-to-air critical angle, making Snell's window fade incorrectly before total reflection.
fn shallow_fresnel(cosine:f32,eta:f32) -> f32 {
    let transmitted=1.0-eta*eta*(1.0-cosine*cosine);
    if(transmitted<=0.0){return 1.0;}
    let ct=sqrt(transmitted);
    let rs=(eta*cosine-ct)/max(eta*cosine+ct,1e-6);
    let rp=(cosine-eta*ct)/max(cosine+eta*ct,1e-6);
    return clamp((rs*rs+rp*rp)*0.5,0.0,1.0);
}

struct ShallowFragment {
    @location(0) color: vec4f,
    @builtin(frag_depth) depth: f32,
};
struct FilmFactors { transmission:vec3f, reflection:vec3f }
fn film_polarized(f:f32,a:vec3f)->FilmFactors {
    let den=max(vec3f(1.0)-f*f*a*a,vec3f(0.00001));
    return FilmFactors((1.0-f)*(1.0-f)*a/den,vec3f(f)+(1.0-f)*(1.0-f)*f*a*a/den);
}
fn film_factors(cosine:f32,thickness:f32,sigma:vec3f)->FilmFactors {
    let ct=sqrt(max(1.0-0.75*0.75*(1.0-cosine*cosine),0.001));
    let rs=(0.75*cosine-ct)/(0.75*cosine+ct);let rp=(cosine-0.75*ct)/(cosine+0.75*ct);
    let a=exp(-sigma*(thickness/ct));let s=film_polarized(rs*rs,a);let p=film_polarized(rp*rp,a);
    // Sum internal returns for each polarization before averaging unpolarized light.
    return FilmFactors((s.transmission+p.transmission)*0.5,(s.reflection+p.reflection)*0.5);
}
// Locally parallel water film: reciprocal Fresnel interfaces plus Beer attenuation. The
// straight background is the thin-slab limit, not an invented deep-water path into the rock.
fn shallow_film(in:ShallowOut,footprint:f32)->ShallowFragment {
    let delta=view.camera_pos.xyz-in.world;let v=select(vec3f(view.view[0].z,view.view[1].z,view.view[2].z),delta/max(length(delta),1e-6),abs(view.proj[3].w)<0.5);let normal=select(-in.normal,in.normal,dot(in.normal,v)>=0.0);
    let tangent=vec2f(select(in.world.x,in.world.z,abs(normal.x)>0.5),in.world.y);
    let phase=fract(shallow.time*0.2);let blend=abs(phase*2.0-1.0);let flow=vec2f(0.0,-in.speed);
    let q0=tangent-flow*phase*5.0;let q1=tangent-flow*fract(phase+0.5)*5.0;
    let foam=foam_cover(q0,q1,blend,footprint,shallow_foam_at(in.world.xz));
    let haze=max(shallow.fog.w,0.0);let sigma=shallow.absorb.rgb+shallow.scatter.rgb+vec3f(haze);
    let factors=film_factors(max(dot(normal,v),0.001),in.depth,sigma);let transmission=factors.transmission;let reflection=factors.reflection;
    let uv=in.clip.xy/vec2f(textureDimensions(water_scene_depth));let background=water_color(uv);
    let reflected=reflect(-v,normal);let sky=textureSampleLevel(env_cube,env_sampler,reflected,0.0).rgb*view.sun_color.w;
    let sun=view.sun_dir.w*view.sun_color.rgb;let light=sh_irradiance(vec3f(0.0,1.0,0.0))*0.08+sun*max(view.sun_dir.y,0.0)*0.012;
    let source=shallow.scatter.rgb*light+shallow.fog.rgb*haze*view.sun_color.w;
    let body=source/max(sigma,vec3f(0.00001))*max(vec3f(1.0)-transmission-reflection,vec3f(0.0));
    let white=vec3f(0.85,0.9,0.92)*(light*6.0+sun*max(view.sun_dir.y,0.0)*0.25);
    var color=mix(background*transmission+sky*reflection+body,white,foam);
    color=mix(view.fog.rgb*view.sun_color.w,color,1.0-fog_amount(in.world));
    return ShallowFragment(vec4f(color,1.0),ledge_depth(in.world));
}
// Fine material-normal tangent filtering, independent of the drawn LOD triangle and raster quad.
fn shallow_material_footprint(world:vec3f,normal:vec3f,pixel:vec2f)->f32 {
    let extent=abs(shallow.cell)*vec2f(shallow.count.xy);
    let scale=max(extent.x,extent.y);
    // Normalize before taking the length so the finite-domain bound cannot overflow its square.
    if(!(scale>0.0 && scale<1e18)){return 1e18;}
    let limit=max(scale*length(extent/scale),1e-3);
    let size=max(vec2f(textureDimensions(water_scene_depth)),vec2f(1.0));
    let px=view.proj[0].x;let py=view.proj[1].y;
    let nn=dot(normal,normal);
    if(!(abs(px)>1e-8 && abs(px)<1e8 && abs(py)>1e-8 && abs(py)<1e8 && nn>1e-12 && nn<1e12)){return limit;}
    let n=normal*inverseSqrt(nn);
    // The world-space camera axes are rows of the world-to-view matrix (WGSL stores columns).
    let right=vec3f(view.view[0].x,view.view[1].x,view.view[2].x);
    let up=vec3f(view.view[0].y,view.view[1].y,view.view[2].y);
    let back=vec3f(view.view[0].z,view.view[1].z,view.view[2].z);
    let dx=right*(2.0/(size.x*px));let dy=up*(-2.0/(size.y*py));
    let perspective=abs(view.proj[3].w)<0.5;
    var ray=-back;var distance=1.0;
    if(perspective){
        let ndc=pixel/size*vec2f(2.0,-2.0)+vec2f(-1.0,1.0);
        ray=right*((ndc.x+view.proj[2].x)/px)+up*((ndc.y+view.proj[2].y)/py)-back;
        distance=-(view.view*vec4f(world,1.0)).z;
        if(!(distance>0.0 && distance<1e12)){return limit;}
    }
    let denominator=dot(n,ray);
    var singular=1e-5;
    // A perspective tangent horizon within this pixel has an unbounded planar footprint.
    // Select the whole finite field instead of reintroducing raster-quad derivatives.
    if(perspective){singular=max(singular,abs(dot(n,dx))+abs(dot(n,dy)));}
    if(!(abs(denominator)>singular && abs(denominator)<1e12)){return limit;}
    let wx=distance*(dx-ray*(dot(n,dx)/denominator));
    let wy=distance*(dy-ray*(dot(n,dy)/denominator));
    let xz=min(abs(wx.xz),vec2f(limit))+min(abs(wy.xz),vec2f(limit));
    return clamp(length(xz),1e-3,limit);
}

@fragment
fn fs_shallow(in: ShallowOut) -> ShallowFragment {
    // Derivatives precede the data-dependent cap/film selection on baseline WebGPU.
    let world_width=fwidth(in.world);
    let wind_dx=dpdx(in.world.xz)*max(view.texture_lod.x,view.texture_lod.y);
    let wind_dy=dpdy(in.world.xz)*max(view.texture_lod.x,view.texture_lod.y);
    var footprint=max(length(world_width.xz),1e-3);
    let film_footprint=max(length(vec2f(select(world_width.x,world_width.z,abs(in.normal.x)>0.5),world_width.y)),1e-3);
    if(in.field_surface==1u&&ledge_at(in.world.xz)){discard;}
    if(in.field_surface==2u){if(in.depth<0.002||-(view.view*vec4f(in.world,1.0)).z<=WATER_VIEW_EPSILON||abs(dot(normalize(in.normal),view.camera_pos.xyz-in.world))<=WATER_VIEW_EPSILON){discard;}return shallow_film(in,film_footprint);}
    var column_depth=in.depth;var base_normal=in.normal;var velocity=in.velocity;var speed=in.speed;
    if(in.field_surface==1u){let a=shallow_attributes(in.world.xz);column_depth=a.normal_depth.w;base_normal=a.normal_depth.xyz;velocity=a.flow.xy;speed=a.flow.z;}
    if (column_depth < 0.002 || -(view.view*vec4f(in.world,1.0)).z <= WATER_VIEW_EPSILON ||
        abs(dot(normalize(base_normal),view.camera_pos.xyz-in.world)) <= WATER_VIEW_EPSILON) {
        discard;
    }
    if(in.field_surface==1u&&water_behind_port(in.world)){discard;}
    if(in.field_surface==1u){footprint=shallow_material_footprint(in.world,base_normal,in.clip.xy);}
    let v_world = view.camera_pos.xyz - in.world;
    let v = select(vec3f(view.view[0].z,view.view[1].z,view.view[2].z),
        v_world / max(length(v_world),1e-6),abs(view.proj[3].w)<0.5);
    let n0 = normalize(base_normal);
    // Two flow phases hide each other's reset, avoiding unbounded UV stretching and sudden jumps.
    let phase = fract(shallow.time * 0.2);
    let blend = abs(phase * 2.0 - 1.0);
    let q0 = in.world.xz - velocity * phase * 5.0;
    let q1 = in.world.xz - velocity * fract(phase + 0.5) * 5.0;
    let ripple = clamp(speed * 0.5, 0.15, 1.0) * shallow.optics.z;
    var g = mix(wavelets(q0, shallow.time, footprint), wavelets(q1, shallow.time, footprint), blend) * ripple * 0.5;
    // Toward a side open on the sea the water turns to the sea's own (water.md 60): its bed fades
    // from view, its local reflections give way to the sky's and its waves to the sea's, so the
    // two meet unseen.
    let open = sea_opening(in.world.xz);
    // The wind sea the sea draws runs on over the region, its waves as high as the water's depth
    // lets them be, the sea's own over the band; the flow's own texture stays where the water
    // runs fast.
    var wind_variance = 0.0;
    var wind_foam = 0.0;
    if (wind_sea.params.x > 0.0) {
        let w = wind_surface(in.world.xz, wind_dx, wind_dy, column_depth / max(1.0 - open, 1e-4));
        g = g * 0.15 + w.slope;
        wind_variance = w.variance;
        wind_foam = w.foam;
    }
    // The ripples bodies stir, finer than the columns (water.md 57); with none, the shading is
    // as it was.
    let stirred = ripple_at(in.world.xz, footprint);
    if (any(stirred != vec3f(0.0))) { g += stirred.xy; }
    let n = normalize(vec3f(n0.x - g.x * n0.y, n0.y, n0.z - g.y * n0.y));
    let above = dot(n0,v) >= 0.0;
    let normal = select(-n,n,above);
    let nv = max(dot(normal,v),1e-4);
    let foam = foam_cover(q0, q1, blend, footprint, max(shallow_foam_at(in.world.xz), wind_foam));
    var smooth_rough = clamp(0.05 + footprint * 0.04, 0.05, 0.3);
    if (wind_sea.params.x > 0.0) { smooth_rough = sqrt(sqrt(0.05 * 0.05 * 0.05 * 0.05 + wind_variance)); }
    var rough = mix(smooth_rough, 0.5, foam);
    // Ripples finer than the pixel widen the glints: their slope variance adds to the lobe's
    // (GGX alpha^2 about twice the slope variance).
    if (stirred.z > 0.0) { rough = sqrt(sqrt(rough * rough * rough * rough + 2.0 * stirred.z)); }
    let a = rough * rough;
    let transmitted_ray = refract(-v,normal,select(1.3333333,0.75,above));
    let total_reflection = dot(transmitted_ray,transmitted_ray)<1e-6;
    let f = shallow_fresnel(nv,select(1.3333333,0.75,above));
    let r = reflect(-v,normal);
    let ry = vec3f(r.x, max(r.y, 0.02), r.z);
    let levels = f32(textureNumLevels(env_cube) - 1);
    var sky = textureSampleLevel(env_cube, env_sampler, ry, rough * levels).rgb * view.sun_color.w;
    let sun = view.sun_dir.xyz;
    let sun_lux = view.sun_dir.w * view.sun_color.rgb;
    let body_light = sh_irradiance(vec3f(0.0, 1.0, 0.0)) * 0.08 + sun_lux * max(sun.y, 0.0) * 0.012;
    let shadow = shadow_factor(in.world, n, dot(in.world - view.camera_pos.xyz,
        -vec3f(view.view[0].z, view.view[1].z, view.view[2].z)));
    var spec = vec3f(0.0);
    if (view.sun_dir.w > 0.0 && above) {
        let h = normalize(sun + v);
        let nl = max(dot(n, sun), 0.0);
        spec = d_ggx(max(dot(n, h), 0.0), a) * v_smith(nv, nl, a) * f_schlick(vec3f(0.02),
            max(dot(v, h), 0.0)) * sun_lux * nl * shadow;
    }
    let uv0 = in.clip.xy / vec2f(textureDimensions(water_scene_depth));
    let hit = water_refract(in.world,transmitted_ray,uv0,column_depth,footprint,shallow.optics.x>0.5 && !total_reflection);
    let haze = max(shallow.fog.w,0.0);
    let extinction = shallow.absorb.rgb + shallow.scatter.rgb + vec3f(haze);
    // Above the surface, light traverses water behind it. From below it exits into air; the
    // camera's existing underwater medium handles the segment before the interface.
    let through = select(vec3f(1.0),exp(-extinction*hit.z)*(1.0-open),above);
    var background = water_color(hit.xy);
    if (hit.w < 0.999) {
        // Blend samples, not coordinates: an interpolated UV could hit an object that neither
        // candidate ray reaches, creating another detached pull.
        let straight = water_color(uv0);
        background = mix(straight,background,hit.w);
    }
    let source = shallow.scatter.rgb * body_light + shallow.fog.rgb * haze * view.sun_color.w;
    let medium_color = source / max(extinction, vec3f(1e-6));
    let body = medium_color * (1.0 - through);
    if (!above) { sky = medium_color; }
    if (shallow.optics.y>0.5 && foam<0.8) { sky=mix(water_reflect(in.world+r*0.03,r,sky,extinction,source,!above),sky,open); }
    let white = vec3f(0.85, 0.9, 0.92) * (body_light * 6.0 + sun_lux * max(sun.y, 0.0) * 0.25 * shadow);
    var c = (background*through + body) * (1.0-f) + sky*f + spec;
    c = mix(c,white,foam);
    let edge = smoothstep(0.002,0.03,column_depth);
    c = mix(view.fog.rgb*view.sun_color.w,c,1.0-fog_amount(in.world));
    return ShallowFragment(vec4f(c,1.0)*edge,shallow_depth(in.world));
}

// Cap partition and vertical thin-film interfaces at bed discontinuities. No storage binding is
// needed for the active IDs: one u32 vertex attribute names each compacted quad instance.
fn ledge_choose(a:LedgePoint,b:LedgePoint,condition:bool)->LedgePoint {return LedgePoint(select(a.grid,b.grid,condition),select(a.value,b.value,condition));}
fn ledge_world(base:vec2i,p:LedgePoint)->vec3f {return vec3f(shallow.origin.x+(f32(base.x)+p.grid.x+0.5)*shallow.cell,p.value.x,shallow.origin.y+(f32(base.y)+p.grid.y+0.5)*shallow.cell);}
fn ledge_raster_vertex(quad_id:u32,vi:u32)->ShallowOut {
    let base=vec2i(i32(quad_id%(shallow.count.x-1u)),i32(quad_id/(shallow.count.x-1u)));let q=ledge_quad(base);var out:ShallowOut;
    if(q.enabled==0u){out.clip=vec4f(2.0,2.0,2.0,1.0);return out;}
    let index=array<u32,6>(0u,1u,2u,0u,2u,3u)[vi%6u];
    if(vi<24u){
        let corner=vi/6u;let triangle=u32(vi%6u>=3u);let t=ledge_triangle(q,corner,triangle);
        let point=ledge_point(q,corner,index);let a=ledge_world(base,t.a);let b=ledge_world(base,t.b);let c=ledge_world(base,t.c);
        out.world=ledge_world(base,point);out.normal=normalize(cross(c-a,b-a));out.depth=point.value.y;out.velocity=point.value.zw;out.speed=length(point.value.zw);out.field_surface=3u;
    }else{
        let edge=((vi-24u)%24u)/6u;let next=(edge+1u)%4u;
        if((q.cuts & (1u<<edge))==0u){out.clip=vec4f(2.0,2.0,2.0,1.0);return out;}
        let a=ledge_point(q,edge,1u);let b=ledge_point(q,edge,2u);let c=ledge_point(q,next,2u);let d=ledge_point(q,next,3u);
        let high_side=q.values[edge].x>q.values[next].x;
        let h0=ledge_choose(d,a,high_side);let h1=ledge_choose(c,b,high_side);let l0=ledge_choose(a,d,high_side);let l1=ledge_choose(b,c,high_side);
        let direction=(ledge_corner(next)-ledge_corner(edge))*select(-1.0,1.0,high_side);
        let endpoint=ledge_choose(h0,h1,index==1u||index==2u);let lower=ledge_choose(l0,l1,index==1u||index==2u);
        let donor=endpoint.value;let bed=donor.x-donor.y;var point=endpoint;
        out.normal=vec3f(direction.x,0.0,direction.y);out.velocity=donor.zw;
        if(vi<48u){
            // The actual cap volume ends at its bed, even with no outgoing flow.
            point.value.x=select(donor.x,min(donor.x,max(bed,lower.value.x)),index>=2u);
            out.world=ledge_world(base,point);out.depth=donor.y;out.speed=length(donor.zw);out.field_surface=4u;
        }else{
            // Below the upstream bed only directed discharge can form a falling film.
            point.value.x=select(max(bed,lower.value.x),lower.value.x,index>=2u);
            out.world=ledge_world(base,point);out.steep=donor.x;
            out.speed=sqrt(dot(donor.zw,donor.zw)+19.62*max(donor.x-out.world.y,0.0));
            let discharge=max(dot(donor.zw,direction),0.0)*donor.y;
            out.depth=select(0.0,max(0.003,min(donor.y,discharge/max(out.speed,0.1))),donor.y>0.002&&discharge>1e-6);
            out.field_surface=2u;
        }
    }
    out.clip=shallow_clip(out.world);return out;
}
@vertex
fn vs_shallow_ledge(@builtin(vertex_index) vi:u32,@location(0) quad_id:u32)->ShallowOut {return ledge_raster_vertex(quad_id,vi);}
@compute @workgroup_size(64)
fn probe_water_ledges(@builtin(global_invocation_id) id:vec3u){
    if(id.x>=arrayLength(&morph_probe_indices)){return;}let encoded=morph_probe_indices[id.x];let out=ledge_raster_vertex(encoded/128u,encoded%128u);
    morph_probe_positions[id.x*2u]=vec4f(out.world,out.depth);morph_probe_positions[id.x*2u+1u]=vec4f(out.normal,f32(out.field_surface));
}

@compute @workgroup_size(64)
fn probe_water_films(@builtin(global_invocation_id) id:vec3u){
    if(id.x>=arrayLength(&morph_probe_indices)/5u){return;}let at=id.x*5u;
    let f=film_factors(bitcast<f32>(morph_probe_indices[at]),bitcast<f32>(morph_probe_indices[at+1u]),bitcast<vec3f>(vec3u(morph_probe_indices[at+2u],morph_probe_indices[at+3u],morph_probe_indices[at+4u])));
    morph_probe_positions[id.x*2u]=vec4f(f.reflection,0.0);morph_probe_positions[id.x*2u+1u]=vec4f(f.transmission,0.0);
}
